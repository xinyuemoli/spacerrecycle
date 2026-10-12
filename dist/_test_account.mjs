/**
 * OneDrive account-card tests.
 *
 * The device-code flow is the one place in this app where the product waits on
 * something it cannot observe: the user is signing in inside their own browser.
 * The failures worth pinning down are therefore all about that wait - a poll
 * that stops rescheduling (the UI spins forever), a "not yet" answer treated as
 * a failure (the sign-in aborts while the user is still typing), and a timer
 * left running after the dialog is dismissed (toasts fire from another tab).
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
let accountStatus = { connected: false, quota: null, detail: "" };
let connectStart = null;
let pollReplies = [];
let clipboard = [];

globalThis.window = {
  __TAURI__: {
    core: {
      invoke: async (cmd) => {
        calls.push(cmd);
        if (cmd === "graph_account_status") return accountStatus;
        if (cmd === "graph_connect_start") {
          if (connectStart instanceof Error) throw connectStart;
          return connectStart;
        }
        if (cmd === "graph_connect_poll") return pollReplies.shift() || { status: "pending", detail: "", intervalSecs: 5, quota: null };
        if (cmd === "graph_sign_out") { accountStatus = { connected: false, quota: null, detail: "" }; return null; }
        if (cmd === "get_config") return { quarantineDir: "C:\\ProgramData\\SpaceRecycle\\Quarantine" };
        if (cmd === "read_audit_log") return [];
        if (cmd === "audit_log_path") return null;
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
// Node exposes `navigator` as a getter-only global, so the clipboard stub has
// to be defined on the instance rather than assigned.
Object.defineProperty(globalThis.navigator, "clipboard", {
  value: { writeText: async (s) => { clipboard.push(s); } },
  configurable: true,
  writable: true,
});
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

S.tab = "settings";
S.config = { quarantineDir: "C:\\ProgramData\\SpaceRecycle\\Quarantine" };
S.auditLog = [];

// ---------- nothing linked ----------

S.onedriveAccount = { connected: false, quota: null, detail: "" };
S.entitlement = null;
await ui.render();
let html = doc();
check("an unlinked account offers a connect button", html.includes("连接 OneDrive"), html.slice(0, 200));
check("an unlinked account does not show a quota", !html.includes("已用"));

// The card is hidden entirely until the backend answers.
S.onedriveAccount = null;
await ui.render();
check("the card is hidden before the backend answers", !doc().includes("连接 OneDrive"));

// ---------- connect ----------

S.onedriveAccount = { connected: false, quota: null, detail: "" };
connectStart = { userCode: "WXYZ-1234", verificationUri: "https://microsoft.com/devicelogin", intervalSecs: 5, expiresIn: 900 };
await st.actions.startConnect();
html = doc();
check("the sign-in code is shown", html.includes("WXYZ-1234"), html.slice(0, 300));
check("the sign-in address is shown as text", html.includes("https://microsoft.com/devicelogin"));
check("the waiting state is explained", html.includes("等待你在浏览器中完成登录"));

// ---------- polling ----------

st.actions.pollConnect.constructor; // keep the reference explicit
pollReplies = [{ status: "pending", detail: "", intervalSecs: 5, quota: null }];
await st.actions.pollConnect();
check("a pending poll keeps the sign-in alive", S.connect !== null && S.connect.status === "pending");

pollReplies = [{ status: "slow_down", detail: "", intervalSecs: 10, quota: null }];
await st.actions.pollConnect();
check("a slow-down raises the interval", S.connect && S.connect.intervalSecs === 10,
  "interval is " + (S.connect && S.connect.intervalSecs));

pollReplies = [{ status: "connected", detail: "", intervalSecs: 0, quota: { totalBytes: 100, usedBytes: 40, remainingBytes: 60, state: "normal" } }];
await st.actions.pollConnect();
check("a completed sign-in clears the waiting panel", S.connect === null);
check("a completed sign-in shows the quota", doc().includes("已用"));
check("a completed sign-in reports success", S.toast && S.toast.msg.includes("已连接"), S.toast && S.toast.msg);

// ---------- denied ----------

// Start from the realistic pre-condition: the connect button is only offered
// when nothing is linked, so the state must actually be unlinked to test it.
S.onedriveAccount = { connected: false, quota: null, detail: "" };
connectStart = { userCode: "AAAA-1111", verificationUri: "https://microsoft.com/devicelogin", intervalSecs: 5, expiresIn: 900 };
await st.actions.startConnect();
pollReplies = [{ status: "denied", detail: "sign-in was declined", intervalSecs: 0, quota: null }];
await st.actions.pollConnect();
check("a declined sign-in clears the panel", S.connect === null);
check("a declined sign-in says why", S.toast && S.toast.msg.includes("declined"), S.toast && S.toast.msg);
check("a declined sign-in leaves the account unlinked",
  S.onedriveAccount.connected === false && doc().includes("连接 OneDrive"));

// ---------- a failed start ----------

connectStart = new Error("未配置 Azure 应用（client）ID");
await st.actions.startConnect();
check("a failed start reports the backend message",
  S.toast && S.toast.msg.includes("client"), S.toast && S.toast.msg);
check("a failed start leaves no half-built panel", S.connect === null);

// ---------- cancel ----------

connectStart = { userCode: "BBBB-2222", verificationUri: "https://microsoft.com/devicelogin", intervalSecs: 5, expiresIn: 900 };
await st.actions.startConnect();
st.actions.cancelConnect();
check("cancelling clears the panel", S.connect === null);
check("cancelling tells the backend", calls.includes("graph_connect_cancel"));
await new Promise(r => setTimeout(r, 20));
check("cancelling stops the poll timer", !calls.includes("graph_connect_poll") ||
  calls.filter(c => c === "graph_connect_poll").length === 4, "an orphaned poll fired");

// ---------- copy the address ----------

connectStart = { userCode: "CCCC-3333", verificationUri: "https://example.test/login", intervalSecs: 5, expiresIn: 900 };
await st.actions.startConnect();
await st.actions.copyConnectLink();
check("copying puts the address on the clipboard", clipboard.includes("https://example.test/login"));
st.actions.cancelConnect();

// ---------- quota unavailable ----------

S.onedriveAccount = { connected: true, quota: null, detail: "graph is down" };
await ui.render();
html = doc();
check("a connected account with no quota still shows as connected", html.includes("已连接"));
check("a missing quota carries its reason", html.includes("graph is down"));
check("a missing quota does not render a usage bar", !html.includes("qbar"));

// ---------- sign out ----------

S.onedriveAccount = { connected: true, quota: { totalBytes: 100, usedBytes: 40, remainingBytes: 60, state: "normal" }, detail: "" };
await ui.render();
await st.actions.signOut();
check("signing out returns to the unlinked state", S.onedriveAccount && S.onedriveAccount.connected === false);
check("signing out tells the backend", calls.includes("graph_sign_out"));

// ---------- the usage bar reflects the numbers ----------

S.onedriveAccount = { connected: true, quota: { totalBytes: 200, usedBytes: 50, remainingBytes: 150, state: "normal" }, detail: "" };
await ui.render();
check("the usage bar reflects used over total", doc().includes("width:25%"), doc().slice(0, 400));

// Leave no timer behind: a pending poll would keep the process alive.
st.stopPolling();

console.log(`\n${pass} passed, ${fail} failed`);
if (fail) process.exitCode = 1;