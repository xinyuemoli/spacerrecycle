use super::{ScanCtx, Scanner};
use crate::model::{Category, Finding};
use rayon::prelude::*;
use std::path::Path;
use std::sync::atomic::Ordering;

/// Drives that get scanned come from the volume enumerator, so a machine
/// with a G: or an M: is scanned rather than reported as full by the UI and
/// then silently skipped here.

pub struct LargeFileScanner;

impl Scanner for LargeFileScanner {
    fn name(&self) -> &'static str {
        "Large files"
    }

    fn scan(&self, ctx: &ScanCtx) -> Vec<Finding> {
        let cfg: &crate::config::Config = ctx.config.as_ref();
        let mut out: Vec<Finding> = Vec::new();

        // Each drive walks independently, and the per-file work is parallelised
        // across cores. A whole-volume walk is the single most expensive thing
        // this app does, so it must not be single-threaded.
        let roots = crate::winapi::volume::scannable_roots();
        let per_drive: Vec<Vec<Finding>> = roots
            .par_iter()
            // A cancel between two volumes should not start the next walk.
            .filter(|_| !ctx.counters.cancelled())
            .map(|root| scan_drive(ctx, root, cfg))
            .collect();

        for v in per_drive {
            out.extend(v);
        }
        out.sort_by(|a, b| b.on_disk_size.cmp(&a.on_disk_size));
        out
    }
}

/// Scan one directory tree for large files.
///
/// Takes an explicit root so it can be exercised against a temp tree in tests
/// rather than only against a whole volume.
pub fn scan_drive(ctx: &ScanCtx, root: &Path, cfg: &crate::config::Config) -> Vec<Finding> {
    use std::os::windows::fs::MetadataExt;

    // walkdir, not jwalk: it reads one directory per step with no
    // read-ahead, so a cancel takes effect at the next directory boundary.
    // jwalk buffers the whole tree on its own thread pool and nothing
    // downstream can stop it - measured at 43k entries per second still being
    // produced with the cancel flag already set, which left the UI blocked for
    // minutes on a whole-volume walk.
    // The per-file work is still spread across cores, so a cancel only waits
    // for the batch in flight rather than the whole volume.
    let results: Vec<Finding> = walkdir::WalkDir::new(root)
        .follow_links(false)   // a junction must not lead us into a cycle
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .take_while(|_| !ctx.counters.cancelled())
        .par_bridge()
        .filter_map(|e| -> Option<Finding> {
            if ctx.counters.cancelled() {
                return None;
            }
            let Ok(md) = e.metadata() else { return None };
            // The deep scan walks the whole volume. Count every file examined so
            // the "已检查 X 个文件" readout keeps advancing; otherwise it stays
            // frozen at the figure produced by the cheap targeted scanners while
            // the walk grinds on invisibly, which reads as a hang.
            ctx.counters.dirs.fetch_add(1, Ordering::Relaxed);
            if md.len() < ctx.large_file_min_bytes {
                return None;
            }
            let p = e.path();
            if !crate::safety::is_allowed(&p, cfg) {
                return None;
            }
            // MetadataExt yields raw FILETIME, which must be converted to
            // Unix seconds before it means anything as a timestamp.
            let la = super::walk::filetime_to_unix(md.last_access_time() as u64);
            let cr = super::walk::filetime_to_unix(md.creation_time() as u64);
            let (age_days, age_basis) = super::walk::age_of(la, cr);
            if age_days < ctx.large_file_age_days {
                return None;
            }
            let logical = md.len();
            let on_disk = crate::winapi::ntfs::on_disk_size(&e.path()).unwrap_or(logical);
            ctx.counters.bytes.fetch_add(logical, Ordering::Relaxed);
            ctx.counters.findings.fetch_add(1, Ordering::Relaxed);
            Some(Finding {
                id: uuid::Uuid::new_v4().to_string(),
                rule_id: "large.file".into(),
                name: e.file_name().to_string_lossy().to_string(),
                path: p.to_string_lossy().to_string(),
                is_dir: false,
                logical_size: logical,
                on_disk_size: on_disk,
                last_access_unix: la,
                created_unix: cr,
                age_days,
                age_basis,
                category: Category::LargeFile,
                risk: Category::LargeFile.risk(),
                disposition: Category::LargeFile.default_disposition(),
                child_count: 0,
            })
        })
        .collect();

    // par_bridge collects in completion order, not size order. Sort here so
    // the guarantee holds for every caller, not just the top-level scan.
    let mut results = results;
    results.sort_by(|a, b| b.on_disk_size.cmp(&a.on_disk_size));
    results
}
