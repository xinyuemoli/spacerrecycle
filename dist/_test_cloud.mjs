/**
 * OneDrive / cloud-placeholder regression tests.
 *
 * Releasing a local copy is the one action in this product that destroys
 * nothing, and it is also the one whose failure mode is silent: the file looks
 * gone from the list while still sitting in Explorer. These tests pin down the
 * three things that make it safe to offer:
 *
 *   - a cloud finding has exactly one disposition and no way to reach purge or
 *     quarantine through the UI;
 *   - the preview says "release", because calling it a quarantine would train
 *     the user to read the dialog as "something is being deleted";
 *   - a completed release is reported as a background request, not as space
 *     already freed.
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
const calls = [];
globalThis.window = {
  __TAURI__: {
    core: {
      invoke: async (cmd, args) => {
        calls.push({ cmd, args });
        if (cmd === "run_cleanup") return cleanupReport;
        if (cmd === "onedrive_status") return { available: true, roots: ["C:\\Users\\me\\OneDrive"] };
        if (cmd === "list_importance") return [];
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

const cloudFinding = {
  id: "c1", ruleId: "onedrive_local_copies", name: "Pictures",
  path: "C:\\Users\\me\\OneDrive\\Pictures",
  isDir: true, logicalSize: 8e9, onDiskSize: 8e9,
  lastAccessUnix: Date.now() / 1000 - 86400 * 5, createdUnix: 0, ageDays: 5,
  ageBasis: "lastAccess", category: "cloud_placeholder", risk: "cloud",
  disposition: "dehydrate", childCount: 1200,
};

S.findings = [cloudFinding];
S.selected = new Set();
S.tab = "findings";
S.marks = new Map();
S.importanceFilter = "all";
S.riskFilter = "all";
await ui.render();

// ---------- the row offers exactly one action ----------

check("a cloud row offers the release action",
  doc().includes("释放本地副本"), doc().slice(0, 200));
check("a cloud row offers no permanent delete",
  !doc().includes(">永久删除<"), "purge leaked into a cloud row");
check("a cloud row offers no quarantine option",
  !doc().includes('value="quarantine"'), "quarantine leaked into a cloud row");

check("the backend disposition for a cloud finding is dehydrate",
  st.effectiveDisposition(cloudFinding) === "dehydrate");

// Even a batch choice that means "purge everything" must not reach a cloud
// item: the backend refuses it, but the UI must not offer it in the first
// place, because the confirm step would then produce only blocked rows.
S.batchDisposition = "purge";
check("a purge-everything batch choice does not reach cloud content",
  st.effectiveDisposition(cloudFinding) === "dehydrate");
S.batchDisposition = "quarantine";
check("a quarantine-everything batch choice does not reach cloud content",
  st.effectiveDisposition(cloudFinding) === "dehydrate");
S.batchDisposition = "default";

// It is not a Critical mark: the row stays selectable, because releasing a
// local copy is something the user may legitimately want to do in bulk.
check("a cloud row is selectable", st.selectable(cloudFinding) === true);
st.actions.toggleSel("c1");
check("a cloud row can be ticked", S.selected.has("c1"));
S.selected.clear();

// ---------- the OneDrive card ----------

S.onedrive = { available: true, roots: ["C:\\Users\\me\\OneDrive"] };
S.tab = "overview";
S.volumes = [];
await ui.render();
check("the OneDrive card appears when a root exists", doc().includes("OneDrive"));
check("the card names the sync root", doc().includes("C:\\Users\\me\\OneDrive"));
check("the card offers a scan", doc().includes('data-cats="cloud_placeholder"'));

S.onedrive = { available: false, roots: [] };
await ui.render();
check("the card is hidden without OneDrive", !doc().includes("扫描 OneDrive 本地占用"));

// ---------- the preview ----------

S.onedrive = { available: true, roots: ["C:\\Users\\me\\OneDrive"] };
S.tab = "findings";
S.preview = {
  groups: [{ disposition: "dehydrate", itemCount: 1, logicalBytes: 8e9, onDiskBytes: 8e9, categoryCounts: { cloud_placeholder: 1 }, containsRedPurge: false }],
  items: [{ finding: cloudFinding, disposition: "dehydrate" }],
  needsRed: false, confirmed: false, busy: false,
};
await ui.render();
let html = doc();
check("the preview names the release group", html.includes("释放本地副本"));
check("the preview never calls it a deletion",
  !html.includes("以下操作不可撤销"), "a non-destructive preview claimed to be irreversible");
check("the confirm button offers release", html.includes("释放本地副本") && !html.includes("移入隔离区"));

// ---------- completing the release ----------

cleanupReport = {
  purged: [], quarantined: [], blocked: [], failed: [],
  dehydrated: ["C:\\Users\\me\\OneDrive\\Pictures"],
  dehydrateRequestedBytes: 8e9, dehydratePinnedFiles: 0,
  needsElevation: false,
};
await st.actions.doCleanup();
check("the released folder leaves the list",
  !S.findings.some(f => f.id === "c1"),
  "a stale row would keep claiming space it no longer holds");
check("the release was sent to the backend",
  calls.some(c => c.cmd === "run_cleanup" && c.args.request.items[0].disposition === "dehydrate"));
check("the toast says release, not delete",
  S.toast && S.toast.msg.includes("释放"), S.toast && S.toast.msg);
check("the toast does not claim anything was deleted",
  S.toast && S.toast.msg.includes("未删除任何内容"), S.toast && S.toast.msg);

console.log(`\n${pass} passed, ${fail} failed`);
if (fail) process.exitCode = 1;