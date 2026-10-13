// SpaceRecycle - user-mode listener for the delete filter.
//
// Milestone 1 verification target. The filter has no other way to prove it
// intercepted anything: DbgPrint needs a kernel debugger attached, which is not
// available here, so the filter reports through the communication port instead
// and this program records what it receives.
//
// It always answers "allow". Policy is a later milestone, and the first version
// of anything that sits in the delete path should not be able to say no.
//
// Usage: srlisten [logfile] [-seconds N]
//
// Logging goes straight to a file handle with CreateFileW/WriteFile rather than
// through stdio. An earlier version used printf and died with
// STATUS_INVALID_CRUNTIME_PARAMETER the moment it wrote its first line, because
// a process launched through VMware's guest operations may have no valid stdout
// to print to. A diagnostic tool that cannot report is worse than useless, so
// this one does not depend on stdio at all - it is all step markers, so if it
// dies the log still says how far it got.

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <fltuser.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string>

#define SR_PORT_NAME       L"\\SpaceRecycleDeletePort"
#define SR_MAX_PATH_CHARS  520
#define SR_OP_DELETE       1
#define SR_OP_DELETE_ON_CLOSE 2

// Mirrors the driver's SR_NOTIFY exactly, including the Size field.
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
static DWORD gRunSeconds = 0;
static BOOL gDenyAll = FALSE;

// The driver reports what FltGetFileNameInformation hands back for a normalized
// name, which is a device path: \Device\HarddiskVolume1\folder\file. Anything
// that touches the file - reading the ADS, showing it to a user - needs the DOS
// form. Doing the translation here rather than in the driver keeps the kernel
// side to a single name lookup and keeps the drive-letter table in a place that
// can be tested without a debugger.
static bool DevicePathToDos(const wchar_t* devicePath, std::wstring& out) {
    wchar_t drives[512] = {0};
    DWORD length = GetLogicalDriveStringsW(_countof(drives) - 1, drives);
    if (length == 0) {
        return false;
    }
    for (const wchar_t* p = drives; *p != L'\0'; p += wcslen(p) + 1) {
        wchar_t letter[4] = {p[0], p[1], L'\0', L'\0'};   // "C:"
        wchar_t target[1024] = {0};
        if (QueryDosDeviceW(letter, target, _countof(target)) == 0) {
            continue;
        }
        const size_t targetLength = wcslen(target);
        if (targetLength > 0 && _wcsnicmp(devicePath, target, targetLength) == 0) {
            out.assign(letter);
            out.append(devicePath + targetLength);        // "C:" + "\folder\file"
            return true;
        }
    }
    return false;
}

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
    if (gLog != INVALID_HANDLE_VALUE) {
        DWORD written = 0;
        WriteFile(gLog, line, (DWORD)m, &written, nullptr);
    }
}

// Optional self-termination, so a test script can run the listener
// synchronously and then read the log. Left at 0 the listener runs until
// killed, which is what a real service wants.
static DWORD WINAPI Watchdog(LPVOID) {
    Sleep(gRunSeconds * 1000);
    Log("watchdog: %lu seconds elapsed, exiting", gRunSeconds);
    if (gLog != INVALID_HANDLE_VALUE) {
        FlushFileBuffers(gLog);
        CloseHandle(gLog);
    }
    ExitProcess(0);
}

int wmain(int argc, wchar_t** argv) {
    const wchar_t* logPath = L"srlisten.log";
    for (int i = 1; i < argc; ++i) {
        if (wcscmp(argv[i], L"-seconds") == 0 && i + 1 < argc) {
            gRunSeconds = (DWORD)_wtoi(argv[++i]);
        } else if (wcscmp(argv[i], L"-deny") == 0) {
            // Refuse everything. This exists to test the driver's denial path on
            // its own: if a delete is still not blocked when the filter is told
            // to refuse it, the fault is in the driver, not in the policy.
            gDenyAll = TRUE;
        } else if (argv[i][0] != L'-') {
            logPath = argv[i];
        }
    }

    gLog = CreateFileW(logPath, FILE_APPEND_DATA,
                       FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr,
                       OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    Log("listener starting, port=%ls, log=%ls", SR_PORT_NAME, logPath);
    if (gLog == INVALID_HANDLE_VALUE) {
        return 2;
    }

    if (gRunSeconds > 0) {
        HANDLE watchdog = CreateThread(nullptr, 0, Watchdog, nullptr, 0, nullptr);
        if (watchdog != nullptr) {
            CloseHandle(watchdog);
        }
        Log("watchdog armed for %lu seconds", gRunSeconds);
    }

    HANDLE port = INVALID_HANDLE_VALUE;
    HRESULT hr = E_FAIL;

    Log("step: about to connect");
    // The driver may load after this starts, so retry rather than exiting.
    for (int attempt = 1; attempt <= 30; ++attempt) {
        hr = FilterConnectCommunicationPort(SR_PORT_NAME, 0, nullptr, 0, nullptr, &port);
        if (SUCCEEDED(hr)) {
            break;
        }
        if (attempt == 1 || attempt % 5 == 0) {
            Log("connect attempt %d failed: hr=0x%08lX", attempt, (unsigned long)hr);
        }
        Sleep(1000);
    }

    if (FAILED(hr)) {
        Log("giving up: hr=0x%08lX", (unsigned long)hr);
        Log("(0x80070005 = access denied: the port's default security descriptor "
            "may require an elevated caller)");
        return 1;
    }
    Log("connected");

    for (;;) {
        SR_MESSAGE message;
        ZeroMemory(&message, sizeof(message));

        hr = FilterGetMessage(port, &message.Header, sizeof(message), nullptr);
        if (FAILED(hr)) {
            Log("FilterGetMessage failed: hr=0x%08lX", (unsigned long)hr);
            break;
        }

        const wchar_t* op = (message.Payload.Operation == SR_OP_DELETE_ON_CLOSE)
                                ? L"DELETE_ON_CLOSE"
                                : L"DELETE";
        std::wstring dos;
        const bool mapped = DevicePathToDos(message.Payload.Path, dos);
        Log("intercepted %ls", op);
        Log("    device : %ls", message.Payload.Path);
        if (mapped) {
            Log("    dos    : %ls", dos.c_str());
        } else {
            Log("    dos    : (no drive letter maps to this device)");
        }
        // ReplyLength is what the filter declared its reply buffer to be. If it
        // is 0 the filter is not expecting an answer at all, which would mean
        // the driver build in use predates the fix that sets it.
        Log("    filter expects %lu reply bytes; sending %lu + %lu header = %lu",
            (unsigned long)message.Header.ReplyLength,
            (unsigned long)sizeof(SR_REPLY),
            (unsigned long)(sizeof(FILTER_REPLY_HEADER)),
            (unsigned long)sizeof(SR_REPLY_MESSAGE));

        SR_REPLY_MESSAGE reply;
        ZeroMemory(&reply, sizeof(reply));
        reply.Header.MessageId = message.Header.MessageId;
        reply.Payload.Allow = gDenyAll ? FALSE : TRUE;

        // The filter manager wants the reply size to match what the filter
        // declared exactly. Passing sizeof(SR_REPLY_MESSAGE) does not: the
        // structure contains a LARGE_INTEGER, so the compiler pads 17 bytes of
        // content up to 24, and a padded size is rejected with ERROR_MORE_DATA.
        const DWORD replySize = (DWORD)(sizeof(FILTER_REPLY_HEADER) + sizeof(SR_REPLY));
        hr = FilterReplyMessage(port, &reply.Header, replySize);
        if (FAILED(hr)) {
            Log("FilterReplyMessage failed: hr=0x%08lX (sent %lu bytes, filter expects %lu)",
                (unsigned long)hr, (unsigned long)replySize,
                (unsigned long)(sizeof(FILTER_REPLY_HEADER) + message.Header.ReplyLength -
                                sizeof(FILTER_REPLY_HEADER)));
        }
    }

    CloseHandle(port);
    if (gLog != INVALID_HANDLE_VALUE) {
        CloseHandle(gLog);
    }
    return 0;
}