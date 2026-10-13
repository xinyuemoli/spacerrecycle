// SpaceRecycle — test host for the shell extension.
//
// Explorer is awkward to automate and its behaviour is only visible in a menu,
// so this drives the real COM objects directly: it creates the same objects
// Explorer would, enumerates the real submenu, reports each item's state, and
// can invoke one of them against a real file. That is how the ADS write is
// verified without asking anyone to right-click, and how the overlay handlers
// are checked without waiting for an icon to appear.
//
// It also gives the ADS reader a second, independent implementation: if this
// says "important" and the DLL disagrees, one of them is wrong.
//
// Usage:
//   menutest list    <path>          enumerate the submenu and item states
//   menutest set     <path> <index>  invoke one submenu entry
//   menutest get     <path>          read the ADS directly
//   menutest overlay <path> [<path>] ask each overlay handler about each path

#define WIN32_LEAN_AND_MEAN
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>
#include <shlobj.h>
#include <shlwapi.h>
#include <stdio.h>
#include <string>

// {7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0C}
static const CLSID CLSID_SpaceRecycleShell =
    {0x7a5e3c21, 0x9b4d, 0x4e8f, {0xa6, 0xc2, 0x1d, 0x3e, 0x5f, 0x7a, 0x9b, 0x0c}};
// {7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0D}
static const CLSID CLSID_OverlayImportant =
    {0x7a5e3c21, 0x9b4d, 0x4e8f, {0xa6, 0xc2, 0x1d, 0x3e, 0x5f, 0x7a, 0x9b, 0x0d}};
// {7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0E}
static const CLSID CLSID_OverlayDisposable =
    {0x7a5e3c21, 0x9b4d, 0x4e8f, {0xa6, 0xc2, 0x1d, 0x3e, 0x5f, 0x7a, 0x9b, 0x0e}};

static const wchar_t* kStreamName = L"SpaceRecycle.Importance";

static std::wstring StreamPath(const std::wstring& path) {
    return path + L":" + kStreamName;
}

// Reads the stream with a deliberately different method from the DLL's: open
// the file and address the stream by name, rather than whatever the DLL does.
// Two independent implementations make a disagreement meaningful.
static bool ReadStreamByName(const std::wstring& path, std::string& out) {
    std::wstring sp = StreamPath(path);
    HANDLE h = CreateFileW(sp.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE,
                           nullptr, OPEN_EXISTING, 0, nullptr);
    if (h == INVALID_HANDLE_VALUE) {
        return false;
    }
    char buf[256] = {0};
    DWORD read = 0;
    BOOL ok = ReadFile(h, buf, sizeof(buf) - 1, &read, nullptr);
    CloseHandle(h);
    if (!ok) {
        return false;
    }
    out.assign(buf, read);
    return true;
}

static void PrintHr(const wchar_t* what, HRESULT hr) {
    wprintf(L"  %-22s hr=0x%08lX\n", what, (unsigned long)hr);
}

// Console output goes through the active code page, so a Chinese title prints
// as "????" and a pass would look identical to a fail. Printing the UTF-16 code
// units sidesteps the console entirely.
static void PrintCodePoints(const wchar_t* s) {
    if (s == nullptr) {
        wprintf(L"(null)");
        return;
    }
    for (const wchar_t* p = s; *p; ++p) {
        wprintf(L"U+%04X ", (unsigned)(*p));
    }
    wprintf(L"[%d chars]", (int)wcslen(s));
}

// When COM cannot find a class that the registry visibly contains, the useful
// question is what *this* process can see, not what a shell can see. Registry
// views, hives and sessions all differ between processes, so the check has to
// happen in the process that is failing.
static void ProbeRegistration(const wchar_t* clsidStr) {
    wchar_t user[256] = {0};
    DWORD n = _countof(user);
    if (GetUserNameW(user, &n)) {
        wprintf(L"  running as           : %s\n", user);
    }

    struct { HKEY root; const wchar_t* label; bool perUserLayout; } roots[] = {
        {HKEY_CURRENT_USER, L"HKCU", true},
        {HKEY_CLASSES_ROOT, L"HKCR", false},
        {HKEY_LOCAL_MACHINE, L"HKLM", false},
    };
    for (int i = 0; i < 3; ++i) {
        wchar_t sub[512];
        _snwprintf_s(sub, _countof(sub), _TRUNCATE, roots[i].perUserLayout
                     ? L"Software\\Classes\\CLSID\\%s\\InprocServer32"
                     : L"CLSID\\%s\\InprocServer32", clsidStr);
        HKEY key = nullptr;
        LONG rc = RegOpenKeyExW(roots[i].root, sub, 0, KEY_READ, &key);
        if (rc != ERROR_SUCCESS) {
            wprintf(L"  %s\\%s\n      -> open failed rc=%ld\n", roots[i].label, sub, rc);
            continue;
        }
        wchar_t value[512] = {0};
        DWORD cb = sizeof(value);
        DWORD type = 0;
        LONG qr = RegQueryValueExW(key, nullptr, nullptr, &type, (LPBYTE)value, &cb);
        wprintf(L"  %s\\%s\n      -> %s\n", roots[i].label, sub,
                qr == ERROR_SUCCESS ? value : L"(value query failed)");
        RegCloseKey(key);
    }
}

// ------------------------------------------------------------------ creation

// STDAPI expands to "extern \"C\" HRESULT __stdcall", which is not valid in a
// typedef, so the calling convention is spelled out instead.
typedef HRESULT(__stdcall* DllGetClassObjectFn)(REFCLSID, REFIID, void**);

/// The DLL lives next to this executable, which is how the staging lays it out.
static std::wstring SiblingDllPath() {
    wchar_t buf[MAX_PATH * 2] = {0};
    DWORD n = GetModuleFileNameW(nullptr, buf, _countof(buf));
    if (n == 0 || n >= _countof(buf)) {
        return std::wstring();
    }
    std::wstring s(buf, n);
    size_t slash = s.find_last_of(L"\\/");
    if (slash == std::wstring::npos) {
        return std::wstring();
    }
    return s.substr(0, slash + 1) + L"SpaceRecycleShell.dll";
}

/// Creates a class the way Explorer does, falling back to loading the DLL
/// directly.
///
/// The fallback exists because COM resolves a CLSID through HKCR, and HKCR only
/// merges HKCU\Software\Classes for a properly loaded user profile. A process
/// launched by VMware's guest operations for an account that has never logged on
/// interactively gets a session where that merge does not happen: the key is
/// readable at HKCU\Software\Classes\CLSID but absent at HKCR\CLSID, so
/// CoCreateInstance reports REGDB_E_CLASSNOTREG even though the registration and
/// the DLL are both correct. Loading the DLL by hand sidesteps the question and
/// lets the object's actual behaviour still be tested.
static HRESULT CreateObject(REFCLSID clsid, REFIID iid, const wchar_t* dllPath,
                            void** out, bool* usedDirect, bool verbose) {
    *out = nullptr;
    *usedDirect = false;

    HRESULT hr = CoCreateInstance(clsid, nullptr, CLSCTX_INPROC_SERVER, iid, out);
    if (SUCCEEDED(hr) && *out != nullptr) {
        return hr;
    }
    if (verbose) {
        wchar_t clsidStr[64] = {0};
        StringFromGUID2(clsid, clsidStr, _countof(clsidStr));
        wprintf(L"CoCreateInstance failed: hr=0x%08lX - falling back to a direct DLL load\n",
                (unsigned long)hr);
        wprintf(L"\n--- what this process can see ---\n");
        ProbeRegistration(clsidStr);
        wprintf(L"\n");
    }

    HMODULE module = LoadLibraryW(dllPath);
    if (module == nullptr) {
        wprintf(L"LoadLibrary failed for %s (error %lu)\n", dllPath, GetLastError());
        return HRESULT_FROM_WIN32(GetLastError());
    }
    DllGetClassObjectFn getClassObject =
        (DllGetClassObjectFn)(void*)GetProcAddress(module, "DllGetClassObject");
    if (getClassObject == nullptr) {
        wprintf(L"DllGetClassObject not exported\n");
        return E_FAIL;
    }

    IClassFactory* factory = nullptr;
    hr = getClassObject(clsid, IID_PPV_ARGS(&factory));
    if (FAILED(hr) || factory == nullptr) {
        wprintf(L"DllGetClassObject failed: hr=0x%08lX\n", (unsigned long)hr);
        return hr;
    }
    hr = factory->CreateInstance(nullptr, iid, out);
    factory->Release();
    if (FAILED(hr) || *out == nullptr) {
        wprintf(L"IClassFactory::CreateInstance failed: hr=0x%08lX\n", (unsigned long)hr);
        return hr;
    }
    *usedDirect = true;
    return S_OK;
}

static IShellItemArray* ArrayFromPath(const wchar_t* path) {
    IShellItem* item = nullptr;
    HRESULT first = SHCreateItemFromParsingName(path, nullptr, IID_PPV_ARGS(&item));
    if (FAILED(first) || item == nullptr) {
        wprintf(L"  SHCreateItemFromParsingName failed: hr=0x%08lX\n", (unsigned long)first);
        return nullptr;
    }
    IShellItemArray* arr = nullptr;
    HRESULT second = SHCreateShellItemArrayFromShellItem(item, IID_PPV_ARGS(&arr));
    item->Release();
    if (FAILED(second)) {
        wprintf(L"  SHCreateShellItemArrayFromShellItem failed: hr=0x%08lX\n",
                (unsigned long)second);
        return nullptr;
    }
    return arr;
}

// ------------------------------------------------------------------- commands

static int RunMenu(const std::wstring& cmd, const std::wstring& path, int wantIndex,
                   const wchar_t* dllPath) {
    IExplorerCommand* root = nullptr;
    bool usedDirect = false;
    HRESULT hr = CreateObject(CLSID_SpaceRecycleShell, IID_IExplorerCommand,
                              dllPath, (void**)&root, &usedDirect, true);
    if (FAILED(hr) || root == nullptr) {
        wprintf(L"could not create the command at all\n");
        return 3;
    }
    wprintf(L"created via: %s\n", usedDirect ? L"direct DLL load" : L"COM registration");

    LPWSTR rootTitle = nullptr;
    root->GetTitle(nullptr, &rootTitle);
    wprintf(L"root title : ");
    PrintCodePoints(rootTitle);
    wprintf(L"\n");
    if (rootTitle) {
        CoTaskMemFree(rootTitle);
    }

    EXPCMDFLAGS flags = ECF_DEFAULT;
    root->GetFlags(&flags);
    wprintf(L"root flags : 0x%08lX (HASSUBCOMMANDS=%s)\n", (unsigned long)flags,
            (flags & ECF_HASSUBCOMMANDS) ? L"yes" : L"NO");

    IEnumExplorerCommand* e = nullptr;
    hr = root->EnumSubCommands(&e);
    if (FAILED(hr) || e == nullptr) {
        wprintf(L"EnumSubCommands failed: 0x%08lX\n", (unsigned long)hr);
        root->Release();
        return 4;
    }

    IExplorerCommand* kids[16] = {0};
    ULONG n = 0;
    e->Next(16, kids, &n);
    wprintf(L"submenu    : %lu entries\n", (unsigned long)n);

    IShellItemArray* arr = ArrayFromPath(path.c_str());
    if (arr == nullptr) {
        wprintf(L"WARNING: could not build a shell item array for [%s]\n", path.c_str());
    }

    for (ULONG i = 0; i < n; ++i) {
        LPWSTR title = nullptr;
        kids[i]->GetTitle(arr, &title);
        EXPCMDSTATE state = ECS_ENABLED;
        kids[i]->GetState(arr, FALSE, &state);

        wprintf(L"  [%lu] ", (unsigned long)i);
        PrintCodePoints(title);
        wprintf(L" state=0x%02lX%s\n", (unsigned long)state,
                (state & ECS_CHECKED) ? L" (checked)" : L"");

        if (cmd == L"set" && (int)i == wantIndex) {
            HRESULT ir = kids[i]->Invoke(arr, nullptr);
            PrintHr(L"Invoke", ir);
        }

        if (title) CoTaskMemFree(title);
        kids[i]->Release();
    }

    if (arr) arr->Release();
    e->Release();
    root->Release();
    return 0;
}

static int RunOverlay(int argc, wchar_t** argv, const wchar_t* dllPath) {
    struct { const wchar_t* label; const CLSID* clsid; } levels[] = {
        {L"important", &CLSID_OverlayImportant},
        {L"disposable", &CLSID_OverlayDisposable},
    };

    // Every path the caller supplied, so the handlers can be shown to
    // discriminate rather than merely to answer.
    const wchar_t* paths[4] = {0};
    int pathCount = 0;
    for (int i = 2; i < argc && pathCount < 4; ++i) {
        paths[pathCount++] = argv[i];
    }

    for (int i = 0; i < 2; ++i) {
        IShellIconOverlayIdentifier* handler = nullptr;
        bool usedDirect = false;
        HRESULT hr = CreateObject(*levels[i].clsid, IID_IShellIconOverlayIdentifier,
                                  dllPath, (void**)&handler, &usedDirect, i == 0);
        wprintf(L"=== handler %s (%s) ===\n", levels[i].label,
                usedDirect ? L"direct DLL load" : L"COM registration");
        if (FAILED(hr) || handler == nullptr) {
            wprintf(L"  could not create handler: hr=0x%08lX\n\n", (unsigned long)hr);
            continue;
        }

        int priority = -1;
        hr = handler->GetPriority(&priority);
        wprintf(L"  GetPriority            hr=0x%08lX value=%d\n", (unsigned long)hr, priority);

        wchar_t icon[MAX_PATH * 2] = {0};
        int index = -1;
        DWORD flags = 0;
        hr = handler->GetOverlayInfo(icon, _countof(icon), &index, &flags);
        wprintf(L"  GetOverlayInfo         hr=0x%08lX flags=0x%08lX index=%d\n",
                (unsigned long)hr, (unsigned long)flags, index);
        wprintf(L"     icon                = %s\n", icon[0] ? icon : L"(none)");
        wprintf(L"     icon exists         = %s\n", icon[0] && GetFileAttributesW(icon) != INVALID_FILE_ATTRIBUTES ? L"yes" : L"NO");

        for (int p = 0; p < pathCount; ++p) {
            HRESULT member = handler->IsMemberOf(paths[p], FILE_ATTRIBUTE_NORMAL);
            const wchar_t* verdict = (member == S_OK) ? L"S_OK  -> this handler claims it"
                                     : (member == S_FALSE) ? L"S_FALSE -> not mine"
                                     : L"unexpected";
            wprintf(L"  IsMemberOf             %s\n", verdict);
            wprintf(L"     hr=0x%08lX path=%s\n", (unsigned long)member, paths[p]);
        }
        wprintf(L"\n");
        handler->Release();
    }
    return 0;
}

int wmain(int argc, wchar_t** argv) {
    CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

    if (argc < 3) {
        wprintf(L"usage:\n");
        wprintf(L"  menutest list    <path>          enumerate the submenu and item states\n");
        wprintf(L"  menutest set     <path> <index>  invoke one submenu entry\n");
        wprintf(L"  menutest get     <path>          read the ADS directly\n");
        wprintf(L"  menutest overlay <path> [<path>] ask each overlay handler about each path\n");
        return 2;
    }

    const std::wstring cmd = argv[1];
    const std::wstring path = argv[2];
    const std::wstring dll = SiblingDllPath();

    int rc = 0;
    if (cmd == L"get") {
        std::string payload;
        if (!ReadStreamByName(path, payload)) {
            wprintf(L"unmarked (no stream)\n");
            rc = 1;
        } else {
            wprintf(L"stream = %hs\n", payload.c_str());
        }
    } else if (cmd == L"overlay") {
        rc = RunOverlay(argc, argv, dll.c_str());
    } else if (cmd == L"list" || cmd == L"set" || cmd == L"menu") {
        int wantIndex = (argc >= 4) ? _wtoi(argv[3]) : -1;
        rc = RunMenu(cmd, path, wantIndex, dll.c_str());
    } else {
        wprintf(L"unknown command: %s\n", cmd.c_str());
        rc = 2;
    }

    CoUninitialize();
    return rc;
}