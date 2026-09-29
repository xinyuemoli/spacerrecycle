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

    // Purge group first: it is the irreversible one and deserves the eye.
    let order = ["purge", "quarantine"];
    order
        .iter()
        .filter_map(|k| {
            groups.get(*k).map(|(count, logical, on_disk, cats, red)| {
                let disposition = if *k == "purge" {
                    Disposition::Purge
                } else {
                    Disposition::Quarantine
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
    let mut report = CleanupReport::default();

    let mut to_purge: Vec<&SelectedItem> = Vec::new();
    let mut to_quarantine: Vec<&SelectedItem> = Vec::new();
    for item in &req.items {
        match item.disposition {
            Disposition::Purge => to_purge.push(item),
            Disposition::Quarantine => to_quarantine.push(item),
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
        if crate::safety::is_allowed(Path::new(&item.finding.path), cfg) {
            to_move.push(item);
        } else {
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
        }
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
        let report = run_cleanup(
            CleanupRequest { items: items.clone(), confirmed_red_purge: false },
            &cfg,
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
        let report = run_cleanup(
            CleanupRequest {
                items: vec![item(&path, Category::TempJunk, Disposition::Purge)],
                confirmed_red_purge: true,
            },
            &cfg,
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
        let report = run_cleanup(
            CleanupRequest { items, confirmed_red_purge: true },
            &cfg,
        );
        assert_eq!(report.purged.len(), 0);
        assert_eq!(report.blocked.len(), 1);
        assert!(report.blocked[0].reason.contains("protected"));
    }
}
