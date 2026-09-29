//! End-to-end report: run the real scanners on this machine and print what
//! the user would actually see. Read-only; nothing is deleted.

use spacerrecycle_lib::scanner::{self, ScanCounters, ScanCtx};
use spacerrecycle_lib::testing::*;
use std::sync::Arc;

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(n - 3).collect::<String>())
    }
}

#[test]
fn report_what_a_quick_scan_would_find() {
    let cfg = Config::default();
    let counters = Arc::new(ScanCounters::default());
    let ctx = ScanCtx::new(Arc::clone(&counters), &cfg);

    let categories = vec![
        Category::TempJunk,
        Category::SystemCache,
        Category::DevArtifact,
        Category::AppLeftover,
    ];
    let mut findings = scanner::run_all(&ctx, &categories);
    findings.sort_by(|a, b| b.reclaimable().cmp(&a.reclaimable()));

    let gb = |n: u64| n as f64 / 1_073_741_824.0;
    let total: u64 = findings.iter().map(|f| f.on_disk_size).sum();

    println!();
    println!(
        "=== QUICK SCAN: {} findings, {:.2} GB reclaimable ===",
        findings.len(),
        gb(total)
    );
    for f in findings.iter().take(25) {
        println!(
            "  [{:>8.3} GB] {:<26} {:<50} ({})",
            gb(f.on_disk_size),
            clip(&f.name, 26),
            clip(&f.path, 50),
            f.rule_id
        );
    }

    println!();
    println!("--- by category ---");
    for cat in categories {
        let group: Vec<_> = findings.iter().filter(|f| f.category == cat).collect();
        if group.is_empty() {
            continue;
        }
        let sum: u64 = group.iter().map(|f| f.on_disk_size).sum();
        let disposition = match group[0].disposition {
            Disposition::Purge => "purge",
            Disposition::Quarantine => "quarantine",
        };
        println!(
            "  {:<14} {:>3} items  {:>8.3} GB  default: {}",
            cat.label(),
            group.len(),
            gb(sum),
            disposition
        );
    }

    assert!(
        !findings.is_empty(),
        "a quick scan on a real machine must find something"
    );
}
