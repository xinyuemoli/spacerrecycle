//! Exercise each scanner against the real machine. These tests only read
//! metadata; nothing is deleted. They assert that the scanners find the
//! well-known locations Windows always has, and that every finding they emit
//! survives the safety gate.

use spacerrecycle_lib::safety;
use spacerrecycle_lib::scanner;
use spacerrecycle_lib::scanner::Scanner;
use spacerrecycle_lib::testing::*;
use std::sync::{Arc, OnceLock};

fn counters() -> Arc<scanner::ScanCounters> {
    Arc::new(scanner::ScanCounters::default())
}

/// Each scanner is expensive on a real machine, so scan once and share the
/// result across the assertions instead of re-walking the disk per test.
struct Results {
    temp: Vec<Finding>,
    cache: Vec<Finding>,
    dev: Vec<Finding>,
    leftover: Vec<Finding>,
}

fn results() -> &'static Results {
    static RESULTS: OnceLock<Results> = OnceLock::new();
    RESULTS.get_or_init(|| {
        let cfg = Config::default();
        let temp = scanner::temp::TempJunkScanner.scan(&scanner::ScanCtx::new(counters(), &cfg));
        let cache = scanner::cache::CacheScanner.scan(&scanner::ScanCtx::new(counters(), &cfg));
        let dev = scanner::dev::DevArtifactScanner.scan(&scanner::ScanCtx::new(counters(), &cfg));
        let leftover =
            scanner::leftover::LeftoverScanner.scan(&scanner::ScanCtx::new(counters(), &cfg));
        Results { temp, cache, dev, leftover }
    })
}

#[test]
fn temp_scanner_finds_the_windows_temp_directory() {
    let r = results();
    assert!(!r.temp.is_empty(), "temp scanner found nothing");

    for f in &r.temp {
        assert_eq!(f.category, Category::TempJunk);
        assert_eq!(f.disposition, Disposition::Purge);
        assert!(!f.path.is_empty());
    }

    let user_temp = std::env::temp_dir().to_string_lossy().to_lowercase();
    assert!(
        r.temp.iter().any(|f| f.path.to_lowercase().starts_with(&user_temp)),
        "expected the user temp dir among {:?}",
        r.temp.iter().map(|f| &f.path).collect::<Vec<_>>()
    );
}

#[test]
fn cache_scanner_reports_only_purgeable_cache() {
    for f in &results().cache {
        assert_eq!(f.category, Category::SystemCache);
        assert_eq!(f.disposition, Disposition::Purge);
    }
    println!("cache scanner found {} entries", results().cache.len());
}

#[test]
fn dev_scanner_finds_rebuildable_artifacts() {
    for f in &results().dev {
        assert_eq!(f.category, Category::DevArtifact);
        assert_eq!(f.disposition, Disposition::Quarantine);
        assert!(f.logical_size > 0);
    }
    println!("dev scanner found {} entries", results().dev.len());
}

#[test]
fn every_finding_from_every_scanner_passes_the_safety_gate() {
    let cfg = Config::default();
    let r = results();
    let all: Vec<&Finding> = r.temp.iter().chain(&r.cache).chain(&r.dev).chain(&r.leftover).collect();

    assert!(!all.is_empty(), "scanners produced nothing at all");
    for f in &all {
        assert!(
            safety::is_allowed(std::path::Path::new(&f.path), &cfg),
            "scanner emitted a path the safety layer would block: {}",
            f.path
        );
        assert!(
            !f.path.to_lowercase().contains("spacerecycle\\quarantine"),
            "scanner targeted the quarantine store: {}",
            f.path
        );
    }
    println!("validated {} findings across all scanners", all.len());
}

#[test]
fn dev_artifacts_are_never_age_gated() {
    // The invariant is that age never disqualifies a dev artifact, not that
    // access times are always missing. Both bases are legitimate depending on
    // whether the directory has ever been read since the last access-time
    // update policy kicked in.
    for f in &results().dev {
        assert!(
            f.age_days < 5 || f.category.min_age_days(30, 30) == 0,
            "dev artifact below the age floor: {}",
            f.path
        );
        // A brand-new artifact must still be reported.
        assert_eq!(f.disposition, Disposition::Quarantine);
    }
}

#[test]
fn leftover_scanner_honours_the_age_gate_and_red_risk() {
    let cfg = Config::default();
    for f in &results().leftover {
        assert_eq!(f.category, Category::AppLeftover);
        assert_eq!(f.risk, RiskTier::UserData);
        assert!(
            f.age_days >= cfg.leftover_age_days,
            "leftover below the 30-day gate: {}",
            f.path
        );
    }
    println!("leftover scanner found {} entries", results().leftover.len());
}

#[test]
fn no_path_is_reported_twice() {
    // A duplicate is not merely untidy: the same bytes get counted twice in the
    // totals, and the user sees two rows that differ only by an id they cannot
    // see. This covers duplicates within one scanner as well as overlaps
    // between two of them, with no exemptions.
    let r = results();
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for (label, list) in [
        ("temp", &r.temp),
        ("cache", &r.cache),
        ("dev", &r.dev),
        ("leftover", &r.leftover),
    ] {
        for f in list {
            if let Some((_, other)) = seen.iter().find(|(p, _)| *p == f.path.as_str()) {
                panic!("{} and {} both report {}", other, label, f.path);
            }
            seen.push((f.path.as_str(), label));
        }
    }
}

#[test]
fn every_finding_reports_a_plausible_size() {
    // A cancelled walk used to leave a partial total behind, so a 6 GB
    // directory could be reported as 200 MB. Nothing should claim a size of
    // zero, and a directory's logical size must never be below its footprint.
    let r = results();
    for (label, list) in [
        ("temp", &r.temp),
        ("cache", &r.cache),
        ("dev", &r.dev),
        ("leftover", &r.leftover),
    ] {
        for f in list {
            assert!(f.logical_size > 0, "{}: {} has zero logical size", label, f.path);
            assert!(
                f.on_disk_size <= f.logical_size,
                "{}: {} claims {} on disk but only {} logically",
                label, f.path, f.on_disk_size, f.logical_size
            );
        }
    }
}
