# SpaceRecycle delete filter

A minifilter driver that sees every delete before it happens, and a user-mode
listener that answers. Milestone 1 is done: the filter loads, intercepts, the
path crosses to user mode and back, and the reply round-trip works. It refuses
nothing yet - policy is the next milestone.

```
srdelete.sys  ── intercept ──►  filter port  ──►  srlisten.exe
     ▲                                                  │
     └──────────── allow / deny (17-byte reply) ◄────────┘
```

## Why a kernel driver at all

Measured on the host, not assumed:

| Mechanism | Blocks a delete? | Can ask the user? | Coverage |
|---|---|---|---|
| DENY DELETE ACE | **yes** | no - "access denied" | all callers |
| open handle without `FILE_SHARE_DELETE` | **yes** | no - "file in use" | while held |
| copy hook (`ICopyHook`) | yes | yes | Explorer, folders only |
| **minifilter** | yes | **yes** | everything |

Both user-mode mechanisms work, and both are wrong for this product:

* They surface as an operating-system error rather than a question, and the
  requirement is "prompt the user or forbid it".
* They break replace-on-save. Word and most editors write a temporary file and
  rename it over the target; that needs `DELETE` on the target, so a deny-ACE or
  a held handle can turn "save" into data loss.

Only the filter manager sees the operation before it happens, on every path -
Explorer, `cmd`, scripts, any application - which is what the requirement needs.

## Distribution is gated on a certificate, not on code

A driver that ships needs an **EV code-signing certificate** (roughly
¥2000-4000/year, and it requires a verified legal entity) **plus a Microsoft
attestation signature** obtained through the Partner Center. Neither can be
produced locally. Without both, the driver will not load on any ordinary
Windows 10 or 11 machine.

Development and testing use test signing instead. That is a local-only
arrangement and does not carry over.

## What was built

| File | Purpose |
|---|---|
| `src/srdelete.c` | the minifilter |
| `tools/srlisten.cpp` | user-mode listener, logs every intercepted delete |
| `build.cmd` | compiles both with `cl` / `link` directly |
| `sign-test.ps1` | creates a test certificate and signs the driver |
| `vm/*.cmd` | guest-side install, update and test scripts |

`build.cmd` calls the toolchain directly rather than going through an MSBuild
driver project: the WDK's Visual Studio integration is not installed here, and
for one source file the raw tools are easier to inspect. Two flags are
load-bearing - `/D_AMD64_`, which the WDK build would normally supply (without
it `ntdef.h` stops at `#error No Target Architecture`), and
`/D_WIN32_WINNT=0x0601`, because a down-level driver loads on newer systems
too and the Windows 7 VM is the only machine available to test on.

## Verified in the Windows 7 SP1 x64 guest

```
connected
intercepted DELETE
    device : \Device\HarddiskVolume1\sr-test\deltest\ordinary.txt
    dos    : C:\sr-test\deltest\ordinary.txt
intercepted DELETE
    device : \Device\HarddiskVolume1\sr-test\deltest\marked.txt
    dos    : C:\sr-test\deltest\marked.txt
```

Four interceptions, zero reply failures. Both files were deleted, which is
correct for this milestone: it observes, it does not refuse.

The driver reports the device path that `FltGetFileNameInformation` returns;
the listener converts it to a DOS path with `QueryDosDevice`. Doing the
translation in user mode keeps the kernel side to one name lookup and keeps the
drive-letter table somewhere it can be tested without a debugger.

---

# Traps, in the order they cost time

## The port refuses an unelevated caller

`FltCreateCommunicationPort` was given a `NULL` security descriptor, so the
filter manager applies its default: only LocalSystem and elevated
administrators may connect. An unelevated administrator account gets
`0x80070005`.

**This is the right default, not a bug to work around.** The thing that answers
the delete question is a service, and a service runs as SYSTEM. The test harness
uses a SYSTEM scheduled task for the same reason.

## The reply size must be exact, and a padded struct is not

`FilterReplyMessage` wants `sizeof(FILTER_REPLY_HEADER) + payload`. Passing
`sizeof(reply_struct)` gives 24 instead of 17, because the structure holds a
`LARGE_INTEGER` and the compiler pads 17 bytes of content out to 24. The filter
manager compares against what the filter declared and rejects a padded size with
**`ERROR_MORE_DATA`** - which reads like the buffer was too small when it was in
fact too big.

The fix is to compute the size rather than take the struct's:

```cpp
const DWORD replySize = (DWORD)(sizeof(FILTER_REPLY_HEADER) + sizeof(SR_REPLY));
FilterReplyMessage(port, &reply.Header, replySize);
```

The listener now logs `message.Header.ReplyLength`, which is how the expected
size was pinned down instead of guessed at.

## A filter with no unload callback can never be unloaded

`FLT_REGISTRATION` documents a `NULL` `FilterUnloadCallback` as meaning the
filter can never be unloaded, and the first attempt to swap in a new build shows
exactly that: `STATUS_FLT_DO_NOT_DETACH` (`0x801f0010`).

The consequence is worse than it sounds - **every development iteration needs a
reboot**, and a shipped filter could not be updated or removed without one. A
`FilterUnloadCallback` that closes the communication port and returns
`STATUS_SUCCESS` fixes it; the filter manager performs the deregistration
itself.

## Unloading erases the instance registration

After a successful unload, the `Instances` key (with `DefaultInstance` and
`Altitude`) is gone from the service key, and `Enum` carries
`INITSTARTFAILED=1`. A reload then fails with `0x80070002` - "the system cannot
find the file specified" - which points at the image rather than at the missing
instance data that is actually the problem.

Without an INF, the altitude has to be rewritten on every reload. A real
installer writes an INF and never hits this.

## Test signing accepts a signed driver, not an unsigned one

`testsigning on` plus `nointegritychecks on` were both set and the load still
failed with `0x80070241` (`STATUS_INVALID_IMAGE_HASH`). Test mode relaxes *which*
certificate is trusted; it does not remove the requirement for a signature.
`sign-test.ps1` creates a self-signed code-signing certificate, signs with
SHA-256, and installs the certificate into the guest's Root and
TrustedPublisher stores.

## stdio dies under VMware guest operations

The first listener used `printf` and exited immediately with
`STATUS_INVALID_CRUNTIME_PARAMETER` (`0xC0000417`), leaving a log containing
nothing but a byte-order mark. A process started through guest operations may
have no valid stdout to print to, and the CRT's response to that is to
terminate.

`srlisten.exe` now logs with `CreateFileW` / `WriteFile` and writes a step
marker before each stage, so if it ever dies again the log still says how far it
got.

## The filter sees every delete on the machine

The first successful run logged Windows Update's `tmp.edb`, a Defender lock
file, and both test files. That is not noise to be filtered out of the log - it
is the performance shape of the feature. A round trip to user mode per delete,
with the deleting thread blocked, is the current cost.

Before this ships, the policy has to live where the common case is cheap: the
driver should hold a set of protected prefixes pushed down from user mode and
answer locally, only calling out for paths it cannot decide. That is the
standard minifilter arrangement and it is the next thing to design, not an
optimisation to bolt on later.

---

# Next: milestone 2

The listener answers a constant `TRUE`. Milestone 2 makes it read
`<path>:SpaceRecycle.Importance` - the same ADS the shell extension writes - and
answer `FALSE` for a file marked 重要, so the delete fails with
`STATUS_ACCESS_DENIED`. The pieces are all in place: the round trip works, the
paths arrive in DOS form, and the ADS already carries the level.
