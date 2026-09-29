//! Cancels a deep scan at a known moment, then stays alive so the harness
//! can watch the process CPU from outside.
//!
//! Whether the work really stopped cannot be answered from inside: returning
//! promptly and actually going idle are different claims. So this prints a
//! marker when the command returns and then idles, and the caller samples
//! CPU time for a few seconds afterwards.
//!
//! Reachable only through `--probe-cancel`.

use crate::config::Config;
use crate::model::Category;
use crate::scanner::{ScanCounters, ScanCtx, Scanner};
use std::io::Write;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn full_scan(counters: &Arc<ScanCounters>, cfg: &Config) -> (Duration, usize) {
    let started = Instant::now();
    let quick = vec![
        Category::TempJunk,
        Category::SystemCache,
        Category::DevArtifact,
        Category::AppLeftover,
    ];
    let ctx = ScanCtx::new(Arc::clone(counters), cfg);
    let mut out = crate::scanner::run_all(&ctx, &quick);
    let lctx = ScanCtx::new(Arc::clone(counters), cfg);
    out.extend(crate::scanner::largefile::LargeFileScanner.scan(&lctx));
    (started.elapsed(), out.len())
}

pub fn run() {
    let file = std::fs::File::create("cancel_probe.txt").expect("probe output");
    let mut w = std::io::BufWriter::new(file);
    let cfg = Config::load();
    let _ = writeln!(w, "ready drives={:?}", crate::winapi::volume::scannable_roots());
    let _ = w.flush();

    for delay_ms in [500u64, 2000, 6000] {
        let c = Arc::new(ScanCounters::default());
        let cfg2 = cfg.clone();
        let c2 = Arc::clone(&c);
        let handle = std::thread::spawn(move || full_scan(&c2, &cfg2));

        std::thread::sleep(Duration::from_millis(delay_ms));
        let bytes_at_cancel = c.bytes.load(Ordering::Relaxed);
        let at_cancel = Instant::now();
        c.request_cancel();
        let (work, n) = handle.join().expect("scan thread");
        let returned = at_cancel.elapsed();

        let _ = writeln!(
            w,
            "cancel at {}ms -> returned in {:?}, work {:?}, {} findings, {} bytes seen",
            delay_ms, returned, work, n, bytes_at_cancel
        );
        let _ = w.flush();

        // Idle, but keep sampling the counters so the harness can tell
        // "returned" apart from "stopped".
        for i in 1..=6 {
            std::thread::sleep(Duration::from_millis(500));
            let _ = writeln!(
                w,
                "  +{:.1}s after return: {} bytes, {} files",
                returned.as_secs_f64() + i as f64 * 0.5,
                c.bytes.load(Ordering::Relaxed),
                c.dirs.load(Ordering::Relaxed)
            );
            let _ = w.flush();
        }
        let _ = writeln!(w, "");
    }
    let _ = w.flush();
}
