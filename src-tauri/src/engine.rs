//! The cleanup pipeline: take a user selection, apply the safety gate,
//! then purge or quarantine. Nothing is deleted without passing through here.

use crate::model::{Disposition, Finding, RiskTier};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// One row of the user's selection, carrying its chosen disposition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedItem {
    pub finding: Finding,
    pub disposition: Disposition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupRequest {
    pub items: Vec<SelectedItem>,
    /// The UI sets this only after the user ticks the explicit confirmation
    /// box in the preview dialog. Without it, red-tagged items are refused.
    pub confirmed_red_purge: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub purged: Vec<String>,
    pub quarantined: Vec<String>,
    pub blocked: Vec<BlockedItem>,
    pub failed: Vec<BlockedItem>,
    pub logical_purged: u64,
    pub on_disk_purged: u64,
    pub quarantine_batch: Option<String>,
    /// Paths whose local cloud copies were released.
    pub dehydrated: Vec<String>,
    /// Bytes whose release was *requested*. OneDrive dehydrates in the
    /// background, so this is not yet reclaimed space and the UI must say so.
    pub dehydrate_requested_bytes: u64,
    /// Files skipped because the user had pinned them.
    pub dehydrate_pinned_files: u64,
    /// True when at least one item failed specifically because the process
    /// lacked permission. The UI uses this to offer elevation instead of
    /// silently reporting a partial cleanup.
    pub needs_elevation: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockedItem {
    pub path: String,
    pub reason: String,
}

/// One group as shown in the preview dialog.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewGroup {
    pub disposition: Disposition,
    pub item_count: usize,
    pub logical_bytes: u64,
    pub on_disk_bytes: u64,
    pub category_counts: BTreeMap<String, usize>,
    /// True when this group contains red-tagged items headed for deletion.
    pub contains_red_purge: bool,
}

/// Whether a failure was caused by insufficient permissions rather than
/// the item being genuinely undeletable.
///
/// The filesystem layer reports errors as formatted strings, so this matches
/// on the shapes the OS actually produces rather than trying to thread error
/// codes through every call. Being wrong here costs one extra UAC prompt, not
/// a wrong deletion, so a generous match is the safe direction.
pub fn is_permission_failure(reason: &str) -> bool {
    let r = reason.to_lowercase();
    r.contains("access is denied")
        || r.contains("permission denied")
        || r.contains("拒绝访问")
        || r.contains("权限不足")
        || r.contains("需要更高的权限")
        || r.contains("管理员权限")
        || r.contains("requires elevation")
        || r.contains("privilege")
}

/// Build the grouped preview the confirmation dialog renders.
pub fn build_preview(items: &[SelectedItem]) -> Vec<PreviewGroup> {
    let mut groups: BTreeMap<&str, (usize, u64, u64, BTreeMap<String, usize>, bool)> = BTreeMap::new();

    for it in items {
        let key = match it.disposition {
            Disposition::Purge => "purge",
            Disposition::Quarantine => "quarantine",
            Disposition::Dehydrate => "dehydrate",
        };
        let e = groups.entry(key).or_insert((0, 0, 0, BTreeMap::new(), false));
        e.0 += 1;
        e.1 += it.finding.logical_size;
        e.2 += it.finding.on_disk_size;
        *e.3.entry(it.finding.category.key().to_string()).or_insert(0) += 1;
        // Only a *permanent purge* of a red-tagged item is dangerous. The spec
        // says quarantine needs no such confirmation (it is restorable), so the
        // warning must key off the disposition, not just the risk tier.
        if it.disposition == Disposition::Purge && it.finding.category.risk() == RiskTier::UserData {
            e.4 = true;
        }
    }

    // Purge first: it is the irreversible one and deserves the eye. Releasing a
    // local copy goes last - it is the only group here that destroys nothing.
    let order = ["purge", "quarantine", "dehydrate"];
    order
        .iter()
        .filter_map(|k| {
            groups.get(*k).map(|(count, logical, on_disk, cats, red)| {
                let disposition = match *k {
                    "purge" => Disposition::Purge,
                    "quarantine" => Disposition::Quarantine,
                    _ => Disposition::Dehydrate,
                };
                PreviewGroup {
                    disposition,
                    item_count: *count,
                    logical_bytes: *logical,
                    on_disk_bytes: *on_disk,
                    category_counts: cats.clone(),
                    contains_red_purge: *red,
                }
            })
        })
        .collect()
}

/// Whether a user-assigned importance level forbids a disposition.
///
/// The level is re-read from the store rather than trusted from the request.
/// The frontend ships each row's level across IPC, and a modified client could
/// relabel a Critical file as Low to get it purged. Category and importance are
/// both server-side truth; only the *choice* of disposition comes from the
/// client, and this is the gate that keeps that choice inside the user's own
/// stated intent.
///
/// Critical blocks everything, including quarantine and cloud backup: the user
/// said this matters, and moving the only accessible copy to a restorable-but-
/// remote store is still a change they did not ask for. High blocks only the
/// irreversible action; quarantine stays available because it can be undone.
pub fn importance_block(
    level: crate::importance::Importance,
    disposition: Disposition,
) -> Option<&'static str> {
    use crate::importance::Importance;
    match (level, disposition) {
        (Importance::Critical, _) => {
            Some("marked critical: this path must be left alone")
        }
        (Importance::High, Disposition::Purge) => {
            Some("marked important: only restorable actions are allowed")
        }
        _ => None,
    }
}

/// True when the selection needs the red-tag confirmation box to be ticked.
pub fn needs_red_confirmation(items: &[SelectedItem]) -> bool {
    // Risk is derived from category, never trusted from the client: the
    // finding's `risk` field is shipped across IPC and could be relabelled to
    // skip the confirmation gate. Category is the server-side truth about what
    // kind of data something is.
    items
        .iter()
        .any(|i| i.disposition == Disposition::Purge && i.finding.category.risk() == RiskTier::UserData)
}

pub fn run_cleanup(req: CleanupRequest, cfg: &crate::config::Config) -> CleanupReport {
    // Read the user's annotations once per run rather than per item: the store
    // is a JSON file on disk, and a batch of ten thousand findings would
    // otherwise re-parse it ten thousand times.
    let marks = crate::importance::Store::load();
    run_cleanup_with_marks(req, cfg, &marks)
}

/// The pipeline proper, with the annotation set injected.
///
/// Split out so the safety gate can be tested against a store built in memory.
/// Loading the real one would make the tests read and depend on the machine's
/// `%APPDATA%`, which is exactly the kind of hidden coupling the deletion
/// layer is required not to have.
pub fn run_cleanup_with_marks(
    req: CleanupRequest,
    cfg: &crate::config::Config,
    marks: &crate::importance::Store,
) -> CleanupReport {
    let mut report = CleanupReport::default();

    let mut to_purge: Vec<&SelectedItem> = Vec::new();
    let mut to_quarantine: Vec<&SelectedItem> = Vec::new();
    let mut to_dehydrate: Vec<&SelectedItem> = Vec::new();
    for item in &req.items {
        match item.disposition {
            Disposition::Purge => to_purge.push(item),
            Disposition::Quarantine => to_quarantine.push(item),
            Disposition::Dehydrate => to_dehydrate.push(item),
        }
    }

    // Red-tagged items headed for permanent deletion need explicit consent.
    if needs_red_confirmation(&req.items) && !req.confirmed_red_purge {
        let mut remaining = Vec::new();
        for item in to_purge {
            if item.finding.category.risk() == RiskTier::UserData {
                crate::log::record(
                    crate::log::Action::Purge,
                    &item.finding.path,
                    item.finding.on_disk_size,
                    "已阻止：标红项目未获二次确认",
                );
                report.blocked.push(BlockedItem {
                    path: item.finding.path.clone(),
                    reason: "red-tagged item needs explicit confirmation before permanent deletion".into(),
                });
            } else {
                remaining.push(item);
            }
        }
        to_purge = remaining;
    }

    for item in &to_purge {
        let path = Path::new(&item.finding.path);
        // Deleting a placeholder inside a sync root deletes the cloud file
        // too, so a cloud-backed finding must never be purged - not even if a
        // modified client asks for it. The only action it has is release.
        if item.finding.category.is_cloud() {
            crate::log::record(
                crate::log::Action::Purge,
                &item.finding.path,
                item.finding.on_disk_size,
                "已阻止：云端文件不能被永久删除",
            );
            report.blocked.push(BlockedItem {
                path: item.finding.path.clone(),
                reason: "cloud-backed items cannot be deleted; only their local copy can be released".into(),
            });
            continue;
        }
        if !crate::safety::is_allowed(path, cfg) {
            crate::log::record(
                crate::log::Action::Purge,
                &item.finding.path,
                item.finding.on_disk_size,
                "已阻止：路径在保护名单中",
            );
            report.blocked.push(BlockedItem {
                path: item.finding.path.clone(),
                reason: "path is on the protected list".into(),
            });
            continue;
        }
        // Re-check the age gate now, not just at scan time: an item that was
        // old enough when scanned may have been touched since, and purge is
        // irreversible. (Quarantine is restorable, so it is deliberately not
        // gated here.)
        if !crate::safety::age_gate_passes(path, item.finding.is_dir, item.finding.category, cfg) {
            crate::log::record(
                crate::log::Action::Purge,
                &item.finding.path,
                item.finding.on_disk_size,
                "已阻止：项目最近仍在使用，未过年龄门槛",
            );
            report.blocked.push(BlockedItem {
                path: item.finding.path.clone(),
                reason: "item is newer than the age gate allows; re-scan to apply".into(),
            });
            continue;
        }
        // The user's own judgement outranks every scanner rule. A path they
        // marked Critical or High must not be purged even if a rule surfaced
        // it, and the check happens here rather than in the UI so it holds for
        // an elevated replay too (which has no UI to consult).
        if let Some(level) = marks.get(path) {
            if let Some(reason) = importance_block(level, Disposition::Purge) {
                crate::log::record(
                    crate::log::Action::Purge,
                    &item.finding.path,
                    item.finding.on_disk_size,
                    "已阻止：用户标记的重要级别不允许永久删除",
                );
                report.blocked.push(BlockedItem {
                    path: item.finding.path.clone(),
                    reason: reason.to_string(),
                });
                continue;
            }
        }
        match crate::quarantine::ops::purge(path, item.finding.is_dir) {
            Ok(()) => {
                crate::log::record_disposition(
                    Disposition::Purge,
                    &item.finding.path,
                    item.finding.on_disk_size,
                    true,
                );
                report.purged.push(item.finding.path.clone());
                report.logical_purged += item.finding.logical_size;
                report.on_disk_purged += item.finding.on_disk_size;
            }
            Err(e) => {
                crate::log::record_disposition(
                    Disposition::Purge,
                    &item.finding.path,
                    item.finding.on_disk_size,
                    false,
                );
                report.failed.push(BlockedItem {
                    path: item.finding.path.clone(),
                    reason: e,
                });
            }
        }
    }

    // The denylist applies to quarantine exactly as it does to purge: the spec
    // says "never-delete / never-quarantine". A misfiring rule that surfaces a
    // protected path must be blocked here too, or it round-trips into the
    // quarantine and can later be destroyed by a purge of the batch.
    let mut to_move: Vec<&SelectedItem> = Vec::new();
    for item in to_quarantine {
        // Moving a placeholder into quarantine would take the cloud file with
        // it. Release is the only meaningful action for cloud-backed content.
        if item.finding.category.is_cloud() {
            crate::log::record(
                crate::log::Action::Quarantine,
                &item.finding.path,
                item.finding.on_disk_size,
                "已阻止：云端文件不能移入隔离区",
            );
            report.blocked.push(BlockedItem {
                path: item.finding.path.clone(),
                reason: "cloud-backed items cannot be quarantined; only their local copy can be released".into(),
            });
            continue;
        }
        if !crate::safety::is_allowed(Path::new(&item.finding.path), cfg) {
            crate::log::record(
                crate::log::Action::Quarantine,
                &item.finding.path,
                item.finding.on_disk_size,
                "已阻止：路径在保护名单中",
            );
            report.blocked.push(BlockedItem {
                path: item.finding.path.clone(),
                reason: "path is on the protected list".into(),
            });
            continue;
        }
        // Quarantine is restorable, so High is allowed through here; only
        // Critical stops it.
        if let Some(level) = marks.get(Path::new(&item.finding.path)) {
            if let Some(reason) = importance_block(level, Disposition::Quarantine) {
                crate::log::record(
                    crate::log::Action::Quarantine,
                    &item.finding.path,
                    item.finding.on_disk_size,
                    "已阻止：用户标记为关键，不允许任何清理动作",
                );
                report.blocked.push(BlockedItem {
                    path: item.finding.path.clone(),
                    reason: reason.to_string(),
                });
                continue;
            }
        }
        to_move.push(item);
    }
    let qfindings: Vec<Finding> = to_move.iter().map(|i| i.finding.clone()).collect();
    if !qfindings.is_empty() {
        match crate::quarantine::ops::quarantine_items(&qfindings, cfg) {
            Ok(qr) => {
                for item in &to_move {
                    if qr.moved.contains(&item.finding.path) {
                        crate::log::record_disposition(
                            Disposition::Quarantine,
                            &item.finding.path,
                            item.finding.on_disk_size,
                            true,
                        );
                    }
                }
                report.quarantined = qr.moved;
                report.quarantine_batch = Some(qr.batch_id);
                for f in qr.failed {
                    crate::log::record_disposition(
                        Disposition::Quarantine,
                        &f.path,
                        0,
                        false,
                    );
                    report.failed.push(BlockedItem { path: f.path, reason: f.reason });
                }
            }
            Err(e) => {
                for item in &to_move {
                    crate::log::record_disposition(
                        Disposition::Quarantine,
                        &item.finding.path,
                        item.finding.on_disk_size,
                        false,
                    );
                    report.failed.push(BlockedItem {
                        path: item.finding.path.clone(),
                        reason: e.clone(),
                    });
                }
            }
        }
    }

    // Releasing local copies: the last group, and the only one that destroys
    // nothing. Two gates apply - the path denylist shared with every other
    // action, and the OneDrive-root check, which keeps the attribute rewrite
    // away from ordinary files even if a scanner rule misfires.
    for item in &to_dehydrate {
        let path = Path::new(&item.finding.path);
        if !crate::safety::is_allowed(path, cfg) {
            crate::log::record(
                crate::log::Action::Dehydrate,
                &item.finding.path,
                item.finding.on_disk_size,
                "已阻止：路径在保护名单中",
            );
            report.blocked.push(BlockedItem {
                path: item.finding.path.clone(),
                reason: "path is on the protected list".into(),
            });
            continue;
        }
        if !crate::cloud::onedrive::is_in_onedrive(path) {
            crate::log::record(
                crate::log::Action::Dehydrate,
                &item.finding.path,
                item.finding.on_disk_size,
                "已阻止：不在 OneDrive 同步目录内",
            );
            report.blocked.push(BlockedItem {
                path: item.finding.path.clone(),
                reason: "path is not inside a OneDrive folder".into(),
            });
            continue;
        }
        if let Some(level) = marks.get(path) {
            if let Some(reason) = importance_block(level, Disposition::Dehydrate) {
                crate::log::record(
                    crate::log::Action::Dehydrate,
                    &item.finding.path,
                    item.finding.on_disk_size,
                    "已阻止：用户标记的重要级别不允许释放",
                );
                report.blocked.push(BlockedItem {
                    path: item.finding.path.clone(),
                    reason: reason.to_string(),
                });
                continue;
            }
        }

        match crate::cloud::onedrive::release_local_copies(path) {
            Ok(r) => {
                // Logged as a success even when some files were skipped: the
                // per-file failures are reported separately below, and the
                // bytes recorded are the ones actually requested.
                crate::log::record_disposition(
                    Disposition::Dehydrate,
                    &item.finding.path,
                    r.requested_bytes,
                    r.failed.is_empty(),
                );
                report.dehydrated.push(item.finding.path.clone());
                report.dehydrate_requested_bytes += r.requested_bytes;
                report.dehydrate_pinned_files += r.pinned_files;
                for f in r.failed {
                    report.failed.push(BlockedItem { path: f.path, reason: f.reason });
                }
            }
            Err(e) => {
                crate::log::record_disposition(
                    Disposition::Dehydrate,
                    &item.finding.path,
                    0,
                    false,
                );
                report.failed.push(BlockedItem {
                    path: item.finding.path.clone(),
                    reason: e,
                });
            }
        }
    }

    // Let the UI offer elevation when the only thing standing in the way was
    // permissions. Anything else is a genuine failure to report as-is.
    report.needs_elevation = report
        .failed
        .iter()
        .any(|f| is_permission_failure(&f.reason));

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgeBasis, Category};

    fn item(path: &str, category: Category, disposition: Disposition) -> SelectedItem {
        let risk = category.risk();
        SelectedItem {
            finding: Finding {
                id: uuid::Uuid::new_v4().to_string(),
                rule_id: "test".into(),
                name: path.rsplit('\\').next().unwrap_or(path).to_string(),
                path: path.to_string(),
                is_dir: false,
                logical_size: 100,
                on_disk_size: 50,
                last_access_unix: 0,
                created_unix: 0,
                age_days: 0,
                age_basis: AgeBasis::Creation,
                category,
                risk,
                disposition,
                child_count: 0,
            },
            disposition,
        }
    }

    #[test]
    fn preview_groups_purge_before_quarantine() {
        let items = vec![
            item("C:/Temp/a.log", Category::TempJunk, Disposition::Purge),
            item("C:/x/node_modules", Category::DevArtifact, Disposition::Quarantine),
            item("C:/y/movie.mkv", Category::LargeFile, Disposition::Quarantine),
        ];
        let groups = build_preview(&items);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].disposition, Disposition::Purge);
        assert_eq!(groups[0].item_count, 1);
        assert_eq!(groups[1].item_count, 2);
    }

    #[test]
    fn red_purge_requires_confirmation() {
        let items = vec![item("C:/AppData/OldApp", Category::AppLeftover, Disposition::Purge)];
        assert!(needs_red_confirmation(&items));

        let cfg = crate::config::Config::default();
        // Without confirmation the item is blocked, not deleted.
        let report = run_cleanup_with_marks(
            CleanupRequest { items: items.clone(), confirmed_red_purge: false },
            &cfg,
            &crate::importance::Store::default(),
        );
        assert_eq!(report.purged.len(), 0);
        assert_eq!(report.blocked.len(), 1);
    }

    #[test]
    fn permission_failures_are_recognised() {
        assert!(is_permission_failure("Access is denied. (os error 5)"));
        assert!(is_permission_failure("拒绝访问。"));
        assert!(is_permission_failure("操作需要更高的权限"));
        assert!(!is_permission_failure("disk is full"));
        assert!(!is_permission_failure("the file is in use by another process"));
    }

    #[test]
    fn a_clean_run_does_not_ask_for_elevation() {
        let dir = std::env::temp_dir().join(format!("sr-elev-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().to_string();

        let cfg = crate::config::Config::default();
        let report = run_cleanup_with_marks(
            CleanupRequest {
                items: vec![item(&path, Category::TempJunk, Disposition::Purge)],
                confirmed_red_purge: true,
            },
            &cfg,
            &crate::importance::Store::default(),
        );
        assert_eq!(report.failed.len(), 0);
        assert!(
            !report.needs_elevation,
            "an unprivileged but successful purge must not prompt for UAC"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn protected_paths_are_blocked_even_when_purge_is_requested() {
        let windir = std::env::var("SystemRoot").unwrap();
        let target = format!("{}\\System32\\drivers\\etc\\hosts", windir.trim_end_matches('\\'));
        let items = vec![item(&target, Category::LargeFile, Disposition::Purge)];
        let cfg = crate::config::Config::default();
        let report = run_cleanup_with_marks(
            CleanupRequest { items, confirmed_red_purge: true },
            &cfg,
            &crate::importance::Store::default(),
        );
        assert_eq!(report.purged.len(), 0);
        assert_eq!(report.blocked.len(), 1);
        assert!(report.blocked[0].reason.contains("protected"));
    }

    // ---------- importance gate ----------
    //
    // These drive `run_cleanup_with_marks` with a store built in memory. Using
    // the real entry point would make the result depend on whatever the
    // developer running the suite happens to have marked in `%APPDATA%`.

    fn marked(path: &str, level: crate::importance::Importance) -> crate::importance::Store {
        let mut s = crate::importance::Store::default();
        s.set(std::path::Path::new(path), level);
        s
    }

    /// The headline guarantee: a path the user marked Critical survives even
    /// when the client explicitly asks for a permanent purge.
    #[test]
    fn a_critical_mark_survives_a_requested_purge() {
        let dir = std::env::temp_dir().join(format!("sr-gate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("thesis.docx");
        std::fs::write(&target, b"the only copy").unwrap();
        let path = target.to_string_lossy().to_string();

        let marks = marked(&path, crate::importance::Importance::Critical);
        let cfg = crate::config::Config::default();
        let report = run_cleanup_with_marks(
            CleanupRequest {
                items: vec![item(&path, Category::TempJunk, Disposition::Purge)],
                confirmed_red_purge: true,
            },
            &cfg,
            &marks,
        );

        assert_eq!(report.purged.len(), 0, "a Critical item was purged");
        assert_eq!(report.blocked.len(), 1);
        assert!(
            target.exists(),
            "the file behind a Critical mark must still be on disk"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// High means "restorable actions only": quarantine is allowed, purge is
    /// not. Both halves are asserted, because a gate that blocks everything
    /// would pass the purge half while silently breaking the product.
    #[test]
    fn a_high_mark_blocks_purge_but_allows_quarantine() {
        let dir = std::env::temp_dir().join(format!("sr-gate-{}", uuid::Uuid::new_v4()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let file = src.join("notes.txt");
        std::fs::write(&file, b"keep me recoverable").unwrap();
        let path = file.to_string_lossy().to_string();
        let marks = marked(&path, crate::importance::Importance::High);

        let mut cfg = crate::config::Config::default();
        // Keep the batch out of the machine's real quarantine store: these
        // tests must not leave anything behind in ProgramData.
        cfg.quarantine_dir = dir.join("quarantine");

        let purged = run_cleanup_with_marks(
            CleanupRequest {
                items: vec![item(&path, Category::DevArtifact, Disposition::Purge)],
                confirmed_red_purge: true,
            },
            &cfg,
            &marks,
        );
        assert_eq!(purged.purged.len(), 0, "High must refuse permanent deletion");
        assert!(file.exists());
        assert_eq!(purged.blocked.len(), 1);

        let quarantined = run_cleanup_with_marks(
            CleanupRequest {
                items: vec![item(&path, Category::DevArtifact, Disposition::Quarantine)],
                confirmed_red_purge: true,
            },
            &cfg,
            &marks,
        );
        assert_eq!(quarantined.quarantined.len(), 1, "quarantine must still work");
        assert!(!file.exists(), "the file should have moved to quarantine");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The frontend ships each row's level across IPC. A client that relabels
    /// a Critical file as Low must not get it deleted: the gate re-reads the
    /// store, and the request's own level is not consulted at all.
    #[test]
    fn the_request_cannot_relabel_its_way_past_the_gate() {
        let dir = std::env::temp_dir().join(format!("sr-gate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("contract.pdf");
        std::fs::write(&target, b"legally binding").unwrap();
        let path = target.to_string_lossy().to_string();

        // The store says Critical. Nothing in the request contradicts it,
        // because nothing in the request is trusted.
        let marks = marked(&path, crate::importance::Importance::Critical);
        let cfg = crate::config::Config::default();
        let report = run_cleanup_with_marks(
            CleanupRequest {
                items: vec![item(&path, Category::TempJunk, Disposition::Purge)],
                confirmed_red_purge: true,
            },
            &cfg,
            &marks,
        );

        assert!(report.purged.is_empty());
        assert!(target.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_store_does_not_stand_in_the_way() {
        let dir = std::env::temp_dir().join(format!("sr-gate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("junk.tmp");
        std::fs::write(&target, b"junk").unwrap();
        let path = target.to_string_lossy().to_string();

        let cfg = crate::config::Config::default();
        let report = run_cleanup_with_marks(
            CleanupRequest {
                items: vec![item(&path, Category::TempJunk, Disposition::Purge)],
                confirmed_red_purge: true,
            },
            &cfg,
            &crate::importance::Store::default(),
        );

        assert_eq!(report.purged.len(), 1, "an unmarked temp file must still be purgeable");
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn importance_gate_matches_its_documented_matrix() {
        use crate::importance::Importance::*;
        // Critical stops both actions.
        assert!(importance_block(Critical, Disposition::Purge).is_some());
        assert!(importance_block(Critical, Disposition::Quarantine).is_some());
        // A Critical mark must also stop a release: pulling the only on-disk
        // copy of something the user called critical is a change they did not
        // ask for, however reversible it is in theory.
        assert!(importance_block(Critical, Disposition::Dehydrate).is_some());
        // High stops only the irreversible one.
        assert!(importance_block(High, Disposition::Purge).is_some());
        assert!(importance_block(High, Disposition::Quarantine).is_none());
        // Normal and Low never block.
        for level in [Normal, Low] {
            assert!(importance_block(level, Disposition::Purge).is_none());
            assert!(importance_block(level, Disposition::Quarantine).is_none());
        }
    }

    // ---------- cloud-backed content ----------

    /// Deleting a placeholder inside a sync root deletes the cloud file. A
    /// client asking for a purge of a cloud finding must be refused, not
    /// obeyed.
    #[test]
    fn a_cloud_finding_can_never_be_purged() {
        let dir = std::env::temp_dir().join(format!("sr-cloud-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("holiday-photos");
        std::fs::create_dir_all(&target).unwrap();
        let path = target.to_string_lossy().to_string();

        let cfg = crate::config::Config::default();
        let report = run_cleanup_with_marks(
            CleanupRequest {
                items: vec![item(&path, Category::CloudPlaceholder, Disposition::Purge)],
                confirmed_red_purge: true,
            },
            &cfg,
            &crate::importance::Store::default(),
        );

        assert!(report.purged.is_empty(), "a cloud item was purged");
        assert_eq!(report.blocked.len(), 1);
        assert!(target.exists(), "the folder behind the placeholder is gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cloud_finding_can_never_be_quarantined() {
        let dir = std::env::temp_dir().join(format!("sr-cloud-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("Documents");
        std::fs::create_dir_all(&target).unwrap();
        let path = target.to_string_lossy().to_string();

        let mut cfg = crate::config::Config::default();
        cfg.quarantine_dir = dir.join("quarantine");
        let report = run_cleanup_with_marks(
            CleanupRequest {
                items: vec![item(&path, Category::CloudPlaceholder, Disposition::Quarantine)],
                confirmed_red_purge: true,
            },
            &cfg,
            &crate::importance::Store::default(),
        );

        assert!(report.quarantined.is_empty());
        assert_eq!(report.blocked.len(), 1);
        assert!(target.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The OneDrive-root check is what keeps the attribute rewrite away from
    /// ordinary files. A dehydrate request aimed at a plain temp folder must be
    /// refused rather than silently reach into the filesystem attributes.
    #[test]
    fn dehydrating_outside_onedrive_is_refused() {
        let dir = std::env::temp_dir().join(format!("sr-cloud-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("not-a-sync-root");
        std::fs::create_dir_all(&target).unwrap();
        let path = target.to_string_lossy().to_string();

        let cfg = crate::config::Config::default();
        let report = run_cleanup_with_marks(
            CleanupRequest {
                items: vec![item(&path, Category::CloudPlaceholder, Disposition::Dehydrate)],
                confirmed_red_purge: true,
            },
            &cfg,
            &crate::importance::Store::default(),
        );

        assert!(report.dehydrated.is_empty());
        assert_eq!(report.blocked.len(), 1);
        assert!(
            report.blocked[0].reason.contains("not inside a OneDrive"),
            "unexpected reason: {}",
            report.blocked[0].reason
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn releasing_a_local_copy_needs_no_red_confirmation() {
        // CloudCopy is not UserData, so the destructive-action gate must not
        // fire. Requiring the red checkbox here would train the user to tick
        // it without reading, which is exactly what it guards.
        let items = vec![item("C:/Users/me/OneDrive/x", Category::CloudPlaceholder, Disposition::Dehydrate)];
        assert!(!needs_red_confirmation(&items));
    }
}
