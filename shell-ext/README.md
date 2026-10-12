# SpaceRecycle Shell Extension

Explorer integration: a right-click submenu that tags files and folders with an
importance level.

```
SpaceRecycle ▸  重要       (orange)   cleanup may quarantine or back it up, never delete
                可清理     (green)    cleanup may delete it permanently
                清除标记   (grey)     remove the mark
```

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

# Icon overlays: what was measured, and why the obvious design does not work

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
2. Do overlay development and testing **in a VM**, where displacing entries
   costs nothing.
3. At install time, compute whether the entry would land inside the 15 and say
   so, rather than registering a handler that will never run.

The probe under `probe/` is kept because it is the instrument that produced
these numbers; re-run it on any new Windows build before trusting the layout.