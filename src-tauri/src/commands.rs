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
    request: crate::engine::CleanupRequest,
) -> crate::engine::CleanupReport {
    offload(move || {
        let cfg = Config::load();
        crate::engine::run_cleanup(request, &cfg)
    })
    .await
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
