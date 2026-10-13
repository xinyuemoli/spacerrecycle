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

static HANDLE gLog = INVALID_HANDLE_VALUE;
static SERVICE_STATUS gStatus = {0};
static SERVICE_STATUS_HANDLE gStatusHandle = nullptr;
static HANDLE gStopEvent = nullptr;
static HANDLE gPort = INVALID_HANDLE_VALUE;
static bool gNoPrompt = false;

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
    if (n > 0 && n < _countof(dir)) {
        path.assign(dir);
        path += L"\\SpaceRecycle";
        CreateDirectoryW(path.c_str(), nullptr);
        path += L"\\guard.log";
    } else {
        path = L"srguard.log";
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

/* --------------------------------------------------------------- main loop */

static DWORD WINAPI GuardLoop(LPVOID) {
    OpenLog();
    Log("guard starting (pid %lu, prompt=%s)", GetCurrentProcessId(),
        gNoPrompt ? "off" : "on");

    HRESULT hr = E_FAIL;
    for (int attempt = 1; attempt <= 30; ++attempt) {
        hr = FilterConnectCommunicationPort(SR_PORT_NAME, 0, nullptr, 0, nullptr, &gPort);
        if (SUCCEEDED(hr)) {
            break;
        }
        if (attempt == 1 || attempt % 5 == 0) {
            Log("connect attempt %d failed: hr=0x%08lX", attempt, (unsigned long)hr);
        }
        Sleep(1000);
    }
    if (FAILED(hr)) {
        Log("cannot reach the filter (hr=0x%08lX); guard idle", (unsigned long)hr);
        return 1;
    }
    Log("connected to the filter");

    for (;;) {
        if (WaitForSingleObject(gStopEvent, 0) == WAIT_OBJECT_0) {
            break;
        }

        SR_MESSAGE message;
        ZeroMemory(&message, sizeof(message));
        hr = FilterGetMessage(gPort, &message.Header, sizeof(message), nullptr);
        if (FAILED(hr)) {
            Log("FilterGetMessage failed: hr=0x%08lX", (unsigned long)hr);
            break;
        }

        std::wstring dos;
        const bool mapped = DevicePathToDos(message.Payload.Path, dos);
        const std::wstring display = mapped ? dos : std::wstring(message.Payload.Path);
        Log("delete requested: %ls", display.c_str());

        const bool allow = mapped ? Decide(dos) : true;

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
        if (gPort != INVALID_HANDLE_VALUE) {
            // FilterGetMessage blocks; cancelling the handle wakes it so the
            // service can reach a stopped state instead of hanging the SCM.
            CancelSynchronousIo(GetCurrentThread());
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