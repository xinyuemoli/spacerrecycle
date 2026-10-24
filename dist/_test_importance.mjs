/**
 * Importance-mark regression tests.
 *
 * Importance is an annotation the user sets by hand, and it is the only place
 * where a user statement outranks every scanner rule. The failures that matter
 * are therefore not "does it look right" but:
 *
 *   - a mark that does not survive the path spelling the frontend happens to
 *     use (forward slashes, different case) is a mark that silently stops
 *     working after a re-scan;
 *   - a row marked 重要 whose irreversible option is still offered produces a
 *     confirm dialog whose only outcome is a "blocked" line, so the option is
 *     clamped up front while the backend refuses it again on its own.
 *
 * There are two levels and this asserts against the two, because the four the
 * backend used to have were never offered anywhere a user could pick them. The
 * names here are the ones the stream carries, so a rename on one side and not
 * the other fails here rather than in the field.
 *
 * Like the other headless harnesses this stubs the DOM; the click paths that
 * need a real browser live in _test_dom.mjs.
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

// A mock backend, installed before the modules load: api.js reads
// window.__TAURI__ once at import time to decide whether a backend exists.
const calls = [];
let serverMarks = [];
globalThis.window = {
  __TAURI__: {
    core: {
      invoke: async (cmd, args) => {
        calls.push({ cmd, args });
        if (cmd === "list_importance") return serverMarks;
        if (cmd === "get_volumes") return [];
        if (cmd === "get_config") return null;
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
  isDir: false, logicalSize: 1024 ** 3, onDiskSize: 512 * 1024 ** 2,
  lastAccessUnix: Date.now() / 1000 - 86400 * 40, createdUnix: 0, ageDays: 40,
  ageBasis: "lastAccess", category: "dev_artifact", risk: "rebuildable",
  disposition: "quarantine", childCount: 0,
}, over);

// ---------- path normalization ----------

check("normPath folds forward slashes",
  st.normPath("C:/Temp/Logs/app.log") === "c:\\temp\\logs\\app.log");
check("normPath folds case",
  st.normPath("C:\\TEMP\\LOGS\\APP.LOG") === st.normPath("C:/Temp/Logs/app.log"));
check("normPath drops a trailing separator",
  st.normPath("C:\\Temp\\Logs\\") === "c:\\temp\\logs");

// ---------- badges and filter ----------

S.findings = [
  f({ id: "crit", name: "thesis.docx", path: "C:\\Users\\me\\thesis.docx", category: "large_file", risk: "personal" }),
  f({ id: "plain", name: "node_modules", path: "C:\\code\\app\\node_modules" }),
  f({ id: "low", name: "old.log", path: "C:\\Temp\\old.log", category: "temp_junk", risk: "safe", disposition: "purge" }),
];
S.marks = new Map([
  [st.normPath("C:\\Users\\me\\thesis.docx"), "important"],
  [st.normPath("C:\\Temp\\old.log"), "disposable"],
]);
S.selected = new Set();
S.importanceFilter = "all";
S.tab = "findings";
await ui.render();
let html = doc();

check("a marked row renders its badge", html.includes("thesis.docx") && html.includes("重要"));
// The unmarked row's name cell must end straight into the name: if a badge had
// leaked onto it, the cell would read "...</span>node_modules</td>" instead.
check("an unmarked row renders no badge", html.includes(">node_modules</td>"),
  "an unmarked row picked up a badge");
check("a disposable mark renders too", html.includes("可清理"), html.slice(0, 120));

S.importanceFilter = "important";
await ui.render();
html = doc();
check("filtering by important hides other rows", html.includes("thesis.docx") && !html.includes("node_modules"),
  "filter did not narrow the table");

S.importanceFilter = "all";

// ---------- every row is selectable ----------
//
// This is what the level collapse changed. A `critical` mark used to mean
// "leave this alone" and the backend refused every disposition for it, so the
// checkbox was withheld. 重要 refuses only the irreversible action and allows
// quarantine and backup - which is what the right-click menu promises - so the
// row stays selectable and the irreversible choice is clamped instead.

check("a marked row is selectable", st.selectable(S.findings[0]) === true);
check("an unmarked row is selectable", st.selectable(S.findings[1]) === true);

st.actions.toggleSel("crit");
check("toggleSel accepts a marked row", S.selected.has("crit"), "got " + JSON.stringify([...S.selected]));

st.actions.selAll(true);
check("select-all includes marked rows",
  S.selected.has("crit") && S.selected.has("plain") && S.selected.has("low"),
  JSON.stringify([...S.selected]));

st.actions.selAll(false);
S.importanceFilter = "all";

// ---------- the irreversible option is clamped ----------

const highFinding = f({ id: "high", name: "contract.pdf", path: "C:\\Users\\me\\contract.pdf", category: "large_file", risk: "personal" });
S.findings = [highFinding];
S.marks = new Map([[st.normPath(highFinding.path), "important"]]);
check("an important mark clamps purge to quarantine",
  st.effectiveDisposition(highFinding) === "quarantine",
  "got " + st.effectiveDisposition(highFinding));

const tempHigh = f({ id: "th", name: "x.tmp", path: "C:\\Temp\\x.tmp", category: "temp_junk", risk: "safe", disposition: "purge" });
S.findings = [tempHigh];
S.marks = new Map([[st.normPath(tempHigh.path), "important"]]);
check("an important mark clamps a temp file too",
  st.effectiveDisposition(tempHigh) === "quarantine",
  "got " + st.effectiveDisposition(tempHigh));

S.marks = new Map();
check("an unmarked category default is untouched",
  st.effectiveDisposition(tempHigh) === "purge",
  "got " + st.effectiveDisposition(tempHigh));

// ---------- the menu ----------

S.findings = [f({ id: "crit", name: "thesis.docx", path: "C:\\Users\\me\\thesis.docx", category: "large_file", risk: "personal" })];
S.marks = new Map([[st.normPath("C:\\Users\\me\\thesis.docx"), "important"]]);
st.actions.openMenu("crit", 40, 60);
await ui.render();
html = doc();
check("the menu opens", S.menu !== null && html.includes("ctxmenu"));
check("the menu offers every level",
  ["重要", "可清理"].every(label => html.includes(label)),
  html.slice(0, 200));
check("the menu offers exactly two levels",
  !html.includes("关键") && !html.includes("普通"),
  "a level from the old four-level model is still offered");
check("the menu offers to clear an existing mark", html.includes("清除标记"));
check("the menu is anchored where the pointer was",
  html.includes("left:40px") && html.includes("top:60px"));

// ---------- the menu is never gated ----------
//
// The subscription tier was dropped before it shipped, so marking is always
// available. Pinned because a gated menu that names a subscription is exactly
// what this file used to assert, and re-introducing it silently would put every
// user back behind a purchase that cannot be made.

const openTier = S.entitlement;
S.entitlement = {
  subscriber: false, source: "not_configured",
  cloudAllowed: true, importanceAllowed: true, guardAllowed: true,
};
st.actions.openMenu("crit", 40, 60);
await ui.render();
html = doc();
check("the menu offers the levels even when nothing is paid",
  html.includes('data-act="set-imp"') && html.includes("重要"),
  html.slice(0, 240));
check("it does not name a subscription as the reason",
  !html.includes("需要有效订阅"));
S.entitlement = openTier;
st.actions.openMenu("crit", 40, 60);
await ui.render();

// ---------- saving ----------

await st.actions.setImportance("C:\\Users\\me\\thesis.docx", "important");
check("saving calls the backend with the level",
  calls.some(c => c.cmd === "set_importance" && c.args.level === "important" && c.args.path === "C:\\Users\\me\\thesis.docx"));
check("saving updates the local map", st.importanceOf(S.findings[0]) === "important");
check("saving closes the menu", S.menu === null);

await st.actions.clearImportance("C:\\Users\\me\\thesis.docx");
check("clearing calls the backend", calls.some(c => c.cmd === "clear_importance"));
check("clearing drops the local mark", st.importanceOf(S.findings[0]) === null);

// ---------- loading ----------

serverMarks = [{ path: "C:\\Users\\me\\thesis.docx", level: "important", markedUnix: 1 }];
await st.actions.loadImportance();
check("loading keys marks by normalized path",
  st.importanceOf(S.findings[0]) === "important",
  "map=" + JSON.stringify([...S.marks]));

console.log(`\n${pass} passed, ${fail} failed`);
if (fail) process.exitCode = 1;