use crate::config::Config;
use crate::model::{Category, CategoryTotal, Finding, ScanProgress, VolumeInfo};
use crate::scanner::ScanCounters;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

/// Run blocking filesystem work off the webview thread.
///
/// A synchronous `#[tauri::command] fn` is dispatched on the thread that
/// also has to keep the webview responsive. Every command here that lists
/// volumes, reads the audit log, or moves files did exactly that, so one tab
/// switch could stall the whole window for as long as the disk took to answer
/// - which is what the user experiences as the UI freezing up.
///
/// Only genuinely blocking work goes through this; anything that is already
/// just a lock or a counter stays synchronous.
fn offload<F, R>(f: F) -> impl std::future::Future<Output = R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    async move {
        tauri::async_runtime::spawn_blocking(f)
            .await
            .unwrap_or_else(|e| panic!("offloaded task failed: {e}"))
    }
}

/// Live scan session, so the UI can cancel a running scan.
#[derive(Default)]
pub struct ScanState {
    /// Swapped at the start of every scan; cancel targets whichever is current.
    pub current: Mutex<Option<Arc<ScanCounters>>>,
}

/// Live OneDrive-link state.
///
/// `session` is the sign-in the user has started and not finished; `access` is
/// the short-lived access token cache. Both are in-memory only: a half-finished
/// sign-in must not survive a restart, and an access token is not worth writing
/// to disk when a refresh token already is.
#[derive(Default)]
pub struct GraphState {
    pub session: Mutex<Option<crate::cloud::account::Session>>,
    pub access: crate::cloud::account::AccessCache,
}

/// The code the user must type at Microsoft's sign-in page.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectInfo {
    pub user_code: String,
    pub verification_uri: String,
    pub interval_secs: i64,
    pub expires_in: i64,
}

/// One poll's outcome.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectPoll {
    /// `pending` | `slow_down` | `denied` | `connected` | `error`.
    pub status: String,
    pub detail: String,
    /// Seconds the caller should wait before polling again.
    pub interval_secs: i64,
    pub quota: Option<crate::cloud::graph::Quota>,
}

fn graph_config() -> Result<Config, String> {
    let cfg = Config::load();
    if cfg.graph_client_id().is_empty() {
        return Err(
            "未配置 Azure 应用（client）ID，无法连接 OneDrive。请在设置中填入 graphClientId。"
                .to_string(),
        );
    }
    Ok(cfg)
}

/// Start a OneDrive sign-in and return the code to display.
#[tauri::command]
pub async fn graph_connect_start(state: State<'_, GraphState>) -> Result<ConnectInfo, String> {
    let cfg = graph_config()?;
    let now = crate::license::now_unix();
    let client_id = cfg.graph_client_id().to_string();
    let session = offload(move || {
        crate::cloud::account::begin(&crate::cloud::graph::WinHttp, &client_id, now)
    })
    .await?;

    let info = ConnectInfo {
        user_code: session.user_code.clone(),
        verification_uri: session.verification_uri.clone(),
        interval_secs: session.interval_secs,
        expires_in: (session.expires_unix - now).max(0),
    };
    *state
        .session
        .lock()
        .map_err(|_| "graph state lock poisoned".to_string())? = Some(session);
    Ok(info)
}

/// Poll a sign-in in progress. The UI calls this on the returned interval.
#[tauri::command]
pub async fn graph_connect_poll(state: State<'_, GraphState>) -> Result<ConnectPoll, String> {
    let session = state
        .session
        .lock()
        .map_err(|_| "graph state lock poisoned".to_string())?
        .clone()
        .ok_or_else(|| "没有进行中的 OneDrive 登录".to_string())?;

    let now = crate::license::now_unix();
    let probe = session.clone();
    let outcome = offload(move || {
        crate::cloud::account::poll(&crate::cloud::graph::WinHttp, &probe, now)
    })
    .await;

    use crate::cloud::graph::Poll;
    match outcome {
        // Keep the session: the user simply has not finished signing in.
        Poll::Pending => Ok(ConnectPoll {
            status: "pending".into(),
            detail: String::new(),
            interval_secs: session.interval_secs,
            quota: None,
        }),
        // Back off exactly as asked; polling faster earns a ban.
        Poll::SlowDown => Ok(ConnectPoll {
            status: "slow_down".into(),
            detail: String::new(),
            interval_secs: (session.interval_secs + 5).min(60),
            quota: None,
        }),
        Poll::Denied(reason) => {
            *state.session.lock().map_err(|_| "graph state lock poisoned".to_string())? = None;
            Ok(ConnectPoll {
                status: "denied".into(),
                detail: reason,
                interval_secs: 0,
                quota: None,
            })
        }
        // Transport trouble: retryable, so the session survives.
        Poll::TransportError(e) => Ok(ConnectPoll {
            status: "error".into(),
            detail: e,
            interval_secs: session.interval_secs,
            quota: None,
        }),
        Poll::Ready(tokens) => {
            *state.session.lock().map_err(|_| "graph state lock poisoned".to_string())? = None;
            let client_id = session.client_id.clone();
            let access = Arc::clone(&state.access);
            let store = crate::cloud::account::DiskAuth;
            offload(move || {
                crate::cloud::account::accept(
                    &tokens,
                    &client_id,
                    &store,
                    &access,
                )
            })
            .await?;

            // The quota is a bonus at this point: the account is linked either
            // way, and failing to read a number must not undo a sign-in.
            let access = Arc::clone(&state.access);
            let quota = offload(move || {
                crate::cloud::account::status(
                    &crate::cloud::graph::WinHttp,
                    &crate::cloud::account::DiskAuth,
                    &access,
                    crate::license::now_unix(),
                )
                .quota
            })
            .await;

            Ok(ConnectPoll {
                status: "connected".into(),
                detail: String::new(),
                interval_secs: 0,
                quota,
            })
        }
    }
}

/// Abandon a sign-in in progress.
#[tauri::command]
pub fn graph_connect_cancel(state: State<'_, GraphState>) {
    if let Ok(mut slot) = state.session.lock() {
        *slot = None;
    }
}

/// Account state and cloud quota.
///
/// Returns a `Result` because Tauri requires it of async commands that take a
/// borrowed `State`; `status` itself does not fail - an account problem is
/// reported inside the value, so the UI can distinguish "not linked" from
/// "quota temporarily unreadable".
#[tauri::command]
pub async fn graph_account_status(
    state: State<'_, GraphState>,
) -> Result<crate::cloud::account::AccountStatus, String> {
    let access = Arc::clone(&state.access);
    Ok(offload(move || {
        crate::cloud::account::status(
            &crate::cloud::graph::WinHttp,
            &crate::cloud::account::DiskAuth,
            &access,
            crate::license::now_unix(),
        )
    })
    .await)
}

/// Unlink the OneDrive account and forget its credential.
#[tauri::command]
pub fn graph_sign_out(state: State<'_, GraphState>) {
    crate::cloud::account::sign_out(
        &crate::cloud::account::DiskAuth,
        &state.access,
    );
    if let Ok(mut slot) = state.session.lock() {
        *slot = None;
    }
}

/// One slice of a scan's findings, streamed to the UI over an event rather
/// than returned as a single giant array.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FindingsChunk {
    findings: Vec<Finding>,
    done: bool,
}

#[tauri::command]
pub async fn get_volumes() -> Vec<VolumeInfo> {
    offload(crate::winapi::list_volumes).await
}

#[tauri::command]
pub async fn get_config() -> Config {
    offload(Config::load).await
}

#[tauri::command]
pub async fn save_config(cfg: Config) -> Result<(), String> {
    offload(move || cfg.save().map_err(|e| e.to_string())).await
}

#[derive(serde::Deserialize)]
pub struct ScanOptions {
    /// Which scanners to run. Large-file scanning is opt-in because a full
    /// volume walk is expensive and it never deletes anything by itself.
    pub categories: Vec<Category>,
}

/// Run every enabled scanner. Findings stream back to the UI as `findings-chunk`
/// events; the returned value is just the total count. Keeping the list out of
/// the IPC return avoids the serializer building a second full copy of hundreds
/// of thousands of items.
///
/// Progress is reported from a background ticker rather than from inside the
/// scanners: a whole-volume walk can take tens of seconds, and a progress bar
/// that only moves at the very end is indistinguishable from a hang.
#[tauri::command]
pub async fn start_scan(
    app: AppHandle,
    state: State<'_, ScanState>,
    options: ScanOptions,
) -> Result<u64, String> {
    let cfg = Config::load();
    let counters = Arc::new(ScanCounters::default());
    {
        let mut slot = state
            .current
            .lock()
            .map_err(|_| "scan state lock poisoned".to_string())?;
        *slot = Some(Arc::clone(&counters));
    }

    let ctx = crate::scanner::ScanCtx::new(Arc::clone(&counters), &cfg);

    // The large-file pass is opt-in and walks whole volumes, so it never runs
    // alongside the cheap targeted scanners.
    let large_requested = options.categories.contains(&Category::LargeFile);
    let parallel_categories: Vec<Category> = options
        .categories
        .iter()
        .copied()
        .filter(|c| *c != Category::LargeFile)
        .collect();

    let tick_app = app.clone();
    let tick_counters = Arc::clone(&counters);
    let ticker = tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            let done = tick_counters.dirs.load(Ordering::Relaxed) > 0
                && tick_counters.finished.load(Ordering::Relaxed);
            let _ = tick_app.emit(
                "scan-progress",
                ScanProgress {
                    scanner: "all".into(),
                    phase: if done { "finishing" } else { "scanning" }.into(),
                    dirs_scanned: tick_counters.dirs.load(Ordering::Relaxed),
                    bytes_scanned: tick_counters.bytes.load(Ordering::Relaxed),
                    findings: tick_counters.findings.load(Ordering::Relaxed),
                    skipped: tick_counters.skipped.load(Ordering::Relaxed),
                    current_path: String::new(),
                    done: false,
                },
            );
            if tick_counters.finished.load(Ordering::Relaxed) {
                break;
            }
        }
    });

    let counters_for_task = Arc::clone(&counters);
    // The progress ticker polls `finished` to know when to stop, and the UI
    // waits on this command returning. Setting it in a guard rather than at
    // the end of the closure means a panic anywhere in the scanners still
    // marks the scan as over, instead of leaving the progress bar running for
    // a scan that no longer exists.
    let finished_flag = Arc::clone(&counters);
    struct MarkFinished(Arc<ScanCounters>);
    impl Drop for MarkFinished {
        fn drop(&mut self) {
            self.0.finished.store(true, Ordering::Relaxed);
        }
    }
    let findings = tauri::async_runtime::spawn_blocking(move || {
        let _guard = MarkFinished(finished_flag);
        let mut out = crate::scanner::run_all(&ctx, &parallel_categories);
        if large_requested {
            use crate::scanner::Scanner;
            let lctx = crate::scanner::ScanCtx::new(Arc::clone(&counters_for_task), &cfg);
            out.extend(crate::scanner::largefile::LargeFileScanner.scan(&lctx));
        }
        out
    })
    .await
    .map_err(|e| format!("scan task failed: {e}"))?;

    ticker.abort();

    let mut findings = findings;
    findings.sort_by(|a, b| b.reclaimable().cmp(&a.reclaimable()));

    // Stream the findings to the UI in bounded chunks instead of returning one
    // giant array: a single monolithic return makes the serializer allocate a
    // full second copy of the list (roughly doubling peak memory on a
    // whole-volume scan) and can exceed Tauri's IPC message budget. The
    // terminal `done` chunk is what the UI waits on, so a dropped intermediate
    // chunk cannot silently lose findings.
    const CHUNK: usize = 4096;
    for chunk in findings.chunks(CHUNK) {
        if app
            .emit(
                "findings-chunk",
                FindingsChunk { findings: chunk.to_vec(), done: false },
            )
            .is_err()
        {
            break;
        }
    }
    let _ = app.emit(
        "findings-chunk",
        FindingsChunk { findings: Vec::new(), done: true },
    );

    let _ = app.emit(
        "scan-progress",
        ScanProgress {
            scanner: "all".into(),
            phase: "done".into(),
            dirs_scanned: counters.dirs.load(Ordering::Relaxed),
            bytes_scanned: counters.bytes.load(Ordering::Relaxed),
            findings: findings.len() as u64,
            skipped: counters.skipped.load(Ordering::Relaxed),
            current_path: String::new(),
            done: true,
        },
    );

    Ok(findings.len() as u64)
}

#[tauri::command]
pub fn cancel_scan(state: State<'_, ScanState>) {
    if let Ok(slot) = state.current.lock() {
        if let Some(c) = slot.as_ref() {
            c.request_cancel();
        }
    }
}

/// Open the item's containing folder in Explorer, with the item selected. The
/// `/select,` switch shows the folder and highlights the item, which is the
/// "show me where this lives" affordance of the findings table.
#[tauri::command]
pub fn reveal_in_explorer(path: String) -> Result<(), String> {
    let path = path.replace('/', "\\");
    std::process::Command::new("explorer.exe")
        .arg("/select,")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("无法打开资源管理器：{e}"))
}

#[tauri::command]
pub fn summarize(findings: Vec<Finding>) -> Vec<CategoryTotal> {
    let mut totals: Vec<CategoryTotal> = Vec::new();
    for f in &findings {
        let slot = match totals.iter_mut().find(|t| t.category == f.category) {
            Some(s) => s,
            None => {
                totals.push(CategoryTotal {
                    category: f.category,
                    count: 0,
                    logical_bytes: 0,
                    on_disk_bytes: 0,
                });
                totals.last_mut().unwrap()
            }
        };
        slot.count += 1;
        slot.logical_bytes += f.logical_size;
        slot.on_disk_bytes += f.on_disk_size;
    }
    totals.sort_by(|a, b| b.on_disk_bytes.cmp(&a.on_disk_bytes));
    totals
}

#[tauri::command]
pub fn build_preview(
    items: Vec<crate::engine::SelectedItem>,
) -> Vec<crate::engine::PreviewGroup> {
    crate::engine::build_preview(&items)
}

#[tauri::command]
pub fn needs_red_confirmation(items: Vec<crate::engine::SelectedItem>) -> bool {
    crate::engine::needs_red_confirmation(&items)
}

#[tauri::command]
pub async fn run_cleanup(
    state: State<'_, GraphState>,
    request: crate::engine::CleanupRequest,
) -> Result<crate::engine::CleanupReport, String> {
    let access = Arc::clone(&state.access);
    Ok(offload(move || {
        let cfg = Config::load();
        let marks = crate::importance::Store::load();
        // Resolved here rather than in the UI: the subscription gate has to
        // hold against a client that asks for a paid feature regardless.
        let entitlement = crate::license::entitlement();

        // Built unconditionally but inert until an item asks for an upload, so
        // a cleanup with no cloud items makes no network call at all.
        let transport = crate::cloud::graph::WinHttp;
        let store = crate::cloud::account::DiskAuth;
        let sink = crate::cloud::backup::OneDriveSink {
            transport: &transport,
            store: &store,
            cache: &access,
            now: crate::license::now_unix(),
        };
        let cloud = crate::engine::CloudContext {
            subscriber: entitlement.cloud_allowed(),
            sink: Some(&sink),
            batch_id: String::new(),
        };
        crate::engine::run_cleanup_full(request, &cfg, &marks, &cloud)
    })
    .await)
}

/// Relaunch the app elevated and replay the given cleanup request.
///
/// The design calls for elevation only when a cleanup actually hits a
/// permission wall, never on launch. The request is staged to a temp file
/// because the elevated process is a fresh instance with no access to this
/// one's memory.
#[tauri::command]
pub async fn retry_cleanup_elevated(
    request: crate::engine::CleanupRequest,
) -> Result<(), String> {
    offload(move || {
        let staged = crate::winapi::elevate::stage_cleanup_request(&request)
            .map_err(|e| format!("无法准备提权清理：{e}"))?;

        // A fresh instance picks the staged request up, runs it, and exits.
        // --elevated-cleanup tells run() not to open a second window. The path
        // is quoted so a %TEMP% containing spaces survives argv splitting.
        let args = format!("--elevated-cleanup \"{}\"", staged.display());
        crate::winapi::elevate::relaunch_elevated(&args)
            .map(|_pid| ())
    })
    .await
}

/// Path of a staged elevated-cleanup request, if this process was started as
/// one. Consumed once so a crash cannot replay a deletion.
#[tauri::command]
pub fn take_staged_cleanup() -> Option<crate::engine::CleanupRequest> {
    let mut args = std::env::args();
    let flag = args.find(|a| a.starts_with("--elevated-cleanup"))?;
    // The path either trails the flag ("--elevated-cleanup=<path>") or is the
    // next argv token (the space-and-quote form). Strip the flag, an optional
    // '=', and any surrounding quotes in either case.
    let rest = flag
        .trim_start_matches("--elevated-cleanup")
        .trim_start_matches('=')
        .trim()
        .trim_matches('"');
    let path = if rest.is_empty() {
        args.next()?.trim_matches('"').to_string()
    } else {
        rest.to_string()
    };
    if path.is_empty() {
        return None;
    }
    crate::winapi::elevate::take_staged_cleanup(std::path::Path::new(&path))
}

#[tauri::command]
pub async fn list_quarantine_batches() -> Vec<crate::quarantine::BatchMeta> {
    offload(|| {
        let cfg = Config::load();
        crate::quarantine::manifest::list_batches(&cfg.quarantine_dir)
    })
    .await
}

#[tauri::command]
pub async fn list_quarantine_entries(
    batch_dir: String,
) -> Result<crate::quarantine::Manifest, String> {
    offload(move || {
        crate::quarantine::Manifest::load(std::path::Path::new(&batch_dir))
            .ok_or_else(|| "batch manifest is missing or unreadable".to_string())
    })
    .await
}

#[tauri::command]
pub async fn restore_batch(
    batch_dir: String,
    policy: crate::quarantine::ConflictPolicy,
) -> Result<crate::quarantine::RestoreReport, String> {
    offload(move || {
        crate::quarantine::ops::restore_batch(std::path::Path::new(&batch_dir), policy)
    })
    .await
}

#[tauri::command]
pub async fn restore_item(
    batch_dir: String,
    original_path: String,
    policy: crate::quarantine::ConflictPolicy,
) -> Result<(), String> {
    offload(move || {
        crate::quarantine::ops::restore_item(
            std::path::Path::new(&batch_dir),
            std::path::Path::new(&original_path),
            policy,
        )
    })
    .await
}

#[tauri::command]
pub async fn purge_batch(batch_dir: String) -> crate::quarantine::PurgeReport {
    offload(move || {
        crate::quarantine::ops::purge_batch(std::path::Path::new(&batch_dir))
    })
    .await
}

/// Expiry notice for the quarantine tab. The software never deletes anything
/// on its own; this only reports what has been sitting long enough that the
/// user may want to clear it.
#[tauri::command]
pub async fn quarantine_expiry() -> ExpirySummary {
    offload(quarantine_expiry_blocking).await
}

/// Summarise what has aged out of the quarantine store.
///
/// This reads the config and walks the batch directory, so it runs on a
/// blocking thread: the UI asks for it on every scan and every tab switch.
fn quarantine_expiry_blocking() -> ExpirySummary {
    let cfg = Config::load();
    let now = crate::scanner::walk::now_unix();
    let mut expired_batches = Vec::new();
    let mut expired_items = 0u64;
    let mut expired_on_disk = 0u64;

    for b in crate::quarantine::manifest::list_batches(&cfg.quarantine_dir) {
        let age_days = ((now - b.created_unix).max(0) as f64 / 86_400.0) as u32;
        if age_days >= cfg.quarantine_expiry_days {
            expired_items += b.item_count as u64;
            expired_on_disk += b.on_disk_bytes;
            expired_batches.push(b.batch_id);
        }
    }

    ExpirySummary {
        expiry_days: cfg.quarantine_expiry_days,
        expired_batches,
        expired_items,
        expired_on_disk_bytes: expired_on_disk,
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpirySummary {
    pub expiry_days: u32,
    pub expired_batches: Vec<String>,
    pub expired_items: u64,
    pub expired_on_disk_bytes: u64,
}

/// Recent audit-log entries, newest first.
#[tauri::command]
pub async fn read_audit_log(limit: Option<usize>) -> Vec<crate::log::Entry> {
    offload(move || crate::log::tail(limit.unwrap_or(200).min(1000))).await
}

#[tauri::command]
pub fn audit_log_path() -> Option<String> {
    crate::log::log_path().map(|p| p.to_string_lossy().to_string())
}

/// Every cloud backup batch, newest first.
///
/// Errors when no account is linked; the UI treats that as "no cloud backups"
/// rather than as a failure, because having no OneDrive is the normal case.
#[tauri::command]
pub async fn list_cloud_backups(
    state: State<'_, GraphState>,
) -> Result<Vec<crate::cloud::backup::CloudBatch>, String> {
    let access = Arc::clone(&state.access);
    offload(move || {
        crate::cloud::backup::list_backups(
            &crate::cloud::graph::WinHttp,
            &crate::cloud::account::DiskAuth,
            &access,
            crate::license::now_unix(),
        )
    })
    .await
}

/// Download a batch back to the locations its manifest recorded.
#[tauri::command]
pub async fn restore_cloud_backup(
    state: State<'_, GraphState>,
    batch_id: String,
    policy: crate::quarantine::ConflictPolicy,
) -> Result<crate::cloud::backup::CloudRestoreReport, String> {
    let access = Arc::clone(&state.access);
    offload(move || {
        crate::cloud::backup::restore_batch(
            &crate::cloud::graph::WinHttp,
            &crate::cloud::account::DiskAuth,
            &access,
            crate::license::now_unix(),
            &batch_id,
            policy,
        )
    })
    .await
}

/// Delete a whole batch from the cloud.
#[tauri::command]
pub async fn delete_cloud_backup(
    state: State<'_, GraphState>,
    batch_id: String,
) -> Result<(), String> {
    let access = Arc::clone(&state.access);
    offload(move || {
        crate::cloud::backup::delete_batch(
            &crate::cloud::graph::WinHttp,
            &crate::cloud::account::DiskAuth,
            &access,
            crate::license::now_unix(),
            &batch_id,
        )
    })
    .await
}

/// Whether OneDrive is present, and where its sync roots are.
///
/// Cheap by design: it reads environment variables and stats the result, so
/// the overview card can ask on every load without walking a tree that may
/// hold hundreds of thousands of files. The byte counts come from the scan.
#[tauri::command]
pub async fn onedrive_status() -> crate::cloud::onedrive::Status {
    offload(crate::cloud::onedrive::status).await
}

/// Current subscription state for the cloud-backup feature.
#[tauri::command]
pub async fn get_entitlement() -> crate::license::EntitlementView {
    offload(|| crate::license::entitlement().view()).await
}

/// Re-check the subscription, ignoring a fresh cached answer.
#[tauri::command]
pub async fn refresh_entitlement() -> crate::license::EntitlementView {
    offload(|| crate::license::entitlement_forced().view()).await
}

/// Every importance mark the user has set, most important first.
///
/// The UI keeps this as a path-keyed map and stamps it onto rendered rows, so
/// a mark is visible immediately after a re-scan without the scanners having
/// to know the store exists.
#[tauri::command]
pub async fn list_importance() -> Vec<crate::importance::Entry> {
    offload(|| crate::importance::Store::load().entries()).await
}

/// Set or replace the importance level of one path.
///
/// Gated, and the gate is on the *write* rather than on the menu that offers
/// it. A mark is what makes the cleanup pipeline refuse to act on a path, so
/// accepting one from a user who has not paid would hand them the protection
/// for free - hiding the menu button would be decoration, not a gate.
#[tauri::command]
pub async fn set_importance(
    path: String,
    level: crate::importance::Importance,
) -> Result<(), String> {
    offload(move || {
        // The entitlement probe can block on the Store, which is why it lives
        // inside offload rather than in the async body.
        if !crate::license::entitlement().importance_allowed() {
            return Err("重要级别标记需要有效订阅".to_string());
        }
        let mut store = crate::importance::Store::load();
        store.set(std::path::Path::new(&path), level);
        store.save().map_err(|e| format!("无法保存重要级别：{e}"))?;

        // The mark is stored; now make the guard agree with it. Only when the
        // guard is actually standing: with the disposition on "allow" the user
        // has said they do not want deletes blocked, and holding a newly marked
        // file would override that.
        if crate::guard::read() != crate::guard::Disposition::Allow {
            crate::protection::sync_holds_from_marks();
        }
        Ok(())
    })
    .await
}

/// Clear the importance mark of one path.
///
/// Deliberately **not** gated. A mark that was made while subscribed outlives
/// the subscription, and the cleanup pipeline refuses to touch a marked path
/// whether or not anyone is still paying. If clearing were behind the gate, a
/// lapsed subscriber would be left holding files they could no longer act on,
/// with no way to release them - the gate would be holding their own data
/// hostage to a renewal.
///
/// It also releases the external guard, for the same reason: clearing the mark
/// is the user saying this file is theirs to act on again.
#[tauri::command]
pub async fn clear_importance(path: String) -> Result<(), String> {
    offload(move || {
        let mut store = crate::importance::Store::load();
        store.remove(std::path::Path::new(&path));
        store.save().map_err(|e| format!("无法保存重要级别：{e}"))?;

        // Release before re-syncing, and by path rather than by rebuilding from
        // the store. The file being cleared may be the one the store could not
        // write, and leaving a handle on a file the user has just released is
        // the one failure of this mechanism that would be immediately visible
        // to them.
        let _ = crate::protection::holds().release(std::path::Path::new(&path));
        Ok(())
    })
    .await
}

/// Which mechanism is protecting marked files on this machine.
///
/// The settings screen needs this because "protected" has more than one
/// meaning here - a kernel filter that can prompt, or nothing at all - and a
/// screen that showed only "protected" would be describing different products
/// with one word. It currently reports that nothing is available, which is the
/// truth: see the note at the top of `protection.rs`.
#[tauri::command]
pub async fn protection_status() -> crate::protection::Status {
    offload(crate::protection::status).await
}

/// The delete guard's disposition, plus whether it is actually running.
///
/// Both halves matter together. With no service attached the driver fails open
/// by design, so a marked file simply deletes - and a screen that showed only
/// the disposition would be describing something that is not happening. The
/// status is what lets it say that the setting was saved but nothing is being
/// protected right now.
#[tauri::command]
pub async fn guard_status() -> crate::guard::Status {
    offload(crate::guard::status).await
}

/// Change what happens to a marked file on delete.
///
/// Written to the service's policy file rather than kept in this process: the
/// service owns the file, watches it, and pushes the value into the kernel
/// filter, so a change lands within a couple of seconds and survives a reboot.
/// The same file is what the user-mode hold backend reads back, so both
/// mechanisms take their orders from one place.
///
/// The disposition is then applied here as well. With no driver installed there
/// is nothing else to do it, and the hold has to be taken or released by
/// whichever process is running - which is this one.
///
/// Turning protection **on** is gated; turning it **off** never is. The
/// asymmetry is the same one as `clear_importance`: a lapsed subscriber must
/// always be able to stand the guard down, and blocking that would leave them
/// unable to delete their own files.
#[tauri::command]
pub async fn set_guard_disposition(disposition: crate::guard::Disposition) -> Result<(), String> {
    offload(move || {
        let stands_down = disposition == crate::guard::Disposition::Allow;
        if !stands_down && !crate::license::entitlement().guard_allowed() {
            return Err("删除防护需要有效订阅".to_string());
        }
        crate::guard::write(disposition).map_err(|e| format!("无法写入守护策略：{e}"))?;

        if stands_down {
            // Every handle, not just the marked ones. Standing down has to be
            // unconditional: the user asked for it, and leaving a file held
            // because the mark store could not be read would be the guard
            // refusing to let go.
            crate::protection::release_all_holds();
        } else {
            // `Ask` and `Deny` both hold. The prompt is a driver capability -
            // a held handle cannot ask anything, it only makes the delete fail -
            // so with no driver the two behave the same, and the alternative
            // would be an "ask me" setting that protects nothing at all.
            crate::protection::sync_holds_from_marks();
        }
        Ok(())
    })
    .await
}
