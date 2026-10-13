# SpaceRecycle Shell Extension

Explorer integration in two parts:

1. A right-click submenu that tags files and folders with an importance level.
2. An icon overlay that marks tagged files in place, with a different badge per
   level.

```
SpaceRecycle ▸  重要       (orange)   cleanup may quarantine or back it up, never delete
                可清理     (green)    cleanup may delete it permanently
                清除标记   (grey)     remove the mark
```

Tagged files then carry one of these in the corner of their icon:

| Level | Badge |
|---|---|
| 重要 | orange ring |
| 可清理 | green check |

The chosen level is written into an **Alternate Data Stream on the file itself**:

```
<path>:SpaceRecycle.Importance   ->   "v1|important|1791818250"
```

## Why ADS

*The mark travels with the file.* Renames and copies within NTFS keep it, so
the level does not silently detach from the thing it describes. A central index
keyed by path cannot promise that.

*The read is one call.* Testing whether a path is marked is a single
`GetFileAttributesW("<path>:SpaceRecycle.Importance")`: `INVALID_FILE_ATTRIBUTES`
means unmarked. No file open, no read, no index, no cache to invalidate. That
matters because the icon-overlay handler that comes next runs this check for
every file Explorer draws.

### What ADS costs

| Limitation | Consequence |
|---|---|
| FAT32 / exFAT / most network shares have no streams | tagging fails there |
| A read-only file cannot take a stream | tagging fails; the extension says so |
| Some sync clients do not carry streams | a cloud-synced copy may lose the mark |
| Security software sometimes flags stream use | it is a known malware-hiding technique |
| `dir /r` is needed to see it | the mark is invisible in the normal UI |

None of these lose the *file*; they lose the annotation. The extension reports
failures rather than failing silently.

## Why only two levels

Fewer levels is a better fit for a right-click menu, and it halves the number of
icon-overlay slots the feature will need. Windows provides **15 overlay slots
machine-wide**, and they are usually all taken.

## Building and installing

```powershell
shell-ext\build.cmd                      # -> shell-ext\bin\SpaceRecycleShell.dll + menutest.exe
shell-ext\register.ps1 -Action register  # per-user, no administrator
shell-ext\register.ps1 -Action status
shell-ext\register.ps1 -Action unregister
```

Registration is **HKCU only**. It needs no elevation, cannot affect another
account, and removes cleanly. `DllRegisterServer` holds the whole registry
layout; the script just calls it through `regsvr32`.

The handler is registered under `HKCU\Software\Classes\*\shell\SpaceRecycle` for
files and `...\Directory\shell\SpaceRecycle` for folders, using
`ExplorerCommandHandler` — the modern hook, which is why it appears in the
Windows 11 context menu rather than under *Show more options*.

## Testing without Explorer

`bin\menutest.exe` drives the real COM object the way Explorer does:

```
menutest list  <path>          # titles, icons, checked state, for the real submenu
menutest set   <path> <index>  # invoke one entry
menutest get   <path>          # read the ADS with a second, independent reader
```

It prints titles as UTF-16 code points, because a console in code page 936
renders Chinese as `????` and a passing test would look exactly like a failing
one.

The whole flow — tag as 重要, tag as 可清理, clear, and tag a *folder* — has been
verified this way, including that the checkmark follows the mark and that the
stream is `v1|important|<unix>`.

---

# Verification environments

## Windows 11 (host) — registration path

The full COM path is exercised here: `register.ps1 -Action register` writes the
per-user keys, `CoCreateInstance` finds the class through HKCR, and the submenu
enumerates with the expected titles and icons.

## Windows 7 SP1 x64 (VM) — behaviour, and the PowerShell 2.0 traps

Verified on a Windows 7 Home Basic x64 guest driven through `vmrun`: the submenu
titles (by code point), the ADS payload for `重要` → `v1|important|<unix>` and
`可清理` → `v1|disposable|<unix>`, clearing back to "no stream", the checkmark
tracking the mark, and **a folder as well as a file**.

Getting there meant fixing four separate Windows 7 incompatibilities, none of
which announce themselves clearly:

| Trap | Symptom |
|---|---|
| `$PSScriptRoot`, `$PSCommandPath` are 3.0+ | script dies resolving its own directory |
| `Get-CimInstance` is 3.0+ | command not found; `Get-WmiObject` is the fallback |
| `Get-ChildItem -File` is 3.0+ | listing silently comes back empty, which reads as "the files were never created" |
| `Get-Content "<file>:<stream>"` is 3.0+ | reading an ADS by path fails, so a guard reports every marked file as unmarked — the marks were fine |
| `Set-Content -NoNewline` is 5.0+ | parameter binding fails, **the fixture file is never created** — which then looks exactly like the shell item API rejecting a valid path |
| `Start-Process -Wait` is broken on 2.0 | throws "the process is not currently running" as soon as a short-lived program exits, with or without `-PassThru` |
| No .NET 4.0 (CLR 2.0.50727) | `[Environment]::Is64BitOperatingSystem` returns `$null` instead of throwing, so a try/catch guard silently reports a 64-bit guest as 32-bit |
| `vmrun` appends a stray whitespace argument, and 2.0 binds it positionally | a `param` block is not safe in a guest-invoked script; and strict parsers reject the command line outright |

That last one deserves emphasis, because it is invisible until it bites:
`tasklist`, `reg`, `whoami` and `shutdown` all fail with **"invalid
argument/option - ' '"** and exit non-zero, while `menutest` ignores the extra
token and works. The rule that follows from it: **run a script file, never an
inline command.** `vm/*.cmd` and `vm/*.ps1` exist for exactly this reason.

`register.ps1` therefore calls the DLL's `DllRegisterServer` **in-process**
instead of shelling out to `regsvr32`, and carries a
`ValueFromRemainingArguments` sink for the stray argument.

## Why the guest falls back to a direct DLL load

`menutest` normally creates the command through `CoCreateInstance`. Inside this
VM that returns `REGDB_E_CLASSNOTREG` for a CLSID that is demonstrably present:

```
running as : sruser
HKCU\Software\Classes\CLSID\{...}\InprocServer32   -> present, points at the DLL
HKCR\CLSID\{...}\InprocServer32                    -> open failed rc=2
```

COM resolves CLSIDs through HKCR, and HKCR merges `HKCU\Software\Classes` only
for a properly loaded user profile. A process started by VMware's guest
operations for an account that has never logged on interactively does not get
that merge. The registration and the DLL are both correct — `DllGetClassObject`
called by hand returns `S_OK` for our CLSID and `CLASS_E_CLASSNOTAVAILABLE` for a
wrong one, which is exactly right.

So `menutest` falls back to loading the DLL and calling `DllGetClassObject`
itself. That is not a workaround for a product bug: the same registration works
through COM on the host, where the user is logged in normally.

**The overlay handlers do not need that fallback.** They are registered under
HKLM, and `HKLM\Software\Classes` *does* merge into HKCR in a guest-operations
session - it is only the `HKCU` merge that is missing. In the VM the overlay
objects are created straight through `CoCreateInstance`, and the log says so:
`created via: COM registration`. The menu, registered per-user, still uses the
direct load.

## Overlay slots on the VM

The Windows 7 guest has **2** overlay handlers registered
(`EnhancedStorageShell`, `SharingPrivate`) against the host's 35. **Thirteen of
the fifteen slots are free**, which is exactly why overlay development belongs
there and not on a working desktop.

# Icon overlays

Two handlers, one per level, in the same DLL:

| CLSID | Level | Badge | Priority |
|---|---|---|---|
| `...9B0D` | 重要 | `icons/important.ico` | 0 |
| `...9B0E` | 可清理 | `icons/disposable.ico` | 1 |

`IsMemberOf` is the hot path - Explorer calls it once per item it draws, on its
UI thread - so the common answer costs one call:

```cpp
if (GetFileAttributesW("<path>:SpaceRecycle.Importance") == INVALID_FILE_ATTRIBUTES)
    return S_FALSE;               // unmarked: one syscall, no handle, no parse
return ReadLevel(path) == token ? S_OK : S_FALSE;   // marked: one read on top
```

The existence test runs first deliberately. `GetFileAttributesW` against a
stream path needs no handle and no read, so a folder full of unmarked files -
the overwhelmingly common case - pays a single syscall each.

Handlers are registered under **HKLM** only: Windows reads overlays from
`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers`
and has no per-user equivalent, unlike the context menu. `DllRegisterServer`
therefore returns **S_FALSE** when the menu registered but the overlays did not,
so an unelevated install reports a partial result instead of a success that did
not happen.

The registry key names carry 13 leading spaces; see "How to place a handler
inside the 15" below for why that number is load-bearing.

## Verified on Windows 7 SP1 x64

![overlay badges on a Windows 7 desktop](vm/evidence/win7-overlay-icons.png)

Six files on the guest desktop: two tagged 重要, two tagged 可清理, two
untagged. Explorer loaded the DLL (`tasklist /m SpaceRecycleShell.dll` reports it
inside `explorer.exe`) and drew the matching badge on the tagged files and
nothing on the untagged ones. The full screenshot is
`vm/evidence/win7-overlay-verified.png`.

That is the end-to-end proof, and it is the only kind available: an overlay
handler is told about paths and never learns what was drawn, so it cannot report
whether Explorer used it. The alternatives would have been a screenshot or
nothing.

Note that the guest's console session had to be restarted for Explorer to
re-read the handler list. Killing `explorer.exe` did **not** bring it back even
with `AutoRestartShell=1`, which left the session with no shell; a reboot fixed
it, and the auto-logon account returned it to the
desktop unattended. Reboot, don't taskkill.

# Icon overlays: what was measured, and why the obvious design cannot work

The natural reading of `IShellIconOverlayIdentifier` is "the handler returns an
icon, so it can return a different one per level". **It cannot.** This was
settled empirically rather than from documentation, because the answer decides
the whole design.

## The API

```cpp
IShellIconOverlayIdentifier : public IUnknown {
    HRESULT IsMemberOf(LPCWSTR pwszPath, DWORD dwAttrib);   // per file: does this handler apply?
    HRESULT GetOverlayInfo(LPWSTR pwszIconFile, int cchMax, int *pIndex, DWORD *pdwFlags);
    HRESULT GetPriority(int *pPriority);
};
```

`GetOverlayInfo` takes **no file path**. `IsMemberOf` gets the path but returns
only S_OK / S_FALSE — there is no channel for an icon.

## The measurement

A probe handler (`probe/`) logged every call from inside `explorer.exe`:

```
#0 GetOverlayInfo n=1  remembered_path=(none yet)     <- once, at load
#1 GetPriority    n=1
#2 … #132 IsMemberOf n=1…131                          <- once per file drawn
```

* `GetOverlayInfo` is called **exactly once per process**, at load time, before
  any file is known. No call to it ever follows an `IsMemberOf`.
* `IsMemberOf` is called once per file (131 in one window, 17 in another).

So a handler gets one chance to name an icon and never learns which file is
being drawn. **N distinct overlay icons require N handlers, and N handlers
require N registry entries — one slot each.** Returning an icon that already
exists saves nothing: the budget counts entries, not images.

## How to place a handler inside the 15

Explorer keeps the first 15 entries in a **character-by-character** comparison
of the `ShellIconOverlayIdentifiers` subkey name. Because a space sorts below
every letter, *more leading spaces wins* — at the first position where two names
differ, the one with a space there comes first.

That is why OneDrive registers `OneDrive1..7` with **13 leading spaces**, and it
is why a name with only 2 spaces sits at position 24 of 35 and is silently
ignored. A handler named with 13 spaces and beginning with a letter before `T`
lands at position 8, displacing only the least important Tortoise entry.

This machine, measured:

| Position | Entry |
|---|---|
| 1–7 | `             OneDrive1..7` |
| 8–15 | `             Tortoise1Normal..Tortoise8Ignored` |
| 16+ | `.WorkspaceExt0..6`, the 2-space Tortoise copies, `Tortoise9Unversioned`, … (ignored) |

35 entries registered, 15 honoured. **There is no free slot.**

## Consequence for this project

Any overlay this app registers displaces an existing one. Two levels would take
two slots, pushing out two Tortoise state icons.

The plan is therefore:

1. Ship the context menu on its own (done — HKCU, zero footprint).
2. Develop and test the overlays **in a VM** (done — the guest has 13 free
   slots, so nothing had to be displaced to prove the design).
3. Before installing on a real machine, compute whether the entries would land
   inside the 15 and say so, rather than registering handlers that will never
   run. `register.ps1 -Action status` already reports the resulting position and
   flags it when it falls outside the honoured range.

The probe under `probe/` is kept because it is the instrument that produced
these numbers; re-run it on any new Windows build before trusting the layout.