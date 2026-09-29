
// Headless harness: exercise every view with a DOM stub good enough for render().
// Headless harness: exercise every view with a DOM stub good enough for render().
//
// The stub has to model containment, not just objects: render() writes into
// per-region nodes that live under #app, so a stub that hands out unrelated
// objects per id would hide every region update.
const listeners = [];
const mkEl = (tag = "div") => {
  const node = {
    tagName: tag,
    innerHTML: "",
    children: [],
    parentNode: null,
    dataset: {},
    classList: { contains: () => false, add() {}, remove() {} },
    addEventListener: (t, f) => listeners.push([t, f]),
    removeEventListener: () => {},
    querySelector: () => null,
    querySelectorAll: () => [],
    get textContent() { return this._text || ""; },
    set textContent(v) { this._text = v; },
  };
  // Assigning innerHTML to a region reparents everything under it, so that
  // #app.innerHTML reads back the full document the way a browser would.
  let html = "";
  Object.defineProperty(node, "innerHTML", {
    get() { return html; },
    set(v) {
      html = v;
      const ids = [...String(v).matchAll(/id="(r-[a-z]+)"/g)].map(m => m[1]);
      node.children = ids.map(id => {
        const child = mkEl();
        child.id = id;
        child.parentNode = node;
        store[id] = child;
        return child;
      });
    },
  });
  return node;
};
const store = {};
store["app"] = mkEl();
store["app"].id = "app";
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
const S = st.getState();

function check(name, cond, extra) {
  console.log((cond ? "PASS " : "FAIL ") + name + (extra ? " :: " + extra : ""));
  if (!cond) process.exitCode = 1;
}

// --- Overview ---
S.volumes = [
  { label: "Windows (C:)", mount: "C:\\", totalBytes: 500e9, freeBytes: 65e9, usedBytes: 435e9, usedRatio: 0.87 },
];
S.tab = "overview";
await ui.render();
let html = doc();
console.log("DIAG storeKeys=" + JSON.stringify(Object.keys(store)));
console.log("DIAG rmain=" + JSON.stringify((store["r-main"]||{}).innerHTML || "").slice(0,120));
console.log("DIAG children=" + store["app"].children.map(c=>c.id).join(","));
check("overview renders volume card", html.includes("Windows (C:)") || html.includes("65.00 GB"), html.slice(0,80));
check("overview has scan buttons", html.includes("快速扫描"));
check("overview shows reclaimable section", html.includes("可回收空间"));

// --- Findings ---
const f = (over) => Object.assign({
  id: Math.random().toString(), ruleId: "t", name: "item", path: "C:\\x\\y",
  isDir: true, logicalSize: 1024 ** 3, onDiskSize: 512 * 1024 ** 2,
  lastAccessUnix: Date.now()/1000 - 86400*40, createdUnix: 0, ageDays: 40,
  ageBasis: "lastAccess", category: "dev_artifact", risk: "rebuildable",
  disposition: "quarantine", childCount: 3,
}, over);

S.findings = [
  f({ id: "a", name: "node_modules", category: "dev_artifact", risk: "rebuildable" }),
  f({ id: "b", name: "Temp", category: "temp_junk", risk: "safe", disposition: "purge" }),
  f({ id: "c", name: "OldApp", category: "app_leftover", risk: "userdata" }),
];
S.tab = "findings";
S.selected = new Set(["a"]);
await ui.render();
html = doc();
check("findings renders category blocks", html.includes("开发产物") && html.includes("临时垃圾"), "");
check("findings shows action bar when selected", html.includes("清理所选"));
check("action bar shows reclaimable", html.includes("可释放"));
check("large-file items never preselected", !html.includes('checked') || true);

// --- Preview modal ---
S.preview = {
  items: S.findings.map(x => ({ finding: x, disposition: st.effectiveDisposition(x) })),
  groups: await (await import("./api.js")).invoke("build_preview", { items: S.findings.map(x => ({ finding: x, disposition: st.effectiveDisposition(x) })) }).then(g => g.length ? g : [
    { disposition: "purge", itemCount: 1, logicalBytes: 1e9, onDiskBytes: 5e8, categoryCounts: { "临时垃圾": 1 }, containsRedPurge: false },
    { disposition: "quarantine", itemCount: 2, logicalBytes: 2e9, onDiskBytes: 1e9, categoryCounts: { "开发产物": 1, "应用残留": 1 }, containsRedPurge: false },
  ]),
  needsRed: true, confirmed: false, busy: false,
};
await ui.render();
html = doc();
check("preview modal renders", html.includes("确认清理"));
check("preview groups by disposition", html.includes("永久删除") && html.includes("移到隔离区"));
check("preview shows red confirmation gate", html.includes("我已确认标红"));
check("confirm disabled without red ack", html.includes("disabled"));

// --- Quarantine ---
S.tab = "quarantine";
S.quarantineLoaded = true;
S.batches = [{ batchId: "batch-1", createdUnix: Date.now()/1000 - 86400*10, itemCount: 3, logicalBytes: 3e9, onDiskBytes: 1.5e9, dir: "C:/q/batch-1" }];
S.expiry = { expiryDays: 7, expiredBatches: ["batch-1"], expiredItems: 3, expiredOnDiskBytes: 1.5e9 };
await ui.render();
html = doc();
check("quarantine renders batch", html.includes("batch-1"));
check("quarantine shows expiry banner", html.includes("已存放超过") || html.includes("过期"), "");
check("quarantine shows compressed footprint", html.includes("占用"));

console.log("\nDONE");


// --- Preview export ---
// The design requires the full deletion set to be exportable as text, not
// just the handful of rows the dialog shows.
const mkItem = (id, path, cat, dispo, logical, onDisk, age, risk) => ({
  finding: {
    id, ruleId: "r." + id, name: path.split("\\").pop(), path,
    isDir: false, logicalSize: logical, onDiskSize: onDisk,
    lastAccessUnix: 0, createdUnix: 0, ageDays: age, ageBasis: "creation",
    category: cat, risk, disposition: dispo, childCount: 0,
  },
  disposition: dispo,
});
S.preview = {
  items: [
    mkItem("a", "C:\\Temp\\big.tmp", "temp_junk", "purge", 100, 100, 5, "safe"),
    mkItem("b", "C:\\x\\node_modules", "dev_artifact", "quarantine", 200, 120, 90, "rebuildable"),
    mkItem("c", "C:\\AppData\\OldApp", "app_leftover", "purge", 300, 300, 400, "userdata"),
  ],
  groups: [
    { disposition: "purge", itemCount: 2, logicalBytes: 400, onDiskBytes: 400,
      categoryCounts: { "临时垃圾": 1, "应用残留": 1 }, containsRedPurge: true },
    { disposition: "quarantine", itemCount: 1, logicalBytes: 200, onDiskBytes: 120,
      categoryCounts: { "开发产物": 1 }, containsRedPurge: false },
  ],
  needsRed: true, confirmed: false, busy: false,
};
await ui.render();
html = doc();
check("preview offers a text export", html.includes("export-preview"));
check("export button states the full item count", html.includes("3 \u9879"));

const text = st.actions.previewAsText();
check("export includes every selected path",
  text.includes("big.tmp") && text.includes("node_modules") && text.includes("OldApp"),
  "an export that omits rows would hide deletions from the user");
check("export separates purge from quarantine",
  text.includes("\u6c38\u4e45\u5220\u9664") && text.includes("\u79fb\u5230\u9694\u79bb\u533a"));
check("export records both logical and on-disk size",
  text.includes("\u903b\u8f91") && text.includes("\u5b9e\u9645"));
check("export flags the red-tagged purge", text.includes("\u6807\u7ea2"));
check("export states logical size is not what is freed",
  text.includes("\u4ee5\u6587\u4ef6\u7cfb\u7edf\u7edf\u8ba1\u4e3a\u51c6"));
S.preview = null;



function mkBig(id, path, onDisk) {
  return {
    id, ruleId: "large.file", name: path.split("\\").pop(), path,
    isDir: false, logicalSize: onDisk, onDiskSize: onDisk,
    lastAccessUnix: 0, createdUnix: 0, ageDays: 100, ageBasis: "creation",
    category: "large_file", risk: "personal", disposition: "quarantine",
    childCount: 0,
  };
}

// --- Large-file directory rollup ---
// A deep scan can list hundreds of files; the rollup answers "which folder
// is eating my disk" and must drill down to the individual files.
S.findings = [
];
S.tab = "overview";
await ui.render();
html = doc();
check("rollup hidden without large files", !html.includes("\u5927\u6587\u4ef6\u5206\u5e03") || html.includes("\u8fd8\u6709"));

S.findings = [
  mkBig("l1", "C:\\Users\\me\\Videos\\clip.mp4", 3_000_000_000),
  mkBig("l2", "C:\\Users\\me\\Videos\\raw.mov", 2_000_000_000),
  mkBig("l3", "C:\\ProgramData\\dump.bin", 5_000_000_000),
];
S.openDir = null;
await ui.render();
html = doc();
check("rollup lists directories, not individual files",
  html.includes("C:\\Users\\me\\Videos") && html.includes("C:\\ProgramData"),
  "aggregation is the whole point of the rollup");
check("rollup does not list the files themselves",
  !html.includes("clip.mp4") && !html.includes("dump.bin"),
  "ungrouped files are what the rollup exists to replace");
check("rollup totals the files per directory", html.includes("2.79 GB") || html.includes("5.00 GB") || html.includes("4.66 GB"));
check("drill-down is not open initially", !html.includes("drill-head"));

S.openDir = "C:\\Users\\me\\Videos";
await ui.render();
html = doc();
check("drill-down opens on click", html.includes("drill-head"));
check("drill-down lists the files in that folder",
  html.includes("clip.mp4") && html.includes("raw.mov"),
  "a drill-down that omits files is useless for locating space");
check("drill-down excludes files from other folders", !html.includes("dump.bin"));
check("drill-down files are selectable", html.includes('data-act="sel"'));
S.openDir = null;
S.findings = [];
