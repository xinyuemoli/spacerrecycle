// Headless harness for the delete-guard card on the settings tab.
//
// The card is the only control on that tab whose value is not stored in this
// app's own config - it belongs to the resident service's policy file - so the
// things worth pinning down are that the choice round-trips, and that the
// screen tells the truth about whether anything is actually being protected.
// With no service attached the kernel filter fails open, which looks exactly
// like "there was nothing to protect" from the outside.

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

function doc() {
  return store["app"].innerHTML + store["app"].children.map(c => c.innerHTML).join("\n");
}

const ui = await import("./ui.js");
const st = await import("./state.js");
const api = await import("./api.js");
const i18n = await import("./i18n.js");
const S = st.getState();

function check(name, cond, extra) {
  console.log((cond ? "PASS " : "FAIL ") + name + (extra ? " :: " + extra : ""));
  if (!cond) process.exitCode = 1;
}

// ---- The card renders, and offers all three dispositions ----

S.config = {
  largeFileAgeDays: 30, leftoverAgeDays: 30, largeFileMinBytes: 524288000,
  quarantineExpiryDays: 7, quarantineDir: "C:\\ProgramData\\SpaceRecycle\\Quarantine",
};
S.auditLog = [];
S.auditPath = "C:\\x\\audit.log";
S.tab = "settings";
S.guard = await api.invoke("guard_status");
await ui.render();
let html = doc();

check("guard card renders", html.includes('name="guard-dispo"'));
check("all three dispositions offered",
  html.includes('value="ask"') && html.includes('value="deny"') && html.includes('value="allow"'));
check("the current disposition is the checked one",
  html.includes('value="ask" data-act="guard" checked'));
check("no other disposition is checked",
  !html.includes('value="deny" data-act="guard" checked') &&
  !html.includes('value="allow" data-act="guard" checked'));

// ---- The status is shown, not assumed ----

check("an uninstalled service is called out",
  html.includes("\u5b88\u62a4\u670d\u52a1\u672a\u5b89\u88c5"), "must not look like a working guard");
check("the uninstalled warning says nothing is protected",
  html.includes("\u6ca1\u6709\u4efb\u4f55\u4fdd\u62a4"));

S.guard = { serviceInstalled: true, serviceRunning: true, policyPath: "C:\\p.ini", disposition: "ask", configured: true };
await ui.render();
html = doc();
check("a running service is reported as running",
  html.includes("\u5b88\u62a4\u670d\u52a1\u8fd0\u884c\u4e2d"));
check("no warning while it is running",
  !html.includes("\u6ca1\u6709\u4efb\u4f55\u4fdd\u62a4"));

S.guard = { serviceInstalled: true, serviceRunning: false, policyPath: "C:\\p.ini", disposition: "ask", configured: true };
await ui.render();
html = doc();
check("installed but stopped is called out as unprotected",
  html.includes("\u5df2\u5b89\u88c5\u4f46\u672a\u8fd0\u884c") && html.includes("\u6ca1\u6709\u4efb\u4f55\u4fdd\u62a4"));

// ---- Changing the disposition ----

S.guard = { serviceInstalled: true, serviceRunning: true, policyPath: "C:\\p.ini", disposition: "ask", configured: true };
await st.actions.setGuardDisposition("deny");
check("the new disposition is what the backend now reports",
  S.guard.disposition === "deny", "got " + S.guard.disposition);
html = doc();
check("the card re-renders with the new choice checked",
  html.includes('value="deny" data-act="guard" checked'));
check("the old choice is no longer checked",
  !html.includes('value="ask" data-act="guard" checked'));

await st.actions.setGuardDisposition("allow");
check("it can be changed again", S.guard.disposition === "allow", "got " + S.guard.disposition);

// ---- A failed save must not leave the screen claiming something untrue ----

S.guard = { serviceInstalled: true, serviceRunning: true, policyPath: "C:\\p.ini", disposition: "deny", configured: true };
await ui.render();
await st.actions.setGuardDisposition("bogus");
check("a rejected save keeps the previous disposition",
  S.guard && S.guard.disposition === "deny", "got " + (S.guard && S.guard.disposition));
html = doc();
check("and the screen still shows the previous choice",
  html.includes('value="deny" data-act="guard" checked'));
check("and it did not silently claim success",
  !html.includes('value="bogus"'));

// ---- The card still works when the status could not be read ----

S.guard = null;
await ui.render();
html = doc();
check("the card renders its choices even without a status",
  html.includes('name="guard-dispo"') && html.includes('value="deny"'));
check("but claims no health it cannot verify",
  !html.includes("\u5b88\u62a4\u670d\u52a1\u8fd0\u884c\u4e2d") && !html.includes("\u5df2\u5b89\u88c5\u4f46\u672a\u8fd0\u884c"));

// ---- Both languages have the strings ----

for (const key of ["set.guard", "set.guardHint", "set.guard.ask", "set.guard.askHint",
                   "set.guard.deny", "set.guard.denyHint", "set.guard.allow", "set.guard.allowHint",
                   "set.guard.active", "set.guard.installedNotRunning", "set.guard.notInstalled",
                   "set.guard.saved", "set.guard.default"]) {
  const zh = i18n.t("zh", key);
  const en = i18n.t("en", key);
  check("string " + key, zh !== key && en !== key && zh !== en,
    "zh=" + zh + " en=" + en);
}

console.log("\nDONE");
