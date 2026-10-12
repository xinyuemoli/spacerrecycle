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
    if (FAILED(SHCreateItemFromParsingName(path, nullptr, IID_PPV_ARGS(&item))) || !item) {
        return nullptr;
    }
    IShellItemArray* arr = nullptr;
    HRESULT hr = SHCreateShellItemArrayFromShellItem(item, IID_PPV_ARGS(&arr));
    item->Release();
    return SUCCEEDED(hr) ? arr : nullptr;
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
    HRESULT hr = CoCreateInstance(CLSID_SpaceRecycleShell, nullptr, CLSCTX_INPROC_SERVER,
                                  IID_PPV_ARGS(&root));
    if (FAILED(hr) || root == nullptr) {
        wprintf(L"CoCreateInstance failed: hr=0x%08lX\n", (unsigned long)hr);
        wprintf(L"(is the DLL registered? run register.ps1 -Action register)\n");
        return 3;
    }

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