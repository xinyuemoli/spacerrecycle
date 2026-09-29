use super::manifest::{Manifest, ManifestEntry};
use crate::model::Finding;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    Skip,
    Overwrite,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuarantineReport {
    pub batch_id: String,
    pub moved: Vec<String>,
    pub failed: Vec<FailedItem>,
    pub logical_moved: u64,
    pub on_disk_moved: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedItem {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub restored: Vec<String>,
    pub skipped: Vec<FailedItem>,
    pub failed: Vec<FailedItem>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PurgeReport {
    pub purged: Vec<String>,
    pub failed: Vec<FailedItem>,
}

/// Move a set of findings into a new compressed batch. Items that fail to
/// move (locked, permission denied) are reported rather than silently dropped.
pub fn quarantine_items(findings: &[Finding], cfg: &crate::config::Config) -> Result<QuarantineReport, String> {
    if findings.is_empty() {
        return Ok(QuarantineReport::default());
    }
    let root = &cfg.quarantine_dir;
    std::fs::create_dir_all(root).map_err(|e| format!("cannot create quarantine: {e}"))?;

    let batch_id = format!("batch-{}", uuid::Uuid::new_v4());
    let batch_dir = root.join(&batch_id);
    std::fs::create_dir_all(&batch_dir).map_err(|e| format!("cannot create batch dir: {e}"))?;

    // Turn on NTFS compression for the batch. Children inherit it. If the
    // volume is not NTFS we simply store uncompressed; restore is unaffected.
    let compressed = crate::winapi::ntfs::set_directory_compression(&batch_dir);

    let mut manifest = Manifest {
        batch_id: batch_id.clone(),
        created_unix: crate::scanner::walk::now_unix(),
        entries: Vec::new(),
    };
    let mut report = QuarantineReport {
        batch_id: batch_id.clone(),
        ..Default::default()
    };

    for (i, f) in findings.iter().enumerate() {
        let src = PathBuf::from(&f.path);
        let stored_rel = format!("{:03}", i);
        let dst = batch_dir.join(&stored_rel);

        match move_path(&src, &dst, f.is_dir) {
            Ok(()) => {
                // Actually compress the moved content. Setting the directory's
                // compressed attribute only marks it so *new* files inherit the
                // flag; a `rename` keeps the item's original (uncompressed)
                // layout. Per-file FSCTL_SET_COMPRESSION is what `compact /c /s`
                // does underneath, and it is what realises the quarantine's
                // "on-disk << logical" saving.
                crate::winapi::ntfs::compress_tree(&dst);
                manifest.entries.push(ManifestEntry {
                    original_path: src.clone(),
                    stored_rel,
                    logical_size: f.logical_size,
                    on_disk_size: f.on_disk_size,
                    is_dir: f.is_dir,
                    rule_id: f.rule_id.clone(),
                    quarantined_unix: crate::scanner::walk::now_unix(),
                });
                report.moved.push(f.path.clone());
                report.logical_moved += f.logical_size;
                report.on_disk_moved += f.on_disk_size;
            }
            Err(e) => report.failed.push(FailedItem {
                path: f.path.clone(),
                reason: e,
            }),
        }
    }

    if manifest.entries.is_empty() {
        // Nothing landed; don't leave an empty batch behind.
        let _ = std::fs::remove_dir(&batch_dir);
        return Err("nothing could be moved into the quarantine".into());
    }

    manifest
        .save(&batch_dir)
        .map_err(|e| format!("cannot write manifest: {e}"))?;

    if !compressed {
        report.failed.push(FailedItem {
            path: batch_dir.to_string_lossy().to_string(),
            reason: "volume is not NTFS; items were stored uncompressed".into(),
        });
    }
    Ok(report)
}

/// Move a file or directory, creating the destination's parent as needed.
fn move_path(src: &Path, dst: &Path, is_dir: bool) -> Result<(), String> {
    if !src.exists() {
        return Err("source no longer exists".into());
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }
    if dst.exists() {
        let _ = remove_path(dst, is_dir);
    }
    match std::fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Cross-volume moves fail on rename; fall back to copy + delete.
            copy_then_delete(src, dst, is_dir)
        }
    }
}

fn copy_then_delete(src: &Path, dst: &Path, is_dir: bool) -> Result<(), String> {
    if is_dir {
        copy_dir_all(src, dst)?;
    } else {
        std::fs::copy(src, dst).map_err(|e| format!("copy failed: {e}"))?;
    }
    // rename() cannot cross volumes, so this fallback runs whenever the
    // quarantine lives on a different disk from the item. The copied tree
    // arrives uncompressed; the caller compresses it afterwards via
    // `compress_tree`, because SetFileAttributes only affects files created
    // after the attribute is set, not ones already present.
    remove_path(src, is_dir).map_err(|e| format!("copied but could not remove source: {e}"))?;
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir failed: {e}"))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read_dir failed: {e}"))? {
        let entry = entry.map_err(|e| format!("entry failed: {e}"))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            copy_dir_all(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| format!("copy failed: {e}"))?;
        }
    }
    Ok(())
}

fn remove_path(p: &Path, is_dir: bool) -> Result<(), String> {
    if is_dir || p.is_dir() {
        std::fs::remove_dir_all(p).map_err(|e| format!("remove_dir_all failed: {e}"))
    } else {
        std::fs::remove_file(p).map_err(|e| format!("remove_file failed: {e}"))
    }
}

/// Permanently delete a path. This is irreversible; callers must have already
/// routed the action through the preview dialog.
pub fn purge(path: &Path, is_dir: bool) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    remove_path(path, is_dir)
}

pub fn purge_batch(batch_dir: &Path) -> PurgeReport {
    let mut report = PurgeReport::default();
    let mut survivors: Vec<ManifestEntry> = Vec::new();

    if let Some(m) = Manifest::load(batch_dir) {
        for e in &m.entries {
            let stored = batch_dir.join(&e.stored_rel);
            let original = e.original_path.to_string_lossy().to_string();
            match purge(&stored, e.is_dir) {
                Ok(()) => report.purged.push(original),
                Err(err) => {
                    report.failed.push(FailedItem {
                        path: original.clone(),
                        reason: err,
                    });
                    // Keep the entry so the data stays listed and restorable.
                    survivors.push(e.clone());
                }
            }
        }
    } else {
        report.failed.push(FailedItem {
            path: batch_dir.to_string_lossy().to_string(),
            reason: "batch manifest is missing or unreadable".into(),
        });
    }

    if survivors.is_empty() {
        // Nothing left: drop the batch directory and its manifest.
        if let Err(e) = std::fs::remove_dir_all(batch_dir) {
            if report.purged.is_empty() {
                report.failed.push(FailedItem {
                    path: batch_dir.to_string_lossy().to_string(),
                    reason: format!("could not remove batch dir: {e}"),
                });
            }
        }
    } else {
        // At least one item could not be deleted. Rewriting the manifest is
        // what keeps the rest recoverable - removing the directory here would
        // orphan data the user can no longer restore.
        let updated = Manifest {
            batch_id: batch_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            created_unix: crate::scanner::walk::now_unix(),
            entries: survivors,
        };
        let _ = updated.save(batch_dir);
    }
    report
}

/// Restore every item in a batch to its original location.
pub fn restore_batch(batch_dir: &Path, policy: ConflictPolicy) -> Result<RestoreReport, String> {
    let manifest = Manifest::load(batch_dir).ok_or("batch manifest is missing or unreadable")?;
    let mut report = RestoreReport::default();
    let mut remaining: Vec<ManifestEntry> = Vec::new();

    for entry in &manifest.entries {
        let stored = batch_dir.join(&entry.stored_rel);
        let target = entry.original_path.clone();

        if !stored.exists() {
            report.failed.push(FailedItem {
                path: entry.original_path.to_string_lossy().to_string(),
                reason: "stored data is missing".into(),
            });
            remaining.push(entry.clone());
            continue;
        }
        if target.exists() {
            match policy {
                ConflictPolicy::Skip => {
                    report.skipped.push(FailedItem {
                        path: target.to_string_lossy().to_string(),
                        reason: "destination already exists".into(),
                    });
                    remaining.push(entry.clone());
                    continue;
                }
                ConflictPolicy::Overwrite => {
                    if let Err(e) = remove_path(&target, entry.is_dir) {
                        report.failed.push(FailedItem {
                            path: target.to_string_lossy().to_string(),
                            reason: format!("could not overwrite: {e}"),
                        });
                        remaining.push(entry.clone());
                        continue;
                    }
                }
            }
        }
        match move_path(&stored, &target, entry.is_dir) {
            Ok(()) => report.restored.push(entry.original_path.to_string_lossy().to_string()),
            Err(e) => {
                report.failed.push(FailedItem {
                    path: entry.original_path.to_string_lossy().to_string(),
                    reason: e,
                });
                remaining.push(entry.clone());
            }
        }
    }

    if remaining.is_empty() {
        let _ = std::fs::remove_dir_all(batch_dir);
    } else {
        let updated = Manifest {
            batch_id: manifest.batch_id,
            created_unix: manifest.created_unix,
            entries: remaining,
        };
        let _ = updated.save(batch_dir);
    }
    Ok(report)
}

/// Restore a single item, leaving the rest of the batch intact.
pub fn restore_item(batch_dir: &Path, original_path: &Path, policy: ConflictPolicy) -> Result<(), String> {
    let mut manifest = Manifest::load(batch_dir).ok_or("batch manifest is missing or unreadable")?;
    let idx = manifest
        .entries
        .iter()
        .position(|e| e.original_path == original_path)
        .ok_or("item not found in this batch")?;
    let entry = manifest.entries[idx].clone();
    let stored = batch_dir.join(&entry.stored_rel);

    if !stored.exists() {
        return Err("stored data is missing".into());
    }
    if original_path.exists() {
        match policy {
            ConflictPolicy::Skip => return Err("destination already exists".to_string()),
            ConflictPolicy::Overwrite => remove_path(original_path, entry.is_dir)?,
        }
    }
    move_path(&stored, original_path, entry.is_dir)?;

    manifest.entries.remove(idx);
    if manifest.entries.is_empty() {
        let _ = std::fs::remove_dir_all(batch_dir);
    } else {
        manifest.save(batch_dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quarantine::list_batches;
    use crate::model::{AgeBasis, Category, Disposition, RiskTier};

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sr-test-{}-{}", tag, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn finding(path: &Path, is_dir: bool) -> Finding {
        Finding {
            id: uuid::Uuid::new_v4().to_string(),
            rule_id: "test".into(),
            name: path.file_name().unwrap().to_string_lossy().to_string(),
            path: path.to_string_lossy().to_string(),
            is_dir,
            logical_size: 0,
            on_disk_size: 0,
            last_access_unix: 0,
            created_unix: 0,
            age_days: 0,
            age_basis: AgeBasis::Creation,
            category: Category::DevArtifact,
            risk: RiskTier::Rebuildable,
            disposition: Disposition::Quarantine,
            child_count: 0,
        }
    }

    #[test]
    fn quarantine_then_restore_round_trips() {
        let src_root = temp_dir("src");
        let q_root = temp_dir("q");
        let file = src_root.join("hello.txt");
        std::fs::write(&file, b"payload").unwrap();

        let cfg = crate::config::Config {
            quarantine_dir: q_root.clone(),
            ..Default::default()
        };
        let items = vec![finding(&file, false)];
        let report = quarantine_items(&items, &cfg).unwrap();
        assert_eq!(report.moved.len(), 1);
        assert!(!file.exists(), "source should be gone after quarantine");

        let batches = list_batches(&q_root);
        assert_eq!(batches.len(), 1);

        let batch_dir = q_root.join(&report.batch_id);
        let restored = restore_batch(&batch_dir, ConflictPolicy::Skip).unwrap();
        assert_eq!(restored.restored.len(), 1);
        assert!(file.exists(), "file should be back at its original path");
        assert_eq!(std::fs::read(&file).unwrap(), b"payload");

        let _ = std::fs::remove_dir_all(&src_root);
        let _ = std::fs::remove_dir_all(&q_root);
    }

    #[test]
    fn purge_batch_keeps_the_manifest_when_an_item_survives() {
        // Regression: purge_batch used to call remove_dir_all unconditionally,
        // so a single locked file meant the manifest was destroyed too and the
        // still-quarantined data could never be restored.
        let src_root = temp_dir("purgesrc");
        let q_root = temp_dir("purgeq");
        let keep = src_root.join("keeper.bin");
        let gone = src_root.join("goner.bin");
        std::fs::write(&keep, b"still here").unwrap();
        std::fs::write(&gone, b"delete me").unwrap();

        let cfg = crate::config::Config {
            quarantine_dir: q_root.clone(),
            ..Default::default()
        };
        let report = quarantine_items(&[finding(&keep, false), finding(&gone, false)], &cfg).unwrap();
        let batch_dir = q_root.join(&report.batch_id);

        // Make one stored item undeletable. std::fs::File::open shares read
        // access on Windows, which still permits deletion, so this needs a
        // handle opened without FILE_SHARE_DELETE.
        let stored_gone = batch_dir.join("001");
        let locked = crate::winapi::ntfs::open_exclusive(&stored_gone);
        assert!(locked.is_some(), "could not lock the fixture file");
        let res = purge_batch(&batch_dir);

        assert!(
            !res.purged.is_empty(),
            "the deletable item should still have been purged"
        );
        drop(locked);
        let _ = std::fs::remove_file(&stored_gone);

        // Whatever survived must remain restorable, which means the manifest
        // has to still be there.
        let survivors = list_batches(&q_root);
        assert_eq!(
            survivors.len(),
            1,
            "a batch with undeleted data must stay listed, otherwise the data is orphaned"
        );
        assert!(
            Manifest::load(&batch_dir).is_some(),
            "the manifest must survive so the remaining item can still be restored"
        );

        let _ = std::fs::remove_dir_all(&src_root);
        let _ = std::fs::remove_dir_all(&q_root);
    }

    #[test]
    fn restore_skip_leaves_conflicting_item_quarantined() {
        let src_root = temp_dir("src2");
        let q_root = temp_dir("q2");
        let file = src_root.join("conflict.txt");
        std::fs::write(&file, b"old").unwrap();

        let cfg = crate::config::Config {
            quarantine_dir: q_root.clone(),
            ..Default::default()
        };
        let report = quarantine_items(&[finding(&file, false)], &cfg).unwrap();
        let batch_dir = q_root.join(&report.batch_id);

        // Something else now occupies the original path.
        std::fs::write(&file, b"new").unwrap();

        let restored = restore_batch(&batch_dir, ConflictPolicy::Skip).unwrap();
        assert_eq!(restored.restored.len(), 0);
        assert_eq!(restored.skipped.len(), 1);
        assert_eq!(std::fs::read(&file).unwrap(), b"new", "new file must be untouched");

        let _ = std::fs::remove_dir_all(&src_root);
        let _ = std::fs::remove_dir_all(&q_root);
    }

    #[test]
    fn restore_overwrite_replaces_the_conflicting_file() {
        let src_root = temp_dir("src3");
        let q_root = temp_dir("q3");
        let file = src_root.join("conflict2.txt");
        std::fs::write(&file, b"old").unwrap();

        let cfg = crate::config::Config {
            quarantine_dir: q_root.clone(),
            ..Default::default()
        };
        let report = quarantine_items(&[finding(&file, false)], &cfg).unwrap();
        let batch_dir = q_root.join(&report.batch_id);
        std::fs::write(&file, b"new").unwrap();

        let restored = restore_batch(&batch_dir, ConflictPolicy::Overwrite).unwrap();
        assert_eq!(restored.restored.len(), 1);
        assert_eq!(std::fs::read(&file).unwrap(), b"old");

        let _ = std::fs::remove_dir_all(&src_root);
        let _ = std::fs::remove_dir_all(&q_root);
    }

    #[test]
    fn purging_a_batch_removes_everything() {
        let src_root = temp_dir("src4");
        let q_root = temp_dir("q4");
        let file = src_root.join("bye.txt");
        std::fs::write(&file, b"data").unwrap();

        let cfg = crate::config::Config {
            quarantine_dir: q_root.clone(),
            ..Default::default()
        };
        let report = quarantine_items(&[finding(&file, false)], &cfg).unwrap();
        let batch_dir = q_root.join(&report.batch_id);

        let purged = purge_batch(&batch_dir);
        assert_eq!(purged.purged.len(), 1);
        assert!(!batch_dir.exists());
        assert!(!file.exists());

        let _ = std::fs::remove_dir_all(&src_root);
        let _ = std::fs::remove_dir_all(&q_root);
    }
}
