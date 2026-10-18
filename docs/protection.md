# How the delete guard works

## The product rule

Nothing is deleted automatically. Every action is started by the user, and the
guard exists to stop *accidents* - a misplaced delete, a cleanup rule that
matched more than it should have. It is not designed to stop a determined user,
and it does not pretend to. Somebody who wants a marked file gone can close the
app and delete it, and that is fine: it is their file.

## The mechanism

A file marked 重要 is **held open** by the application, with a share mode that
omits `FILE_SHARE_DELETE`. Windows then refuses to delete it:

```
DeleteFile  ->  ERROR_SHARING_VIOLATION
```

This is enforced by the I/O manager when the handle is opened, not by the access
check. It needs no driver, no kernel signature, no elevation and no certificate.
The file's permissions are untouched, the file itself is untouched, and releasing
the mark closes the handle.

Two settings decide whether it works, both of which default to the wrong value
and neither of which reports anything:

- **Share mode.** Rust's default is `FILE_SHARE_READ | FILE_SHARE_WRITE |
  FILE_SHARE_DELETE`, matching POSIX semantics where an open file can still be
  unlinked. Left alone, the guard protects nothing.
- **Access mode.** Asking for no rights at all - which looks right, since the
  handle is only wanted for its sharing effect - also protects nothing: a
  zero-access handle does not join the delete sharing check. `GENERIC_READ` is
  required.

`src-tauri/src/hold.rs` pins all four combinations in a test, because a guard
that protects nothing looks exactly like one that works until something is
deleted.

## What it does not do

| Limit | Why |
|---|---|
| Protection lasts as long as the process | A held handle dies with its owner. The app registers itself to start at logon so the guard is up after a reboot; closing it ends the protection. |
| A held file cannot be renamed or moved | Rename needs `DELETE`, and the handle refuses it. Expected, but worth knowing. |
| The error is a sharing violation | Not "access denied". Honest, and confusing to anyone reading it. |
| "Ask me" behaves as "block it" | A handle cannot prompt. The prompt is a driver capability, so with no driver the two settings do the same thing - and the UI says so. |

## What was rejected, and on what evidence

### A deny entry in the file's ACL

Measured on a standard user in a Windows 11 guest. With `DENY Everyone:(D)` on
the file:

- `CreateFileW(DELETE)` returned `ACCESS_DENIED`, **and**
- `del` deleted the file anyway.

Windows takes deletion rights from the parent directory's `FILE_DELETE_CHILD`
as well, so a deny on the file alone does not stop it. Denying that on the parent
does block it - and freezes every file in the folder, which is not something you
can do to somebody's Documents directory because one file in it is marked
important.

### A minifilter driver

`driver/` contains a working one, at altitude 385100, with a policy port and a
resident service. It is better than the hold in one way that matters - it can
prompt - and it is unusable by this project: a kernel driver needs a
kernel-mode signature, which for a new driver means an EV code-signing
certificate, which is issued to organisations rather than individuals.

The code is kept because it is correct and because it is the upgrade path if the
signing situation ever changes. It is not on the shipping path, and
`protection::current()` selects it only when it is actually running.

## Where marks live

Two places, and both entry points write both of them:

- **`<path>:SpaceRecycle.Importance`** - an alternate data stream, payload
  `v1|<level>|<unix>`. Written by the right-click menu and by the app. It travels
  with the file and survives the app being uninstalled.
- **`%APPDATA%\SpaceRecycle\importance.json`** - the app's own store, keyed by
  normalized path.

A stream is a property of one file and says nothing about where the other marked
files are, so the marks cannot be enumerated from them. `%ProgramData%\SpaceRecycle\marked.txt`
is the index that makes them enumerable: the extension rewrites it on a mark, the
app reads it.

The index is **not authoritative**. A path in it counts only while its stream
still says so, which means clearing a mark needs no write to a shared file, a
stale entry protects nothing and costs nothing, and the app can prune the file
after reading it.

## Levels

Two, matching the menu: `important` (重要) and `disposable` (可清理).

- **重要** refuses the irreversible action and the cloud release. Quarantine and
  cloud backup stay available, which is what the menu promises in the same words:
  "cleanup may quarantine or back it up, but never delete it". The file is held
  open, so nothing else can delete it either.
- **可清理** is the user saying the file may go. Nothing is held.

The backend used to have four levels. `critical` and `high` load as 重要, `normal`
and `low` as 可清理, through serde aliases - an existing store loads with no
migration step and no marks are lost.

## Rebuilding the driver (not on the shipping path)

```powershell
cd driver
.\build.cmd            # needs the WDK
.\vm\install.cmd       # needs an elevated shell and a test-signed machine
```

## Verifying the guard

`driver/vm/win11/verify-hold-guard.ps1` runs against a sideloaded MSIX in the
guest: it marks a file, launches the app, attempts a delete **from a separate
process**, and then stands the guard down and attempts it again. A handle does not
stop the process that owns it, so a same-process delete would prove nothing.

`_checkall.ps1` at the repository root runs the Rust suite, the frontend
harnesses, the shell extension build, the encoding check and the author-history
leak check.
