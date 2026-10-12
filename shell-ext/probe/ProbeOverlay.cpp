// SpaceRecycle — shell icon overlay probe.
//
// This is a diagnostic build, not the shipping handler. It exists to answer one
// question that decides the whole overlay design:
//
//   Does Explorer call GetOverlayInfo once per handler, or once per file?
//
// GetOverlayInfo has no file-path parameter, so a single handler can only return
// a per-file icon if Explorer calls it per file *after* IsMemberOf(path) for the
// same file, letting the handler remember the path. If it is instead called once
// at startup, the handler has no way to know which file it is drawing and one
// handler can only ever produce one icon.
//
// Every call is appended to probe.log with a sequence number so the ordering can
// be read off directly.

#define WIN32_LEAN_AND_MEAN
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>
#include <shlobj.h>
#include <new>
#include <stdio.h>

namespace {

// Absolute so it does not depend on Explorer's working directory. Explorer runs
// as the logged-on user, so this is writable without elevation.
const wchar_t* kLogPath = L"C:\\d\\codes\\spacerrecycle\\shell-ext\\probe\\probe.log";

CRITICAL_SECTION g_cs;
bool g_csReady = false;

LONG g_seq = 0;
LONG g_isMemberOf = 0;
LONG g_getOverlayInfo = 0;
LONG g_getPriority = 0;

wchar_t g_lastPath[MAX_PATH * 2] = L"";
DWORD g_lastAttrib = 0;

// {6F1B2C3D-4E5A-4B7C-9D8E-1A2B3C4D5E6F}
const CLSID CLSID_SpaceRecycleProbe =
    {0x6f1b2c3d, 0x4e5a, 0x4b7c, {0x9d, 0x8e, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e, 0x6f}};

void Log(const wchar_t* fmt, ...) {
    wchar_t line[2048];
    va_list args;
    va_start(args, fmt);
    _vsnwprintf_s(line, _countof(line), _TRUNCATE, fmt, args);
    va_end(args);

    HANDLE h = CreateFileW(kLogPath, FILE_APPEND_DATA, FILE_SHARE_READ | FILE_SHARE_WRITE,
                           nullptr, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) {
        return;
    }
    SYSTEMTIME st;
    GetLocalTime(&st);
    wchar_t stamped[2300];
    _snwprintf_s(stamped, _countof(stamped), _TRUNCATE,
                 L"%02d:%02d:%02d.%03d pid=%lu %s\r\n",
                 st.wHour, st.wMinute, st.wSecond, st.wMilliseconds,
                 GetCurrentProcessId(), line);
    DWORD written = 0;
    WriteFile(h, stamped, (DWORD)(wcslen(stamped) * sizeof(wchar_t)), &written, nullptr);
    CloseHandle(h);
}

void LogLocked(const wchar_t* fmt, ...) {
    if (!g_csReady) {
        return;
    }
    EnterCriticalSection(&g_cs);
    wchar_t line[2048];
    va_list args;
    va_start(args, fmt);
    _vsnwprintf_s(line, _countof(line), _TRUNCATE, fmt, args);
    va_end(args);
    LeaveCriticalSection(&g_cs);
    Log(L"%s", line);
}

class ProbeOverlay final : public IShellIconOverlayIdentifier {
public:
    ProbeOverlay() : m_ref(1) {}

    // ---- IUnknown ----
    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override {
        if (ppv == nullptr) {
            return E_POINTER;
        }
        *ppv = nullptr;
        if (IsEqualIID(riid, IID_IUnknown) ||
            IsEqualIID(riid, IID_IShellIconOverlayIdentifier)) {
            *ppv = static_cast<IShellIconOverlayIdentifier*>(this);
            AddRef();
            return S_OK;
        }
        return E_NOINTERFACE;
    }

    STDMETHODIMP_(ULONG) AddRef() override {
        return (ULONG)InterlockedIncrement(&m_ref);
    }

    STDMETHODIMP_(ULONG) Release() override {
        LONG n = InterlockedDecrement(&m_ref);
        if (n == 0) {
            delete this;
        }
        return (ULONG)n;
    }

    // ---- IShellIconOverlayIdentifier ----

    STDMETHODIMP IsMemberOf(LPCWSTR pwszPath, DWORD dwAttrib) override {
        LONG n = InterlockedIncrement(&g_isMemberOf);
        // Remember the path: this is exactly the state the documented API does
        // not give GetOverlayInfo, and the whole experiment is whether it is
        // still valid when GetOverlayInfo runs.
        if (pwszPath != nullptr) {
            if (g_csReady) {
                EnterCriticalSection(&g_cs);
            }
            wcsncpy_s(g_lastPath, _countof(g_lastPath), pwszPath, _TRUNCATE);
            g_lastAttrib = dwAttrib;
            if (g_csReady) {
                LeaveCriticalSection(&g_cs);
            }
        }
        LogLocked(L"#%ld IsMemberOf n=%ld attrib=0x%08lX path=%s", g_seq, n, dwAttrib,
                  pwszPath ? pwszPath : L"(null)");
        InterlockedIncrement(&g_seq);
        // S_FALSE on purpose: the probe draws nothing, so it cannot interfere
        // with the real icons already registered on this machine.
        return S_FALSE;
    }

    STDMETHODIMP GetOverlayInfo(LPWSTR pwszIconFile, int cchMax, int* pIndex,
                               DWORD* pdwFlags) override {
        LONG n = InterlockedIncrement(&g_getOverlayInfo);
        wchar_t remembered[MAX_PATH * 2] = L"";
        if (g_csReady) {
            EnterCriticalSection(&g_cs);
        }
        wcsncpy_s(remembered, _countof(remembered), g_lastPath, _TRUNCATE);
        if (g_csReady) {
            LeaveCriticalSection(&g_cs);
        }
        LogLocked(L"#%ld GetOverlayInfo n=%ld  remembered_path=%s", g_seq, n,
                  remembered[0] ? remembered : L"(none yet)");
        InterlockedIncrement(&g_seq);

        if (pwszIconFile == nullptr || pIndex == nullptr || pdwFlags == nullptr) {
            return E_POINTER;
        }
        // A real icon, so Explorer keeps the handler instead of discarding it.
        const wchar_t* icon = L"%SystemRoot%\\System32\\shell32.dll";
        wcsncpy_s(pwszIconFile, (size_t)cchMax, icon, _TRUNCATE);
        *pIndex = 44;
        *pdwFlags = ISIOI_ICONFILE | ISIOI_ICONINDEX;
        return S_OK;
    }

    STDMETHODIMP GetPriority(int* pPriority) override {
        LONG n = InterlockedIncrement(&g_getPriority);
        LogLocked(L"#%ld GetPriority n=%ld", g_seq, n);
        InterlockedIncrement(&g_seq);
        if (pPriority != nullptr) {
            *pPriority = 0;
        }
        return S_OK;
    }

private:
    ~ProbeOverlay() = default;
    LONG m_ref;
};

class ProbeFactory final : public IClassFactory {
public:
    ProbeFactory() : m_ref(1) {}

    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override {
        if (ppv == nullptr) {
            return E_POINTER;
        }
        *ppv = nullptr;
        if (IsEqualIID(riid, IID_IUnknown) || IsEqualIID(riid, IID_IClassFactory)) {
            *ppv = static_cast<IClassFactory*>(this);
            AddRef();
            return S_OK;
        }
        return E_NOINTERFACE;
    }
    STDMETHODIMP_(ULONG) AddRef() override {
        return (ULONG)InterlockedIncrement(&m_ref);
    }
    STDMETHODIMP_(ULONG) Release() override {
        LONG n = InterlockedDecrement(&m_ref);
        if (n == 0) {
            delete this;
        }
        return (ULONG)n;
    }

    STDMETHODIMP CreateInstance(IUnknown* outer, REFIID riid, void** ppv) override {
        if (ppv == nullptr) {
            return E_POINTER;
        }
        *ppv = nullptr;
        if (outer != nullptr) {
            return CLASS_E_NOAGGREGATION;
        }
        ProbeOverlay* obj = new (std::nothrow) ProbeOverlay();
        if (obj == nullptr) {
            return E_OUTOFMEMORY;
        }
        HRESULT hr = obj->QueryInterface(riid, ppv);
        obj->Release();
        return hr;
    }

    STDMETHODIMP LockServer(BOOL) override { return S_OK; }

private:
    ~ProbeFactory() = default;
    LONG m_ref;
};

HINSTANCE g_module = nullptr;
LONG g_locks = 0;

}  // namespace

BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        g_module = inst;
        DisableThreadLibraryCalls(inst);
        InitializeCriticalSection(&g_cs);
        g_csReady = true;
        Log(L"--- DllMain PROCESS_ATTACH (handler loaded) ---");
    } else if (reason == DLL_PROCESS_DETACH) {
        Log(L"--- DllMain PROCESS_DETACH ---");
        if (g_csReady) {
            DeleteCriticalSection(&g_cs);
            g_csReady = false;
        }
    }
    return TRUE;
}

STDAPI DllGetClassObject(REFCLSID rclsid, REFIID riid, void** ppv) {
    if (ppv == nullptr) {
        return E_POINTER;
    }
    *ppv = nullptr;
    if (!IsEqualCLSID(rclsid, CLSID_SpaceRecycleProbe)) {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    ProbeFactory* f = new (std::nothrow) ProbeFactory();
    if (f == nullptr) {
        return E_OUTOFMEMORY;
    }
    HRESULT hr = f->QueryInterface(riid, ppv);
    f->Release();
    return hr;
}

STDAPI DllCanUnloadNow() {
    return g_locks == 0 ? S_OK : S_FALSE;
}

STDAPI DllRegisterServer() { return S_OK; }
STDAPI DllUnregisterServer() { return S_OK; }