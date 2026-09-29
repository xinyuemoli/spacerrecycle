// Regression harness for the two scan/tab bugs.
//
// 1. Switching tabs during a scan froze the UI: every progress event caused a
//    full innerHTML rewrite plus a fresh set of event listeners on the root.
// 2. Switching away and back mid-scan showed the "start scan" button again,
//    because findings had been cleared but scanning was still true.
//
// These assert the observable behaviour, not the internals.

let renderCount = 0;
let bindCount = 0;
const listeners = [];
// Models containment so region updates land where a browser would put them.
const mkEl = () => {
  const node = {
    innerHTML: "",
    children: [],
    dataset: {},
    classList: { contains: () => false, add() {}, remove() {} },
    addEventListener: (t, f) => { listeners.push([t, f]); bindCount++; },
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

// The progress readout is patched in place rather than re-rendered, so the
// stub needs real child nodes for pg-dirs / pg-bytes / pg-found. A plain
// innerHTML string cannot express that, which would make the test unable to
// see the very thing it is checking.
function ensureTextNode(id) {
  return (store[id] ||= { textContent: "", __text: true });
}

// The progress readout lives inside the progress region's HTML, so a lookup
// has to consider the region's contents, not just registered nodes.
function findInRegions(id) {
  for (const key of Object.keys(store)) {
    if (!key.startsWith("r-")) continue;
    if ((store[key].innerHTML || "").includes('id="' + id + '"')) return true;
  }
  return false;
}
globalThis.window = { __TAURI__: undefined };
// Compose the document from the region nodes, the way a browser would.
function doc() {
  return store["app"].innerHTML + store["app"].children.map(c => c.innerHTML).join("\n");
}
globalThis.document = {
  getElementById: (id) => {
    if (id.startsWith("pg-")) return ensureTextNode(id);
    if (id === "progress-live") return findInRegions(id) ? {} : null;
    return store[id] || null;
  },
  querySelector: (sel) => (sel === "main" ? { scrollTop: 0 } : null),
};
globalThis.confirm = () => true;

const ui = await import("./ui.js");
const st = await import("./state.js");
const S = st.getState();

let pass = 0, fail = 0;
function check(name, cond, extra) {
  if (cond) { pass++; console.log("PASS " + name); }
  else { fail++; console.log("FAIL " + name + (extra ? " :: " + extra : "")); process.exitCode = 1; }
}

// ---- Bug 2: mid-scan the findings tab must not offer to start a new scan ----
S.tab = "findings";
S.scanning = true;
S.findings = [];
S.progress = { scanner: "all", phase: "scanning", dirsScanned: 120, bytesScanned: 5e9, findings: 7 };
await ui.render();
let html = doc();

check("mid-scan, findings tab is not empty", !html.includes("\u8fd8\u6ca1\u6709\u626b\u63cf\u7ed3\u679c"),
  "an in-progress scan must not be rendered as 'nothing scanned yet'");
check("mid-scan, the scan-in-progress panel is shown",
  html.includes("\u6b63\u5728\u626b\u63cf") || html.includes("\u53d6\u6d88\u626b\u63cf"),
  "the user needs to see progress and be able to cancel");
check("mid-scan, no start-scan button is offered",
  !html.includes('data-act="scan"'),
  "starting a second scan while one runs is impossible and misleading");

// ---- Bug 1: progress updates must not thrash the DOM ----
// Simulate a burst of progress events and count the resulting full renders.
listeners.length = 0;
bindCount = 0;
const before = bindCount;
S.tab = "findings";
for (let i = 0; i < 50; i++) {
  st.actions.onProgress({ scanner: "all", phase: "scanning", dirsScanned: i, bytesScanned: i * 1e8, findings: i });
}
check("a burst of progress events does not re-render the whole tree",
  bindCount === before,
  "re-rendering on every progress event is what froze the tab switching");

// Throttling must not freeze the display outright: the throttled paint has
// to land shortly after the burst stops.
await new Promise(r => setTimeout(r, 250));
check("progress patches the directory count in place",
  store["pg-dirs"]?.textContent === "49",
  "got " + JSON.stringify(store["pg-dirs"]?.textContent));
check("progress patches the byte count in place",
  typeof store["pg-bytes"]?.textContent === "string" && store["pg-bytes"].textContent.includes("GB"),
  "got " + JSON.stringify(store["pg-bytes"]?.textContent));
check("progress patches the finding count in place",
  store["pg-found"]?.textContent === "49",
  "got " + JSON.stringify(store["pg-found"]?.textContent));

// Repeated tab switches must not accumulate handlers.
listeners.length = 0;
S.tab = "overview";
await ui.render();
S.tab = "findings";
await ui.render();
S.tab = "settings";
await ui.render();
S.tab = "findings";
await ui.render();
check("listeners are not re-attached on every render",
  listeners.length <= 3,
  "got " + listeners.length + " listeners after 4 renders; each render used to add 3 more");

console.log("\n" + pass + " passed, " + fail + " failed");
