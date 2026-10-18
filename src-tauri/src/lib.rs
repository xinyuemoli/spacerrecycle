pub mod ads;
pub mod autostart;
pub mod cloud;
pub mod commands;
pub mod config;
pub mod engine;
pub mod guard;
pub mod hold;
pub mod importance;
pub mod license;
pub mod log;
pub mod model;
pub mod protection;
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
    pub use crate::importance::{Importance, Store as ImportanceStore};
    pub use crate::log::{Action as LogAction, Entry as LogEntry};
    pub use crate::model::{AgeBasis, Category, Disposition, Finding, RiskTier};
    pub use crate::quarantine::{
        list_batches, purge, purge_batch, quarantine_items, restore_batch, restore_item,
        BatchMeta, ConflictPolicy, Manifest, PurgeReport, QuarantineReport, RestoreReport,
    };
}

use commands::{GraphState, ScanState};

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
        .manage(GraphState::default())
        .setup(|_app| {
            // Restore the hold guard at startup.
            //
            // A held handle lives only as long as the process holding it, so
            // protection does not survive the app being closed and reopened. The
            // marks are on disk, so the guard is rebuilt from them rather than
            // from anything remembered - and only if the disposition says the
            // guard is standing, or the app would start up protecting files the
            // user had switched protection off for.
            //
            // Failures are not logged or printed here: they are held for
            // protection_status to report, which is the screen where somebody is
            // actually asking whether their files are protected.
            if crate::guard::read() != crate::guard::Disposition::Allow {
                crate::protection::sync_holds_from_marks();
            }
            Ok(())
        })
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
            commands::list_importance,
            commands::set_importance,
            commands::clear_importance,
            commands::guard_status,
            commands::protection_status,
            commands::set_guard_disposition,
            commands::autostart_status,
            commands::onedrive_status,
            commands::get_entitlement,
            commands::refresh_entitlement,
            commands::graph_connect_start,
            commands::graph_connect_poll,
            commands::graph_connect_cancel,
            commands::graph_account_status,
            commands::graph_sign_out,
            commands::list_cloud_backups,
            commands::restore_cloud_backup,
            commands::delete_cloud_backup,
        ])
        .run(tauri::generate_context!())
        .expect("error while running SpaceRecycle");
}
