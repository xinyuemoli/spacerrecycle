/**
 * Cloud-restore UI tests.
 *
 * Restoring writes files back onto the user's disk, from a manifest that came
 * back over the network. The failures worth pinning down are the ones where
 * the dialog would mislead: offering restore for a batch whose manifest is
 * missing (nothing to restore from), and reporting a partial restore as if it
 * had fully succeeded.
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

const calls = [];
let backups = [];
let restoreResult = null;
let listThrows = false;

globalThis.window = {
  __TAURI__: {
    core: {
      invoke: async (cmd, args) => {
        calls.push({ cmd, args });
        if (cmd === "list_cloud_backups") {
          if (listThrows) throw new Error("no OneDrive account is linked");
          return backups;
        }
        if (cmd === "restore_cloud_backup") return restoreResult;
        if (cmd === "delete_cloud_backup") return null;
        if (cmd === "list_quarantine_batches") return [];
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

const manifest = {
  version: 1,
  batchId: "batch-1",
  createdUnix: 1700000000,
  items: [{
    originalPath: "C:\\code\\app\\node_modules",
    name: "node_modules",
    isDir: true,
    bytes: 2048,
    files: [{ relative: "a/b.js", size: 1024 }, { relative: "c.js", size: 1024 }],
    uploadedUnix: 1700000000,
  }],
  totalBytes: 2048,
  totalFiles: 2,
};

S.tab = "quarantine";
S.batches = [];
S.quarantineLoaded = true;

// ---------- nothing to show ----------

backups = [];
await st.actions.loadCloudBackups();
await ui.render();
check("no batches means no section", !doc().includes("云端备份（OneDrive）"));

// An unlinked account must look the same as having no backups, not like a
// failure the user has to understand.
listThrows = true;
await st.actions.loadCloudBackups();
await ui.render();
check("an unlinked account shows no section either", !doc().includes("云端备份（OneDrive）"));
listThrows = false;

// ---------- a batch with a manifest ----------

backups = [{ batchId: "batch-1", manifest, error: null }];
await st.actions.loadCloudBackups();
await ui.render();
let html = doc();
check("the batch is listed", html.includes("batch-1"), html.slice(0, 200));
check("the batch shows what it holds", html.includes("2 个文件"));
check("the batch shows the recorded size", html.includes("2.00 KB"));
check("the original location is shown", html.includes("C:\\code\\app\\node_modules"),
  "the user must be able to see where a restore would write");
check("restore is offered in both conflict modes",
  html.includes('data-policy="skip"') && html.includes('data-policy="overwrite"'));
check("deleting the cloud batch is offered", html.includes('data-act="cb-delete"'));

// ---------- a batch whose manifest is missing ----------

backups = [{ batchId: "batch-broken", manifest: null, error: "manifest is unreadable: bad json" }];
await st.actions.loadCloudBackups();
await ui.render();
html = doc();
check("an unreadable batch is still listed", html.includes("batch-broken"),
  "its files are in the user's drive; hiding it would be worse");
check("an unreadable batch says so", html.includes("清单缺失"));
check("an unreadable batch reports the reason", html.includes("bad json"));
check("an unreadable batch offers no restore",
  !html.includes('data-act="cb-restore"'), "restore was offered with nothing to restore from");
check("an unreadable batch can still be deleted", html.includes('data-act="cb-delete"'));

// ---------- restoring ----------

backups = [{ batchId: "batch-1", manifest, error: null }];
await st.actions.loadCloudBackups();
restoreResult = { batchId: "batch-1", restoredFiles: 2, restoredBytes: 2048, skipped: [], failed: [] };
await st.actions.restoreCloudBatch("batch-1", "overwrite");
const restoreCall = calls.filter(c => c.cmd === "restore_cloud_backup").pop();
check("restoring calls the backend", !!restoreCall);
check("restoring passes the batch and policy",
  restoreCall.args.batchId === "batch-1" && restoreCall.args.policy === "overwrite");
check("restoring reports what came back", S.toast && S.toast.msg.includes("已恢复 2 个文件"), S.toast && S.toast.msg);

// ---------- a partial restore is not dressed up as a full one ----------

restoreResult = {
  batchId: "batch-1", restoredFiles: 1, restoredBytes: 1024,
  skipped: ["C:\\code\\app\\node_modules\\c.js"],
  failed: [{ path: "C:\\code\\app\\node_modules\\a\\b.js", reason: "network" }],
};
await st.actions.restoreCloudBatch("batch-1", "skip");
html = S.toast && S.toast.msg;
check("a partial restore reports the skips", html && html.includes("跳过 1"), html);
check("a partial restore reports the failures", html && html.includes("1 个文件失败"), html);
check("a partial restore is shown as an error, not a success",
  S.toast && S.toast.kind === "err", S.toast && S.toast.kind);

// ---------- deleting ----------

const before = calls.length;
await st.actions.deleteCloudBatch("batch-1");
const delCall = calls.filter(c => c.cmd === "delete_cloud_backup").pop();
check("deleting calls the backend", !!delCall && delCall.args.batchId === "batch-1");
check("deleting reloads the list", calls.length > before + 1, "the deleted batch would linger on screen");
check("deleting reports success", S.toast && S.toast.msg.includes("已删除"), S.toast && S.toast.msg);

console.log(`\n${pass} passed, ${fail} failed`);
if (fail) process.exitCode = 1;