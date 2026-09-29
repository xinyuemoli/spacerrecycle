/**
 * Real-DOM regression tests.
 *
 * The other harnesses stub `document`, so they can check what markup is
 * produced but not whether a click actually reaches its handler. That gap is
 * how a confirm dialog whose every button was dead shipped: the markup was
 * right, the listeners were right, and the click still went nowhere because
 * an inline stopPropagation ate it on the way up.
 *
 * This harness runs the real UI in a real browser and dispatches real events.
 */
import { createRequire } from "node:module";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";

const require = createRequire(import.meta.url);
const ROOT = new URL(".", import.meta.url).pathname.replace(/^\/(.:)/, "$1");

let pw;
try {
  pw = require("playwright");
} catch (_) {
  console.log("SKIP playwright is not installed");
  process.exit(0);
}

const CHROME = process.env.CHROME_PATH; // optional: override playwright's auto-detected Chromium

const MIME = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".mjs": "text/javascript" };

const server = createServer(async (req, res) => {
  try {
    const rel = normalize(decodeURIComponent(req.url.split("?")[0])).replace(/^([/\\])+/, "");
    const file = join(ROOT, rel === "" ? "index.html" : rel);
    const body = await readFile(file);
    res.writeHead(200, { "content-type": MIME[extname(file)] || "application/octet-stream" });
    res.end(body);
  } catch (e) {
    res.writeHead(404);
    res.end("not found");
  }
});

await new Promise((r) => server.listen(0, "127.0.0.1", r));
const port = server.address().port;

let pass = 0, fail = 0;
function check(name, cond, detail) {
  if (cond) { pass++; console.log("PASS " + name); }
  else { fail++; console.log("FAIL " + name + (detail ? " :: " + detail : "")); }
}

const browser = await pw.chromium.launch({ headless: true, ...(CHROME ? { executablePath: CHROME } : {}) });
const page = await browser.newPage();
const consoleErrors = [];
page.on("pageerror", (e) => consoleErrors.push(String(e)));

// A mock backend. cleanupCalls counts the confirm clicks that actually landed.
await page.addInitScript(() => {
  window.__cleanupCalls = 0;
  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        switch (cmd) {
          case "get_volumes": return [{ mount: "C:", label: "System", totalBytes: 100e9, freeBytes: 40e9, usedBytes: 60e9, usedRatio: 0.6 }];
          case "get_config": return { largeFileAgeDays: 30, leftoverAgeDays: 30, largeFileMinBytes: 524288000, quarantineExpiryDays: 7, quarantineDir: "C:\\ProgramData\\SpaceRecycle\\Quarantine", extraExcludedRoots: [] };
          case "quarantine_expiry": return { expiryDays: 7, expiredBatches: [], expiredItems: 0, expiredOnDiskBytes: 0 };
          case "list_quarantine_batches": return [];
          case "build_preview": return [{ disposition: "purge", itemCount: 1, onDiskBytes: 1024, categoryCounts: { temp_junk: 1 }, containsRedPurge: false }];
          case "needs_red_confirmation": return false;
          case "run_cleanup": window.__cleanupCalls++; return { purged: ["C:/temp/a.tmp"], quarantined: [], blocked: [], failed: [], needsElevation: false };
          case "audit_log_path": return "C:/cfg/audit.log";
          case "read_audit_log": return [];
          case "cancel_scan": return null;
          default: return null;
        }
      },
    },
    event: { listen: async () => () => {} },
  };
});

await page.goto("http://127.0.0.1:" + port + "/index.html");
await page.waitForSelector(".tab");
await page.evaluate(async () => {
  window.__st = await import("./state.js");
  window.__clipboard = null;
  // copyPreview prefers the clipboard and only falls back to a download, so
  // the clipboard is what a real click has to reach.
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: async (t) => { window.__clipboard = t; } },
  });
});

// Seed one finding and open the preview dialog.
await page.evaluate(async () => {
  const st = await import("./state.js");
  st.state.findings = [{
    id: "f1", ruleId: "temp.a", name: "a.tmp", path: "C:/temp/a.tmp",
    category: "temp_junk", risk: "safe", disposition: "purge",
    logicalSize: 1024, onDiskSize: 1024, ageDays: 3, lastAccessUnix: 1700000000,
    isDir: false,
  }];
  st.state.selected.add("f1");
  await st.actions.openPreview();
});
await page.waitForSelector(".modal", { timeout: 5000 });

// --- The bug that shipped: every button in the dialog was dead. ---
await page.click('[data-act="do-cleanup"]');
await page.waitForTimeout(300);
check("confirm button reaches the backend", await page.evaluate(() => window.__cleanupCalls) === 1,
  "cleanupCalls=" + await page.evaluate(() => window.__cleanupCalls));
check("dialog closes after confirming", await page.locator(".modal").count() === 0);

// Reopen for the remaining dialog tests.
await page.evaluate(async () => {
  const st = window.__st;
  st.state.findings = [{
    id: "f1", ruleId: "temp.a", name: "a.tmp", path: "C:/temp/a.tmp",
    category: "temp_junk", risk: "safe", disposition: "purge",
    logicalSize: 1024, onDiskSize: 1024, ageDays: 3, lastAccessUnix: 1700000000,
    isDir: false,
  }];
  st.state.selected = new Set(["f1"]);
  st.state.openDir = null;
  await st.actions.openPreview();
});
await page.waitForSelector(".modal");

// Cancel button.
await page.click('.modal-foot [data-act="close-preview"]');
await page.waitForTimeout(200);
check("cancel button closes the dialog", await page.locator(".modal").count() === 0);

await page.evaluate(async () => {
  const st = window.__st;
  st.state.findings = [{
    id: "f1", ruleId: "temp.a", name: "a.tmp", path: "C:/temp/a.tmp",
    category: "temp_junk", risk: "safe", disposition: "purge",
    logicalSize: 1024, onDiskSize: 1024, ageDays: 3, lastAccessUnix: 1700000000,
    isDir: false,
  }];
  st.state.selected = new Set(["f1"]);
  st.state.openDir = null;
  await st.actions.openPreview();
});
await page.waitForSelector(".modal");
// Clicking inside the dialog must NOT dismiss it.
await page.click(".modal-head h3");
await page.waitForTimeout(200);
check("clicking inside the dialog does not close it", await page.locator(".modal").count() === 1);
// Clicking the backdrop does dismiss it.
await page.mouse.click(5, 5);
await page.waitForTimeout(200);
check("clicking the overlay closes the dialog", await page.locator(".modal").count() === 0);

// Export button (was bound only to 'change', so it never fired).
await page.evaluate(async () => {
  const st = window.__st;
  st.state.findings = [{
    id: "f1", ruleId: "temp.a", name: "a.tmp", path: "C:/temp/a.tmp",
    category: "temp_junk", risk: "safe", disposition: "purge",
    logicalSize: 1024, onDiskSize: 1024, ageDays: 3, lastAccessUnix: 1700000000,
    isDir: false,
  }];
  st.state.selected = new Set(["f1"]);
  st.state.openDir = null;
  await st.actions.openPreview();
});
await page.waitForSelector(".modal");
await page.click('[data-act="export-preview"]');
await page.waitForTimeout(300);
check("export button copies the preview text",
  (await page.evaluate(() => window.__clipboard || "")) .includes("C:/temp/a.tmp"),
  "clipboard=" + JSON.stringify(await page.evaluate(() => window.__clipboard)));
await page.click('.modal-foot [data-act="close-preview"]');
await page.waitForTimeout(200);

// --- Category card drill-down (was a copy-paste no-op). ---
await page.evaluate(() => { window.__st.state.preview = null; });
await page.evaluate(async () => {
  const st = window.__st;
  st.state.findings.push({
    id: "f2", ruleId: "large.b", name: "big.mkv", path: "D:/m/big.mkv",
    category: "large_file", risk: "personal", disposition: "quarantine",
    logicalSize: 5e9, onDiskSize: 5e9, ageDays: 90, lastAccessUnix: 1700000000, isDir: false,
  });
  st.state.tab = "overview";
  const ui = await import("./ui.js");
  await ui.render();
});
await page.click('.stat[data-act="goto-cat"]');
await page.waitForTimeout(200);
check("category card switches to the findings tab",
  await page.evaluate(() => window.__st.state.tab) === "findings");
check("category card applies a risk filter",
  await page.evaluate(() => window.__st.state.riskFilter) !== "all",
  "riskFilter=" + await page.evaluate(() => window.__st.state.riskFilter));
check("category card actually filters the list",
  await page.locator('tbody tr[data-act], tbody tr').count() > 0);

// --- Search box must keep focus and caret while typing. ---
await page.evaluate(async () => {
  const st = window.__st;
  st.state.riskFilter = "all";
  st.state.query = "";
  const ui = await import("./ui.js");
  await ui.render();
});
await page.waitForSelector('input[data-act="search"]');
await page.click('input[data-act="search"]');
await page.keyboard.type("temp");
await page.waitForTimeout(300);
check("search box keeps focus while typing",
  await page.evaluate(() => document.activeElement && document.activeElement.dataset.act) === "search",
  "activeElement=" + await page.evaluate(() => document.activeElement && document.activeElement.tagName));
check("search box keeps the typed text",
  await page.evaluate(() => window.__st.state.query) === "temp",
  "query=" + await page.evaluate(() => JSON.stringify(window.__st.state.query)));
check("search box keeps the caret at the end",
  await page.evaluate(() => document.activeElement && document.activeElement.selectionStart) === 4,
  "caret=" + await page.evaluate(() => document.activeElement && document.activeElement.selectionStart));

// --- Layout sanity: the region wrappers must not disturb the flex column. ---
await page.evaluate(async () => {
  const st = window.__st;
  st.state.tab = "findings";
  st.state.selected = new Set(["f0"]);
  const ui = await import("./ui.js");
  ui.invalidate();
  await ui.render();
  window.__st.actions.openPreview();
});
await page.waitForTimeout(300);
const layout = await page.evaluate(() => {
  const main = document.querySelector("main");
  const tabs = document.querySelector(".tabs");
  const app = document.getElementById("app");
  return {
    mainHeight: main.getBoundingClientRect().height,
    appHeight: app.getBoundingClientRect().height,
    tabsVisible: tabs.getBoundingClientRect().height > 0,
    mainScrolls: main.scrollHeight >= main.clientHeight,
  };
});
check("main fills the window height", layout.mainHeight > 300, "main=" + layout.mainHeight);
check("the tab bar is still visible", layout.tabsVisible);
check("the app column is full height", layout.appHeight > 300, "app=" + layout.appHeight);

// The modal must still cover the whole window from inside its wrapper.
await page.evaluate(async () => {
  const st = window.__st;
  st.state.findings = [{
    id: "L0", ruleId: "temp.a", name: "layout.tmp", path: "C:/temp/layout.tmp",
    category: "temp_junk", risk: "safe", disposition: "purge",
    logicalSize: 1024, onDiskSize: 1024, ageDays: 1, lastAccessUnix: 1700000000, isDir: false,
  }];
  st.state.selected = new Set(["L0"]);
  st.state.preview = null;
  const ui = await import("./ui.js");
  ui.invalidate();
  await ui.render();
  await st.actions.openPreview();
});
await page.waitForSelector(".overlay", { timeout: 5000 });
const ov = await page.evaluate(() => {
  const o = document.querySelector(".overlay").getBoundingClientRect();
  return { w: o.width, h: o.height };
});
check("the dialog overlay still covers the window", ov.w > 600 && ov.h > 400, JSON.stringify(ov));

// Close the dialog before measuring the bar underneath it.
await page.click('.modal-foot [data-act="close-preview"]');
await page.waitForTimeout(200);

// The action bar is position:fixed at the bottom.
const ab = await page.evaluate(() => {
  const a = document.querySelector(".actionbar").getBoundingClientRect();
  return { bottom: Math.round(a.bottom), h: Math.round(a.height), vh: window.innerHeight };
});
check("the action bar sits at the bottom of the window",
  Math.abs(ab.bottom - ab.vh) < 4, JSON.stringify(ab));

// --- A null findings response must not white-screen the app. ---
await page.evaluate(async () => {
  window.__TAURI__.core.invoke = async (cmd) => {
    if (cmd === "start_scan") return null;   // a panic or a stubbed backend
    return null;
  };
  await window.__st.actions.startScan(["temp_junk"]);
});
await page.waitForTimeout(300);
check("a null scan result does not break rendering",
  await page.evaluate(() => Array.isArray(window.__st.state.findings)));
check("a null scan result is reported as a failure",
  await page.locator(".toast.err").count() === 1);
check("the scan button is offered again after a failed scan",
  await page.evaluate(() => window.__st.state.scanning) === false);

// --- Selection must be toggled in place, and must still be correct. ---
await page.evaluate(async () => {
  const st = window.__st;
  st.state.findings = Array.from({ length: 300 }, (_, i) => ({
    id: "f" + i, ruleId: "r" + i, name: "item-" + i + ".tmp",
    path: "C:/temp/item-" + i + ".tmp",
    category: ["temp_junk", "system_cache", "dev_artifact"][i % 3],
    risk: ["safe", "rebuildable", "userdata"][i % 3],
    disposition: "purge", logicalSize: 1048576, onDiskSize: 524288,
    ageDays: 40, lastAccessUnix: 1700000000, isDir: false,
  }));
  st.state.selected = new Set();
  st.state.riskFilter = "all";
  st.state.query = "";
  st.state.tab = "findings";
  const ui = await import("./ui.js");
  ui.invalidate();
  await ui.render();
});

// Clicking a row checkbox must tick it, in place.
const rowsBefore = await page.evaluate(() => document.querySelectorAll("tbody tr[data-id]").length);
await page.click('tbody tr[data-id="f1"] input[type="checkbox"]');
await page.waitForTimeout(200);
check("clicking a row ticks it",
  await page.evaluate(() => window.__st.state.selected.has("f1")));
check("the ticked row is highlighted",
  await page.evaluate(() => document.querySelector('tbody tr[data-id="f1"]').classList.contains("selected")));
check("ticking a row does not rebuild the table",
  (await page.evaluate(() => document.querySelectorAll("tbody tr[data-id]").length)) === rowsBefore,
  "row count changed: " + rowsBefore + " -> " + await page.evaluate(() => document.querySelectorAll("tbody tr[data-id]").length));
check("ticking a row updates the action bar",
  await page.locator(".actionbar").count() === 1);
check("ticking a row shows the item in the action bar",
  (await page.locator(".actionbar").innerText()).includes("1"));

// Un-ticking must clear it.
await page.click('tbody tr[data-id="f1"] input[type="checkbox"]');
await page.waitForTimeout(200);
check("clicking a ticked row unticks it",
  !(await page.evaluate(() => window.__st.state.selected.has("f1"))));
check("unticking removes the action bar",
  await page.locator(".actionbar").count() === 0);

// The category checkbox selects a whole category.
await page.click('.cat-head[data-cat="temp_junk"] input[type="checkbox"]');
await page.waitForTimeout(200);
check("the category checkbox selects its category",
  await page.evaluate(() => {
    const st = window.__st;
    return st.state.findings.filter(f => f.category === "temp_junk")
      .every(f => st.state.selected.has(f.id));
  }));
check("the category checkbox does not select other categories",
  await page.evaluate(() => {
    const st = window.__st;
    return st.state.findings.filter(f => f.category !== "temp_junk")
      .every(f => !st.state.selected.has(f.id));
  }));
check("selecting a category updates the action bar count",
  (await page.locator(".actionbar").innerText()).includes("100"));

await browser.close();
server.close();
console.log("\n" + pass + " passed, " + fail + " failed");
if (consoleErrors.length) {
  console.log("page errors:");
  for (const e of consoleErrors) console.log("  " + e);
}
process.exit(fail ? 1 : 0);
