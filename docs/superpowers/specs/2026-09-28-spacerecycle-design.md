# SpaceRecycle Design Spec

Date: 2026-09-28
Status: Confirmed, awaiting implementation

## 1. Product Position

A Windows disk space reclamation tool. **Core principle: never delete anything automatically.** No background scheduled tasks, no boot-time scans, no silent cleanup. Every action is initiated by the user from the UI. The software does exactly three things: **tell the user where the space went** -> **let the user pick** -> **let the user confirm**.

Tech stack: **Rust + Tauri 2**, producing a single exe (~10MB), depending on the system WebView2 runtime.

Build prerequisites verified on this machine:
- Rust 1.91.0 (x86_64-pc-windows-msvc), links and runs correctly
- Visual Studio 2022 Community 17.13, MSVC 14.43.34808 + Windows SDK 10.0.26100
- Node 26.7.0 / npm 11.19.0
- WebView2 Runtime 153.0.4234.48
- NTFS compression verified: a 3MB all-zero file compresses to 0 bytes on disk

## 2. Core Concept Model

The UI is organized around three tabs.

### 2.1 Overview

One card per volume: C / D / E... showing used / total / free, plus a horizontal usage bar. Below it, a "reclaimable space" summary labelled by source (temp junk, caches, dev artifacts, etc.), clickable to drill into the details.

### 2.2 Findings

Candidate list grouped by category. Each row shows: name, path, logical size, on-disk size, last access time, age, source rule, current disposition.

- Multi-select, searchable, sortable by size / age
- Each item carries a colored risk tag: `temp/cache` (green, safe), `dev artifact` (blue, rebuildable), `large file` (yellow, personal data), `app leftover` (red, may contain user data)
- A persistent summary bar at the bottom

### 2.3 Quarantine

Already-"deleted" but restorable content. Each entry records: original path, quarantine time, days stored, logical size, on-disk size. Actions: `restore to original location` / `purge permanently` (with confirmation). A prominent banner shows expired item count and footprint.

## 3. Scanner Architecture

Five scanners sharing a unified `Finding` struct (`rule_id`, `path`, `logical_size`, `on_disk_size`, `last_access`, `age_days`, `category`), running in parallel without blocking each other.

| Scanner | What it finds | Typical magnitude |
|---|---|---|
| Temp junk | `%TEMP%`, `C:\Windows\Temp`, Prefetch, old logs, crash dumps (`MEMORY.DMP` / `*.hdmp`), Windows Update leftovers | 1-20 GB |
| System cache | Browser caches (Chrome/Edge/Firefox), thumbnail/icon cache, font cache, DirectX shader cache, Windows error reports | 0.5-10 GB |
| Dev artifacts | `node_modules`, large `.git` packfiles, `Cargo\target`, pip/npm/pnpm caches, `__pycache__`, `.venv`, Gradle/Maven repos, `.nuget\packages` | 10-100+ GB |
| Large files | Files over a threshold (default 500MB) across a volume or a chosen directory; aggregates by parent directory into "who is eating the space", drillable. Shown only when the user opts in; default disposition is quarantine | locates and can be quarantined |
| App leftovers | Directories left by uninstalled apps in `AppData\Local` / `Roaming` / `ProgramData` (cross-checked against live programs via registry `Uninstall` keys); stale installers (`*.msi` / `*.exe`) older than N days in Downloads | 5-50 GB |

**Technical notes:**

- Full-volume traversal uses `jwalk` (rayon parallel). Single-threaded traversal of C: takes minutes; parallel is much faster.
- Age is judged by real NTFS access time (`FILE_BASIC_INFO`.`LastAccessTime`). Win10+ disables LastAccess updates by default, so `CreationTime` (approximating install / first-use) is the fallback, and the UI labels which basis was used.
- Permission / in-use handling: inaccessible directories are counted as "skipped" and do not abort the scan; files locked by other processes are skipped at cleanup time and reported.
- Performance guardrails: the scan is cancellable at any time; the UI streams progress (directories scanned, bytes scanned, findings so far).

## 4. Disposition Model (confirmed by user)

Disposition is an **explicit two-dimensional choice**, not a fixed rule derived from category.

| Category | Default disposition | Available dispositions |
|---|---|---|
| Temp junk / system cache | Purge permanently | none (meaningless, not offered) |
| Dev artifacts | Move to quarantine | purge permanently |
| Large files | Move to quarantine | purge permanently |
| App leftovers | Move to quarantine | purge permanently |

**Rationale:** quarantine exists for the "unsure" case and is a needless extra step for the "sure" case. Making disposition an explicit second statement keeps the default on the safe side and never collapses "select this item" and "confirm permanent destruction" into the same click.

**UI operations:**

- Per item: right-click menu -> `Purge permanently` / `Move to quarantine`
- Batch: a dropdown plus a primary button in the bottom summary bar

Bottom summary bar:

```
Selected 28 items - logical 34.7 GB -> reclaimable 34.7 GB
Disposition: [ By category default v ]        [ Clean selected ]
```

The `Disposition` dropdown has four values: `By category default` / `Purge all permanently` / `Move all to quarantine` / `Purge temp and cache, quarantine the rest`.

**Preview dialog** groups by effective disposition:

```
Purge permanently (3 items, 4.2 GB)      <- temp junk 3
Move to quarantine (28 items, 9.1 GB)    <- dev artifacts 14, large files 11, app leftovers 3
Purge permanently (12 items, 6.3 GB)     <- you manually switched these

[x] I confirm the red-tagged items contain no personal data
```

Risk tags add one extra gate for "purge permanently": red items (app leftovers) chosen for permanent purge require a separate confirmation checkbox in the preview. Quarantine needs no such confirmation (it is restorable anyway).

**Pipeline:**

```
Findings -> user selection -> preview dialog (grouped by disposition + risk warnings)
         -> purge permanently: delete, nothing retained
         -> move to quarantine: NTFS-compressed, restorable
```

## 5. Quarantine Implementation

- Location: fixed at `%ProgramData%\SpaceRecycle\Quarantine`, **no auto-migration**. Manually changeable in settings.
- Structure: `Quarantine\<batch-id>\...`, each batch with a manifest recording each item's original path, timestamp, rule id, category, and disposition time.
- Compression: NTFS built-in compression (`compact /c`, NTFS compression attribute) applied to the quarantine directory. Restoring is a transparent `MoveFile`; the user never decompresses anything. Already-compressed formats (zip/jpg/mp4/7z) and non-NTFS volumes are skipped.
- Footprint: `GetCompressedFileSizeW` for on-disk size; the UI shows "logical XX GB -> on-disk XX GB".
- Lifecycle (option 2, confirmed): **the software never auto-deletes.** The UI surfaces an "stored more than 7 days" expiry notice, and only the user's click purges anything.
- Restore edge cases: if the original location is now occupied, prompt for a conflict choice of `skip` / `overwrite`. Try `MoveFileEx` first, then fall back to per-file move.
- Batch atomicity: if some files fail to move (locked), the batch is marked "partially complete"; restore is best-effort and reports which items succeeded.
- Why not zip archives: restoring requires decompressing first, adding a failure point, and a single multi-GB archive is itself fragile. NTFS compression has zero dependencies, zero CPU cost (handled by the filesystem in the background), and transparent restore.

## 6. Safety Design

Because the software handles files going back many years, safety mechanisms matter more than the cleanup algorithms.

- **Nothing preselected** - the UI preselects no item on open.
- **Risk tier tags** - see 2.2. Red items headed for permanent purge need extra preview confirmation.
- **Path denylist** - hardcoded never-delete / never-quarantine paths: `C:\Windows`, `Program Files`, user Documents/Desktop/Pictures, `System32`, the user profile root itself, core `%ProgramData%` items. Even if a rule misfires, these are only skipped.
- **Age thresholds** - dev artifacts (`node_modules`, `.venv`, `target`, `__pycache__`) have **no threshold**, listed as found; large files and app leftovers require **30 days** since last access (configurable).
- **Preview first** - every deletion passes a preview dialog listing the full set by category, exportable to text. There is no "clean all" path that skips the preview.
- **Admin rights** - not required for the whole session. Scanning proceeds normally; if a permission error hits during cleanup, elevate via a UAC subprocess, avoiding a "blue box the moment you open it".
- **Audit log** - every cleanup / restore / purge is written to a log file (logical path, on-disk size, time, result), for user traceability and for the software to be auditable.
- **Testable deletion layer** - the low-level delete / move is implemented as pure functions with explicit path parameters; unit tests run against a temp directory and never touch real data.

## 7. Technical Architecture

```
+---------------------------------------------+
|  Frontend (TypeScript + Web)                |  Overview / Findings / Quarantine tabs
|  Tauri 2 - refined dark UI                  |  streaming progress, usage charts
+--------------------+------------------------+
                     | Tauri IPC (invoke / event)
+--------------------v------------------------+
|  Rust Core                                  |
|  +-- scanner/   5 scanners + progress events |
|  +-- engine     aggregate / sort / filter   |
|  +-- quarantine NTFS-compressed store+manifest|
|  +-- safety     denylist / age gate / risk   |
|  +-- winapi     NTFS attrs / volumes / elevate|
|  +-- log        audit log                   |
+---------------------------------------------+
```

- Clean unit boundaries: `scanner` does not depend on `quarantine`; `quarantine` does not depend on the UI. Each scanner implements a common trait and is independently testable (feed it a temp directory tree).
- **No cross-platform abstraction** (user confirmed Windows-only) - call `GetCompressedFileSizeW`, volume info, and `MoveFileExW` directly through the `windows` crate. No trait abstraction layer.
- Config: JSON at `%APPDATA%\SpaceRecycle\config.json` (age thresholds, large-file threshold, quarantine location, default disposition).

## 8. Delivery Phases

Each phase runs independently.

1. **Skeleton + temp junk scanner** - Tauri window, Overview tab, volume stats, volume scan bar. Real numbers on C: immediately.
2. **Large file scanner + drill-down** - find large files across a volume, directory aggregation, sort and filter. The main value of the safety net.
3. **Quarantine** - NTFS compression, manifest, restore, expiry notice.
4. **Dev artifact scanner** - usually the largest class of reclaimable space.
5. **System cache + app leftover scanners** + registry cross-check.
6. **Safety layer hardening** - risk tag UI, preview dialog, audit log.

## 9. Explicit Non-Goals

- No background auto-cleanup, no scheduled tasks, no boot-time scans - everything is initiated from the UI by the user.
- No file recovery tooling (quarantine already covers the need).
- No disk partitioning / formatting / defragmentation.
- No cloud sync, no duplicate-file hash dedup - out of scope for V1, limited benefit and it slows scans.
- No cross-platform support.
