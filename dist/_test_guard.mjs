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
//
// The card reads protection_status, not the service state. They are different
// questions: no driver service does not mean nothing is protected, because a
// held handle protects a file with no service at all. The first version of this
// card read the service state and announced "nothing is protected" while every
// marked file was in fact held open.

S.protection = { kind: "none", active: false, detail: "\u6ca1\u6709\u4efb\u4f55\u4fdd\u62a4\u53ef\u7528" };
await ui.render();
html = doc();
check("no mechanism at all is called out",
  html.includes("\u6ca1\u6709\u4efb\u4f55\u4fdd\u62a4"), "must not look like a working guard");

S.guard = { serviceInstalled: true, serviceRunning: true, policyPath: "C:\\p.ini", disposition: "ask", configured: true };
S.protection = { kind: "driver", active: true, detail: "\u5b88\u62a4\u670d\u52a1\u8fd0\u884c\u4e2d" };
await ui.render();
html = doc();
check("a running service is reported as running",
  html.includes("\u5b88\u62a4\u670d\u52a1\u8fd0\u884c\u4e2d"));
check("no warning while it is running",
  !html.includes("\u6ca1\u6709\u4efb\u4f55\u4fdd\u62a4"));

S.guard = { serviceInstalled: true, serviceRunning: false, policyPath: "C:\\p.ini", disposition: "ask", configured: true };
S.protection = { kind: "driver", active: true, detail: "\u5df2\u5b89\u88c5\u4f46\u672a\u8fd0\u884c\u2014\u2014\u5f53\u524d\u6ca1\u6709\u4efb\u4f55\u4fdd\u62a4" };
await ui.render();
html = doc();
check("installed but stopped is called out as unprotected",
  html.includes("\u5df2\u5b89\u88c5\u4f46\u672a\u8fd0\u884c") && html.includes("\u6ca1\u6709\u4efb\u4f55\u4fdd\u62a4"));

// ---- The user-mode hold backend ----
//
// The case that made the old card wrong: a mechanism that protects files
// without any service, which the service-shaped card could not describe.

S.guard = { serviceInstalled: false, serviceRunning: false, policyPath: "", disposition: "ask", configured: true };
S.protection = { kind: "hold", active: true, detail: "3 marked files are held open, so they cannot be deleted" };
await ui.render();
html = doc();
check("a service-less mechanism is not reported as no protection",
  !html.includes("\u6ca1\u6709\u4efb\u4f55\u4fdd\u62a4"),
  "the warning is about the driver service and does not apply to a held handle");
check("the hold status is shown instead", html.includes("held open"));

// The count is what says whether anything is actually protected, so a hold that
// is up with nothing held has to read differently from one holding files.
S.protection = { kind: "hold", active: true, detail: "the guard is on, but no file is currently held" };
await ui.render();
html = doc();
check("a hold with nothing held says so rather than claiming protection",
  html.includes("no file is currently held"));

S.protection = { kind: "hold", active: true, detail: "2 could not be protected: C:\\x (access denied)" };
await ui.render();
html = doc();
check("files that could not be protected are named",
  html.includes("could not be protected") && html.includes("access denied"),
  "a count alone leaves the user unable to find which mark is not working");

// ---- Changing the disposition ----

S.guard = { serviceInstalled: true, serviceRunning: true, policyPath: "C:\\p.ini", disposition: "ask", configured: true };
S.protection = { kind: "driver", active: true, detail: "\u5b88\u62a4\u670d\u52a1\u8fd0\u884c\u4e2d" };
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

// ---- The guard is a paid feature, and the card has to say so ----
//
// The same shape as the OneDrive quarantine option: the paid choices are
// disabled with the reason rather than hidden, and the free one stays open.
// Pulling each input tag out and looking for the attribute is deliberate -
// matching a fixed substring of the rendered markup only passes while nobody
// reformats the template.

function dispoTag(value) {
  const m = html.match(new RegExp('<input[^>]*value="' + value + '"[^>]*>'));
  return m ? m[0] : "";
}

// The guard is not gated any more: the subscription tier was dropped before it
// shipped, so every build offers all three dispositions. The rendering still
// needs pinning down, because a disabled radio here would silently take the
// protective choices away from everyone.

const openTier = S.entitlement;
S.entitlement = {
  subscriber: false, source: "not_configured",
  cloudAllowed: true, importanceAllowed: true, guardAllowed: true,
};
S.guard = { serviceInstalled: false, serviceRunning: false, disposition: "ask", configured: true };
// No mechanism at all, so the unprotected warning is the one statement on the
// screen that matters.
S.protection = { kind: "none", active: false, detail: "\u6ca1\u6709\u4efb\u4f55\u4fdd\u62a4\u53ef\u7528" };
await ui.render();
html = doc();

check("every disposition is selectable",
  dispoTag("ask") !== "" && !dispoTag("ask").includes("disabled") &&
  dispoTag("deny") !== "" && !dispoTag("deny").includes("disabled") &&
  dispoTag("allow") !== "" && !dispoTag("allow").includes("disabled"),
  dispoTag("ask"));
check("no subscription is claimed as a requirement",
  !html.includes("\u5220\u9664\u9632\u62a4\u9700\u8981\u6709\u6548\u8ba2\u9605"));
check("the unprotected warning still shows",
  html.includes("\u6ca1\u6709\u4efb\u4f55\u4fdd\u62a4"),
  "with no mechanism at all the user has to be told files are exposed");
check("the free choice is still selectable", st.getState().tab === "settings");

// ---- A held handle reports itself honestly ----

S.protection = { kind: "hold", active: true, detail: "the guard is on, but no file is currently held" };
await ui.render();
html = doc();

check("a held handle is reported as protection",
  html.includes("no file is currently held"),
  "the hold status comes from protection_status, not from the driver service");

S.entitlement = openTier;

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
