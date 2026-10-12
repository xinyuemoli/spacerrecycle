/**
 * Subscription-card tests.
 *
 * The card is the only place a paying user can find out *why* they are being
 * refused, so the two failures worth pinning down are: a subscriber who is not
 * shown as one, and a "no" with no reason attached. The Store call itself
 * cannot run here (it needs package identity), which is exactly why the
 * backend policy is tested separately and this file only covers the display.
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

const answer = { entitlement: null, refresh: null };
const calls = [];
globalThis.window = {
  __TAURI__: {
    core: {
      invoke: async (cmd) => {
        calls.push(cmd);
        if (cmd === "get_entitlement") return answer.entitlement;
        if (cmd === "refresh_entitlement") return answer.refresh;
        if (cmd === "get_config") return { quarantineDir: "C:\\ProgramData\\SpaceRecycle\\Quarantine" };
        if (cmd === "read_audit_log") return [];
        if (cmd === "audit_log_path") return null;
        if (cmd === "list_importance") return [];
        if (cmd === "onedrive_status") return { available: false, roots: [] };
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

// ---------- subscriber ----------

answer.entitlement = {
  subscriber: true,
  source: "store",
  checkedUnix: 1700000000,
  expiresUnix: 1800000000,
  detail: "active subscription",
};
await st.actions.loadEntitlement();
S.tab = "settings";
S.config = { quarantineDir: "C:\\ProgramData\\SpaceRecycle\\Quarantine" };
S.auditLog = [];
await ui.render();
let html = doc();

check("a subscriber is shown as subscribed", html.includes("已订阅"), html.slice(0, 200));
check("the card explains how it was checked", html.includes("微软商店（实时校验）"));
check("the card shows the expiry", html.includes("有效期至"));
check("the card offers a re-check", html.includes('data-act="refresh-entitlement"'));

// ---------- non-subscriber, with a reason ----------

answer.entitlement = {
  subscriber: false,
  source: "unavailable",
  checkedUnix: 1700000000,
  expiresUnix: null,
  detail: "the Microsoft Store is unavailable (needs a packaged build)",
};
await st.actions.loadEntitlement();
await ui.render();
html = doc();
check("a non-subscriber is shown as not subscribed", html.includes("未订阅"));
check("the refusal carries a reason",
  html.includes("needs a packaged build"),
  "a bare 'not subscribed' leaves a paying user with nothing to act on");
check("the source is named", html.includes("微软商店不可用"));

// ---------- the dev override reads differently ----------

answer.entitlement = {
  subscriber: true,
  source: "dev_override",
  checkedUnix: 1700000000,
  expiresUnix: null,
  detail: "development override is enabled in the config file",
};
await st.actions.loadEntitlement();
await ui.render();
check("the dev override is labelled as one", doc().includes("开发配置覆盖"));

// ---------- unconfigured build ----------

answer.entitlement = {
  subscriber: false,
  source: "not_configured",
  checkedUnix: 1700000000,
  expiresUnix: null,
  detail: "no subscription add-on is configured",
};
await st.actions.loadEntitlement();
await ui.render();
check("an unconfigured build says so", doc().includes("尚未配置订阅加载项"));

// ---------- no answer at all ----------

S.entitlement = null;
await ui.render();
check("the card is hidden when the backend did not answer",
  !doc().includes('data-act="refresh-entitlement"'));

// ---------- re-check ----------

answer.refresh = {
  subscriber: true,
  source: "store",
  checkedUnix: 1700000001,
  expiresUnix: null,
  detail: "active subscription",
};
await st.actions.refreshEntitlement();
check("re-checking calls the refresh command", calls.includes("refresh_entitlement"));
check("re-checking updates what is shown", S.entitlement && S.entitlement.subscriber === true);
check("re-checking reports the outcome", S.toast && S.toast.msg.includes("订阅"), S.toast && S.toast.msg);

console.log(`\n${pass} passed, ${fail} failed`);
if (fail) process.exitCode = 1;