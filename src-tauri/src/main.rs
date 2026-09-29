#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // A hidden self-check: measures how long a cancelled deep scan takes to
    // release the UI. Not reachable from the UI, and it exits without opening
    // a window.
    if std::env::args().any(|a| a == "--probe-cancel") {
        spacerrecycle_lib::probe_cancel::run();
        return;
    }
    spacerrecycle_lib::run();
}
