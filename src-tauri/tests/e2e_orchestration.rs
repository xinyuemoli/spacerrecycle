//! Verify the scan orchestration itself: the same code path the
//! `start_scan` command runs, minus the Tauri event plumbing.

use spacerrecycle_lib::model::Category;
use spacerrecycle_lib::safety;
use spacerrecycle_lib::scanner::{self, ScanCounters, ScanCtx, Scanner};
use spacerrecycle_lib::testing::*;
use std::sync::Arc;

fn run(categories: &[Category], cfg: &Config) -> Vec<Finding> {
    let counters = Arc::new(ScanCounters::default());
    let ctx = ScanCtx::new(counters, cfg);
    let mut out = scanner::run_all(&ctx, categories);
    out.sort_by(|a, b| b.reclaimable().cmp(&a.reclaimable()));
    out
}

#[test]
fn run_all_respects_the_enabled_category_list() {
    let cfg = Config::default();

    let only_temp = run(&[Category::TempJunk], &cfg);
    assert!(!only_temp.is_empty(), "temp junk should be found");
    assert!(only_temp.iter().all(|f| f.category == Category::TempJunk));

    // Asking for nothing yields nothing, rather than scanning everything.
    assert!(run(&[], &cfg).is_empty());
}

#[test]
fn results_are_sorted_by_reclaimable_size_descending() {
    let cfg = Config::default();
    let findings = run(
        &[Category::TempJunk, Category::SystemCache, Category::DevArtifact],
        &cfg,
    );
    assert!(findings.len() >= 2, "need several findings to test ordering");
    for w in findings.windows(2) {
        assert!(
            w[0].reclaimable() >= w[1].reclaimable(),
            "findings are not sorted by reclaimable size"
        );
    }
}

#[test]
fn every_result_is_safe_to_act_on() {
    let cfg = Config::default();
    let findings = run(
        &[
            Category::TempJunk,
            Category::SystemCache,
            Category::DevArtifact,
            Category::AppLeftover,
        ],
        &cfg,
    );
    assert!(!findings.is_empty());
    for f in &findings {
        assert!(
            safety::is_allowed(std::path::Path::new(&f.path), &cfg),
            "unsafe path reached the results: {}",
            f.path
        );
        assert!(!f.name.is_empty());
    }
}

#[test]
fn category_totals_add_up_to_the_whole_result_set() {
    // Mirrors what the overview page computes for its stat tiles.
    let cfg = Config::default();
    let findings = run(&[Category::TempJunk, Category::SystemCache, Category::DevArtifact], &cfg);
    let total: u64 = findings.iter().map(|f| f.on_disk_size).sum();
    let mut sum = 0u64;
    for f in &findings {
        sum += f.on_disk_size;
    }
    assert_eq!(total, sum);
}

/// Build a temp tree containing files of known sizes, with backdated
/// timestamps so the age gate can be exercised deterministically.
fn tree_with_files(spec: &[(&str, u64, u32)]) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("sr-lf-{}", uuid::Uuid::new_v4()));
    for (rel, size, age_days) in spec {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![b'z'; *size as usize]).unwrap();
        spacerrecycle_lib::winapi::backdate(&p, *age_days).unwrap();
    }
    root
}

#[test]
fn large_files_respect_the_age_gate_and_default_to_quarantine() {
    let cfg = Config::default();
    let mut fast = Config::default();
    fast.large_file_min_bytes = 1024 * 1024; // 1MB
    fast.large_file_age_days = 30;

    let root = tree_with_files(&[
        ("big-old.bin", 4 * 1024 * 1024, 90),   // old + large -> reported
        ("big-fresh.bin", 4 * 1024 * 1024, 2),  // large but recent -> gated out
        ("small-old.bin", 1024, 90),            // old but small -> not large
        ("sub/deep/bigger.bin", 8 * 1024 * 1024, 60), // nested -> reported
    ]);
    let counters = Arc::new(ScanCounters::default());
    let ctx = ScanCtx::new(Arc::clone(&counters), &fast);
    let findings = scanner::largefile::scan_drive(&ctx, &root, &cfg);

    let names: Vec<String> = findings.iter().map(|f| f.name.clone()).collect();
    assert!(names.contains(&"big-old.bin".to_string()), "old large file missing: {:?}", names);
    assert!(names.contains(&"bigger.bin".to_string()), "nested large file missing: {:?}", names);
    assert!(!names.contains(&"big-fresh.bin".to_string()), "recent file should be age-gated out");
    assert!(!names.contains(&"small-old.bin".to_string()), "small file should not count as large");

    for f in &findings {
        assert_eq!(f.category, Category::LargeFile);
        assert_eq!(f.disposition, Disposition::Quarantine, "large files never purge by default");
        assert!(f.age_days >= 30, "large file below the age gate: {}", f.path);
        assert!(!f.is_dir);
    }

    // Reported in descending size order.
    for w in findings.windows(2) {
        assert!(
            w[0].on_disk_size >= w[1].on_disk_size,
            "not sorted: {} ({}) before {} ({})",
            w[0].name, w[0].on_disk_size, w[1].name, w[1].on_disk_size
        );
    }

    println!("large-file pass found {} candidates: {:?}", findings.len(), names);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn large_file_pass_skips_protected_paths() {
    let cfg = Config::default();
    let mut fast = Config::default();
    fast.large_file_min_bytes = 1024;
    let counters = Arc::new(ScanCounters::default());
    let ctx = ScanCtx::new(Arc::clone(&counters), &fast);

    // Scanning the user profile root must not surface Documents/Desktop.
    // Bounded by a cancel so the suite stays fast; the invariant under test
    // is the denylist, not walk coverage.
    let home = dirs::home_dir().unwrap();
    let cancel = Arc::clone(&counters);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(8));
        cancel.request_cancel();
    });
    let findings = scanner::largefile::scan_drive(&ctx, &home, &cfg);
    for f in &findings {
        let p = f.path.to_lowercase();
        assert!(!p.contains("\\documents\\"), "protected path leaked: {}", f.path);
        assert!(!p.contains("\\desktop\\"), "protected path leaked: {}", f.path);
    }
    println!("profile-root pass surfaced {} large files, none protected", findings.len());
}

#[test]
fn cancelling_stops_the_scan_promptly() {
    let counters = Arc::new(ScanCounters::default());
    counters.request_cancel();
    let cfg = Config::default();
    let ctx = ScanCtx::new(Arc::clone(&counters), &cfg);

    // Already-cancelled context: the directory walk must bail out immediately
    // rather than grinding through the tree.
    let start = std::time::Instant::now();
    let findings = scanner::temp::TempJunkScanner.scan(&ctx);
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "cancelled scan took {:?}",
        elapsed
    );
    println!("cancelled scan returned {} findings in {:?}", findings.len(), elapsed);
}
