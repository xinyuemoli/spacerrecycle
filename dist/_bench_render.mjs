/**
 * Measures what a full re-render actually costs at realistic sizes.
 *
 * The audit assumed "innerHTML is fast enough" and skipped partial updates.
 * This checks that claim instead of asserting it.
 */
import { createRequire } from "node:module";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";

const require = createRequire(import.meta.url);
const ROOT = new URL(".", import.meta.url).pathname.replace(/^\/(.:)/, "$1");
let pw;
try { pw = require("playwright"); } catch (_) { console.log("SKIP"); process.exit(0); }
const CHROME = process.env.CHROME_PATH; // optional: override playwright's auto-detected Chromium
const MIME = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css" };
const server = createServer(async (req, res) => {
  try {
    const rel = normalize(decodeURIComponent(req.url.split("?")[0])).replace(/^([/\\])+/, "");
    const file = join(ROOT, rel === "" ? "index.html" : rel);
    const body = await readFile(file);
    res.writeHead(200, { "content-type": MIME[extname(file)] || "application/octet-stream" });
    res.end(body);
  } catch { res.writeHead(404); res.end("nope"); }
});
await new Promise(r => server.listen(0, "127.0.0.1", r));
const port = server.address().port;

const browser = await pw.chromium.launch({ headless: true, ...(CHROME ? { executablePath: CHROME } : {}) });
const page = await browser.newPage();
await page.addInitScript(() => {
  window.__TAURI__ = {
    core: { invoke: async (cmd) => {
      switch (cmd) {
        case "get_volumes": return [];
        case "get_config": return { largeFileAgeDays: 30, leftoverAgeDays: 30, largeFileMinBytes: 524288000, quarantineExpiryDays: 7, quarantineDir: "C:/q", extraExcludedRoots: [] };
        case "quarantine_expiry": return { expiryDays: 7, expiredBatches: [], expiredItems: 0, expiredOnDiskBytes: 0 };
        case "list_quarantine_batches": return [];
        case "read_audit_log": return [];
        case "audit_log_path": return "C:/cfg/audit.log";
        default: return null;
      }
    } },
    event: { listen: async () => () => {} },
  };
});
await page.goto("http://127.0.0.1:" + port + "/index.html");
await page.waitForSelector(".tab");

const res = await page.evaluate(async () => {
  const st = await import("./state.js");
  const ui = await import("./ui.js");
  const mk = (n) => Array.from({ length: n }, (_, i) => ({
    id: "f" + i, ruleId: "r" + i, name: "file-" + i + ".tmp",
    path: "C:/Users/me/AppData/Local/Temp/some/deep/path/file-" + i + ".tmp",
    category: ["temp_junk", "system_cache", "dev_artifact", "large_file", "app_leftover"][i % 5],
    risk: ["safe", "rebuildable", "personal", "userdata"][i % 4],
    disposition: "quarantine", logicalSize: 1048576, onDiskSize: 524288,
    ageDays: 40, lastAccessUnix: 1700000000, isDir: false,
  }));
  const out = {};
  for (const n of [100, 1000, 5000]) {
    st.state.findings = mk(n);
    st.state.tab = "findings";
    st.state.riskFilter = "all";
    st.state.query = "";
    await ui.render();
    // Alternate the tab so the main region genuinely changes every iteration;
    // rendering identical state would only measure the region cache.
    const t0 = performance.now();
    const R = 6;
    for (let i = 0; i < R; i++) {
      st.state.tab = i % 2 ? "findings" : "overview";
      await ui.render();
    }
    out["findings_" + n] = (performance.now() - t0) / R;
  }
  // Tab switch cost with a realistic finding set loaded.
  st.state.findings = mk(2000);
  const t1 = performance.now();
  for (let i = 0; i < 10; i++) { st.state.tab = "overview"; await ui.render(); st.state.tab = "findings"; await ui.render(); }
  out.tab_switch_2000 = (performance.now() - t1) / 10;
  // Search keystroke cost (the focus-restore path).
  st.state.tab = "findings";
  await ui.render();
  const t2 = performance.now();
  for (let i = 0; i < 10; i++) { st.state.query = "file-" + i; await ui.render(); }
  out.search_keystroke_2000 = (performance.now() - t2) / 10;

  // Toggling one checkbox is the most frequent interaction in the table.
  st.state.tab = "findings";
  st.state.query = "";
  await ui.render();
  st.state.selected.clear();
  st.state.selected.add("f3");
  await ui.render();
  const t3 = performance.now();
  for (let i = 0; i < 10; i++) {
    st.actions.toggleSel("f" + (i % 50));
  }
  out.toggle_one_row_2000 = (performance.now() - t3) / 10;

  // For contrast: the same state change forced through a full render.
  st.state.selected.clear();
  await ui.render();
  const t4 = performance.now();
  for (let i = 0; i < 10; i++) { st.state.selected.clear(); st.state.selected.add("f" + i); await ui.render(); }
  out.full_render_for_one_row_2000 = (performance.now() - t4) / 10;
  st.state.query = "";
  st.state.tab = "findings";
  await ui.render();
  out.dom_nodes = document.querySelectorAll("#app *").length;
  out.rows_rendered = document.querySelectorAll("tbody tr").length;
  return out;
});
for (const [k, v] of Object.entries(res)) console.log(k.padEnd(26), typeof v === "number" ? v.toFixed(1) + " ms" : v);
await browser.close();
server.close();
