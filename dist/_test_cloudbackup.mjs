/**
 * Cloud-backup UI tests.
 *
 * The backend owns the two invariants that matter (upload before delete, and
 * fall back to quarantine on failure); this file covers the three things the UI
 * must not get wrong:
 *
 *   - offering a paid feature to someone who has not paid for it;
 *   - offering a backup of something a backup is meaningless for (temp files,
 *     or content already in OneDrive);
 *   - reporting a fallback as if the user had got the cloud copy they asked for.
 */

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

let cleanupReport = null;
globalThis.window = {
  __TAURI__: {
    core: {
      invoke: async (cmd) => {
        if (cmd === "run_cleanup") return cleanupReport;
        if (cmd === "quarantine_expiry") return { expiryDays: 7, expiredBatches: [], expiredItems: 0, expiredOnDiskBytes: 0 };
        return null;
      },
    },
    event: { listen: async () => () => {} },
  },
};
globalThis.document = {
  getElementById: (id) => store[id] || null,
  querySelector: () => null,
  querySelectorAll: () => [],
  activeElement: null,
};
globalThis.confirm = () => true;

const ui = await import("./ui.js");
const st = await import("./state.js");
const S = st.getState();

let pass = 0, fail = 0;
function check(name, cond, extra) {
  if (cond) { pass++; console.log("PASS " + name); }
  else { fail++; console.log("FAIL " + name + (extra ? " :: " + extra : "")); }
}
const doc = () => store["app"].innerHTML + store["app"].children.map(c => c.innerHTML).join("\n");

const f = (over) => Object.assign({
  id: "x", ruleId: "t", name: "item", path: "C:\\x\\y",
  isDir: false, logicalSize: 1024, onDiskSize: 1024,
  lastAccessUnix: Date.now() / 1000 - 86400 * 40, createdUnix: 0, ageDays: 40,
  ageBasis: "lastAccess", category: "dev_artifact", risk: "rebuildable",
  disposition: "quarantine", childCount: 0,
}, over);

const dev = f({ id: "dev", name: "node_modules", path: "C:\\code\\app\\node_modules" });
const temp = f({ id: "tmp", name: "Temp", path: "C:\\Temp\\x", category: "temp_junk", risk: "safe", disposition: "purge" });
const cloud = f({ id: "od", name: "Pictures", path: "C:\\Users\\me\\OneDrive\\Pictures", category: "cloud_placeholder", risk: "cloud", disposition: "dehydrate" });

// ---------- a non-subscriber is not offered the feature ----------

S.entitlement = { subscriber: false, source: "store", checkedUnix: 0, expiresUnix: null, detail: "" };
S.findings = [dev];
S.selected = new Set();
S.tab = "findings";
S.marks = new Map();
S.importanceFilter = "all";
S.riskFilter = "all";
S.batchDisposition = "default";
await ui.render();
check("a non-subscriber gets no cloud-backup option", !doc().includes('value="cloud_backup"'), doc().slice(0, 200));
check("a non-subscriber gets no batch backup option", !doc().includes("全部备份到云端"));

// ---------- a subscriber is ----------

S.entitlement = { subscriber: true, source: "store", checkedUnix: 0, expiresUnix: null, detail: "" };
S.selected = new Set(["dev"]);
await ui.render();
check("a subscriber gets the cloud-backup option", doc().includes('value="cloud_backup"'));
// The batch dropdown lives in the action bar, which only exists once something
// is selected - hence the selection above.
check("a subscriber gets the batch backup option", doc().includes("全部备份到云端"));
S.selected = new Set();

// ---------- but never for things a backup is meaningless for ----------

S.findings = [temp, cloud];
await ui.render();
check("temp junk is not offered a cloud backup",
  !doc().includes('value="cloud_backup"'), "temp junk was offered an upload");

check("canCloudBackup agrees about temp junk", st.canCloudBackup(temp) === false);
check("canCloudBackup agrees about OneDrive content", st.canCloudBackup(cloud) === false);
check("canCloudBackup agrees about dev artifacts", st.canCloudBackup(dev) === true);

// ---------- the batch choice ----------

S.findings = [dev, temp];
S.batchDisposition = "backup";
check("a backup batch choice reaches an eligible row",
  st.effectiveDisposition(dev) === "cloud_backup");
check("a backup batch choice does not reach temp junk",
  st.effectiveDisposition(temp) === "purge",
  "got " + st.effectiveDisposition(temp));

// Losing the subscription mid-session must not leave the batch on a choice the
// user can no longer make.
S.entitlement = { subscriber: false, source: "store", checkedUnix: 0, expiresUnix: null, detail: "" };
check("losing the subscription drops the backup batch choice",
  st.effectiveDisposition(dev) === "quarantine",
  "got " + st.effectiveDisposition(dev));
S.entitlement = { subscriber: true, source: "store", checkedUnix: 0, expiresUnix: null, detail: "" };
S.batchDisposition = "default";

// ---------- the preview ----------

S.findings = [dev];
S.preview = {
  groups: [{ disposition: "cloud_backup", itemCount: 1, logicalBytes: 1024, onDiskBytes: 1024, categoryCounts: { dev_artifact: 1 }, containsRedPurge: false }],
  items: [{ finding: dev, disposition: "cloud_backup" }],
  needsRed: false, confirmed: false, busy: false,
};
await ui.render();
let html = doc();
check("the preview names the cloud-backup group", html.includes("备份到 OneDrive 后删除本地"));
check("the preview explains upload-then-delete", html.includes("确认成功后才删除本地"));
check("the preview does not claim to be irreversible", !html.includes("以下操作不可撤销"));
// Scoped to the dialog: the findings table is still rendered behind the
// overlay and its disposition dropdown legitimately contains the word.
const modal = store["r-modal"].innerHTML;
check("the preview does not offer a plain delete", !modal.includes("永久删除"), modal.slice(0, 200));
check("the confirm button offers backup and clean", html.includes("备份并清理"));

// ---------- completing it ----------

cleanupReport = {
  purged: [], quarantined: [], blocked: [], failed: [],
  dehydrated: [], dehydrateRequestedBytes: 0, dehydratePinnedFiles: 0,
  backedUp: ["C:\\code\\app\\node_modules"], backupBytes: 2048, backupFallbacks: [],
  needsElevation: false,
};
await st.actions.doCleanup();
check("a backed-up item leaves the list", !S.findings.some(x => x.id === "dev"));
check("the toast reports the backup", S.toast && S.toast.msg.includes("已备份"), S.toast && S.toast.msg);
check("the toast does not claim anything was deleted",
  S.toast && S.toast.msg.includes("未删除任何内容"), S.toast && S.toast.msg);

// ---------- a fallback is reported as a fallback ----------

S.findings = [f({ id: "dev2", name: "target", path: "C:\\code\\target" })];
S.selected = new Set();
S.preview = {
  groups: [{ disposition: "cloud_backup", itemCount: 1, logicalBytes: 1024, onDiskBytes: 1024, categoryCounts: { dev_artifact: 1 }, containsRedPurge: false }],
  items: [{ finding: S.findings[0], disposition: "cloud_backup" }],
  needsRed: false, confirmed: false, busy: false,
};
cleanupReport = {
  purged: [], quarantined: ["C:\\code\\target"], blocked: [], failed: [],
  dehydrated: [], dehydrateRequestedBytes: 0, dehydratePinnedFiles: 0,
  backedUp: [], backupBytes: 0, backupFallbacks: ["C:\\code\\target"],
  needsElevation: false,
};
await st.actions.doCleanup();
html = S.toast && S.toast.msg;
check("a failed upload is reported as a fallback", html && html.includes("改为本地隔离"), html);
check("a fallback is not dressed up as a backup", html && !html.includes("已备份"), html);

console.log(`\n${pass} passed, ${fail} failed`);
if (fail) process.exitCode = 1;