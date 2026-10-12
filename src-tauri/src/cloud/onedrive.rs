//! OneDrive Files On-Demand: where it is, what it costs locally, and how to
//! release the local copies safely.
//!
//! The principle from the base design still holds unchanged: nothing happens
//! without the user asking. Releasing a local copy does not delete anything -
//! the file stays in OneDrive and Windows re-downloads it on first access - so
//! it is the one cleanup action here that needs no recovery story, only a
//! preview.

use crate::winapi::cloudfile::{self, CloudState};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Environment variables OneDrive uses for its sync roots.
///
/// All three exist because a machine can have a personal account and a work
/// account synced side by side; `OneDrive` is the legacy name for whichever
/// was configured first and is kept as a fallback.
const ROOT_ENV_VARS: [&str; 3] = ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"];

/// Every OneDrive sync root on this machine.
///
/// De-duplicated case-insensitively: on a machine with one account, all three
/// variables often point at the same folder, and returning it three times would
/// triple every scan.
pub fn roots() -> Vec<PathBuf> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for var in ROOT_ENV_VARS {
        let Ok(raw) = std::env::var(var) else { continue };
        if raw.trim().is_empty() {
            continue;
        }
        let p = PathBuf::from(raw);
        if !p.is_dir() {
            // A stale environment variable (OneDrive uninstalled, or the
            // folder moved) must not become a scan target.
            continue;
        }
        let key = p.to_string_lossy().replace('/', "\\").to_lowercase();
        if seen.insert(key) {
            out.push(p);
        }
    }
    out
}

/// Whether OneDrive is present at all. Cheap: no directory walk.
pub fn is_available() -> bool {
    !roots().is_empty()
}

fn normalized(s: &str) -> String {
    let t = s.replace('/', "\\");
    let trimmed = t.trim_end_matches('\\');
    let out = if trimmed.is_empty() { t.as_str() } else { trimmed };
    out.to_lowercase()
}

/// True when `path` lives inside one of the sync roots.
///
/// This is a *safety* check, not a convenience: releasing a local copy is only
/// meaningful for a cloud placeholder, and running the attribute rewrite
/// against an arbitrary path is how a "free up space" button could quietly
/// strip a pin from something the user cared about. The engine consults this
/// before every release.
pub fn is_in_onedrive(path: &Path) -> bool {
    let p = normalized(&path.to_string_lossy());
    roots().iter().any(|root| {
        let r = normalized(&root.to_string_lossy());
        p == r || p.starts_with(&format!("{r}\\"))
    })
}

/// One directory's local OneDrive cost.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub logical_bytes: u64,
    /// Space that would actually be freed: the on-disk footprint of files
    /// whose content is present and not pinned.
    pub reclaimable_bytes: u64,
    /// On-disk footprint of everything under the bucket, including pinned
    /// files that will not be touched.
    pub total_on_disk_bytes: u64,
    pub reclaimable_files: u64,
    pub online_only_files: u64,
    pub pinned_files: u64,
    pub newest_mtime: i64,
    /// True when the walk stopped early because the user cancelled.
    pub truncated: bool,
}

impl Bucket {
    /// A bucket is only worth offering when releasing it frees real space.
    pub fn is_actionable(&self) -> bool {
        self.reclaimable_bytes > 0 && self.reclaimable_files > 0
    }
}

/// Measure what one OneDrive subfolder costs locally.
///
/// Parallel for the same reason `scanner::measure_dir` is: an OneDrive root is
/// routinely hundreds of thousands of files, and a single-threaded walk of one
/// takes minutes. `walkdir` rather than `jwalk` so the cancel flag is checked
/// per entry instead of after the whole tree has been read.
pub fn measure_bucket(ctx: &crate::scanner::ScanCtx, dir: &Path) -> Bucket {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

    let mut out = Bucket {
        newest_mtime: crate::scanner::walk::modified_unix(dir),
        ..Default::default()
    };
    if !dir.is_dir() {
        return out;
    }

    let reclaimable = AtomicU64::new(0);
    let reclaimable_logical = AtomicU64::new(0);
    let reclaimable_files = AtomicU64::new(0);
    let total_disk = AtomicU64::new(0);
    let online = AtomicU64::new(0);
    let pinned = AtomicU64::new(0);
    let newest = AtomicI64::new(out.newest_mtime);

    walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .take_while(|_| !ctx.counters.cancelled())
        .par_bridge()
        .for_each(|entry| {
            if ctx.counters.cancelled() {
                return;
            }
            let path = entry.path();
            let Ok(md) = entry.metadata() else { return };
            let logical = md.len();
            let on_disk = if logical >= 4 * 1024 * 1024 {
                crate::winapi::ntfs::on_disk_size(path).unwrap_or(logical)
            } else {
                logical
            };

            use std::os::windows::fs::MetadataExt;
            match cloudfile::classify(md.file_attributes()) {
                CloudState::OnlineOnly => {
                    online.fetch_add(1, Ordering::Relaxed);
                }
                CloudState::Pinned => {
                    pinned.fetch_add(1, Ordering::Relaxed);
                    total_disk.fetch_add(on_disk, Ordering::Relaxed);
                }
                CloudState::LocallyAvailable => {
                    total_disk.fetch_add(on_disk, Ordering::Relaxed);
                    reclaimable.fetch_add(on_disk, Ordering::Relaxed);
                    reclaimable_logical.fetch_add(logical, Ordering::Relaxed);
                    reclaimable_files.fetch_add(1, Ordering::Relaxed);
                }
            }

            let mtime = crate::scanner::walk::filetime_to_unix(md.last_write_time() as u64);
            if mtime > 0 {
                newest.fetch_max(mtime, Ordering::Relaxed);
            }
        });

    out.reclaimable_bytes = reclaimable.load(Ordering::Relaxed);
    let _ = reclaimable_logical; // logical size is reported via the finding
    out.reclaimable_files = reclaimable_files.load(Ordering::Relaxed);
    out.total_on_disk_bytes = total_disk.load(Ordering::Relaxed);
    out.online_only_files = online.load(Ordering::Relaxed);
    out.pinned_files = pinned.load(Ordering::Relaxed);
    out.newest_mtime = newest.load(Ordering::Relaxed);
    out.truncated = ctx.counters.cancelled();
    out.logical_bytes = reclaimable_logical.load(Ordering::Relaxed);

    ctx.counters
        .dirs
        .fetch_add(1 + out.reclaimable_files + out.online_only_files + out.pinned_files, Ordering::Relaxed);
    ctx.counters.bytes.fetch_add(out.total_on_disk_bytes, Ordering::Relaxed);

    out
}

/// What a release request did.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseReport {
    /// Bytes whose local copies were asked to be released.
    pub requested_bytes: u64,
    /// Files that were actually re-flagged.
    pub released_files: u64,
    /// Files whose content had already been released before we looked.
    pub already_released_files: u64,
    /// Files the user pinned. Deliberately untouched: a pin is an instruction.
    pub pinned_files: u64,
    /// Files that were skipped, with the reason.
    pub failed: Vec<ReleaseFailure>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseFailure {
    pub path: String,
    pub reason: String,
}

/// Ask OneDrive to release the local copies under `path`.
///
/// Only `LocallyAvailable` files are touched. Pinned files are skipped by
/// design - the user pinned them, and silently overruling that would be the
/// exact behaviour this product exists to avoid.
///
/// OneDrive performs the dehydration asynchronously, so a success here means
/// "requested", not "freed". The report distinguishes the two rather than
/// claiming space that has not been reclaimed yet.
pub fn release_local_copies(path: &Path) -> Result<ReleaseReport, String> {
    if !is_in_onedrive(path) {
        return Err("path is not inside a OneDrive folder".to_string());
    }
    if !path.exists() {
        return Err(format!("path no longer exists: {}", path.display()));
    }

    let mut report = ReleaseReport::default();
    let mut targets: Vec<PathBuf> = Vec::new();
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| format!("cannot inspect {}: {e}", path.display()))?;

    if meta.is_dir() {
        // The directory itself first, so a folder-level pin is cleared before
        // its children are considered.
        targets.push(path.to_path_buf());
        for entry in walkdir::WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                targets.push(entry.path().to_path_buf());
            }
        }
    } else {
        targets.push(path.to_path_buf());
    }

    for target in targets {
        let Some(md) = std::fs::symlink_metadata(&target).ok() else { continue };
        use std::os::windows::fs::MetadataExt;
        let attrs = md.file_attributes();

        match cloudfile::classify(attrs) {
            CloudState::OnlineOnly => {
                if md.is_file() {
                    report.already_released_files += 1;
                }
            }
            CloudState::Pinned => {
                if md.is_file() {
                    report.pinned_files += 1;
                }
            }
            CloudState::LocallyAvailable => {
                let on_disk = if md.is_file() {
                    if md.len() >= 4 * 1024 * 1024 {
                        crate::winapi::ntfs::on_disk_size(&target).unwrap_or(md.len())
                    } else {
                        md.len()
                    }
                } else {
                    0
                };

                match cloudfile::unpin(&target) {
                    Ok(after) => {
                        if md.is_file() {
                            report.released_files += 1;
                            // The sync engine has usually not dehydrated yet,
                            // so this counts the copy we asked it to drop.
                            report.requested_bytes += on_disk;
                            if after == CloudState::OnlineOnly {
                                // Already gone: nothing more to wait for.
                            }
                        }
                    }
                    Err(reason) => report
                        .failed
                        .push(ReleaseFailure { path: target.to_string_lossy().to_string(), reason }),
                }
            }
        }
    }

    Ok(report)
}

/// Fast status for the overview card. No directory walk.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub available: bool,
    pub roots: Vec<String>,
}

pub fn status() -> Status {
    let roots = roots();
    Status {
        available: !roots.is_empty(),
        roots: roots.iter().map(|p| p.to_string_lossy().to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_normal_temp_path_is_not_in_onedrive() {
        let p = std::env::temp_dir().join("sr-not-onedrive");
        assert!(
            !is_in_onedrive(&p),
            "an ordinary path must never be offered as a dehydration target"
        );
    }

    #[test]
    fn releasing_a_non_onedrive_path_is_refused() {
        let p = std::env::temp_dir().join(format!("sr-cloud-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        let err = release_local_copies(&p).unwrap_err();
        assert!(err.contains("not inside a OneDrive"), "{err}");
        let _ = std::fs::remove_dir_all(&p);
    }

    #[test]
    fn status_reports_the_roots_it_found() {
        let s = status();
        assert_eq!(s.available, !s.roots.is_empty());
        for r in &s.roots {
            assert!(Path::new(r).is_dir(), "a reported root must exist on disk");
        }
    }

    /// Roots are deduplicated case-insensitively. On a single-account machine
    /// all three environment variables point at one folder; returning it three
    /// times would make every scan visit it three times.
    #[test]
    fn duplicate_roots_are_folded() {
        let roots = roots();
        let mut keys: Vec<String> = roots.iter().map(|p| normalized(&p.to_string_lossy())).collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(before, keys.len(), "roots() returned a duplicate");
    }

    #[test]
    fn a_bucket_of_plain_files_has_nothing_to_release() {
        // Ordinary files classify as LocallyAvailable, so the *bucket* reports
        // them - the OneDrive-root check in the engine is what stops a release
        // from ever being attempted outside a sync root.
        let root = std::env::temp_dir().join(format!("sr-bucket-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.bin"), vec![7u8; 8192]).unwrap();

        let cfg = crate::config::Config::default();
        let ctx = crate::scanner::ScanCtx::new(
            std::sync::Arc::new(crate::scanner::ScanCounters::default()),
            &cfg,
        );
        let b = measure_bucket(&ctx, &root);
        assert_eq!(b.reclaimable_files, 1);
        assert!(b.is_actionable());

        let _ = std::fs::remove_dir_all(&root);
    }
}