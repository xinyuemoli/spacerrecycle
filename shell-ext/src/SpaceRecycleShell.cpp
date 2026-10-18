// SpaceRecycle — Explorer context menu shell extension.
//
// Registers a cascading submenu on files and folders:
//
//     SpaceRecycle ▸  重要  /  可清理  /  清除标记
//
// The chosen level is written into an Alternate Data Stream on the file itself:
//
//     <path>:SpaceRecycle.Importance   ->   "v1|important|1730000000"
//
// ADS rather than a central index, because the marking then travels with the
// file across renames and copies, and because the icon-overlay handler (added
// later) can test for it in a single GetFileAttributesW call on the hot path.
//
// Two levels, not four. Fewer levels is a better fit for a right-click menu,
// and it halves the number of icon-overlay slots the feature will eventually
// need — of which Windows provides only 15 machine-wide.
//
// This file is compiled into a DLL that Explorer loads in-process. Everything
// here is written defensively on that assumption: every entry point returns a
// failure HRESULT rather than throwing, no work is done on DllMain's thread
// beyond initialisation, and nothing blocks.

#define WIN32_LEAN_AND_MEAN
#define _CRT_SECURE_NO_WARNINGS
#include <windows.h>
#include <shlobj.h>
#include <shlwapi.h>
#include <new>
#include <string>
#include <time.h>
#include <vector>

namespace {

// ------------------------------------------------------------------ identity

// {7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0C}
const CLSID CLSID_SpaceRecycleShell =
    {0x7a5e3c21, 0x9b4d, 0x4e8f, {0xa6, 0xc2, 0x1d, 0x3e, 0x5f, 0x7a, 0x9b, 0x0c}};

// One overlay CLSID per level. Explorer instantiates a handler per registered
// entry and asks it whether it applies to each file, so a distinct icon per
// level means a distinct handler per level - there is no way to vary the icon
// from within a single handler, because GetOverlayInfo is called once per
// process at load time and never receives a file path.
//
// {7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0D}
const CLSID CLSID_OverlayImportant =
    {0x7a5e3c21, 0x9b4d, 0x4e8f, {0xa6, 0xc2, 0x1d, 0x3e, 0x5f, 0x7a, 0x9b, 0x0d}};
// {7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0E}
const CLSID CLSID_OverlayDisposable =
    {0x7a5e3c21, 0x9b4d, 0x4e8f, {0xa6, 0xc2, 0x1d, 0x3e, 0x5f, 0x7a, 0x9b, 0x0e}};

// Stable per-command GUIDs. Explorer caches by these, so they must never
// change once shipped.
const GUID kCanonicalImportant =
    {0x1a2b3c4d, 0x0001, 0x4a10, {0x90, 0x01, 0x00, 0x11, 0x22, 0x33, 0x44, 0x01}};
const GUID kCanonicalDisposable =
    {0x1a2b3c4d, 0x0002, 0x4a10, {0x90, 0x01, 0x00, 0x11, 0x22, 0x33, 0x44, 0x02}};
const GUID kCanonicalClear =
    {0x1a2b3c4d, 0x0003, 0x4a10, {0x90, 0x01, 0x00, 0x11, 0x22, 0x33, 0x44, 0x03}};

/// The stream every marking lives in. Exposed by name so other tools (and the
/// main application) can read it without guessing.
const wchar_t* kStreamName = L"SpaceRecycle.Importance";
/// Payload version. Bump only if the shape changes; readers must tolerate
/// unknown versions by treating the file as unmarked.
const char* kPayloadVersion = "v1";

/// Registry path of the submenu under HKCU, per class.
const wchar_t* kVerbKey = L"Software\\Classes\\%s\\shell\\SpaceRecycle";

/// Built after both command types exist (they refer to each other), so the
/// enumeration helper is declared here and defined below.
IEnumExplorerCommand* CreateSubCommandEnumerator();

HINSTANCE g_module = nullptr;
LONG g_locks = 0;

// -------------------------------------------------------------------- levels

enum class Kind { Root, Important, Disposable, Clear };

struct LevelDef {
    Kind kind;
    const wchar_t* token;   // stored in the ADS
    const wchar_t* zh;
    const wchar_t* en;
    const wchar_t* ico;     // file name inside the "icons" folder next to the DLL
    const wchar_t* tip_zh;
    const wchar_t* tip_en;
    const GUID* canonical;
};

const LevelDef kLevels[] = {
    {Kind::Important, L"important", L"重要", L"Important", L"important.ico",
     L"标记为重要：清理时不允许永久删除，只能移到隔离区或备份到云端。",
     L"Mark as important: cleanup may quarantine or back it up, but never delete it.",
     &kCanonicalImportant},
    {Kind::Disposable, L"disposable", L"可清理", L"Disposable", L"disposable.ico",
     L"标记为可清理：清理时允许永久删除。",
     L"Mark as disposable: cleanup may delete it permanently.",
     &kCanonicalDisposable},
    {Kind::Clear, L"", L"清除标记", L"Clear mark", L"clear.ico",
     L"移除重要级别标记。",
     L"Remove the importance mark.",
     &kCanonicalClear},
};

constexpr int kLevelCount = (int)(sizeof(kLevels) / sizeof(kLevels[0]));

bool IsChineseUi() {
    // 0x0804 = zh-CN, 0x0404 = zh-TW, 0x0C04 = zh-HK.
    LANGID lang = GetUserDefaultUILanguage();
    return PRIMARYLANGID(lang) == LANG_CHINESE;
}

const LevelDef* FindLevel(Kind kind) {
    for (int i = 0; i < kLevelCount; ++i) {
        if (kLevels[i].kind == kind) {
            return &kLevels[i];
        }
    }
    return nullptr;
}

// ---------------------------------------------------------------- ADS access

/// Build `<path>:<stream>`. Callers pass a path with no trailing separator.
std::wstring StreamPath(const wchar_t* path) {
    std::wstring s(path);
    s += L':';
    s += kStreamName;
    return s;
}

/// Read the marked level for a path.
///
/// Returns an empty string for "unmarked", which includes every failure mode:
/// no such file, no such stream, a version this build does not understand, or a
/// stream that is not a directory entry we can read. A shell extension must
/// never make a file look marked because it could not read it.
std::wstring ReadLevel(const wchar_t* path) {
    std::wstring stream = StreamPath(path);
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

    // "v1|important|1730000000"
    std::string s(buf);
    size_t p1 = s.find('|');
    if (p1 == std::string::npos || s.compare(0, p1, kPayloadVersion) != 0) {
        return std::wstring();
    }
    size_t p2 = s.find('|', p1 + 1);
    std::string token = s.substr(p1 + 1, (p2 == std::string::npos ? s.size() : p2) - p1 - 1);
    if (token.empty()) {
        return std::wstring();
    }
    std::wstring w(token.begin(), token.end());
    // Only levels this build knows about count as marked.
    for (int i = 0; i < kLevelCount; ++i) {
        if (kLevels[i].kind != Kind::Clear && w == kLevels[i].token) {
            return w;
        }
    }
    return std::wstring();
}

// ---------------------------------------------------------------- the index
//
// A stream is a property of one file and says nothing about where the other
// marked files are, so the marks cannot be enumerated from them. The
// application therefore had no way to find a file marked from this menu, which
// made the menu an icon and nothing more: the delete guard protects what it can
// see, and it could not see these.
//
// This is the smallest thing that closes that - a plain text list of paths, one
// per line, that this menu appends to and the application reads.
//
// Deliberately **not** authoritative. The application re-reads each path's
// stream and ignores the entries that no longer have one, so:
//
//   * clearing a mark needs no write here at all - the stream goes, the entry
//     stops counting, and the application rewrites the file without it;
//   * a stale or unreadable line costs nothing and protects nothing;
//   * a failure to write this file loses protection for one file temporarily,
//     rather than corrupting anything.
//
// That is also why it lives in ProgramData rather than in the user's profile:
// the menu runs inside whatever process Explorer hosts it in, and the guard
// needs the same list it would see. Both run as the same user, but the shell's
// view of the profile is not something to depend on.

/// `%ProgramData%\SpaceRecycle\`, created if absent. Empty on failure.
std::wstring IndexDir() {
    wchar_t buf[MAX_PATH] = {0};
    DWORD n = GetEnvironmentVariableW(L"ProgramData", buf, MAX_PATH);
    if (n == 0 || n >= MAX_PATH) {
        return std::wstring();
    }
    std::wstring dir(buf, n);
    dir += L"\\SpaceRecycle";
    CreateDirectoryW(dir.c_str(), nullptr);
    // A second call distinguishes "created" from "already there" without
    // treating the already-there case as an error.
    if (GetLastError() != ERROR_ALREADY_EXISTS && !CreateDirectoryW(dir.c_str(), nullptr) &&
        GetLastError() != ERROR_ALREADY_EXISTS) {
        return std::wstring();
    }
    return dir;
}

/// Reads the index into a set of paths. Used to avoid appending a duplicate.
bool ReadIndex(std::vector<std::wstring>& out) {
    std::wstring dir = IndexDir();
    if (dir.empty()) {
        return false;
    }
    std::wstring file = dir + L"\\marked.txt";
    HANDLE h = CreateFileW(file.c_str(), GENERIC_READ,
                           FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                           nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) {
        // Not there yet is the normal first-run state, not a failure.
        return GetLastError() == ERROR_FILE_NOT_FOUND;
    }
    std::string text;
    char chunk[4096];
    DWORD read = 0;
    while (ReadFile(h, chunk, sizeof(chunk), &read, nullptr) && read > 0) {
        text.append(chunk, read);
    }
    CloseHandle(h);

    // A BOM would glue itself to the first path and make it match nothing. The
    // application strips it too; doing it on both sides means neither has to
    // trust the other.
    size_t start = 0;
    if (text.size() >= 3 && (unsigned char)text[0] == 0xEF &&
        (unsigned char)text[1] == 0xBB && (unsigned char)text[2] == 0xBF) {
        start = 3;
    }
    size_t pos = start;
    while (pos <= text.size()) {
        size_t nl = text.find('\n', pos);
        if (nl == std::string::npos) {
            nl = text.size();
        }
        std::string line = text.substr(pos, nl - pos);
        while (!line.empty() && (line.back() == '\r' || line.back() == ' ')) {
            line.pop_back();
        }
        size_t first = line.find_first_not_of(' ');
        if (first != std::string::npos && line[first] != '#') {
            std::wstring wide(line.begin() + first, line.end());
            out.push_back(wide);
        }
        if (nl == text.size()) {
            break;
        }
        pos = nl + 1;
    }
    return true;
}

/// Adds one path to the index, rewriting the file rather than appending.
///
/// Rewriting keeps the file bounded: marking and unmarking the same file
/// repeatedly would otherwise grow it without limit. Written through a
/// temporary file and renamed, so an interrupted write cannot truncate the list
/// and silently unprotected every other marked file.
void IndexAdd(const wchar_t* path) {
    std::vector<std::wstring> paths;
    if (!ReadIndex(paths)) {
        return;
    }
    std::wstring want(path);
    for (const auto& p : paths) {
        if (p == want) {
            return; // already listed
        }
    }
    paths.push_back(want);

    std::wstring dir = IndexDir();
    if (dir.empty()) {
        return;
    }
    std::wstring file = dir + L"\\marked.txt";
    std::wstring tmp = file + L".tmp";

    std::string body =
        "# Paths the right-click menu has marked.\n"
        "# Written by the shell extension and by the application; not\n"
        "# authoritative - a path only counts while its\n"
        "# SpaceRecycle.Importance stream says so.\n";
    for (const auto& p : paths) {
        for (wchar_t c : p) {
            body.push_back(c < 128 ? (char)c : '?');
        }
        body.push_back('\n');
    }

    HANDLE h = CreateFileW(tmp.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS,
                           FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) {
        return;
    }
    DWORD written = 0;
    BOOL ok = WriteFile(h, body.data(), (DWORD)body.size(), &written, nullptr) &&
              written == (DWORD)body.size();
    CloseHandle(h);
    if (!ok) {
        DeleteFileW(tmp.c_str());
        return;
    }
    MoveFileExW(tmp.c_str(), file.c_str(), MOVEFILE_REPLACE_EXISTING);
}

bool WriteLevel(const wchar_t* path, const wchar_t* token) {
    std::wstring stream = StreamPath(path);
    HANDLE h = CreateFileW(stream.c_str(), GENERIC_WRITE, FILE_SHARE_READ, nullptr,
                           CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (h == INVALID_HANDLE_VALUE) {
        return false;
    }
    char buf[128];
    // The timestamp is for the application's benefit (and for anyone reading
    // the stream by hand); the shell extension itself ignores it.
    int n = _snprintf_s(buf, sizeof(buf), _TRUNCATE, "%s|%ls|%lld", kPayloadVersion,
                        token, (long long)time(nullptr));
    DWORD written = 0;
    BOOL ok = (n > 0) && WriteFile(h, buf, (DWORD)n, &written, nullptr) &&
              written == (DWORD)n;
    CloseHandle(h);
    if (!ok) {
        // Never leave a half-written marker behind: an empty or short stream
        // would read back as unmarked and silently lose the user's choice.
        DeleteFileW(stream.c_str());
        return false;
    }
    // The stream is the mark; this is only how the application finds it. A
    // failure here is not reported as a failure to mark, because the mark is
    // written and the overlay will show - but it does mean the delete guard
    // will not know about the file, so it must not fail silently either. See
    // IndexPath below.
    IndexAdd(path);
    return true;
}

bool ClearLevel(const wchar_t* path) {
    std::wstring stream = StreamPath(path);
    // Deleting a stream that is not there is a success: the caller asked for
    // "no mark", and there is no mark.
    if (DeleteFileW(stream.c_str())) {
        return true;
    }
    return GetLastError() == ERROR_FILE_NOT_FOUND ||
           GetLastError() == ERROR_PATH_NOT_FOUND;
}

// ------------------------------------------------------------------ utilities

std::wstring ModuleDirectory() {
    wchar_t buf[MAX_PATH * 2] = {0};
    DWORD n = GetModuleFileNameW(g_module, buf, _countof(buf));
    if (n == 0 || n >= _countof(buf)) {
        return std::wstring();
    }
    std::wstring s(buf, n);
    size_t slash = s.find_last_of(L"\\/");
    return slash == std::wstring::npos ? std::wstring() : s.substr(0, slash);
}

std::wstring IconPath(const wchar_t* file) {
    std::wstring dir = ModuleDirectory();
    if (dir.empty()) {
        return std::wstring();
    }
    dir += L"\\icons\\";
    dir += file;
    return dir;
}

/// Collect the filesystem paths out of a shell item array.
std::vector<std::wstring> PathsFrom(IShellItemArray* items) {
    std::vector<std::wstring> out;
    if (items == nullptr) {
        return out;
    }
    DWORD count = 0;
    if (FAILED(items->GetCount(&count))) {
        return out;
    }
    for (DWORD i = 0; i < count; ++i) {
        IShellItem* item = nullptr;
        if (FAILED(items->GetItemAt(i, &item)) || item == nullptr) {
            continue;
        }
        PWSTR path = nullptr;
        if (SUCCEEDED(item->GetDisplayName(SIGDN_FILESYSPATH, &path)) && path != nullptr) {
            out.emplace_back(path);
            CoTaskMemFree(path);
        }
        item->Release();
    }
    return out;
}

// ------------------------------------------------------------- command object

class CommandEnumerator;

class SpaceRecycleCommand final : public IExplorerCommand {
public:
    explicit SpaceRecycleCommand(Kind kind) : m_ref(1), m_kind(kind) {}

    // ---- IUnknown ----
    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override {
        if (ppv == nullptr) {
            return E_POINTER;
        }
        *ppv = nullptr;
        if (IsEqualIID(riid, IID_IUnknown) || IsEqualIID(riid, IID_IExplorerCommand)) {
            *ppv = static_cast<IExplorerCommand*>(this);
            AddRef();
            return S_OK;
        }
        return E_NOINTERFACE;
    }
    STDMETHODIMP_(ULONG) AddRef() override { return (ULONG)InterlockedIncrement(&m_ref); }
    STDMETHODIMP_(ULONG) Release() override {
        LONG n = InterlockedDecrement(&m_ref);
        if (n == 0) {
            delete this;
        }
        return (ULONG)n;
    }

    // ---- IExplorerCommand ----
    STDMETHODIMP GetTitle(IShellItemArray*, LPWSTR* ppszName) override {
        if (ppszName == nullptr) {
            return E_POINTER;
        }
        *ppszName = nullptr;
        const bool zh = IsChineseUi();
        const wchar_t* text = nullptr;
        if (m_kind == Kind::Root) {
            text = zh ? L"重要级别" : L"Importance";
        } else {
            const LevelDef* def = FindLevel(m_kind);
            if (def == nullptr) {
                return E_FAIL;
            }
            text = zh ? def->zh : def->en;
        }
        return SHStrDupW(text, ppszName);
    }

    STDMETHODIMP GetIcon(IShellItemArray*, LPWSTR* ppszIcon) override {
        if (ppszIcon == nullptr) {
            return E_POINTER;
        }
        *ppszIcon = nullptr;
        if (m_kind == Kind::Root) {
            return S_FALSE;  // no icon on the parent is fine
        }
        const LevelDef* def = FindLevel(m_kind);
        if (def == nullptr) {
            return S_FALSE;
        }
        std::wstring path = IconPath(def->ico);
        if (path.empty()) {
            return S_FALSE;
        }
        return SHStrDupW(path.c_str(), ppszIcon);
    }

    STDMETHODIMP GetToolTip(IShellItemArray*, LPWSTR* ppszInfotip) override {
        if (ppszInfotip == nullptr) {
            return E_POINTER;
        }
        *ppszInfotip = nullptr;
        const bool zh = IsChineseUi();
        if (m_kind == Kind::Root) {
            const wchar_t* tip = zh ? L"给文件打上重要级别，标记会写入文件自身。"
                                    : L"Tag a file with an importance level; the mark is stored in the file itself.";
            return SHStrDupW(tip, ppszInfotip);
        }
        const LevelDef* def = FindLevel(m_kind);
        if (def == nullptr) {
            return S_FALSE;
        }
        return SHStrDupW(zh ? def->tip_zh : def->tip_en, ppszInfotip);
    }

    STDMETHODIMP GetCanonicalName(GUID* pguidCommandName) override {
        if (pguidCommandName == nullptr) {
            return E_POINTER;
        }
        if (m_kind == Kind::Root) {
            *pguidCommandName = CLSID_SpaceRecycleShell;
            return S_OK;
        }
        const LevelDef* def = FindLevel(m_kind);
        if (def == nullptr || def->canonical == nullptr) {
            return E_FAIL;
        }
        *pguidCommandName = *def->canonical;
        return S_OK;
    }

    STDMETHODIMP GetState(IShellItemArray* psiItemArray, BOOL, EXPCMDSTATE* pCmdState) override {
        if (pCmdState == nullptr) {
            return E_POINTER;
        }
        *pCmdState = ECS_ENABLED;
        if (m_kind == Kind::Root) {
            return S_OK;
        }
        std::vector<std::wstring> paths = PathsFrom(psiItemArray);
        if (paths.empty()) {
            return S_OK;
        }
        const LevelDef* def = FindLevel(m_kind);
        if (def == nullptr) {
            return S_OK;
        }
        // A checkmark only when the whole selection already agrees. Showing it
        // for a mixed selection would claim a state that is not true of every
        // file the user is looking at.
        bool all = true;
        for (size_t i = 0; i < paths.size(); ++i) {
            std::wstring current = ReadLevel(paths[i].c_str());
            bool matches = (m_kind == Kind::Clear) ? current.empty()
                                                   : (current == def->token);
            if (!matches) {
                all = false;
                break;
            }
        }
        if (all) {
            *pCmdState = (EXPCMDSTATE)(ECS_ENABLED | ECS_CHECKED);
        }
        return S_OK;
    }

    STDMETHODIMP Invoke(IShellItemArray* psiItemArray, IBindCtx*) override {
        if (m_kind == Kind::Root) {
            // The parent only opens a submenu; nothing to do if it is invoked.
            return S_OK;
        }
        const LevelDef* def = FindLevel(m_kind);
        if (def == nullptr) {
            return E_FAIL;
        }
        std::vector<std::wstring> paths = PathsFrom(psiItemArray);
        if (paths.empty()) {
            return S_OK;
        }

        std::vector<std::wstring> failed;
        for (size_t i = 0; i < paths.size(); ++i) {
            const wchar_t* p = paths[i].c_str();
            bool ok = (m_kind == Kind::Clear) ? ClearLevel(p) : WriteLevel(p, def->token);
            if (ok) {
                // Refresh the item so anything drawn from the stream (a future
                // icon overlay, the details pane) redraws.
                SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_PATHW | SHCNF_FLUSHNOWAIT, p, nullptr);
            } else {
                failed.push_back(paths[i]);
            }
        }

        if (!failed.empty()) {
            // Only on failure: a shell extension that pops dialogs on the happy
            // path is an annoyance, but silently dropping the user's click is
            // worse. Read-only files have no write access and cannot take a
            // stream at all.
            std::wstring msg = IsChineseUi()
                ? L"以下文件无法标记（可能只读，或不在 NTFS 卷上）：\n\n"
                : L"These files could not be tagged (read-only, or not on an NTFS volume):\n\n";
            for (size_t i = 0; i < failed.size() && i < 10; ++i) {
                msg += failed[i];
                msg += L"\n";
            }
            if (failed.size() > 10) {
                msg += L"…\n";
            }
            MessageBoxW(nullptr, msg.c_str(),
                        IsChineseUi() ? L"SpaceRecycle" : L"SpaceRecycle",
                        MB_OK | MB_ICONWARNING);
        }
        return S_OK;
    }

    STDMETHODIMP GetFlags(EXPCMDFLAGS* pFlags) override {
        if (pFlags == nullptr) {
            return E_POINTER;
        }
        *pFlags = (m_kind == Kind::Root) ? ECF_HASSUBCOMMANDS : ECF_DEFAULT;
        return S_OK;
    }

    STDMETHODIMP EnumSubCommands(IEnumExplorerCommand** ppEnum) override {
        if (ppEnum == nullptr) {
            return E_POINTER;
        }
        *ppEnum = nullptr;
        if (m_kind != Kind::Root) {
            // A leaf has no children. Returning an empty enumerator is also
            // valid, but E_NOTIMPL lets Explorer skip the call entirely.
            return E_NOTIMPL;
        }
        IEnumExplorerCommand* created = CreateSubCommandEnumerator();
        if (created == nullptr) {
            return E_OUTOFMEMORY;
        }
        *ppEnum = created;
        return S_OK;
    }

private:
    ~SpaceRecycleCommand() = default;
    LONG m_ref;
    Kind m_kind;
};

// -------------------------------------------------------------- enumeration

class CommandEnumerator final : public IEnumExplorerCommand {
public:
    static CommandEnumerator* Create() {
        CommandEnumerator* e = new (std::nothrow) CommandEnumerator();
        if (e == nullptr) {
            return nullptr;
        }
        for (int i = 0; i < kLevelCount; ++i) {
            SpaceRecycleCommand* c = new (std::nothrow) SpaceRecycleCommand(kLevels[i].kind);
            if (c == nullptr) {
                e->Release();
                return nullptr;
            }
            e->m_children.push_back(c);
        }
        return e;
    }

    STDMETHODIMP QueryInterface(REFIID riid, void** ppv) override {
        if (ppv == nullptr) {
            return E_POINTER;
        }
        *ppv = nullptr;
        if (IsEqualIID(riid, IID_IUnknown) || IsEqualIID(riid, IID_IEnumExplorerCommand)) {
            *ppv = static_cast<IEnumExplorerCommand*>(this);
            AddRef();
            return S_OK;
        }
        return E_NOINTERFACE;
    }
    STDMETHODIMP_(ULONG) AddRef() override { return (ULONG)InterlockedIncrement(&m_ref); }
    STDMETHODIMP_(ULONG) Release() override {
        LONG n = InterlockedDecrement(&m_ref);
        if (n == 0) {
            delete this;
        }
        return (ULONG)n;
    }

    STDMETHODIMP Next(ULONG celt, IExplorerCommand** pUICommand, ULONG* pceltFetched) override {
        if (pUICommand == nullptr) {
            return E_POINTER;
        }
        ULONG fetched = 0;
        while (fetched < celt && m_index < m_children.size()) {
            IExplorerCommand* c = m_children[m_index];
            c->AddRef();
            pUICommand[fetched] = c;
            ++m_index;
            ++fetched;
        }
        if (pceltFetched != nullptr) {
            *pceltFetched = fetched;
        }
        return fetched == celt ? S_OK : S_FALSE;
    }

    STDMETHODIMP Skip(ULONG celt) override {
        m_index += celt;
        if (m_index > m_children.size()) {
            m_index = m_children.size();
            return S_FALSE;
        }
        return S_OK;
    }

    STDMETHODIMP Reset() override {
        m_index = 0;
        return S_OK;
    }

    STDMETHODIMP Clone(IEnumExplorerCommand** ppEnum) override {
        if (ppEnum == nullptr) {
            return E_POINTER;
        }
        *ppEnum = nullptr;
        CommandEnumerator* e = Create();
        if (e == nullptr) {
            return E_OUTOFMEMORY;
        }
        e->m_index = m_index;
        *ppEnum = e;
        return S_OK;
    }

private:
    CommandEnumerator() : m_ref(1), m_index(0) {}
    ~CommandEnumerator() {
        for (size_t i = 0; i < m_children.size(); ++i) {
            m_children[i]->Release();
        }
    }

    LONG m_ref;
    size_t m_index;
    std::vector<SpaceRecycleCommand*> m_children;
};

/// Both command types are complete here, which is what the forward declaration
/// near the top was waiting for: the two classes refer to each other, so the
/// builder cannot live inside either one.
IEnumExplorerCommand* CreateSubCommandEnumerator() {
    CommandEnumerator* e = CommandEnumerator::Create();
    if (e == nullptr) {
        return nullptr;
    }
    IEnumExplorerCommand* out = nullptr;
    HRESULT hr = e->QueryInterface(IID_PPV_ARGS(&out));
    e->Release();
    return SUCCEEDED(hr) ? out : nullptr;
}

// ----------------------------------------------------------- icon overlays

/// Reports whether a path carries one particular importance level.
///
/// Explorer calls IsMemberOf once for every item it draws, on its UI thread, so
/// this is the hottest path in the extension and it has to stay cheap. The
/// common case - an unmarked file - costs exactly one GetFileAttributesW
/// against the stream path: no stream, nothing to open, nothing to parse. A
/// file that is actually marked pays for one read on top of that, and a marked
/// file of the *other* level pays the same, which is unavoidable when each
/// level is a separate handler.
class OverlayHandler final : public IShellIconOverlayIdentifier {
public:
    explicit OverlayHandler(Kind kind) : m_ref(1), m_kind(kind) {
        const LevelDef* def = (kind == Kind::Clear) ? nullptr : FindLevel(kind);
        if (def != nullptr) {
            m_token = def->token;
            m_iconFile = def->ico;
        }
    }

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
    STDMETHODIMP_(ULONG) AddRef() override { return (ULONG)InterlockedIncrement(&m_ref); }
    STDMETHODIMP_(ULONG) Release() override {
        LONG n = InterlockedDecrement(&m_ref);
        if (n == 0) {
            delete this;
        }
        return (ULONG)n;
    }

    // ---- IShellIconOverlayIdentifier ----

    STDMETHODIMP IsMemberOf(LPCWSTR pwszPath, DWORD) override {
        if (pwszPath == nullptr || m_token.empty()) {
            return S_FALSE;
        }
        std::wstring stream = StreamPath(pwszPath);
        // Existence first. GetFileAttributesW on a stream path is one call and
        // needs no handle, so the overwhelmingly common "this file is not
        // marked at all" answer costs a single syscall.
        if (GetFileAttributesW(stream.c_str()) == INVALID_FILE_ATTRIBUTES) {
            return S_FALSE;
        }
        return ReadLevel(pwszPath) == m_token ? S_OK : S_FALSE;
    }

    STDMETHODIMP GetOverlayInfo(LPWSTR pwszIconFile, int cchMax, int* pIndex,
                               DWORD* pdwFlags) override {
        if (pwszIconFile == nullptr || pIndex == nullptr || pdwFlags == nullptr) {
            return E_POINTER;
        }
        if (cchMax <= 0) {
            return E_INVALIDARG;
        }
        std::wstring icon = IconPath(m_iconFile.c_str());
        if (icon.empty()) {
            return E_FAIL;
        }
        wcsncpy_s(pwszIconFile, (size_t)cchMax, icon.c_str(), _TRUNCATE);
        *pIndex = 0;
        *pdwFlags = ISIOI_ICONFILE | ISIOI_ICONINDEX;
        return S_OK;
    }

    STDMETHODIMP GetPriority(int* pPriority) override {
        if (pPriority == nullptr) {
            return E_POINTER;
        }
        // Lower number wins. Only one overlay is drawn per item, so this only
        // decides what happens if a file somehow carries both marks - and then
        // the more cautious badge is the one worth showing.
        *pPriority = (m_kind == Kind::Important) ? 0 : 1;
        return S_OK;
    }

private:
    ~OverlayHandler() = default;
    LONG m_ref;
    Kind m_kind;
    std::wstring m_token;
    std::wstring m_iconFile;
};

// ------------------------------------------------------------- class factory

class ShellFactory final : public IClassFactory {
public:
    explicit ShellFactory(REFCLSID clsid) : m_ref(1), m_clsid(clsid) {}

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
    STDMETHODIMP_(ULONG) AddRef() override { return (ULONG)InterlockedIncrement(&m_ref); }
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

        // One factory serves every class in the DLL; which object to build is
        // decided by the CLSID the factory was created for.
        IUnknown* created = nullptr;
        if (IsEqualCLSID(m_clsid, CLSID_SpaceRecycleShell)) {
            created = new (std::nothrow) SpaceRecycleCommand(Kind::Root);
        } else if (IsEqualCLSID(m_clsid, CLSID_OverlayImportant)) {
            created = new (std::nothrow) OverlayHandler(Kind::Important);
        } else if (IsEqualCLSID(m_clsid, CLSID_OverlayDisposable)) {
            created = new (std::nothrow) OverlayHandler(Kind::Disposable);
        } else {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        if (created == nullptr) {
            return E_OUTOFMEMORY;
        }
        HRESULT hr = created->QueryInterface(riid, ppv);
        created->Release();
        return hr;
    }

    STDMETHODIMP LockServer(BOOL lock) override {
        g_locks += lock ? 1 : -1;
        return S_OK;
    }

private:
    ~ShellFactory() = default;
    LONG m_ref;
    CLSID m_clsid;
};

// ------------------------------------------------------------- registration

// Declared before use: the overlay registration below needs them, and they are
// defined at the end of this namespace.
bool SetKeyValue(HKEY root, const std::wstring& subkey, const wchar_t* name,
                 const std::wstring& value);
bool DeleteKeyTree(HKEY root, const std::wstring& subkey);
std::wstring GuidToString(REFGUID g);

/// 13 leading spaces, and that number is not decoration.
///
/// Explorer keeps only the first 15 entries under ShellIconOverlayIdentifiers,
/// ordered by a character-by-character comparison of the key NAME. A space
/// sorts below every letter, so at the first position where two names differ
/// the one holding a space wins - meaning more leading spaces means a better
/// slot. OneDrive uses 13 spaces and owns seven slots on a machine with 35
/// entries registered. Matching that, and starting the real name with "S",
/// places these after OneDrive's entries and before Tortoise's.
const wchar_t* kOverlayNamePrefix = L"             ";

const wchar_t* kOverlayKey =
    L"SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Explorer\\ShellIconOverlayIdentifiers\\";

struct OverlayRegistration {
    const CLSID* clsid;
    const wchar_t* name;        // without the leading spaces
    const wchar_t* description;
};

const OverlayRegistration kOverlays[] = {
    {&CLSID_OverlayImportant, L"SpaceRecycleImportant",
     L"SpaceRecycle overlay: important"},
    {&CLSID_OverlayDisposable, L"SpaceRecycleDisposable",
     L"SpaceRecycle overlay: disposable"},
};
constexpr int kOverlayCount = (int)(sizeof(kOverlays) / sizeof(kOverlays[0]));

/// Registers the two overlay handlers under HKLM.
///
/// Returns true only if every key was written. Overlays have no per-user
/// equivalent - Windows reads them from HKLM alone - so this needs an elevated
/// caller, and a failure here is a real, reportable condition rather than
/// something to swallow.
bool RegisterOverlays(const std::wstring& dllPath) {
    bool allOk = true;
    for (int i = 0; i < kOverlayCount; ++i) {
        const std::wstring clsid = GuidToString(*kOverlays[i].clsid);
        const std::wstring clsidKey = L"SOFTWARE\\Classes\\CLSID\\" + clsid;
        bool ok = SetKeyValue(HKEY_LOCAL_MACHINE, clsidKey, nullptr,
                              kOverlays[i].description) &&
                  SetKeyValue(HKEY_LOCAL_MACHINE, clsidKey + L"\\InprocServer32", nullptr,
                              dllPath) &&
                  SetKeyValue(HKEY_LOCAL_MACHINE, clsidKey + L"\\InprocServer32",
                              L"ThreadingModel", L"Apartment");
        const std::wstring overlayKey =
            std::wstring(kOverlayKey) + kOverlayNamePrefix + kOverlays[i].name;
        ok = ok && SetKeyValue(HKEY_LOCAL_MACHINE, overlayKey, nullptr, clsid);
        if (!ok) {
            allOk = false;
        }
    }
    return allOk;
}

void UnregisterOverlays() {
    for (int i = 0; i < kOverlayCount; ++i) {
        const std::wstring clsid = GuidToString(*kOverlays[i].clsid);
        DeleteKeyTree(HKEY_LOCAL_MACHINE, L"SOFTWARE\\Classes\\CLSID\\" + clsid);
        DeleteKeyTree(HKEY_LOCAL_MACHINE,
                      std::wstring(kOverlayKey) + kOverlayNamePrefix + kOverlays[i].name);
    }
}

/// Write one registry string value, creating the key path as needed.
bool SetKeyValue(HKEY root, const std::wstring& subkey, const wchar_t* name,
                 const std::wstring& value) {
    HKEY key = nullptr;
    if (RegCreateKeyExW(root, subkey.c_str(), 0, nullptr, REG_OPTION_NON_VOLATILE,
                        KEY_WRITE, nullptr, &key, nullptr) != ERROR_SUCCESS) {
        return false;
    }
    LONG rc = RegSetValueExW(key, name, 0, REG_SZ,
                             reinterpret_cast<const BYTE*>(value.c_str()),
                             (DWORD)((value.size() + 1) * sizeof(wchar_t)));
    RegCloseKey(key);
    return rc == ERROR_SUCCESS;
}

bool DeleteKeyTree(HKEY root, const std::wstring& subkey) {
    LONG rc = RegDeleteTreeW(root, subkey.c_str());
    return rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND;
}

std::wstring GuidToString(REFGUID g) {
    wchar_t buf[64] = {0};
    StringFromGUID2(g, buf, _countof(buf));
    return std::wstring(buf);
}

}  // namespace

// ------------------------------------------------------------------- exports

BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID) {
    if (reason == DLL_PROCESS_ATTACH) {
        g_module = inst;
        DisableThreadLibraryCalls(inst);
    }
    return TRUE;
}

STDAPI DllGetClassObject(REFCLSID rclsid, REFIID riid, void** ppv) {
    if (ppv == nullptr) {
        return E_POINTER;
    }
    *ppv = nullptr;
    if (!IsEqualCLSID(rclsid, CLSID_SpaceRecycleShell) &&
        !IsEqualCLSID(rclsid, CLSID_OverlayImportant) &&
        !IsEqualCLSID(rclsid, CLSID_OverlayDisposable)) {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    ShellFactory* f = new (std::nothrow) ShellFactory(rclsid);
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

/// Register per-user under HKCU.
///
/// HKCU rather than HKLM on purpose: it needs no administrator, it cannot
/// break another user's Explorer, and it unregisters cleanly. The icon-overlay
/// handler that comes later has no such option, since Windows only reads
/// overlays from HKLM - which is one more reason the two are kept separate.
STDAPI DllRegisterServer() {
    std::wstring dll = ModuleDirectory();
    if (dll.empty()) {
        return E_FAIL;
    }
    dll += L"\\SpaceRecycleShell.dll";

    std::wstring clsid = GuidToString(CLSID_SpaceRecycleShell);
    std::wstring clsidKey = L"Software\\Classes\\CLSID\\" + clsid;
    if (!SetKeyValue(HKEY_CURRENT_USER, clsidKey, nullptr, L"SpaceRecycle importance menu")) {
        return E_FAIL;
    }
    if (!SetKeyValue(HKEY_CURRENT_USER, clsidKey + L"\\InprocServer32", nullptr, dll)) {
        return E_FAIL;
    }
    if (!SetKeyValue(HKEY_CURRENT_USER, clsidKey + L"\\InprocServer32", L"ThreadingModel",
                     L"Apartment")) {
        return E_FAIL;
    }

    // `*` covers files, `Directory` covers folders. ExplorerCommandHandler is
    // the modern hook, so this appears in the Windows 11 menu rather than
    // under "Show more options".
    const wchar_t* classes[] = {L"*", L"Directory"};
    for (int i = 0; i < 2; ++i) {
        wchar_t buf[128] = {0};
        _snwprintf_s(buf, _countof(buf), _TRUNCATE, kVerbKey, classes[i]);
        if (!SetKeyValue(HKEY_CURRENT_USER, buf, L"ExplorerCommandHandler", clsid)) {
            return E_FAIL;
        }
        if (!SetKeyValue(HKEY_CURRENT_USER, buf, L"MUIVerb", L"重要级别")) {
            return E_FAIL;
        }
    }

    // Overlays need HKLM, so they can legitimately fail on an unelevated
    // install. S_FALSE says "the menu is registered, the overlays are not" -
    // which is a state the caller can act on, and is honest about a partial
    // result rather than reporting a success that did not happen.
    if (!RegisterOverlays(dll)) {
        return S_FALSE;
    }
    return S_OK;
}

STDAPI DllUnregisterServer() {
    std::wstring clsid = GuidToString(CLSID_SpaceRecycleShell);
    DeleteKeyTree(HKEY_CURRENT_USER, L"Software\\Classes\\CLSID\\" + clsid);

    const wchar_t* classes[] = {L"*", L"Directory"};
    for (int i = 0; i < 2; ++i) {
        wchar_t buf[128] = {0};
        _snwprintf_s(buf, _countof(buf), _TRUNCATE, kVerbKey, classes[i]);
        DeleteKeyTree(HKEY_CURRENT_USER, buf);
    }

    UnregisterOverlays();
    return S_OK;
}