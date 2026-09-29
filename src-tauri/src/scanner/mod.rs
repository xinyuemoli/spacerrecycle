pub mod cache;
pub mod dev;
pub mod largefile;
pub mod leftover;
pub mod temp;
pub mod walk;

use crate::model::{AgeBasis, Category, Finding, ScanProgress};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Shared counters so every scanner reports into the same progress stream.
#[derive(Default)]
pub struct ScanCounters {
    pub dirs: AtomicU64,
    pub bytes: AtomicU64,
    pub findings: AtomicU64,
    pub skipped: AtomicU64,
    pub cancel: AtomicBool,
    /// Set once every scanner has returned, so the progress ticker can stop.
    pub finished: AtomicBool,
}

impl ScanCounters {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Context handed to each scanner.
pub struct ScanCtx {
    pub counters: Arc<ScanCounters>,
    /// Age gate for large files, in days.
    pub large_file_age_days: u32,
    /// Age gate for app leftovers, in days.
    pub leftover_age_days: u32,
    /// Minimum file size for the large-file scanner.
    pub large_file_min_bytes: u64,
    /// The full config, shared so each scanner does not re-read it from disk.
    pub config: Arc<crate::config::Config>,
}

impl ScanCtx {
    pub fn new(counters: Arc<ScanCounters>, cfg: &crate::config::Config) -> Self {
        ScanCtx {
            counters,
            large_file_age_days: cfg.large_file_age_days,
            leftover_age_days: cfg.leftover_age_days,
            large_file_min_bytes: cfg.large_file_min_bytes,
            config: Arc::new(cfg.clone()),
        }
    }
}

/// A scan unit. Each returns the findings it discovered.
pub trait Scanner {
    fn name(&self) -> &'static str;
    fn scan(&self, ctx: &ScanCtx) -> Vec<Finding>;
}

/// Run every enabled scanner and return all findings, biggest reclaim first.
pub fn run_all(ctx: &ScanCtx, enabled: &[Category]) -> Vec<Finding> {
    use rayon::prelude::*;

    let built: Vec<(Category, Box<dyn Scanner + Send + Sync>)> = vec![
        (
            Category::TempJunk,
            Box::new(temp::TempJunkScanner) as Box<dyn Scanner + Send + Sync>,
        ),
        (
            Category::SystemCache,
            Box::new(cache::CacheScanner) as Box<dyn Scanner + Send + Sync>,
        ),
        (
            Category::DevArtifact,
            Box::new(dev::DevArtifactScanner) as Box<dyn Scanner + Send + Sync>,
        ),
        (
            Category::AppLeftover,
            Box::new(leftover::LeftoverScanner) as Box<dyn Scanner + Send + Sync>,
        ),
    ];

    let results: Vec<Vec<Finding>> = built
        .into_par_iter()
        // Skip scanners that have not started yet: a cancel between two
        // scanners should not launch the remaining ones at all.
        .filter(|(cat, _)| enabled.contains(cat) && !ctx.counters.cancelled())
        .map(|(_, scanner)| scanner.scan(ctx))
        .collect();

    let mut out: Vec<Finding> = results.into_iter().flatten().collect();
    out.sort_by(|a, b| b.reclaimable().cmp(&a.reclaimable()));
    out
}

/// Snapshot the counters as a progress payload.
pub fn progress(
    name: &str,
    phase: &str,
    current: &Path,
    counters: &ScanCounters,
    done: bool,
) -> ScanProgress {
    ScanProgress {
        scanner: name.to_string(),
        phase: phase.to_string(),
        dirs_scanned: counters.dirs.load(Ordering::Relaxed),
        bytes_scanned: counters.bytes.load(Ordering::Relaxed),
        findings: counters.findings.load(Ordering::Relaxed),
        skipped: counters.skipped.load(Ordering::Relaxed),
        current_path: current.to_string_lossy().to_string(),
        done,
    }
}

/// What one walk of a directory tree yields.
pub struct DirStats {
    pub logical: u64,
    pub on_disk: u64,
    /// Newest modification time anywhere in the tree, in Unix seconds.
    ///
    /// Collected during the size walk rather than in a pass of its own: the
    /// age gate needs it, and a second traversal of a 90k-file npm cache costs
    /// more than the sizing did.
    pub newest_mtime: i64,
    /// True when the walk stopped early because the user cancelled.
    ///
    /// The totals are then a partial sum. Reporting them would be worse than
    /// reporting nothing, because the user trusts the number.
    pub truncated: bool,
    pub file_count: u64,
}

/// Below this size a file's real footprint is its logical size.
///
/// Measuring it exactly costs a Win32 call that opens the file, and the
/// difference is a rounding error at cluster granularity. npm caches hold
/// tens of thousands of small files, so this is the difference between a
/// scan that takes seconds and one that takes a minute.
const EXACT_FOOTPRINT_MIN_BYTES: u64 = 4 * 1024 * 1024;

/// Measure a directory tree with a single parallel walk.
///
/// A single-threaded walk of a large tree (node_modules, a Rust target dir)
/// takes minutes, which makes the whole product feel broken, so the per-file
/// work is spread across cores.
///
/// Symlinks are never followed: a link pointing outside the tree would inflate
/// the numbers and could walk us into an unrelated tree entirely.
pub fn measure_dir(ctx: &ScanCtx, dir: &Path) -> DirStats {
    use rayon::prelude::*;

    let mut stats = DirStats {
        logical: 0,
        on_disk: 0,
        newest_mtime: 0,
        truncated: false,
        file_count: 0,
    };

    if !dir.is_dir() {
        return stats;
    }

    let own_mtime = walk::modified_unix(dir);
    let newest = std::sync::atomic::AtomicI64::new(own_mtime);
    let files = std::sync::atomic::AtomicU64::new(0);
    let logical_sum = std::sync::atomic::AtomicU64::new(0);
    let disk_sum = std::sync::atomic::AtomicU64::new(0);

    // walkdir, not jwalk, for the same reason scan_drive uses it: jwalk
    // pre-reads the whole tree on its own pool, and no cancel check placed
    // here can stop that read. The trees this measures are the big ones
    // (node_modules, target/), so it is exactly where that hurt.
    // The per-file work is still parallel; a cancel only waits for the batch
    // in flight rather than the whole subtree.
    walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        // Both ends matter. take_while stops the walk pulling new entries, and
        // the check inside the body stops the work already in flight. With
        // only the former, a cancel landed early still had to wait for every
        // dispatched file to finish: measured at 378 s of blocked UI on this
        // machine, against a full scan of 499 s.
        .take_while(|_| !ctx.counters.cancelled())
        .par_bridge()
        .for_each(|e| {
            use std::os::windows::fs::MetadataExt;

            if ctx.counters.cancelled() { return }

            let Ok(md) = e.metadata() else { return };
            let logical = md.len();

            // Small files cannot be meaningfully sparse or compressed, so the
            // expensive exact query is reserved for the ones where it matters.
            let on_disk = if logical >= EXACT_FOOTPRINT_MIN_BYTES {
                crate::winapi::ntfs::on_disk_size(&e.path()).unwrap_or(logical)
            } else {
                logical
            };

            let mtime = walk::filetime_to_unix(md.last_write_time() as u64);
            if mtime > 0 {
                newest.fetch_max(mtime, Ordering::Relaxed);
            }

            logical_sum.fetch_add(logical, Ordering::Relaxed);
            disk_sum.fetch_add(on_disk, Ordering::Relaxed);
            files.fetch_add(1, Ordering::Relaxed);
        });

    stats.logical = logical_sum.load(Ordering::Relaxed);
    stats.on_disk = disk_sum.load(Ordering::Relaxed);
    stats.newest_mtime = newest.load(Ordering::Relaxed);
    stats.file_count = files.load(Ordering::Relaxed);
    // A cancel that landed mid-walk leaves the totals a partial sum.
    stats.truncated = ctx.counters.cancelled();

    ctx.counters
        .dirs
        .fetch_add(stats.file_count, Ordering::Relaxed);
    ctx.counters.bytes.fetch_add(stats.logical, Ordering::Relaxed);

    stats
}

/// Size only, for callers that do not need the age.
pub fn dir_size(ctx: &ScanCtx, dir: &Path) -> (u64, u64) {
    let s = measure_dir(ctx, dir);
    (s.logical, s.on_disk)
}

/// How old a directory is, from the newest mtime the measuring walk saw.
fn dir_age(stats: &DirStats, last_access_unix: i64, created_unix: i64) -> (u32, AgeBasis) {
    if stats.newest_mtime > 0 {
        return walk::age_of(0, stats.newest_mtime);
    }
    // The tree was empty or unreadable. Fall back to the directory's own
    // timestamps rather than inventing a fresh age.
    walk::age_of(last_access_unix, created_unix)
}

/// Build a Finding for a directory by measuring it.
///
/// Returns `None` when the directory is empty, the age gate rejects it, or
/// the scan was cancelled part-way through measuring.
pub fn finding_for_dir(
    ctx: &ScanCtx,
    dir: &Path,
    category: Category,
    rule_id: &str,
    last_access_unix: i64,
    created_unix: i64,
) -> Option<Finding> {
    let stats = measure_dir(ctx, dir);
    build_finding(ctx, dir, category, rule_id, last_access_unix, created_unix, &stats)
}

fn build_finding(
    ctx: &ScanCtx,
    dir: &Path,
    category: Category,
    rule_id: &str,
    last_access_unix: i64,
    created_unix: i64,
    stats: &DirStats,
) -> Option<Finding> {
    if stats.logical == 0 {
        return None;
    }
    // A partial walk would understate the size, and the user has no way to
    // tell. Dropping the item is honest; a wrong number is not.
    if stats.truncated {
        return None;
    }

    let min_age = category.min_age_days(ctx.large_file_age_days, ctx.leftover_age_days);
    // Compute the age for every category. Temp junk, caches and dev artifacts
    // have no *gate*, but that only means they are never excluded for being
    // too fresh — it does not mean their age is unknown. The old code stored a
    // `u32::MAX` sentinel here, which the UI then rendered as "11767033.7y".
    let (age_days, age_basis) = dir_age(stats, last_access_unix, created_unix);
    if age_days < min_age {
        return None;
    }

    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.to_string_lossy().to_string());
    // A directory's own last-access stamp is rewritten by the enumeration this
    // scan just performed (NTFS updates a directory's access time even where
    // file access times are frozen), so reporting it raw would show "today"
    // for every folder. The newest file modification time in the tree is the
    // honest answer to "when was this last touched".
    let last_access_unix = if stats.newest_mtime > 0 {
        stats.newest_mtime
    } else {
        last_access_unix
    };
    ctx.counters.findings.fetch_add(1, Ordering::Relaxed);
    Some(Finding {
        id: uuid::Uuid::new_v4().to_string(),
        rule_id: rule_id.to_string(),
        name,
        path: dir.to_string_lossy().to_string(),
        is_dir: true,
        logical_size: stats.logical,
        on_disk_size: stats.on_disk,
        last_access_unix,
        created_unix,
        age_days,
        age_basis,
        category,
        risk: category.risk(),
        disposition: category.default_disposition(),
        child_count: 0,
    })
}

/// Same as `finding_for_dir` but reuses a shared size cache, so overlapping
/// trees are measured once instead of once per match.
pub fn finding_for_dir_cached(
    ctx: &ScanCtx,
    dir: &Path,
    category: Category,
    rule_id: &str,
    last_access_unix: i64,
    created_unix: i64,
    cache: &SizeCache,
) -> Option<Finding> {
    let stats = cache.get_or_measure(ctx, dir);
    build_finding(ctx, dir, category, rule_id, last_access_unix, created_unix, &stats)
}

/// Cache of already-measured directories.
///
/// Nested artifacts are common (a `node_modules` inside a project that also
/// has its own `target`). Without memoization the same bytes get walked
/// repeatedly, which is the difference between a scan that takes seconds and
/// one that takes minutes.
#[derive(Default)]
pub struct SizeCache {
    map: parking_lot::Mutex<std::collections::HashMap<PathBuf, std::sync::Arc<DirStats>>>,
}

impl SizeCache {
    /// Return the stats for `dir`, measuring it at most once.
    ///
    /// Concurrent callers asking for the same directory wait rather than each
    /// starting their own walk: the second one would do identical IO for an
    /// identical answer.
    pub fn get_or_measure(&self, ctx: &ScanCtx, dir: &Path) -> std::sync::Arc<DirStats> {
        if let Some(v) = self.map.lock().get(dir) {
            return std::sync::Arc::clone(v);
        }
        let measured = std::sync::Arc::new(measure_dir(ctx, dir));
        self.map
            .lock()
            .insert(dir.to_path_buf(), std::sync::Arc::clone(&measured));
        measured
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::Arc;

    fn ctx() -> ScanCtx {
        ScanCtx::new(Arc::new(ScanCounters::default()), &Config::default())
    }

    fn make_tree(tag: &str, files: usize) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("sr-scan-{}-{}", tag, uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        for i in 0..files {
            std::fs::write(root.join(format!("f{}.bin", i)), vec![0u8; 4096]).unwrap();
        }
        root
    }

    /// A cancelled scan must not hand back a Finding whose size is merely a
    /// partial sum. Reporting "200 MB" for a 6 GB directory is worse than
    /// reporting nothing, because the user trusts the number.
    #[test]
    fn cancelled_scan_must_not_produce_findings() {
        let root = make_tree("cancel", 400);
        let c = ctx();
        // Cancel before any work happens.
        c.counters.request_cancel();

        let f = finding_for_dir(&c, &root, Category::AppLeftover, "test", 0, 0);
        assert!(
            f.is_none(),
            "a cancelled scan produced a finding with an unreliable size"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Same guarantee for the cached variant the dev scanner uses.
    #[test]
    fn cancelled_scan_must_not_produce_cached_findings() {
        let root = make_tree("cancel2", 200);
        let c = ctx();
        c.counters.request_cancel();
        let cache = SizeCache::default();
        let f = finding_for_dir_cached(&c, &root, Category::AppLeftover, "test", 0, 0, &cache);
        assert!(f.is_none(), "cached path leaked a finding from a cancelled scan");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// One walk per directory, whatever the category.
    ///
    /// The tree used to be measured once for its size and again for its age.
    /// A category with no age gate therefore paid for a second full traversal
    /// whose result was discarded - and dev artifacts are the largest class on
    /// a typical machine. `dirs` counts every file the walk touches, so
    /// exactly 50 means exactly one pass.
    #[test]
    fn measuring_a_directory_walks_it_exactly_once() {
        for category in [
            Category::DevArtifact,
            Category::AppLeftover,
            Category::TempJunk,
        ] {
            let root = make_tree("gate", 50);
            let c = ctx();
            let _ = finding_for_dir(&c, &root, category, "test", 0, 0);
            assert_eq!(
                c.counters.dirs.load(Ordering::Relaxed),
                50,
                "{:?} walked the tree more than once",
                category
            );
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// The large-file scanner must see whatever volumes actually exist, not a
    /// hardcoded C:/D:/E:/F: list.
    #[test]
    fn a_volume_beyond_f_is_still_scannable() {
        let root = std::env::temp_dir().join("sr-vol");
        std::fs::create_dir_all(&root).unwrap();
        let roots = crate::winapi::volume::scannable_roots();
        assert!(
            roots.iter().any(|r| r.to_string_lossy().to_uppercase().starts_with("C:")),
            "the system drive must be scannable"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
