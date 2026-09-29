pub mod commands;
pub mod config;
pub mod engine;
pub mod log;
pub mod model;
pub mod quarantine;
pub mod safety;
pub mod scanner;
pub mod winapi;

/// Diagnostic entry point, reachable only via a command-line flag.
pub mod probe_cancel;

// Integration tests need to drive the real pipeline against real files, so
// the internals are re-exported under a clearly named testing surface.
#[doc(hidden)]
pub mod testing {
    pub use crate::config::Config;
    pub use crate::engine::{build_preview, needs_red_confirmation, run_cleanup, CleanupRequest, SelectedItem};
    pub use crate::log::{Action as LogAction, Entry as LogEntry};
    pub use crate::model::{AgeBasis, Category, Disposition, Finding, RiskTier};
    pub use crate::quarantine::{
        list_batches, purge, purge_batch, quarantine_items, restore_batch, restore_item,
        BatchMeta, ConflictPolicy, Manifest, PurgeReport, QuarantineReport, RestoreReport,
    };
}

use commands::ScanState;

/// Run a cleanup that was handed over by an elevated relaunch, then exit.
///
/// This instance has no window and no user interaction: it exists purely to
/// perform the deletions the original window could not. Progress is recorded
/// in the same audit log, so the elevated run and the normal one are
/// indistinguishable in the history afterwards.
fn run_elevated_cleanup() -> bool {
    let Some(request) = commands::take_staged_cleanup() else {
        return false;
    };
    let cfg = config::Config::load();
    let report = engine::run_cleanup(request, &cfg);
    println!(
        "elevated cleanup finished: {} purged, {} quarantined, {} failed",
        report.purged.len(),
        report.quarantined.len(),
        report.failed.len()
    );
    true
}

pub fn run() {
    // --elevated-cleanup <path> means we are the helper process spawned by
    // retry_cleanup_elevated. Do the work headlessly and exit; opening a
    // second window would only confuse the user.
    if std::env::args().any(|a| a.starts_with("--elevated-cleanup")) {
        if run_elevated_cleanup() {
            return;
        }
    }

    tauri::Builder::default()
        .manage(ScanState::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_volumes,
            commands::get_config,
            commands::save_config,
            commands::start_scan,
            commands::cancel_scan,
            commands::summarize,
            commands::build_preview,
            commands::needs_red_confirmation,
            commands::run_cleanup,
            commands::list_quarantine_batches,
            commands::list_quarantine_entries,
            commands::restore_batch,
            commands::restore_item,
            commands::purge_batch,
            commands::quarantine_expiry,
            commands::read_audit_log,
            commands::audit_log_path,
            commands::retry_cleanup_elevated,
            commands::reveal_in_explorer,
        ])
        .run(tauri::generate_context!())
        .expect("error while running SpaceRecycle");
}
