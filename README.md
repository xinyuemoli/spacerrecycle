# SpaceRecycle

> A Windows disk-space reclamation tool that **never deletes anything on its own.**
> It does exactly three things: **tell you where the space went** → **let you pick** → **let you confirm.**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Built with **Rust + Tauri 2**. A single ~6 MB executable, no bundler, no npm, no background services, no boot-time scans, no silent cleanup.

## Why SpaceRecycle?

Disk cleaners have a trust problem: they bundle junk, run scheduled background tasks, and delete things you never told them to. SpaceRecycle takes the opposite stance — **every action is initiated by you, from the UI, with a preview and a confirm step before anything happens.**

## Features

Five parallel scanners sharing one unified finding model:

| Scanner | Finds | Typical size |
|---|---|---|
| **Temp junk** | `%TEMP%`, `C:\Windows\Temp`, Prefetch, old logs, crash dumps, Windows Update leftovers | 1–20 GB |
| **System cache** | Browser caches, thumbnail/icon/font caches, shader cache, error reports | 0.5–10 GB |
| **Dev artifacts** | `node_modules`, `Cargo\target`, `.venv`, pip/npm/pnpm caches, `__pycache__`, Gradle/Maven, `.nuget` | 10–100+ GB |
| **Large files** | Files over a threshold (default 500 MB), aggregated by directory into a drill-down | locates, then quarantines |
| **App leftovers** | Directories left by uninstalled apps, stale installers | 5–50 GB |
| **OneDrive local copies** | Cloud-backed content whose local download can be released — the file stays in OneDrive | whatever you have cached |

Plus:

- **Quarantine with NTFS compression** — "deleted" items are moved to a restorable, transparently-compressed store, never destroyed until *you* purge them.
- **Explicit disposition** — each item is either *purged permanently*, *moved to quarantine*, or (for OneDrive content) *released locally*, never collapsed into a single ambiguous click.
- **Risk tags** — `temp/cache` (safe), `dev artifact` (rebuildable), `large file` (personal data), `app leftover` (may contain user data), `cloud copy` (a cached download). Red items need an extra confirmation before permanent purge.
- **Importance marks** — right-click any row to mark it *Critical / Important / Normal / Disposable*. The level is stored by path, survives a re-scan, and is enforced in the cleanup pipeline itself: Critical refuses every action, Important refuses permanent deletion.
- **Cloud backup** (subscription) — upload an item to your own OneDrive and remove the local copy only after the upload is confirmed. If the upload fails it falls back to a local quarantine, so nothing is ever lost.
- **Audit log** — every cleanup / release / restore / purge is written to a traceable log.

## Cloud backup: free the disk, keep a copy

For the items you are unsure about, the safest answer is not "delete" or "quarantine" but "keep it somewhere that is not this disk". With an active subscription and a linked OneDrive account, SpaceRecycle uploads the item to its own folder in your OneDrive and removes the local copy **only after the upload has been confirmed**.

The order is the whole point. Every failure path returns before anything is removed, and a failed upload hands the item to the local quarantine instead — the space is still reclaimed, and nothing is destroyed on the way. The app asks for `Files.ReadWrite.AppFolder`, so it can only reach its own folder and cannot see any of your other files.

## OneDrive: reclaim without deleting

OneDrive's Files On-Demand keeps a placeholder on disk for every file. SpaceRecycle reads those placeholders and reports, per top-level folder, how much this machine is holding that the cloud already has.

Releasing a copy sets `UNPINNED` and clears `PINNED` — the same thing Explorer's *Free up space* does. Files you pinned are never touched, and the operation is refused outside a real sync root. Nothing is uploaded, deleted, or sent anywhere: **no account, no network request, no API key.** Windows re-downloads the file the moment you open it.

Cloud findings can only ever be *released*. Deleting a placeholder inside a sync root deletes the cloud file too, so purge and quarantine are refused outright for them regardless of what the UI is asked for.

## Core principle

There is **no background auto-cleanup, no scheduled task, no boot-time scan**. The software will surface an expiry notice for quarantined items, but only *your* click ever purges anything. Nothing is ever preselected.

## Download

- **[GitHub Releases](https://github.com/xinyuemoli/spacerrecycle/releases)** — Windows 10 1809+ / Windows 11 (requires the WebView2 runtime, preinstalled on Windows 11 and most Windows 10 systems).
- **[Microsoft Store](https://apps.microsoft.com/detail/9N8NBM3QKNQD)** — Store ID `9N8NBM3QKNQD` (`ms-windows-store://pdp/?productid=9N8NBM3QKNQD`). Windows 10 1809+ / Windows 11; WebView2 runtime required.

## Build from source

Prerequisites:

- Rust 1.90+ (MSVC toolchain)
- Visual Studio 2022 (MSVC + Windows SDK)
- [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)

The frontend is plain ES modules with **no npm build step** — `dist/` is the source and the shipped artifact. Build with:

```sh
cargo install tauri-cli --locked
cargo tauri build
```

Run tests:

```sh
cargo test                        # Rust integration tests (scanners, cleanup, quarantine, audit)

# Frontend tests: standalone harnesses. Most stub the DOM and need no browser;
# run them all with plain node.
node dist/_test_render.mjs
node dist/_test_scan.mjs
node dist/_test_settings.mjs
node dist/_test_importance.mjs
node dist/_test_cloud.mjs
node dist/_test_subscription.mjs
node dist/_test_account.mjs
node dist/_test_cloudbackup.mjs

# These two drive a real headless browser and need `playwright` + Chromium.
# Without it they print SKIP and exit 0 rather than failing.
node dist/_test_dom.mjs
node dist/_test_scan_state.mjs
```

## Safety model

- **Path denylist** — `C:\Windows`, `Program Files`, user Documents/Desktop/Pictures, `System32`, and the profile root are never deleted or quarantined, even if a rule misfires.
- **Server-side gates** — the cleanup pipeline re-reads the importance store and refuses on its own; a modified client cannot relabel its way past it.
- **Preview first** — every cleanup passes a preview dialog grouped by disposition; there is no "clean all" path that skips it.
- **No admin required** — scanning runs unprivileged; cleanup elevates via a UAC subprocess only when a permission error demands it.
- **Testable deletion layer** — delete/move are pure functions with explicit path parameters, unit-tested against a temp directory.

## License

[MIT](LICENSE)
