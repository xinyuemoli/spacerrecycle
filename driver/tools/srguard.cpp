// SpaceRecycle - delete guard service.
//
// Milestone 2 and 3. It answers the filter's question with real policy: a file
// marked 重要 in its ADS is refused, anything else is allowed. It runs as a
// service under LocalSystem, which is what lets it connect to the filter port at
// all - FltCreateCommunicationPort was given a NULL security descriptor, so the
// filter manager admits only LocalSystem and elevated administrators.
//
// Order matters and is deliberate: the answer is sent BEFORE the user is asked.
//
//   delete attempt -> filter -> service -> "deny" (immediately)
//                                       -> then prompt the user
//
// The alternative - hold the kernel waiting while a dialog is up - is worse in
// both directions. The filter's timeout is 3 seconds, so a user who has not
// clicked by then gets the delete allowed, which is the protection failing
// quietly. Raising the timeout instead parks the deleting thread for as long as
// the dialog is open, which hangs Explorer and any command prompt doing the
// delete. Deny-then-ask makes the protection deterministic and keeps every
// timeout short.
//
// If the user then says "allow", the service performs the delete itself after
// clearing the mark, so the outcome they asked for still happens.
//
// Usage:
//   srguard                 run as a service (started by the SCM)
//   srguard console         run in the foreground, for testing
//   srguard install         register the service (needs elevation)
//   srguard uninstall       remove the service
//   srguard -noprompt       console mode without dialogs, for automated tests

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <fltuser.h>
#include <wtsapi32.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <string>
#include <new>

#define SR_PORT_NAME       L"\\SpaceRecycleDeletePort"
#define SR_SERVICE_NAME    L"SpaceRecycleGuard"
#define SR_SERVICE_DISPLAY L"SpaceRecycle delete guard"
#define SR_STREAM_NAME     L":SpaceRecycle.Importance"
#define SR_MAX_PATH_CHARS  520
#define SR_OP_DELETE       1

typedef struct _SR_NOTIFY {
    ULONG Size;
    ULONG Operation;
    WCHAR Path[SR_MAX_PATH_CHARS];
} SR_NOTIFY, *PSR_NOTIFY;

typedef struct _SR_REPLY {
    BOOLEAN Allow;
} SR_REPLY, *PSR_REPLY;

typedef struct _SR_MESSAGE {
    FILTER_MESSAGE_HEADER Header;
    SR_NOTIFY Payload;
} SR_MESSAGE, *PSR_MESSAGE;

typedef struct _SR_REPLY_MESSAGE {
    FILTER_REPLY_HEADER Header;
    SR_REPLY Payload;
} SR_REPLY_MESSAGE, *PSR_REPLY_MESSAGE;

/* Must match the driver's definitions exactly. */
#define SR_ACTION_ASK   0
#define SR_ACTION_DENY  1
#define SR_ACTION_ALLOW 2
#define SR_CMD_SET_POLICY 1
#define SR_CMD_GET_POLICY 2

typedef struct _SR_POLICY {
    ULONG Size;
    ULONG Version;
    ULONG ActionForMarked;
    ULONG ActionForUnmarked;
    ULONG CheckMark;
} SR_POLICY, *PSR_POLICY;

/* Must match the driver's SR_STATUS. The counters are how a driver with no
 * console reports what its decision path is actually doing. */
typedef struct _SR_STATUS {
    SR_POLICY Policy;
    ULONG MarkChecked;
    ULONG MarkYes;
    ULONG MarkNo;
    ULONG Decisions;
    ULONG Escalated;
    ULONG LastMarkStatus;
    ULONG Connects;
    ULONG Disconnects;
    ULONG PortReplaced;
} SR_STATUS, *PSR_STATUS;

static HANDLE gLog = INVALID_HANDLE_VALUE;
static SERVICE_STATUS gStatus = {0};
static SERVICE_STATUS_HANDLE gStatusHandle = nullptr;
static HANDLE gStopEvent = nullptr;
static HANDLE gPort = INVALID_HANDLE_VALUE;
static bool gNoPrompt = false;
static std::wstring gPolicyPath;
static volatile LONG gPolicyPushCount = 0;
static DWORD gGuardThreadId = 0;

/* ------------------------------------------------------------------ logging */

static void Log(const char* fmt, ...) {
    char body[1800];
    va_list args;
    va_start(args, fmt);
    int n = _vsnprintf_s(body, sizeof(body), _TRUNCATE, fmt, args);
    va_end(args);
    if (n < 0) {
        return;
    }
    SYSTEMTIME st;
    GetLocalTime(&st);
    char line[2048];
    int m = _snprintf_s(line, sizeof(line), _TRUNCATE,
                        "%04d-%02d-%02d %02d:%02d:%02d %s\r\n",
                        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, body);
    if (m < 0) {
        return;
    }
    // WriteFile rather than stdio: a service has no console, and an earlier
    // stdio-based version died with STATUS_INVALID_CRUNTIME_PARAMETER when its
    // stdout was not valid.
    if (gLog != INVALID_HANDLE_VALUE) {
        DWORD written = 0;
        WriteFile(gLog, line, (DWORD)m, &written, nullptr);
    }
}

static void OpenLog() {
    wchar_t dir[MAX_PATH] = {0};
    DWORD n = GetEnvironmentVariableW(L"ProgramData", dir, _countof(dir));
    std::wstring path;
    std::wstring base;
    if (n > 0 && n < _countof(dir)) {
        base.assign(dir);
        base += L"\\SpaceRecycle";
        CreateDirectoryW(base.c_str(), nullptr);
        path = base + L"\\guard.log";
        gPolicyPath = base + L"\\policy.ini";
    } else {
        path = L"srguard.log";
        gPolicyPath = L"policy.ini";
    }
    gLog = CreateFileW(path.c_str(), FILE_APPEND_DATA,
                       FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr,
                       OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
}

/* --------------------------------------------------------------- path + ADS */

/// The filter reports \Device\HarddiskVolumeN\... ; everything else wants C:\...
static bool DevicePathToDos(const wchar_t* devicePath, std::wstring& out) {
    wchar_t drives[512] = {0};
    if (GetLogicalDriveStringsW(_countof(drives) - 1, drives) == 0) {
        return false;
    }
    for (const wchar_t* p = drives; *p != L'\0'; p += wcslen(p) + 1) {
        wchar_t letter[4] = {p[0], p[1], L'\0', L'\0'};
        wchar_t target[1024] = {0};
        if (QueryDosDeviceW(letter, target, _countof(target)) == 0) {
            continue;
        }
        const size_t len = wcslen(target);
        if (len > 0 && _wcsnicmp(devicePath, target, len) == 0) {
            out.assign(letter);
            out.append(devicePath + len);
            return true;
        }
    }
    return false;
}

/// Reads the marks level. Empty means unmarked, and every failure mode is
/// treated as unmarked: a file must never be protected because the reader could
/// not read it, and it must never be unprotected because the reader lied.
static std::wstring ReadLevel(const std::wstring& path) {
    std::wstring stream = path + SR_STREAM_NAME;
    if (GetFileAttributesW(stream.c_str()) == INVALID_FILE_ATTRIBUTES) {
        return std::wstring();
    }
    HANDLE h = CreateFileW(stream.c_str(), GENERIC_READ,
                           FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                           nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) {
        return std::wstring();
    }
    char buf[128] = {0};
    DWORD read = 0;
    BOOL ok = ReadFile(h, buf, sizeof(buf) - 1, &read, nullptr);
    CloseHandle(h);
    if (!ok || read == 0) {
        return std::wstring();
    }
    buf[read] = '\0';

    std::string s(buf);
    size_t p1 = s.find('|');
    if (p1 == std::string::npos || s.compare(0, p1, "v1") != 0) {
        return std::wstring();
    }
    size_t p2 = s.find('|', p1 + 1);
    std::string token = s.substr(p1 + 1, (p2 == std::string::npos ? s.size() : p2) - p1 - 1);
    return std::wstring(token.begin(), token.end());
}

/* ------------------------------------------------------------------ prompts */

// Defined in the policy section below; the prompt worker needs it first.
static void DeleteWithMarkRemoved(const std::wstring& dosPath);

/// Asks the interactive user whether to let this particular delete through.
/// Returns true only for an explicit yes; a timeout, a closed dialog or no
/// session at all all mean "keep the file".
static bool AskUser(const std::wstring& dosPath) {
    if (gNoPrompt) {
        Log("    prompt suppressed (test mode); keeping the file");
        return false;
    }
    DWORD session = WTSGetActiveConsoleSessionId();
    if (session == 0xFFFFFFFF) {
        Log("    no interactive session to ask; keeping the file");
        return false;
    }

    std::wstring message =
        L"这个文件被标记为重要，已阻止删除：\n\n" + dosPath +
        L"\n\n要允许删除并移除它的标记吗？";

    DWORD response = 0;
    BOOL shown = WTSSendMessageW(
        WTS_CURRENT_SERVER_HANDLE, session,
        const_cast<LPWSTR>(L"SpaceRecycle"),
        (DWORD)(wcslen(L"SpaceRecycle") * sizeof(wchar_t)),
        const_cast<LPWSTR>(message.c_str()),
        (DWORD)(message.size() * sizeof(wchar_t)),
        MB_YESNO | MB_ICONWARNING | MB_SYSTEMMODAL | MB_SETFOREGROUND,
        30, &response, TRUE);

    if (!shown) {
        Log("    WTSSendMessage failed (%lu); keeping the file", GetLastError());
        return false;
    }
    Log("    user answered %s", response == IDYES ? "YES" : "no/timeout");
    return response == IDYES;
}

/// Prompts on its own thread, and deletes if the user says yes.
///
/// This must not run on the decision loop. The first version did, and the
/// result was a hole rather than a slowdown: the loop is the only thing calling
/// FilterGetMessage, so while it sat inside a dialog nothing answered the
/// filter. Every further delete waited in the port queue until the driver's
/// three-second timeout expired, and the driver fails open on timeout - so
/// while one protected file was being asked about, every other delete on the
/// machine was silently allowed. A prompt that costs the protection it is
/// supposed to provide is worse than no prompt.
static DWORD WINAPI PromptThread(LPVOID parameter) {
    std::wstring* path = static_cast<std::wstring*>(parameter);
    if (path != nullptr) {
        if (AskUser(*path)) {
            DeleteWithMarkRemoved(*path);
        }
        delete path;
    }
    return 0;
}

static void PromptAsync(const std::wstring& dosPath) {
    std::wstring* copy = new (std::nothrow) std::wstring(dosPath);
    if (copy == nullptr) {
        return;
    }
    HANDLE thread = CreateThread(nullptr, 0, PromptThread, copy, 0, nullptr);
    if (thread == nullptr) {
        Log("    could not start the prompt thread (%lu)", GetLastError());
        delete copy;
        return;
    }
    CloseHandle(thread);
}

/* ------------------------------------------------------------------- policy */

/// Handles one intercepted delete. Returns the answer to send back to the
/// filter: true allows the delete.
static bool Decide(const std::wstring& dosPath) {
    std::wstring level = ReadLevel(dosPath);
    if (level.empty()) {
        return true;                    // unmarked: not our business
    }
    if (level != L"important") {
        Log("    marked '%ls' (not protected); allowing", level.c_str());
        return true;
    }
    return false;                       // important: refuse
}

/// After the user has explicitly allowed the delete, the service carries it out
/// itself. The stream is removed first, because a second delete would otherwise
/// be refused by this same policy and the file could never be removed.
static void DeleteWithMarkRemoved(const std::wstring& dosPath) {
    std::wstring stream = dosPath + SR_STREAM_NAME;
    if (!DeleteFileW(stream.c_str()) && GetLastError() != ERROR_FILE_NOT_FOUND) {
        Log("    could not clear the mark (error %lu)", GetLastError());
        return;
    }
    if (DeleteFileW(dosPath.c_str())) {
        Log("    deleted on the user's instruction");
    } else {
        Log("    delete failed (error %lu)", GetLastError());
    }
}

/* ------------------------------------------------------------------ policy */

static const char* ActionName(ULONG action) {
    switch (action) {
    case SR_ACTION_ASK:   return "ask";
    case SR_ACTION_DENY:  return "deny";
    case SR_ACTION_ALLOW: return "allow";
    default:              return "?";
    }
}

static bool ParseAction(const std::string& value, ULONG* out) {
    if (value == "ask")   { *out = SR_ACTION_ASK;   return true; }
    if (value == "deny")  { *out = SR_ACTION_DENY;  return true; }
    if (value == "allow") { *out = SR_ACTION_ALLOW; return true; }
    return false;
}

/// Reads the settings file. Absent or unreadable is not an error: it is the
/// state before anyone has configured anything, and the built-in defaults apply.
///
/// A flat key=value file rather than JSON on purpose - the service would need a
/// parser to read two integers, and that parser would be more code than the
/// feature it feeds. When the settings screen arrives it can write this format
/// directly, or talk to the service over a pipe and skip the file entirely.
///
///   action_marked=ask|deny|allow
///   action_unmarked=allow|deny|ask
///   check_mark=1|0
static void LoadPolicyFile(SR_POLICY* policy) {
    HANDLE h = CreateFileW(gPolicyPath.c_str(), GENERIC_READ,
                           FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr,
                           OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) {
        return;
    }
    char buffer[512] = {0};
    DWORD read = 0;
    ReadFile(h, buffer, sizeof(buffer) - 1, &read, nullptr);
    CloseHandle(h);
    buffer[read] = '\0';

    char* context = nullptr;
    for (char* line = strtok_s(buffer, "\r\n", &context); line != nullptr;
         line = strtok_s(nullptr, "\r\n", &context)) {
        while (*line == ' ' || *line == '\t') {
            ++line;
        }
        if (*line == '\0' || *line == '#' || *line == ';') {
            continue;
        }
        char* equals = strchr(line, '=');
        if (equals == nullptr) {
            continue;
        }
        *equals = '\0';
        std::string key(line);
        std::string value(equals + 1);
        while (!value.empty() && (value[value.size() - 1] == ' ' || value[value.size() - 1] == '\t')) {
            value.erase(value.size() - 1);
        }
        while (!key.empty() && (key[key.size() - 1] == ' ' || key[key.size() - 1] == '\t')) {
            key.erase(key.size() - 1);
        }

        if (key == "action_marked") {
            if (!ParseAction(value, &policy->ActionForMarked)) {
                Log("policy: unknown action_marked '%s' (ignored)", value.c_str());
            }
        } else if (key == "action_unmarked") {
            if (!ParseAction(value, &policy->ActionForUnmarked)) {
                Log("policy: unknown action_unmarked '%s' (ignored)", value.c_str());
            }
        } else if (key == "check_mark") {
            policy->CheckMark = (value == "0") ? 0 : 1;
        } else {
            Log("policy: unknown key '%s' (ignored)", key.c_str());
        }
    }
}

/// Sends the policy and checks what came back.
///
/// The push answers with the applied policy and the driver's counters, so one
/// call both applies and verifies. FilterSendMessage succeeding only means the
/// message was delivered; without the echo, a driver that rejected the structure
/// would look exactly like a successful push and the settings screen would
/// report a change that never took effect.
static bool PushPolicy(const SR_POLICY& policy) {
    if (gPort == INVALID_HANDLE_VALUE) {
        return false;
    }

    unsigned char inBuffer[sizeof(ULONG) + sizeof(SR_POLICY)];
    ULONG command = SR_CMD_SET_POLICY;
    memcpy(inBuffer, &command, sizeof(command));
    memcpy(inBuffer + sizeof(command), &policy, sizeof(policy));

    SR_STATUS status;
    ZeroMemory(&status, sizeof(status));
    DWORD returned = 0;
    HRESULT hr = FilterSendMessage(gPort, inBuffer, (DWORD)sizeof(inBuffer),
                                   &status, sizeof(status), &returned);
    if (FAILED(hr)) {
        Log("policy push rejected: hr=0x%08lX", (unsigned long)hr);
        return false;
    }
    if (returned < sizeof(SR_STATUS)) {
        Log("policy push returned %lu bytes, expected %lu",
            (unsigned long)returned, (unsigned long)sizeof(SR_STATUS));
        return false;
    }
    if (memcmp(&status.Policy, &policy, sizeof(SR_POLICY)) != 0) {
        Log("policy MISMATCH: asked marked=%s unmarked=%s, driver holds marked=%s unmarked=%s",
            ActionName(policy.ActionForMarked), ActionName(policy.ActionForUnmarked),
            ActionName(status.Policy.ActionForMarked), ActionName(status.Policy.ActionForUnmarked));
        return false;
    }

    InterlockedIncrement(&gPolicyPushCount);
    Log("policy applied: marked=%s unmarked=%s check_mark=%lu",
        ActionName(policy.ActionForMarked), ActionName(policy.ActionForUnmarked),
        (unsigned long)policy.CheckMark);
    Log("  driver counters: decisions=%lu escalated=%lu  mark_checked=%lu yes=%lu no=%lu  last_mark_status=0x%08lX",
        (unsigned long)status.Decisions, (unsigned long)status.Escalated,
        (unsigned long)status.MarkChecked, (unsigned long)status.MarkYes,
        (unsigned long)status.MarkNo, (unsigned long)status.LastMarkStatus);
    Log("  driver port: connects=%lu disconnects=%lu replaced=%lu",
        (unsigned long)status.Connects, (unsigned long)status.Disconnects,
        (unsigned long)status.PortReplaced);
    return true;
}

/// Watches the settings file and re-pushes when it changes.
///
/// This is what makes the disposition changeable while the driver keeps running:
/// nothing here reloads a driver or restarts a service, it just sends a new
/// structure down the port that was already open. An edited file takes effect
/// within a couple of seconds.
///
/// Known limitation, and the next thing to fix. A single port handle is used for
/// both directions, and a FilterSendMessage issued from this thread while the
/// guard thread is blocked in FilterGetMessage on the same handle puts the port
/// in a bad state: the receive comes back reporting success with the buffer
/// never written, and the request that was genuinely waiting goes unanswered,
/// so the driver times out and fails open. The failure is visible in the log as
/// "discarding an unfilled receive" and it costs one delete its protection.
/// The fix is a second connection - the driver already receives a connect
/// context and a disconnect cookie, so it can hold one port for requests and
/// another for policy, and the two never overlap. Deferring the push to the
/// guard thread was tried instead and does not work: CancelSynchronousIo does
/// not interrupt a blocked FilterGetMessage.
static DWORD WINAPI PolicyWatcher(LPVOID) {
    SR_POLICY policy = {sizeof(SR_POLICY), 1, SR_ACTION_ASK, SR_ACTION_ALLOW, 1};
    LoadPolicyFile(&policy);

    FILETIME lastWrite = {0, 0};
    WIN32_FILE_ATTRIBUTE_DATA attributes;
    if (GetFileAttributesExW(gPolicyPath.c_str(), GetFileExInfoStandard, &attributes)) {
        lastWrite = attributes.ftLastWriteTime;
    }

    for (;;) {
        if (WaitForSingleObject(gStopEvent, 2000) == WAIT_OBJECT_0) {
            break;
        }
        if (!GetFileAttributesExW(gPolicyPath.c_str(), GetFileExInfoStandard, &attributes)) {
            continue;
        }
        if (CompareFileTime(&attributes.ftLastWriteTime, &lastWrite) == 0) {
            continue;
        }
        lastWrite = attributes.ftLastWriteTime;

        SR_POLICY updated = policy;
        LoadPolicyFile(&updated);
        if (memcmp(&updated, &policy, sizeof(SR_POLICY)) != 0) {
            policy = updated;
            PushPolicy(policy);
        }
    }
    return 0;
}

/* --------------------------------------------------------------- main loop */

/// Attaches to the filter port, retrying while the driver is still starting.
static bool ConnectToFilter() {
    HRESULT hr = E_FAIL;
    for (int attempt = 1; attempt <= 30; ++attempt) {
        hr = FilterConnectCommunicationPort(SR_PORT_NAME, 0, nullptr, 0, nullptr, &gPort);
        if (SUCCEEDED(hr)) {
            return true;
        }
        if (attempt == 1 || attempt % 5 == 0) {
            Log("connect attempt %d failed: hr=0x%08lX", attempt, (unsigned long)hr);
        }
        Sleep(1000);
    }
    Log("cannot reach the filter (hr=0x%08lX)", (unsigned long)hr);
    return false;
}

static DWORD WINAPI GuardLoop(LPVOID) {
    gGuardThreadId = GetCurrentThreadId();
    OpenLog();
    Log("guard starting (pid %lu, prompt=%s)", GetCurrentProcessId(),
        gNoPrompt ? "off" : "on");

    if (!ConnectToFilter()) {
        Log("guard idle");
        return 1;
    }
    Log("connected to the filter");

    /* Push the configured policy before starting the watcher, and do it on this
     * thread. The first version let the watcher do it, which meant the very
     * first FilterSendMessage ran at the same moment as the first
     * FilterGetMessage on the same port handle - and the receive failed with
     * E_FAIL, ending the loop and silently disabling the protection. Sending
     * once here removes that overlap at startup; the watcher only sends when
     * the settings file actually changes. */
    {
        SR_POLICY initial = {sizeof(SR_POLICY), 1, SR_ACTION_ASK, SR_ACTION_ALLOW, 1};
        LoadPolicyFile(&initial);
        PushPolicy(initial);
    }

    HANDLE watcher = CreateThread(nullptr, 0, PolicyWatcher, nullptr, 0, nullptr);
    if (watcher != nullptr) {
        CloseHandle(watcher);
    } else {
        Log("could not start the policy watcher (%lu); driver keeps its default",
            GetLastError());
    }

    HRESULT hr = E_FAIL;
    int receiveFailures = 0;
    int unfilledReceives = 0;
    for (;;) {
        if (WaitForSingleObject(gStopEvent, 0) == WAIT_OBJECT_0) {
            break;
        }

        SR_MESSAGE message;
        /* Fill with a sentinel rather than zeroing. FilterGetMessage can return
         * success without writing anything - the receive is simply not filled -
         * and against a zeroed buffer that is indistinguishable from a message
         * that legitimately contains zeros. The sentinel makes the difference
         * visible, which is how this was found. */
        memset(&message, 0xAB, sizeof(message));
        hr = FilterGetMessage(gPort, &message.Header, sizeof(message), nullptr);
        /* Nothing to do here: the policy watcher pushes changes on its own
         * thread. Deferring the push to this thread was tried and does not work
         * - CancelSynchronousIo does not interrupt a blocked FilterGetMessage,
         * so a change would sit unapplied until the next delete arrived, and
         * that delete would then be consumed applying the policy instead of
         * being answered. The driver failed open on the unanswered request and
         * the file was deleted. See the notes in PushPolicy about the real
         * remaining limitation. */

        if (FAILED(hr)) {
            /* A failed receive used to end the loop, and ending the loop is the
             * worst possible response: the driver keeps running and keeps
             * asking, and with nobody answering every delete fails open. The
             * protection disappears and the only trace is one line in a log.
             * Reconnect instead, and give up only if the port cannot be
             * re-established at all. */
            Log("FilterGetMessage failed: hr=0x%08lX; reconnecting", (unsigned long)hr);
            if (gPort != INVALID_HANDLE_VALUE) {
                CloseHandle(gPort);
                gPort = INVALID_HANDLE_VALUE;
            }
            if (++receiveFailures > 5) {
                Log("giving up after %d consecutive receive failures", receiveFailures);
                break;
            }
            Sleep(1000);
            if (!ConnectToFilter()) {
                Sleep(2000);
                continue;
            }
            Log("reconnected to the filter");
            continue;
        }
        receiveFailures = 0;

        /* Validate before trusting anything in it.
         *
         * This is the bug that cost the most to find. An unfilled receive used
         * to be processed as a real request: the path came out empty, the reply
         * carried MessageId 0, the filter manager ignored it, and the delete
         * that was genuinely waiting for an answer never got one. Three seconds
         * later the driver timed out and failed open, so a file the service had
         * just logged as REFUSED was deleted anyway. From the outside that
         * looked like a policy or a mark-check problem; it was a message that
         * was never written.
         *
         * MessageId is the reliable tell: the filter manager assigns one to
         * every message and it is never zero. The payload size confirms it. */
        if (message.Header.MessageId == 0 || message.Payload.Size != sizeof(SR_NOTIFY)) {
            Log("discarding an unfilled receive: message_id=%llu payload_size=0x%08lX",
                (unsigned long long)message.Header.MessageId,
                (unsigned long)message.Payload.Size);
            if (++unfilledReceives > 20) {
                Log("giving up after %d unfilled receives; reconnecting", unfilledReceives);
                unfilledReceives = 0;
                if (gPort != INVALID_HANDLE_VALUE) {
                    CloseHandle(gPort);
                    gPort = INVALID_HANDLE_VALUE;
                }
                Sleep(1000);
                ConnectToFilter();
            }
            continue;   /* never reply: there is nothing here to answer */
        }
        unfilledReceives = 0;

        std::wstring dos;
        const bool mapped = DevicePathToDos(message.Payload.Path, dos);
        const std::wstring display = mapped ? dos : std::wstring(message.Payload.Path);
        if (message.Payload.Path[0] == L'\0') {
            /* A real request with no name. Not the same as a path that failed
             * to map: the driver escalated this file, so it has an opinion
             * about it, and letting it through would silently permit something
             * we were asked about. */
            Log("  !! EMPTY PATH on a valid request: operation=%lu; refusing rather than guessing",
                (unsigned long)message.Payload.Operation);
        }
        Log("delete requested: %ls", display.c_str());

        /* Three cases, three different answers.
         *
         * A mapped path is the normal one. A named path on a volume with no
         * drive letter is not ours to judge and stays allowed. A request with
         * no name at all is a file the driver escalated - so it carries a mark -
         * and waving it through because we could not read its name is the
         * silent-failure shape this whole feature exists to avoid. */
        bool allow;
        if (mapped) {
            allow = Decide(dos);
        } else if (message.Payload.Path[0] == L'\0') {
            allow = false;
        } else {
            allow = true;
        }

        SR_REPLY_MESSAGE reply;
        ZeroMemory(&reply, sizeof(reply));
        reply.Header.MessageId = message.Header.MessageId;
        reply.Payload.Allow = allow ? TRUE : FALSE;
        // The exact size, not sizeof(SR_REPLY_MESSAGE): the structure is padded
        // to 24 bytes by the LARGE_INTEGER in the header, and the filter manager
        // rejects a padded size with ERROR_MORE_DATA.
        const DWORD replySize = (DWORD)(sizeof(FILTER_REPLY_HEADER) + sizeof(SR_REPLY));
        hr = FilterReplyMessage(gPort, &reply.Header, replySize);
        if (FAILED(hr)) {
            Log("    FilterReplyMessage failed: hr=0x%08lX", (unsigned long)hr);
            continue;
        }

        if (!allow) {
            Log("    REFUSED (marked important)");
            if (mapped) {
                // Dispatch, never wait: the loop must stay available to answer
                // the next delete, or the driver starts failing open.
                PromptAsync(dos);
            }
        }
    }

    if (gPort != INVALID_HANDLE_VALUE) {
        CloseHandle(gPort);
        gPort = INVALID_HANDLE_VALUE;
    }
    Log("guard stopped");
    return 0;
}

/* ------------------------------------------------------------ service glue */

static void ReportStatus(DWORD state, DWORD exitCode, DWORD waitHint) {
    if (gStatusHandle == nullptr) {
        return;
    }
    gStatus.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
    gStatus.dwCurrentState = state;
    gStatus.dwWin32ExitCode = exitCode;
    gStatus.dwWaitHint = waitHint;
    gStatus.dwControlsAccepted = (state == SERVICE_RUNNING) ? SERVICE_ACCEPT_STOP : 0;
    SetServiceStatus(gStatusHandle, &gStatus);
}

static VOID WINAPI ServiceCtrlHandler(DWORD control) {
    if (control == SERVICE_CONTROL_STOP || control == SERVICE_CONTROL_SHUTDOWN) {
        ReportStatus(SERVICE_STOP_PENDING, NO_ERROR, 5000);
        if (gStopEvent != nullptr) {
            SetEvent(gStopEvent);
        }
        /* FilterGetMessage blocks, and the guard loop is the only caller, so the
         * SCM would otherwise wait out its timeout on every stop. Cancelling the
         * call needs the *guard* thread, not the handler's own thread - passing
         * GetCurrentThread() here cancels the wrong one and the stop appears to
         * hang. */
        if (gGuardThreadId != 0) {
            HANDLE thread = OpenThread(THREAD_TERMINATE, FALSE, gGuardThreadId);
            if (thread != nullptr) {
                CancelSynchronousIo(thread);
                CloseHandle(thread);
            }
        }
    }
}

static VOID WINAPI ServiceMain(DWORD argc, LPWSTR* argv) {
    UNREFERENCED_PARAMETER(argc);
    UNREFERENCED_PARAMETER(argv);

    gStatusHandle = RegisterServiceCtrlHandlerW(SR_SERVICE_NAME, ServiceCtrlHandler);
    if (gStatusHandle == nullptr) {
        return;
    }
    ReportStatus(SERVICE_START_PENDING, NO_ERROR, 5000);
    gStopEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    ReportStatus(SERVICE_RUNNING, NO_ERROR, 0);

    GuardLoop(nullptr);

    ReportStatus(SERVICE_STOPPED, NO_ERROR, 0);
}

static bool InstallService() {
    wchar_t exe[MAX_PATH * 2] = {0};
    if (GetModuleFileNameW(nullptr, exe, _countof(exe)) == 0) {
        return false;
    }

    SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_CREATE_SERVICE);
    if (scm == nullptr) {
        printf("OpenSCManager failed: %lu (run elevated)\n", GetLastError());
        return false;
    }
    SC_HANDLE svc = CreateServiceW(
        scm, SR_SERVICE_NAME, SR_SERVICE_DISPLAY,
        SERVICE_ALL_ACCESS, SERVICE_WIN32_OWN_PROCESS, SERVICE_AUTO_START,
        SERVICE_ERROR_NORMAL, exe, nullptr, nullptr, nullptr, L"LocalSystem", nullptr);
    if (svc == nullptr && GetLastError() == ERROR_SERVICE_EXISTS) {
        svc = OpenServiceW(scm, SR_SERVICE_NAME, SERVICE_ALL_ACCESS);
    }
    if (svc == nullptr) {
        printf("CreateService failed: %lu\n", GetLastError());
        CloseServiceHandle(scm);
        return false;
    }
    SERVICE_DESCRIPTIONW description = {
        const_cast<LPWSTR>(L"Answers the SpaceRecycle delete filter: files marked "
                           L"important are protected from deletion.")};
    ChangeServiceConfig2W(svc, SERVICE_CONFIG_DESCRIPTION, &description);

    // Restart on failure, so a crash does not silently leave deletes unanswered.
    SC_ACTION actions[3] = {
        {SC_ACTION_RESTART, 5000}, {SC_ACTION_RESTART, 15000}, {SC_ACTION_RESTART, 60000}};
    SERVICE_FAILURE_ACTIONSW failure = {0};
    failure.dwResetPeriod = 86400;
    failure.cActions = 3;
    failure.lpsaActions = actions;
    ChangeServiceConfig2W(svc, SERVICE_CONFIG_FAILURE_ACTIONS, &failure);

    printf("installed %ls -> %ls\n", SR_SERVICE_NAME, exe);
    CloseServiceHandle(svc);
    CloseServiceHandle(scm);
    return true;
}

static bool UninstallService() {
    SC_HANDLE scm = OpenSCManagerW(nullptr, nullptr, SC_MANAGER_CONNECT);
    if (scm == nullptr) {
        printf("OpenSCManager failed: %lu\n", GetLastError());
        return false;
    }
    SC_HANDLE svc = OpenServiceW(scm, SR_SERVICE_NAME,
                                 SERVICE_STOP | DELETE | SERVICE_QUERY_STATUS);
    if (svc == nullptr) {
        printf("OpenService failed: %lu\n", GetLastError());
        CloseServiceHandle(scm);
        return false;
    }
    SERVICE_STATUS status;
    ControlService(svc, SERVICE_CONTROL_STOP, &status);
    const bool ok = DeleteService(svc) != FALSE;
    printf(ok ? "uninstalled\n" : "DeleteService failed: %lu\n", GetLastError());
    CloseServiceHandle(svc);
    CloseServiceHandle(scm);
    return ok;
}

int wmain(int argc, wchar_t** argv) {
    for (int i = 1; i < argc; ++i) {
        if (_wcsicmp(argv[i], L"-noprompt") == 0) {
            gNoPrompt = true;
        }
    }
    if (argc > 1 && _wcsicmp(argv[1], L"install") == 0) {
        return InstallService() ? 0 : 1;
    }
    if (argc > 1 && _wcsicmp(argv[1], L"uninstall") == 0) {
        return UninstallService() ? 0 : 1;
    }
    if (argc > 1 && _wcsicmp(argv[1], L"console") == 0) {
        gStopEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr);
        return (int)GuardLoop(nullptr);
    }

    SERVICE_TABLE_ENTRYW table[] = {
        {const_cast<LPWSTR>(SR_SERVICE_NAME), ServiceMain},
        {nullptr, nullptr}};
    if (!StartServiceCtrlDispatcherW(table)) {
        printf("StartServiceCtrlDispatcher failed: %lu\n", GetLastError());
        printf("run 'srguard console' to test in the foreground\n");
        return 1;
    }
    return 0;
}