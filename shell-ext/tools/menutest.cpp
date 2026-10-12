// SpaceRecycle — test host for the shell extension.
//
// Explorer is awkward to automate and its behaviour is only visible in a menu,
// so this drives the real COM object directly: it creates the same command
// Explorer would, enumerates the real submenu, reports each item's state, and
// can invoke one of them against a real file. That is how the ADS write is
// verified without asking anyone to right-click.
//
// It also gives the ADS reader a second, independent implementation: if this
// says "important" and the DLL disagrees, one of them is wrong.

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

static const wchar_t* kStreamName = L"SpaceRecycle.Importance";

static std::wstring StreamPath(const std::wstring& path) {
    return path + L":" + kStreamName;
}

// Reads the stream with a deliberately different method from the DLL's: open
// the file and enumerate its streams, rather than addressing the stream by
// name. Two independent implementations make a disagreement meaningful.
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

    struct { HKEY root; const wchar_t* label; } roots[] = {
        {HKEY_CURRENT_USER, L"HKCU"},
        {HKEY_CLASSES_ROOT, L"HKCR"},
    };
    for (int i = 0; i < 2; ++i) {
        wchar_t sub[512];
        // The per-user layout the DLL's DllRegisterServer writes.
        _snwprintf_s(sub, _countof(sub), _TRUNCATE,
                     L"Software\\Classes\\CLSID\\%s\\InprocServer32", clsidStr);
        if (i == 1) {
            // HKCR is the merged view; the class is directly under CLSID there.
            _snwprintf_s(sub, _countof(sub), _TRUNCATE,
                         L"CLSID\\%s\\InprocServer32", clsidStr);
        }
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

// Creates the command the way Explorer does, falling back to loading the DLL
// directly.
//
// The fallback exists because COM resolves a CLSID through HKCR, and HKCR only
// merges HKCU\Software\Classes for a properly loaded user profile. A process
// launched by VMware's guest operations for an account that has never logged on
// interactively gets a session where that merge does not happen: the key is
// readable at HKCU\Software\Classes\CLSID but absent at HKCR\CLSID, so
// CoCreateInstance reports REGDB_E_CLASSNOTREG even though the registration and
// the DLL are both correct. Loading the DLL by hand sidesteps the question and
// lets the command's actual behaviour still be tested.
// STDAPI expands to "extern \"C\" HRESULT __stdcall", which is not valid in a
// typedef, so the calling convention is spelled out instead.
typedef HRESULT(__stdcall* DllGetClassObjectFn)(REFCLSID, REFIID, void**);

static IExplorerCommand* CreateRoot(const wchar_t* dllPath, bool* usedDirect) {
    *usedDirect = false;

    IExplorerCommand* root = nullptr;
    HRESULT hr = CoCreateInstance(CLSID_SpaceRecycleShell, nullptr, CLSCTX_INPROC_SERVER,
                                  IID_PPV_ARGS(&root));
    if (SUCCEEDED(hr) && root != nullptr) {
        return root;
    }
    wprintf(L"CoCreateInstance failed: hr=0x%08lX - falling back to a direct DLL load\n",
            (unsigned long)hr);
    wprintf(L"\n--- what this process can see ---\n");
    ProbeRegistration(L"{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0C}");
    wprintf(L"\n");

    HMODULE module = LoadLibraryW(dllPath);
    if (module == nullptr) {
        wprintf(L"LoadLibrary failed for %s (error %lu)\n", dllPath, GetLastError());
        return nullptr;
    }
    DllGetClassObjectFn getClassObject =
        (DllGetClassObjectFn)(void*)GetProcAddress(module, "DllGetClassObject");
    if (getClassObject == nullptr) {
        wprintf(L"DllGetClassObject not exported\n");
        return nullptr;
    }

    IClassFactory* factory = nullptr;
    hr = getClassObject(CLSID_SpaceRecycleShell, IID_PPV_ARGS(&factory));
    if (FAILED(hr) || factory == nullptr) {
        wprintf(L"DllGetClassObject failed: hr=0x%08lX\n", (unsigned long)hr);
        return nullptr;
    }
    hr = factory->CreateInstance(nullptr, IID_PPV_ARGS(&root));
    factory->Release();
    if (FAILED(hr) || root == nullptr) {
        wprintf(L"IClassFactory::CreateInstance failed: hr=0x%08lX\n", (unsigned long)hr);
        return nullptr;
    }
    *usedDirect = true;
    return root;
}

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

int wmain(int argc, wchar_t** argv) {
    CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

    if (argc < 3) {
        wprintf(L"usage:\n");
        wprintf(L"  menutest list  <path>          enumerate the submenu and item states\n");
        wprintf(L"  menutest set   <path> <index>  invoke one submenu entry\n");
        wprintf(L"  menutest get   <path>          read the ADS directly\n");
        return 2;
    }

    const std::wstring cmd = argv[1];
    const std::wstring path = argv[2];

    if (cmd == L"get") {
        std::string payload;
        if (!ReadStreamByName(path, payload)) {
            wprintf(L"unmarked (no stream)\n");
            return 1;
        }
        wprintf(L"stream = %hs\n", payload.c_str());
        return 0;
    }

    IExplorerCommand* root = nullptr;
    bool usedDirect = false;
    std::wstring dllPath = SiblingDllPath();
    root = CreateRoot(dllPath.c_str(), &usedDirect);
    if (root == nullptr) {
        wprintf(L"could not create the command at all (dll: %s)\n", dllPath.c_str());
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
    HRESULT hr = root->EnumSubCommands(&e);
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
        LPWSTR icon = nullptr;
        LPWSTR tip = nullptr;
        kids[i]->GetTitle(arr, &title);
        kids[i]->GetIcon(arr, &icon);
        kids[i]->GetToolTip(arr, &tip);
        EXPCMDSTATE state = ECS_ENABLED;
        kids[i]->GetState(arr, FALSE, &state);

        wprintf(L"  [%lu] ", (unsigned long)i);
        PrintCodePoints(title);
        wprintf(L" state=0x%02lX%s\n", (unsigned long)state,
                (state & ECS_CHECKED) ? L" (checked)" : L"");

        if (cmd == L"set" && argc >= 4 && (ULONG)_wtoi(argv[3]) == i) {
            HRESULT ir = kids[i]->Invoke(arr, nullptr);
            PrintHr(L"Invoke", ir);
            wprintf(L"  -> invoked [%s]\n", title ? title : L"?");
        }

        if (title) CoTaskMemFree(title);
        if (icon) CoTaskMemFree(icon);
        if (tip) CoTaskMemFree(tip);
        kids[i]->Release();
    }

    if (arr) arr->Release();
    e->Release();
    root->Release();
    CoUninitialize();
    return 0;
}