// Regression tests for scan-state consistency across tabs.
//
// Reported symptoms:
//  1. The findings tab and the overview each had their own idea of whether a
//     scan was running, so a scan started in one place could be started again
//     from the other.
//  2. The overview's two scan buttons stayed clickable during a scan, and
//     cancelling from the overview appeared to do nothing.
//
// The backend mock holds start_scan open until cancelled, so the scanning
// state is observable for as long as the test needs it.
import { createRequire } from "node:module";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";

const require = createRequire(import.meta.url);
const ROOT = new URL(".", import.meta.url).pathname.replace(/^\/(.:)/, "$1");
let pw;
try { pw = require("playwright"); } catch (_) { console.log("SKIP playwright not installed"); process.exit(0); }
const CHROME = process.env.CHROME_PATH; // optional: override playwright's auto-detected Chromium
const MIME = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };

const server = createServer(async (req, res) => {
  try {
    const rel = normalize(decodeURIComponent(req.url.split("?")[0])).replace(/^([/\\])+/, "");
    const f = join(ROOT, rel === "" ? "index.html" : rel);
    if (!res.headersSent) res.writeHead(200, { "content-type": MIME[extname(f)] || "application/octet-stream" });
    res.end(await readFile(f));
  } catch { if (!res.headersSent) res.writeHead(404); res.end("x"); }
});
await new Promise(r => server.listen(0, "127.0.0.1", r));
const port = server.address().port;

let pass = 0, fail = 0;
const check = (name, cond, extra) => {
  if (cond) { pass++; console.log("PASS " + name); }
  else { fail++; console.log("FAIL " + name + (extra ? " :: " + extra : "")); }
};

const browser = await pw.chromium.launch({ headless: true, ...(CHROME ? { executablePath: CHROME } : {}) });
const page = await browser.newPage();

await page.addInitScript(() => {
  window.__calls = { start_scan: 0, cancel_scan: 0 };
  window.__release = null;
  window.__TAURI__ = {
    core: { invoke: async (cmd) => {
      if (cmd === "start_scan") {
        window.__calls.start_scan++;
        return new Promise(res => { window.__release = () => res([]); });
      }
      if (cmd === "cancel_scan") {
        window.__calls.cancel_scan++;
        // The real backend stops walking and start_scan then returns.
        if (window.__release) { const r = window.__release; window.__release = null; r(); }
        return null;
      }
      if (cmd === "get_config") return { largeFileAgeDays:30, leftoverAgeDays:30, largeFileMinBytes:1, quarantineExpiryDays:7, quarantineDir:"C:/q", extraExcludedRoots:[] };
      if (cmd === "quarantine_expiry") return { expiryDays:7, expiredBatches:[], expiredItems:0, expiredOnDiskBytes:0 };
      if (cmd === "get_volumes") return [];
      if (cmd === "list_quarantine_batches") return [];
      if (cmd === "read_audit_log") return [];
      return null;
    } },
    event: { listen: async () => () => {} },
  };
});

await page.goto("http://127.0.0.1:" + port + "/index.html");
await page.waitForSelector(".tab");
await page.evaluate(async () => { window.__st = await import("./state.js"); });

const st = () => page.evaluate(() => ({
  scanning: window.__st.state.scanning, cancelling: window.__st.state.cancelling,
  tab: window.__st.state.tab, toast: window.__st.state.toast?.msg ?? null,
}));
const scanBtns = () => page.evaluate(() =>
  [...document.querySelectorAll('[data-act="scan"]')].map(b => ({ t: b.textContent.trim(), disabled: b.disabled })));
const cancelBtns = () => page.evaluate(() =>
  [...document.querySelectorAll('[data-act="cancel"]')].map(b => b.textContent.trim()));

// ---- Start a scan from the findings tab ----
await page.click('[data-act="tab"][data-tab="findings"]');
await page.waitForTimeout(150);
await page.click('[data-act="scan"]');
await page.waitForTimeout(250);

check("the findings tab reports a scan in progress", (await st()).scanning);
check("the findings tab no longer offers a second scan", (await scanBtns()).length === 0);
const findCancels = await cancelBtns();
check("the findings tab offers a cancel", findCancels.length >= 1, JSON.stringify(findCancels));

// ---- The overview must agree that a scan is running ----
await page.click('[data-act="tab"][data-tab="overview"]');
await page.waitForTimeout(200);
const ovScan = await scanBtns();
check("the overview shows the scan buttons during a scan", ovScan.length === 2,
  "got " + JSON.stringify(ovScan));
check("the overview's scan buttons are disabled during a scan",
  ovScan.length === 2 && ovScan.every(b => b.disabled),
  JSON.stringify(ovScan));
check("the overview has exactly one cancel control, not two",
  (await cancelBtns()).length === 1, JSON.stringify(await cancelBtns()));

// ---- Even a forced click must not start a second scan ----
const forced = await page.evaluate(() => {
  const btns = [...document.querySelectorAll('[data-act="scan"]')];
  const before = window.__calls.start_scan;
  for (const b of btns) { b.disabled = false; b.click(); }
  return { before, after: window.__calls.start_scan };
});
check("a second scan cannot be started while one runs",
  forced.after === forced.before, JSON.stringify(forced));

// ---- Cancelling from the overview must take effect ----
await page.click('[data-act="cancel"]');
await page.waitForTimeout(400);
const after = await st();
check("cancel clears the scanning state", !after.scanning, JSON.stringify(after));
check("cancel is not reported as a successful scan",
  after.toast !== "扫描完成，发现 0 项可回收内容", "toast=" + after.toast);
check("the overview's scan buttons come back after cancelling",
  (await scanBtns()).every(b => !b.disabled), JSON.stringify(await scanBtns()));
check("the cancel control is gone after cancelling", (await cancelBtns()).length === 0);

// ---- And a new scan can be started again ----
await page.click('[data-act="scan"]');
await page.waitForTimeout(250);
check("a new scan can be started after cancelling",
  (await page.evaluate(() => window.__calls.start_scan)) === 2,
  "calls=" + await page.evaluate(() => window.__calls.start_scan));

// ---- A rejected scan must not leave the UI stuck in "scanning" forever. ----
await page.evaluate(() => {
  window.__TAURI__.core.invoke = async (cmd) => {
    if (cmd === "start_scan") throw new Error("backend exploded");
    return null;
  };
  window.__st.state.scanning = false;
  window.__st.state.cancelling = false;
});
await page.click('[data-act="tab"][data-tab="overview"]');
await page.waitForTimeout(150);
await page.click('[data-act="scan"]:not([disabled])');
await page.waitForTimeout(400);
const afterFail = await st();
check("a failed scan clears the scanning state", !afterFail.scanning, JSON.stringify(afterFail));
check("a failed scan reports the error",
  afterFail.toast && afterFail.toast.includes("扫描失败"), "toast=" + afterFail.toast);
check("a failed scan leaves the overview usable",
  (await scanBtns()).length === 2 && (await scanBtns()).every(b => !b.disabled),
  JSON.stringify(await scanBtns()));


await page.close();
const page2 = await browser.newPage();
await page2.addInitScript(() => {
  window.__release = null;
  window.__TAURI__ = {
    core: { invoke: async (cmd) => {
      if (cmd === "start_scan") {
        window.__n = (window.__n || 0) + 1;
        if (window.__n === 1) {
          return [{ id:"old1", ruleId:"temp.a", name:"OLD-RESULT.tmp",
            path:"C:/temp/OLD-RESULT.tmp", category:"temp_junk", risk:"safe",
            disposition:"purge", logicalSize:5e9, onDiskSize:5e9,
            ageDays:10, lastAccessUnix:1700000000, isDir:false }];
        }
        return new Promise(res => { window.__release = () => res([]); });
      }
      if (cmd === "cancel_scan") {
        if (window.__release) { const r = window.__release; window.__release = null; r(); }
        return null;
      }
      if (cmd === "get_config") return { largeFileAgeDays:30, leftoverAgeDays:30, largeFileMinBytes:1, quarantineExpiryDays:7, quarantineDir:"C:/q", extraExcludedRoots:[] };
      if (cmd === "quarantine_expiry") return { expiryDays:7, expiredBatches:[], expiredItems:0, expiredOnDiskBytes:0 };
      if (cmd === "get_volumes") return [];
      if (cmd === "list_quarantine_batches") return [];
      if (cmd === "read_audit_log") return [];
      return null;
    } },
    event: { listen: async () => () => {} },
  };
});
await page2.goto("http://127.0.0.1:" + port + "/index.html");
await page2.waitForSelector(".tab");
await page2.evaluate(async () => { window.__st = await import("./state.js"); });

const view = () => page2.evaluate(() => ({
  scanning: window.__st.state.scanning,
  findings: window.__st.state.findings.length,
  selected: window.__st.state.selected.size,
  rows: document.querySelectorAll("tbody tr[data-id]").length,
  names: window.__st.state.findings.map(f => f.name),
}));
const scan = async () => {
  await page2.click('[data-act="tab"][data-tab="overview"]');
  await page2.waitForTimeout(150);
  await page2.click('[data-act="scan"]');
  await page2.waitForTimeout(400);
};
const toFindings = async () => {
  await page2.click('[data-act="tab"][data-tab="findings"]');
  await page2.waitForTimeout(200);
};

// ---- Everything on screen belongs to the scan currently running ----
await scan();
await toFindings();
const first = await view();
check("a finished scan shows its own results",
  first.findings === 1 && first.names[0] === "OLD-RESULT.tmp", JSON.stringify(first));

await scan();
await toFindings();
const during = await view();
check("starting a re-scan clears the previous results at once",
  during.findings === 0, JSON.stringify(during));
check("no stale rows are drawn while the new scan runs",
  during.rows === 0, "rows=" + during.rows);
check("the findings tab says a scan is running",
  (await page2.locator("main").innerText()).includes("正在扫描"));

await page2.click('[data-act="tab"][data-tab="overview"]');
await page2.waitForTimeout(150);
await page2.click('[data-act="cancel"]');
await page2.waitForTimeout(500);
await toFindings();
const cancelled = await view();
check("a cancelled scan leaves nothing behind", cancelled.findings === 0, JSON.stringify(cancelled));
check("the selection does not survive a cancel", cancelled.selected === 0,
  "selected=" + cancelled.selected);

await scan();
await toFindings();
check("starting again after a cancel is still clean",
  (await view()).findings === 0, JSON.stringify(await view()));

await page2.click('[data-act="tab"][data-tab="overview"]');
await page2.waitForTimeout(200);
const ovText = await page2.locator("main").innerText();
check("the overview stops quoting figures from a finished scan",
  !ovText.includes("来自最近一次扫描"), ovText.slice(0, 100).replace(/\n/g, " "));

await browser.close();
server.close();
console.log("\n" + pass + " passed, " + fail + " failed");
process.exit(fail ? 1 : 0);
