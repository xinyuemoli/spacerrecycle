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
    /// Paths uploaded to OneDrive and then removed locally.
    pub backed_up: Vec<String>,
    /// Bytes confirmed uploaded before the local copies were removed.
    pub backup_bytes: u64,
    /// Paths whose upload failed and which were quarantined locally instead.
    /// Not a failure - the space was still reclaimed, nothing was destroyed -
    /// but the user asked for a cloud copy and did not get one.
    pub backup_fallbacks: Vec<String>,
    /// Set when the batch's files uploaded but its manifest did not. The files
    /// are safe in OneDrive; only in-app restore is affected.
    pub backup_manifest_failed: Option<String>,
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
            Disposition::CloudBackup => "cloud_backup",
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

    // Purge first: it is the irreversible one and deserves the eye. A cloud
    // backup ranks next - the local copy still goes away, it is just kept
    // somewhere else. Releasing a local copy comes last: it destroys nothing.
    let order = ["purge", "cloud_backup", "quarantine", "dehydrate"];
    order
        .iter()
        .filter_map(|k| {
            groups.get(*k).map(|(count, logical, on_disk, cats, red)| {
                let disposition = match *k {
                    "purge" => Disposition::Purge,
                    "quarantine" => Disposition::Quarantine,
                    "cloud_backup" => Disposition::CloudBackup,
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

/// Whether a path is a directory, re-read from the filesystem.
///
/// The request carries an `is_dir` captured at scan time, and the client could
/// have sent anything. A wrong answer fails safely in every call site (removing
/// a directory as a file, or the reverse, errors out), but a stat is cheap and
/// removes the question entirely - the deletion layer should not depend on the
/// caller's description of what it is pointing at.
fn is_dir_now(path: &Path, fallback: bool) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.is_dir())
        .unwrap_or(fallback)
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

/// Everything the pipeline needs to know about cloud backup.
///
/// Defaults to "unavailable": a caller that does not explicitly wire this up
/// gets no cloud backup, and any item that asks for one is blocked with a
/// reason rather than silently treated as something else.
#[derive(Default)]
pub struct CloudContext<'a> {
    /// Only subscribers may use [`Disposition::CloudBackup`].
    pub subscriber: bool,
    /// Where an upload goes. `None` means the feature cannot run here - for
    /// instance in the elevated helper process, which has no UI with which to
    /// complete a sign-in.
    pub sink: Option<&'a dyn crate::cloud::backup::BackupSink>,
    /// Folder the batch is written under. Generated when empty.
    pub batch_id: String,
}

impl CloudContext<'_> {
    fn batch(&self) -> String {
        if self.batch_id.is_empty() {
            format!("batch-{}", uuid::Uuid::new_v4())
        } else {
            self.batch_id.clone()
        }
    }
}

/// Run a cleanup with the annotations loaded from disk.
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
    run_cleanup_full(req, cfg, marks, &CloudContext::default())
}

/// The pipeline, with every external dependency injected.
pub fn run_cleanup_full(
    req: CleanupRequest,
    cfg: &crate::config::Config,
    marks: &crate::importance::Store,
    cloud: &CloudContext<'_>,
) -> CleanupReport {
    let mut report = CleanupReport::default();

    let mut to_purge: Vec<&SelectedItem> = Vec::new();
    let mut to_quarantine: Vec<&SelectedItem> = Vec::new();
    let mut to_dehydrate: Vec<&SelectedItem> = Vec::new();
    let mut to_backup: Vec<&SelectedItem> = Vec::new();
    for item in &req.items {
        match item.disposition {
            Disposition::Purge => to_purge.push(item),
            Disposition::Quarantine => to_quarantine.push(item),
            Disposition::Dehydrate => to_dehydrate.push(item),
            Disposition::CloudBackup => to_backup.push(item),
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
        if !crate::safety::age_gate_passes(path, is_dir_now(path, item.finding.is_dir), item.finding.category, cfg) {
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
        match crate::quarantine::ops::purge(path, is_dir_now(path, item.finding.is_dir)) {
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

    // Cloud backup: upload, confirm, and only then remove the local copy.
    //
    // An item that cannot be uploaded is never deleted. It is handed to the
    // quarantine pass below instead, so a network failure costs the user a
    // trip to the quarantine tab rather than their data.
    let mut backup_fallback: Vec<&SelectedItem> = Vec::new();
    if !to_backup.is_empty() {
        let batch = cloud.batch();
        // Resolved once so the manifest can be written after the loop.
        let sink_opt = cloud.sink;
        let mut manifest_items: Vec<crate::cloud::backup::ManifestEntry> = Vec::new();
        for item in &to_backup {
            let path = Path::new(&item.finding.path);

            if !item.finding.category.allows_cloud_backup() {
                // Uploading garbage to the user's cloud spends their quota to
                // preserve something they already decided to throw away.
                crate::log::record(
                    crate::log::Action::CloudBackup,
                    &item.finding.path,
                    item.finding.on_disk_size,
                    "已阻止：该类别不支持云端备份",
                );
                report.blocked.push(BlockedItem {
                    path: item.finding.path.clone(),
                    reason: "this category is not a meaningful cloud-backup target".into(),
                });
                continue;
            }
            // The subscription gate is enforced here, not in the UI: the
            // frontend ships the disposition across IPC and could ask for a
            // paid feature it has not paid for.
            if !cloud.subscriber {
                crate::log::record(
                    crate::log::Action::CloudBackup,
                    &item.finding.path,
                    item.finding.on_disk_size,
                    "已阻止：云端备份需要有效订阅",
                );
                report.blocked.push(BlockedItem {
                    path: item.finding.path.clone(),
                    reason: "cloud backup requires an active subscription".into(),
                });
                continue;
            }
            let Some(sink) = sink_opt else {
                crate::log::record(
                    crate::log::Action::CloudBackup,
                    &item.finding.path,
                    item.finding.on_disk_size,
                    "已阻止：此运行环境无法上传",
                );
                report.blocked.push(BlockedItem {
                    path: item.finding.path.clone(),
                    reason: "cloud backup is unavailable in this instance".into(),
                });
                continue;
            };
            if !crate::safety::is_allowed(path, cfg) {
                crate::log::record(
                    crate::log::Action::CloudBackup,
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
            if let Some(level) = marks.get(path) {
                if let Some(reason) = importance_block(level, Disposition::CloudBackup) {
                    crate::log::record(
                        crate::log::Action::CloudBackup,
                        &item.finding.path,
                        item.finding.on_disk_size,
                        "已阻止：用户标记的重要级别不允许",
                    );
                    report.blocked.push(BlockedItem {
                        path: item.finding.path.clone(),
                        reason: reason.to_string(),
                    });
                    continue;
                }
            }

            match sink.upload(path, is_dir_now(path, item.finding.is_dir), &batch) {
                Ok(outcome) => {
                    // The upload is confirmed. Only now may the local copy go.
                    match crate::quarantine::ops::purge(path, is_dir_now(path, item.finding.is_dir)) {
                        Ok(()) => {
                            crate::log::record_disposition(
                                Disposition::CloudBackup,
                                &item.finding.path,
                                outcome.bytes,
                                true,
                            );
                            report.backed_up.push(item.finding.path.clone());
                            report.backup_bytes += outcome.bytes;
                            // Recorded so the batch can be restored from
                            // anywhere, not just from this machine's memory of
                            // what it uploaded.
                            manifest_items.push(crate::cloud::backup::ManifestEntry {
                                original_path: item.finding.path.clone(),
                                name: path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default(),
                                is_dir: is_dir_now(path, item.finding.is_dir),
                                bytes: outcome.bytes,
                                files: outcome.files,
                                uploaded_unix: crate::license::now_unix(),
                            });
                        }
                        Err(e) => {
                            // The data is safe in the cloud, so this is not a
                            // loss - but the space was not reclaimed, and
                            // reporting success would claim otherwise.
                            crate::log::record_disposition(
                                Disposition::CloudBackup,
                                &item.finding.path,
                                0,
                                false,
                            );
                            report.failed.push(BlockedItem {
                                path: item.finding.path.clone(),
                                reason: format!(
                                    "uploaded to OneDrive, but the local copy could not be removed: {e}"
                                ),
                            });
                        }
                    }
                }
                Err(e) => {
                    crate::log::record_disposition(
                        Disposition::CloudBackup,
                        &item.finding.path,
                        0,
                        false,
                    );
                    // Hand it to the quarantine pass rather than leaving it
                    // where it was: the user asked for the space back, and a
                    // local quarantine gives it to them without risking the
                    // data.
                    report.backup_fallbacks.push(item.finding.path.clone());
                    backup_fallback.push(item);
                    let _ = e;
                }
            }
        }

        // The manifest goes last, once every upload has finished. A batch with
        // files but no manifest is visibly incomplete, which is the honest
        // state for an interrupted run.
        if let Some(sink) = sink_opt {
            if !manifest_items.is_empty() {
                if let Err(e) = sink.finish_batch(&batch, &manifest_items) {
                    // Nothing is lost - the files are uploaded - but the batch
                    // cannot be restored from inside the app, and saying
                    // nothing would leave the user to find that out later.
                    crate::log::record(
                        crate::log::Action::CloudBackup,
                        &batch,
                        0,
                        "备份清单写入失败，该批次需在 OneDrive 中手动取回",
                    );
                    report.backup_manifest_failed = Some(e);
                }
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
    // Items whose cloud upload failed are quarantined instead. They were already
    // checked against the denylist in the cloud pass, so they skip it here.
    to_move.extend(backup_fallback);

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

    // ---------- cloud backup ----------

    /// Records what it was asked to upload, and can be made to fail.
    #[derive(Default)]
    struct FakeSink {
        calls: std::sync::Mutex<Vec<(String, bool, String)>>,
        /// Whether the source still existed at the moment of upload. This is
        /// the evidence for the module's central invariant.
        existed_at_upload: std::sync::Mutex<Vec<bool>>,
        /// How many items each batch manifest was asked to record.
        manifests: std::sync::Mutex<Vec<usize>>,
        fail: bool,
        bytes: u64,
    }

    impl FakeSink {
        fn ok() -> Self {
            FakeSink { bytes: 1024, ..Default::default() }
        }
        fn failing() -> Self {
            FakeSink { fail: true, ..Default::default() }
        }
        fn calls(&self) -> Vec<(String, bool, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl crate::cloud::backup::BackupSink for FakeSink {
        fn upload(
            &self,
            path: &Path,
            is_dir: bool,
            batch: &str,
        ) -> Result<crate::cloud::backup::UploadOutcome, String> {
            // Recorded *before* anything else: if the engine ever deleted the
            // file first, this is what catches it.
            self.existed_at_upload
                .lock()
                .unwrap()
                .push(path.exists());
            self.calls
                .lock()
                .unwrap()
                .push((path.to_string_lossy().to_string(), is_dir, batch.to_string()));
            if self.fail {
                Err("simulated upload failure".to_string())
            } else {
                Ok(crate::cloud::backup::UploadOutcome {
                    bytes: self.bytes,
                    files: vec![crate::cloud::backup::ManifestFile {
                        relative: "f".to_string(),
                        size: self.bytes,
                    }],
                })
            }
        }
        fn finish_batch(
            &self,
            _batch: &str,
            _items: &[crate::cloud::backup::ManifestEntry],
        ) -> Result<(), String> {
            self.manifests.lock().unwrap().push(_items.len());
            Ok(())
        }
        fn describe(&self) -> String {
            "fake".to_string()
        }
    }

    fn scratch(tag: &str) -> (std::path::PathBuf, crate::config::Config) {
        let dir = std::env::temp_dir().join(format!("sr-cb-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = crate::config::Config::default();
        cfg.quarantine_dir = dir.join("quarantine");
        (dir, cfg)
    }

    /// The headline invariant: the local copy is removed only after the upload
    /// has been confirmed.
    #[test]
    fn a_cloud_backup_uploads_before_it_removes_anything() {
        let (dir, cfg) = scratch("order");
        let target = dir.join("project");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("a.txt"), b"data").unwrap();
        let path = target.to_string_lossy().to_string();

        let sink = FakeSink::ok();
        let cloud = CloudContext {
            subscriber: true,
            sink: Some(&sink),
            batch_id: "batch-1".to_string(),
        };
        let report = run_cleanup_full(
            CleanupRequest {
                items: vec![item(&path, Category::DevArtifact, Disposition::CloudBackup)],
                confirmed_red_purge: false,
            },
            &cfg,
            &crate::importance::Store::default(),
            &cloud,
        );

        assert_eq!(report.backed_up.len(), 1, "the item was not backed up");
        assert_eq!(report.backup_bytes, 1024);
        assert!(!target.exists(), "the local copy survived a confirmed upload");
        assert_eq!(
            sink.existed_at_upload.lock().unwrap().as_slice(),
            &[true],
            "the file was gone before the upload started"
        );
        // The fixture claims `is_dir: false` for a directory, which is exactly the
        // lie a modified client could send. The engine re-reads it, so the sink
        // must be told `true`.
        assert_eq!(sink.calls(), vec![(path, true, "batch-1".to_string())]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failed upload must never delete. The item is quarantined locally
    /// instead: the user still gets the space back, and nothing is destroyed.
    #[test]
    fn a_failed_upload_quarantines_instead_of_deleting() {
        let (dir, cfg) = scratch("fallback");
        let target = dir.join("photos");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("a.jpg"), b"image").unwrap();
        let path = target.to_string_lossy().to_string();

        let sink = FakeSink::failing();
        let cloud = CloudContext {
            subscriber: true,
            sink: Some(&sink),
            batch_id: "batch-1".to_string(),
        };
        let report = run_cleanup_full(
            CleanupRequest {
                items: vec![item(&path, Category::LargeFile, Disposition::CloudBackup)],
                confirmed_red_purge: false,
            },
            &cfg,
            &crate::importance::Store::default(),
            &cloud,
        );

        assert!(report.backed_up.is_empty());
        assert_eq!(report.backup_fallbacks.len(), 1, "the failure was not reported");
        assert_eq!(report.quarantined.len(), 1, "the item was not quarantined");
        assert!(!target.exists(), "the item should have moved to quarantine");
        assert!(
            cfg.quarantine_dir.exists(),
            "nothing was actually quarantined"
        );
        assert!(
            report.failed.is_empty(),
            "a handled fallback must not also be reported as a failure"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The subscription gate is enforced server-side: a client asking for a
    /// paid feature it has not paid for gets a refusal, not the feature.
    #[test]
    fn a_non_subscriber_cannot_use_cloud_backup() {
        let (dir, cfg) = scratch("gate");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("stuff");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("f"), b"x").unwrap();
        let path = target.to_string_lossy().to_string();

        let sink = FakeSink::ok();
        let cloud = CloudContext {
            subscriber: false,
            sink: Some(&sink),
            batch_id: "b".to_string(),
        };
        let report = run_cleanup_full(
            CleanupRequest {
                items: vec![item(&path, Category::DevArtifact, Disposition::CloudBackup)],
                confirmed_red_purge: false,
            },
            &cfg,
            &crate::importance::Store::default(),
            &cloud,
        );

        assert!(report.backed_up.is_empty());
        assert_eq!(report.blocked.len(), 1);
        assert!(report.blocked[0].reason.contains("subscription"), "{:?}", report.blocked[0].reason);
        assert!(target.exists(), "a refused item was removed anyway");
        assert!(sink.calls().is_empty(), "the upload was attempted without a subscription");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With nowhere to upload to, the feature refuses rather than silently
    /// degrading into a delete.
    #[test]
    fn cloud_backup_without_a_sink_refuses_rather_than_deleting() {
        let (dir, cfg) = scratch("nosink");
        let target = dir.join("stuff");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("f"), b"x").unwrap();
        let path = target.to_string_lossy().to_string();

        let cloud = CloudContext { subscriber: true, sink: None, batch_id: "b".into() };
        let report = run_cleanup_full(
            CleanupRequest {
                items: vec![item(&path, Category::DevArtifact, Disposition::CloudBackup)],
                confirmed_red_purge: false,
            },
            &cfg,
            &crate::importance::Store::default(),
            &cloud,
        );

        assert!(report.blocked.len() == 1);
        assert!(report.blocked[0].reason.contains("unavailable"), "{:?}", report.blocked[0]);
        assert!(target.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporary_junk_is_not_a_cloud_backup_target() {
        let (dir, cfg) = scratch("junk");
        let target = dir.join("junk.tmp");
        std::fs::write(&target, b"x").unwrap();
        let path = target.to_string_lossy().to_string();

        let sink = FakeSink::ok();
        let cloud = CloudContext { subscriber: true, sink: Some(&sink), batch_id: "b".into() };
        let report = run_cleanup_full(
            CleanupRequest {
                items: vec![item(&path, Category::TempJunk, Disposition::CloudBackup)],
                confirmed_red_purge: false,
            },
            &cfg,
            &crate::importance::Store::default(),
            &cloud,
        );

        assert!(report.backed_up.is_empty());
        assert_eq!(report.blocked.len(), 1);
        assert!(target.exists());
        assert!(sink.calls().is_empty(), "garbage was uploaded to the user's cloud");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Critical mark blocks every action, including this one.
    #[test]
    fn a_critical_mark_blocks_a_cloud_backup() {
        let (dir, cfg) = scratch("crit");
        let target = dir.join("thesis");
        std::fs::create_dir_all(&target).unwrap();
        let path = target.to_string_lossy().to_string();

        let sink = FakeSink::ok();
        let cloud = CloudContext { subscriber: true, sink: Some(&sink), batch_id: "b".into() };
        let marks = marked(&path, crate::importance::Importance::Critical);
        let report = run_cleanup_full(
            CleanupRequest {
                items: vec![item(&path, Category::LargeFile, Disposition::CloudBackup)],
                confirmed_red_purge: false,
            },
            &cfg,
            &marks,
            &cloud,
        );

        assert!(report.backed_up.is_empty());
        assert_eq!(report.blocked.len(), 1);
        assert!(target.exists());
        assert!(sink.calls().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// High means "restorable actions only", and a confirmed upload is
    /// restorable - so this must be allowed through.
    #[test]
    fn a_high_mark_allows_a_cloud_backup() {
        let (dir, cfg) = scratch("high");
        let target = dir.join("contracts");
        std::fs::create_dir_all(&target).unwrap();
        let path = target.to_string_lossy().to_string();

        let sink = FakeSink::ok();
        let cloud = CloudContext { subscriber: true, sink: Some(&sink), batch_id: "b".into() };
        let marks = marked(&path, crate::importance::Importance::High);
        let report = run_cleanup_full(
            CleanupRequest {
                items: vec![item(&path, Category::LargeFile, Disposition::CloudBackup)],
                confirmed_red_purge: false,
            },
            &cfg,
            &marks,
            &cloud,
        );

        assert_eq!(report.backed_up.len(), 1, "High should permit a restorable action");
        assert!(report.blocked.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cloud_backup_needs_no_red_confirmation() {
        // The local copy goes, but a confirmed copy exists in the cloud, so
        // this is not one of the irreversible actions the checkbox guards.
        let items = vec![item("C:/x/big.mkv", Category::LargeFile, Disposition::CloudBackup)];
        assert!(!needs_red_confirmation(&items));

        let (dir, cfg) = scratch("red");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("leftover");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("f"), b"x").unwrap();
        let path = p.to_string_lossy().to_string();

        let sink = FakeSink::ok();
        let cloud = CloudContext { subscriber: true, sink: Some(&sink), batch_id: "b".into() };
        let report = run_cleanup_full(
            CleanupRequest {
                items: vec![item(&path, Category::AppLeftover, Disposition::CloudBackup)],
                // Deliberately not confirmed: a red-tagged item must still be
                // allowed through the cloud-backup path.
                confirmed_red_purge: false,
            },
            &cfg,
            &crate::importance::Store::default(),
            &cloud,
        );
        assert_eq!(report.backed_up.len(), 1);
        assert!(report.blocked.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The elevated helper has no UI to complete a sign-in, so a staged
    /// request that contains a cloud backup must refuse it rather than fall
    /// back to deleting.
    #[test]
    fn the_default_context_refuses_cloud_backup() {
        let (dir, cfg) = scratch("default");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("x");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("f"), b"x").unwrap();
        let path = target.to_string_lossy().to_string();

        let report = run_cleanup_full(
            CleanupRequest {
                items: vec![item(&path, Category::DevArtifact, Disposition::CloudBackup)],
                confirmed_red_purge: false,
            },
            &cfg,
            &crate::importance::Store::default(),
            &CloudContext::default(),
        );

        assert!(report.backed_up.is_empty());
        assert_eq!(report.blocked.len(), 1);
        assert!(target.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A missing batch id must still produce one folder for the whole run, or
    /// a batch would scatter across the cloud.
    #[test]
    fn an_unspecified_batch_still_groups_the_run() {
        let (dir, cfg) = scratch("batch");
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let sink = FakeSink::ok();
        let cloud = CloudContext { subscriber: true, sink: Some(&sink), batch_id: String::new() };

        let report = run_cleanup_full(
            CleanupRequest {
                items: vec![
                    item(&a.to_string_lossy(), Category::DevArtifact, Disposition::CloudBackup),
                    item(&b.to_string_lossy(), Category::DevArtifact, Disposition::CloudBackup),
                ],
                confirmed_red_purge: false,
            },
            &cfg,
            &crate::importance::Store::default(),
            &cloud,
        );

        assert_eq!(report.backed_up.len(), 2);
        let calls = sink.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].2, calls[1].2, "the batch was split across folders");
        assert!(calls[0].2.starts_with("batch-"), "unexpected batch id: {}", calls[0].2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
