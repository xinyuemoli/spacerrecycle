
// Headless harness covering the settings tab and the audit-log view.
//
// The stub models containment: render() writes into per-region nodes under
// #app, so handing out unrelated objects per id would hide every update.
const mkEl = () => {
  const node = {
    innerHTML: "", children: [], dataset: {},
    classList: { contains: () => false, add() {}, remove() {} },
    addEventListener: () => {}, removeEventListener: () => {},
    querySelector: () => null, querySelectorAll: () => [],
    get textContent() { return this._text || ""; },
    set textContent(v) { this._text = v; },
  };
  let html = "";
  Object.defineProperty(node, "innerHTML", {
    get() { return html; },
    set(v) {
      html = v;
      // Keep the full id: ui.js looks regions up as "r-" + name.
      const ids = [...String(v).matchAll(/id="(r-[a-z]+)"/g)].map(m => m[1]);
      node.children = ids.map(id => {
        const child = mkEl();
        child.id = id;
        store[id] = child;
        return child;
      });
    },
  });
  return node;
};
const store = {};
store["app"] = mkEl();
globalThis.window = { __TAURI__: undefined };
globalThis.document = {
  getElementById: (id) => store[id] || null,
  querySelector: () => null,
  querySelectorAll: () => [],
  activeElement: null,
};
globalThis.confirm = () => true;

// The document is now assembled from regions, so assertions read the composed
// tree rather than the shell's own innerHTML.
function doc() {
  return store["app"].innerHTML + store["app"].children.map(c => c.innerHTML).join("\n");
}

const ui = await import("./ui.js");
const st = await import("./state.js");
const api = await import("./api.js");
const S = st.getState();

function check(name, cond, extra) {
  console.log((cond ? "PASS " : "FAIL ") + name + (extra ? " :: " + extra : ""));
  if (!cond) process.exitCode = 1;
}

S.config = {
  largeFileAgeDays: 45, leftoverAgeDays: 60,
  largeFileMinBytes: 1048576, quarantineExpiryDays: 14,
  quarantineDir: "C:\\ProgramData\\SpaceRecycle\\Quarantine",
};
S.auditLog = [
  { unix: 1756000000, action: "purge", path: "C:\\Temp\\a.tmp", bytes: 1024, outcome: "\u6210\u529f" },
  { unix: 1756000100, action: "quarantine", path: "C:\\x\\node_modules", bytes: 9999, outcome: "\u6210\u529f" },
  { unix: 1756000200, action: "restore", path: "C:\\x\\node_modules", bytes: 9999, outcome: "\u6210\u529f" },
];
S.auditPath = "C:\\Users\\me\\AppData\\Roaming\\SpaceRecycle\\audit.log";
S.tab = "settings";
await ui.render();
const html = doc();

check("settings tab renders", html.includes("\u8bbe\u7f6e"));
check("age thresholds are editable", html.includes("largeFileAgeDays") && html.includes("leftoverAgeDays"));
check("quarantine dir is shown", html.includes("SpaceRecycle") && html.includes("Quarantine"));
check("audit log table renders", html.includes("\u64cd\u4f5c\u65e5\u5fd7"));
check("audit entries listed", html.includes("a.tmp") && html.includes("node_modules"));
check("log file path shown", html.includes("audit.log"));
check("large-file size shown in MB", html.includes('value="1"'), "1048576 bytes should render as 1 MB");
check("expiry notice states no auto-delete", html.includes("\u7edd\u4e0d\u81ea\u52a8\u5220\u9664"));

console.log("\nDONE");


// ---- Elevation prompt ----
// The banner must appear only when a cleanup actually hit a permission wall.
const before = doc();
check("no elevation banner without a permission failure",
  !(store["r-elevbanner"].innerHTML || "").includes("以管理员身份重试"));

S.elevateRequest = { items: [], confirmedRedPurge: true };
await ui.render();
const withBanner = doc();
check("elevation banner appears when elevation is needed", withBanner.includes("elevbanner"));
check("banner explains the retry", withBanner.includes("\u4ee5\u7ba1\u7406\u5458\u8eab\u4efd\u91cd\u8bd5"));
check("banner offers a way to decline", withBanner.includes("dismiss-elevate"));
check("banner states the rest will not be redone", withBanner.includes("\u4e0d\u4f1a\u91cd\u590d\u6e05\u7406"));

st.actions.dismissElevate();
await ui.render();
check("banner can be dismissed",
  !(store["r-elevbanner"].innerHTML || "").includes("以管理员身份重试"));
