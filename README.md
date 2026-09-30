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

Plus:

- **Quarantine with NTFS compression** — "deleted" items are moved to a restorable, transparently-compressed store, never destroyed until *you* purge them.
- **Explicit disposition** — each item is either *purged permanently* or *moved to quarantine*, never collapsed into a single ambiguous click.
- **Risk tags** — `temp/cache` (safe), `dev artifact` (rebuildable), `large file` (personal data), `app leftover` (may contain user data). Red items need an extra confirmation before permanent purge.
- **Audit log** — every cleanup / restore / purge is written to a traceable log.

## Core principle

There is **no background auto-cleanup, no scheduled task, no boot-time scan**. The software will surface an expiry notice for quarantined items, but only *your* click ever purges anything. Nothing is ever preselected.

## Download

- **[GitHub Releases](https://github.com/xinyuemoli/spacerrecycle/releases)** — Windows 10 1809+ / Windows 11 (requires the WebView2 runtime, preinstalled on Windows 11 and most Windows 10 systems).
- **Microsoft Store** — coming soon.

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

# Frontend tests: standalone .mjs harnesses, each driving a real headless
# browser. They require `playwright` and its Chromium (see each file's CHROME path).
node dist/_test_dom.mjs
node dist/_test_scan_state.mjs
node dist/_test_render.mjs
node dist/_test_scan.mjs
node dist/_test_settings.mjs
```

## Safety model

- **Path denylist** — `C:\Windows`, `Program Files`, user Documents/Desktop/Pictures, `System32`, and the profile root are never deleted or quarantined, even if a rule misfires.
- **Preview first** — every cleanup passes a preview dialog grouped by disposition; there is no "clean all" path that skips it.
- **No admin required** — scanning runs unprivileged; cleanup elevates via a UAC subprocess only when a permission error demands it.
- **Testable deletion layer** — delete/move are pure functions with explicit path parameters, unit-tested against a temp directory.

## License

[MIT](LICENSE)
