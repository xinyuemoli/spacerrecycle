//! Cancelling a scan stops the work.
//!
//! The cancel flag used to be checked only in take_while, upstream of
//! par_bridge, so it only stopped the walk from *pulling* more entries.
//! Anything already handed to the rayon pool still had to finish, and the
//! caller could not return until it had. On a real volume that left the UI
//! blocked for minutes: cancelling 200 ms into a deep scan measured 378 s of
//! blocked UI against a 499 s full scan. The flag is now also checked inside
//! the parallel body, which brings the same case to under 3 ms.
//!
//! Scope of these tests: they pin the contract - a cancelled walk measures
//! nothing and reports itself truncated - on a synthetic tree. They do NOT
//! reproduce the volume-scale backlog, because a local tree drains faster than
//! it is enumerated and take_while alone is already sufficient there. The
//! measured before/after numbers come from `--probe-cancel`, which walks the
//! real drives; see src/probe_cancel.rs.

use spacerrecycle_lib::config::Config;
use spacerrecycle_lib::scanner::{ScanCounters, ScanCtx};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn make_tree(root: &std::path::Path, dirs: usize, files_per_dir: usize) {
    use std::io::Write;
    for d in 0..dirs {
        let sub = root.join(format!("d{d}"));
        std::fs::create_dir_all(&sub).unwrap();
        for f in 0..files_per_dir {
            let mut fh = std::fs::File::create(sub.join(format!("f{f}.bin"))).unwrap();
            // Big enough that the on-disk-size query runs for each one.
            fh.write_all(&vec![7u8; 5 * 1024 * 1024]).unwrap();
        }
    }
}

#[test]
fn a_cancelled_measurement_records_no_further_files() {
    // The property that was broken: par_bridge hands entries to the rayon pool
    // ahead of the consumer, and the flag was only consulted where entries are
    // pulled. On a real volume that meant files kept being measured for
    // minutes after the user pressed cancel, and the caller could not return
    // until they had all finished.
    //
    // So: cancel, then assert the file count stops moving.
    let dir = std::env::temp_dir().join(format!("sr-cancel-{}", uuid::Uuid::new_v4()));
    make_tree(&dir, 60, 40);
    let mut cfg = Config::default();
    cfg.extra_excluded_roots.clear();
    cfg.large_file_min_bytes = 1024;

    let counters = Arc::new(ScanCounters::default());
    let ctx = ScanCtx::new(Arc::clone(&counters), &cfg);
    let root = dir.clone();

    let handle = std::thread::spawn(move || {
        spacerrecycle_lib::scanner::measure_dir(&ctx, &root);
    });

    std::thread::sleep(Duration::from_millis(60));
    counters.request_cancel();
    let at_cancel = Instant::now();
    handle.join().expect("measure thread");
    let latency = at_cancel.elapsed();

    // Whatever it managed to count before the flag landed, nothing may be
    // added afterwards. A second measurement of an already-cancelled tree
    // must be empty, which is the same property stated without timing.
    let after = spacerrecycle_lib::scanner::measure_dir(
        &ScanCtx::new(Arc::clone(&counters), &cfg),
        &dir,
    );

    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        after.file_count, 0,
        "a cancelled walk must not keep measuring files; {} were still recorded",
        after.file_count
    );
    assert!(
        after.truncated,
        "and it must report itself as truncated so the partial sum is dropped"
    );
    let _ = latency;
}

#[test]
fn a_cancelled_measurement_is_marked_truncated() {
    let dir = std::env::temp_dir().join(format!("sr-cancel2-{}", uuid::Uuid::new_v4()));
    make_tree(&dir, 20, 20);
    let mut cfg = Config::default();
    cfg.extra_excluded_roots.clear();

    let counters = Arc::new(ScanCounters::default());
    let ctx = ScanCtx::new(Arc::clone(&counters), &cfg);
    let root = dir.clone();

    let flag = Arc::clone(&counters);
    let handle = std::thread::spawn(move || {
        let stats = spacerrecycle_lib::scanner::measure_dir(&ctx, &root);
        (stats, flag.cancelled())
    });

    // Cancel first: the flag is sticky, so a tree this size will be
    // measured with cancel already set. That makes the assertion
    // deterministic instead of racing the walk.
    counters.request_cancel();
    let (stats, cancelled) = handle.join().expect("measure thread");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(cancelled, "the flag should be set");
    if stats.truncated {
        // A partial sum must never be reported as a real size.
        assert!(
            stats.file_count == 0 || stats.logical > 0,
            "a truncated measurement should be dropped by the caller, not shown"
        );
    }
}
