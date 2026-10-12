//! OneDrive local-copy scanner.
//!
//! Reports one finding per *top-level* folder inside each OneDrive root,
//! carrying the bytes that folder is holding on this disk. Not one finding per
//! file: an OneDrive root routinely holds hundreds of thousands of files, and a
//! list that long is not a decision aid, it is a second disk listing. Explorer
//! and OneDrive's own UI both present "release this folder" as the unit of
//! work, and that is the unit offered here.

use crate::cloud::onedrive;
use crate::model::{AgeBasis, Category, Disposition, Finding};
use crate::scanner::{ScanCtx, Scanner};
use std::path::Path;

/// Rule id recorded on every finding and in the audit log.
const RULE_ID: &str = "onedrive_local_copies";

pub struct CloudScanner;

impl Scanner for CloudScanner {
    fn name(&self) -> &'static str {
        "cloud_placeholder"
    }

    fn scan(&self, ctx: &ScanCtx) -> Vec<Finding> {
        let mut out = Vec::new();

        for root in onedrive::roots() {
            if ctx.counters.cancelled() {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&root) else {
                // A sync root that cannot be listed (permissions, a stale
                // environment variable) is skipped rather than aborting the
                // scan: one unreadable root must not hide the others.
                ctx.counters
                    .skipped
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                continue;
            };

            for entry in entries.filter_map(|e| e.ok()) {
                if ctx.counters.cancelled() {
                    break;
                }
                let path = entry.path();
                // Hidden sync metadata (`.8494108a...`, `desktop.ini`) is not
                // user content and has nothing worth releasing.
                if is_internal(&path) {
                    continue;
                }
                let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
                if meta.file_type().is_symlink() {
                    continue;
                }

                let bucket = onedrive::measure_bucket(ctx, &path);
                // Cancelled mid-walk: the totals are a partial sum, and a
                // wrong number is worse than no number.
                if bucket.truncated {
                    continue;
                }
                // A folder holding nothing but cloud-only placeholders has
                // already given its space back, and one holding nothing but
                // pinned files is exactly what the user asked to keep.
                if !bucket.is_actionable() {
                    continue;
                }

                if let Some(f) = build(ctx, &path, meta.is_dir(), &bucket) {
                    out.push(f);
                }
            }
        }

        out
    }
}

/// OneDrive's own bookkeeping files, which live beside user content.
fn is_internal(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) else {
        return true;
    };
    name.starts_with('.') || name == "desktop.ini" || name == "thumbs.db"
}

fn build(
    ctx: &ScanCtx,
    path: &Path,
    _is_dir: bool,
    bucket: &onedrive::Bucket,
) -> Option<Finding> {
    let (last_access, created) = crate::scanner::walk::timestamps(path);
    // Same reasoning as the other scanners: a directory's own access stamp is
    // rewritten by the enumeration that just happened, so the newest file
    // modification time in the tree is the honest "last touched" answer.
    let newest = if bucket.newest_mtime > 0 {
        bucket.newest_mtime
    } else {
        last_access
    };
    let (age_days, age_basis): (u32, AgeBasis) = crate::scanner::walk::age_of(last_access, created);
    let age_days = if bucket.newest_mtime > 0 {
        crate::scanner::walk::age_of(0, bucket.newest_mtime).0
    } else {
        age_days
    };

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    ctx.counters
        .findings
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    Some(Finding {
        id: uuid::Uuid::new_v4().to_string(),
        rule_id: RULE_ID.to_string(),
        name,
        path: path.to_string_lossy().to_string(),
        is_dir: std::fs::symlink_metadata(path).map(|m| m.is_dir()).unwrap_or(false),
        logical_size: bucket.logical_bytes,
        on_disk_size: bucket.reclaimable_bytes,
        last_access_unix: newest,
        created_unix: created,
        age_days,
        age_basis,
        category: Category::CloudPlaceholder,
        risk: Category::CloudPlaceholder.risk(),
        disposition: Disposition::Dehydrate,
        // How many files would actually be released, so the UI can say
        // "12,431 files" rather than only a byte count.
        child_count: bucket.reclaimable_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::Arc;

    fn ctx() -> ScanCtx {
        ScanCtx::new(Arc::new(crate::scanner::ScanCounters::default()), &Config::default())
    }

    #[test]
    fn one_drive_bookkeeping_files_are_ignored() {
        assert!(is_internal(Path::new("C:\\OneDrive\\desktop.ini")));
        assert!(is_internal(Path::new("C:\\OneDrive\\.8494108")));
        assert!(is_internal(Path::new("C:\\OneDrive\\Thumbs.db")));
        assert!(!is_internal(Path::new("C:\\OneDrive\\Documents")));
    }

    /// The scanner must produce nothing on a machine with no OneDrive root
    /// rather than inventing findings or failing the whole scan.
    #[test]
    fn no_onedrive_roots_means_no_findings() {
        let c = ctx();
        let out = CloudScanner.scan(&c);
        if !onedrive::is_available() {
            assert!(out.is_empty(), "findings appeared with no OneDrive root");
        }
    }

    #[test]
    fn a_cancelled_scan_reports_nothing() {
        let c = ctx();
        c.counters.request_cancel();
        let out = CloudScanner.scan(&c);
        assert!(
            out.is_empty(),
            "a cancelled cloud scan must not report partial folders"
        );
    }
}