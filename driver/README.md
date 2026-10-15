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

## The filter originally saw every delete on the machine

The first successful run logged Windows Update's `tmp.edb`, a Defender lock
file, and both test files. That is not noise to be filtered out of the log - it
is the shape of the feature, and it was the wrong shape. A round trip to user
mode per delete, with the deleting thread blocked, cost 132 escalations in 27
idle minutes to make 8 decisions that mattered. A refusal made it worse: Windows
Search re-attempted the same temp log thousands of times in four seconds, each
attempt another round trip.

The driver now tests the mark itself before asking anyone.
`SpaceRecycle.Importance` is a stream on the file, so the test is one open
against a name that is not in the file's own record - no message, no context
switch. Measured after the change, in a run that made 961 delete decisions: 46
escalations, every one of them a file somebody had marked.

The failure handling is deliberately asymmetric. Only `STATUS_OBJECT_NAME_NOT_FOUND`
counts as unmarked; any other error escalates, because a mark that cannot be read
is not the same as a mark that is not there and treating the two alike is a
silent hole.

## The disposition is pushed at runtime, over its own port

`FltCreateCommunicationPort` opens with `MaxConnections = 2` and the service
connects twice, stating its role in the connect context:

| Connection | Role | Used for |
|---|---|---|
| requests | `SR_PORT_ROLE_REQUESTS` | `FltSendMessage` asks; a thread blocks in `FilterGetMessage` |
| policy | `SR_PORT_ROLE_POLICY` | `FilterSendMessage` pushes; nothing ever blocks on it |

Two connections are not tidiness. One handle cannot carry both directions at
once: a `FilterSendMessage` issued while the other side is blocked in
`FilterGetMessage` on the same handle corrupts the receive - it returns success
without writing the buffer. The request that was genuinely waiting is then
answered by nobody, the three-second timeout expires, the driver fails open, and
the file the service just refused is deleted. It looked exactly like a policy
bug, and the log showed it as the push, then an unfilled receive, then the retry
that got refused too late.

`ConnectionPortCookie` carries the role back into the disconnect callback, so a
policy client going away does not tear down the request port.

Two findings from getting there are worth keeping:

- The receive buffer was zeroed before the call, which made an unfilled receive
  indistinguishable from a message that legitimately contained zeros. It is
  filled with a sentinel now, and `MessageId` is checked because the filter
  manager never assigns zero.
- Deferring the push onto the guard thread and waking it with
  `CancelSynchronousIo` does not work - that call does not interrupt a blocked
  `FilterGetMessage`. The change sat unapplied until the next delete arrived, and
  that delete was then consumed applying the policy instead of being answered.

## Verified in the guest, end to end

Four phases, with the guard log as the meter: it records every delete the driver
escalated, so "the log did not grow" is a direct measurement of "nothing crossed
to user mode".

| Phase | Policy | Log | File | Meaning |
|---|---|---|---|---|
| A1 unmarked | ask | flat | deleted | never reaches user mode |
| A2 marked | ask | +2 | refused | escalated and denied |
| B marked | deny | flat | refused | refused locally, no IPC |
| C marked | allow | flat | deleted | permitted locally, no IPC |
| D marked | ask | +2 | refused | back to escalating |

The run reported `connects=2 disconnects=0 replaced=0 request_port=1 policy_port=1`,
and contained no unfilled receives, no reconnects and no reply failures.

Two harness faults had been reporting product defects that did not exist, and
both are guarded against in `vm/policy-test.cmd`. Phases used to run after the
service had been deleted and not reinstalled - and with no connected service the
driver fails open by design, so every file deletes, which is exactly what a
working "allow" policy looks like. And the wait for a policy change searched the
whole append-only log, so it matched a line from an earlier run and returned
instantly, "confirming" a policy that had not been applied yet. The phases now
assert the service is running and wait only on log lines written after the phase
began.

