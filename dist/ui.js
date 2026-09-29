import {
  state, actions, fmtBytes, fmtAge, fmtWhen, effectiveDisposition,
  selectedItems, selectionTotals, filteredFindings, categoryTotals,
  dirTotals, findingsInDir,
  categoryLabel, riskLabelOf, RISK_DOT, isExpired, ageDays, PAGE_SIZE,
} from "./state.js";
import { listen } from "./api.js";
import { t } from "./i18n.js";

/** Localised string lookup against the current language. */
const T = (key, vars) => t(state.lang, key, vars);

const el = (id) => document.getElementById(id);
// HTML-escape.
//
// The original built a fresh five-key lookup object inside the replace
// callback, so it allocated once per escaped character - and this runs five
// or six times per table row. The table is now the hot path rather than the
// whole tree, and at a few thousand rows that allocation was the single
// largest cost in a render.
//
// Most values in a findings table contain nothing that needs escaping at all,
// so the regex test short-circuits those to a plain String() conversion.
const ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };
const NEEDS_ESCAPE = /[&<>"']/;
const esc = (s) => {
  const str = s ?? "";
  const text = typeof str === "string" ? str : String(str);
  return NEEDS_ESCAPE.test(text) ? text.replace(/[&<>"']/g, (c) => ESCAPES[c]) : text;
};

// Row rendering is dominated by per-row string work, so the parts that do not
// change between renders are built once per category and reused.
const h = (strings, ...v) => strings.reduce((a, s, i) => a + s + (v[i] ?? ""), "");

/**
 * Remember the focused field across a re-render.
 *
 * The tree is rebuilt with innerHTML on every state change, which detaches
 * whatever the user was typing into. The search box fires setQuery on each
 * keystroke, so without this the caret jumps to the start after every
 * character and the field loses focus entirely - the single biggest source of
 * the "the UI feels laggy" impression.
 */
function captureFocus() {
  const a = document.activeElement;
  if (!a || !a.dataset || !a.dataset.act) return null;
  return {
    act: a.dataset.act,
    id: a.dataset.id ?? null,
    key: a.dataset.key ?? null,
    start: a.selectionStart ?? null,
    end: a.selectionEnd ?? null,
  };
}

function restoreFocus(saved) {
  if (!saved) return;
  const sel = 'input[data-act="' + saved.act + '"], select[data-act="' + saved.act + '"]';
  let node = null;
  for (const el of document.querySelectorAll(sel)) {
    if (saved.id !== null && el.dataset.id !== saved.id) continue;
    if (saved.key !== null && el.dataset.key !== saved.key) continue;
    node = el;
    break;
  }
  if (!node) return;
  node.focus();
  // Restoring a caret in a <select> throws in some engines; the focus alone
  // is what matters there.
  if (saved.start !== null && typeof node.setSelectionRange === "function") {
    try { node.setSelectionRange(saved.start, saved.end); } catch (_) { /* not a text field */ }
  }
}

/**
 * The document is a fixed set of regions, and each one is only rewritten when
 * its HTML actually changes.
 *
 * The tree used to be rebuilt wholesale on every state change, so a single
 * keystroke in the search box re-rendered every row of the findings table:
 * ~190 ms at 1000 rows and ~1.5 s at 5000. Region updates make the cost
 * proportional to what actually moved - typing filters the rows and nothing
 * else, ticking a checkbox touches one <tr>.
 *
 * Region identity is the key of lastRegionHtml, so a view function that
 * returns byte-identical HTML costs one string comparison.
 */
const REGIONS = ["elevbanner", "tabs", "progress", "main", "actionbar", "modal", "toast"];

const lastRegionHtml = new Map();

/** Force the next render() to rewrite every region. */
export function invalidate() {
  lastRegionHtml.clear();
}

/**
 * Re-render after a pure selection change, without touching the table.
 *
 * Ticking a row changes its checkbox, its row class, the per-category
 * checkbox, the "显示 n / m" counter and the action bar - but not one byte of
 * the table body. Rebuilding it anyway cost a full parse of the findings
 * table, which at a few thousand rows is the interaction the user performs
 * most often.
 *
 * Any change to the finding set itself (a new scan, a cleanup) invalidates
 * the region cache, so the next full render rebuilds the table as usual.
 */
export function renderSelectionOnly() {
  // The table is the expensive part; leave it exactly as it is.
  const table = el("r-main");
  if (!table) { render(); return; }

  // Patch each row's checkbox and highlight class in place.
  for (const tr of table.querySelectorAll("tbody tr[data-id]")) {
    const id = tr.dataset.id;
    const on = state.selected.has(id);
    const box = tr.querySelector('input[type="checkbox"]');
    if (box) box.checked = on;
    tr.classList.toggle("selected", on);
  }
  // Patch each category header checkbox and the "all selected" state.
  for (const head of table.querySelectorAll(".cat-head[data-cat]")) {
    const cat = head.dataset.cat;
    const vis = table.querySelectorAll('tbody tr[data-cat="' + cat + '"]');
    let all = true, some = false;
    for (const tr of vis) {
      const on = state.selected.has(tr.dataset.id);
      if (on) some = true; else all = false;
    }
    const box = head.querySelector('input[type="checkbox"]');
    if (box) { box.checked = some; box.dataset.on = all ? "0" : "1"; }
  }

  // The action bar and the visible-count line are cheap; refresh just those.
  const bar = el("r-actionbar");
  const wantBar = state.tab === "findings" && state.selected.size ? actionBar() : "";
  if (bar && bar.innerHTML !== wantBar) bar.innerHTML = wantBar;

  const counter = table.querySelector("[data-role='shown-count']");
  if (counter) {
    counter.textContent = T("find.shown", { shown: filteredFindings().length, total: state.findings.length });
  }
}

export async function render() {
  const app = el("app");

  // Build the tree once, then keep the regions. On the very first pass (and
  // whenever invalidate() was called) the whole shell is laid down at once.
  const parts = layoutParts();
  if (!lastRegionHtml.size) {
    app.innerHTML = shell();
    bind();
    lastRegionHtml.clear();
  }

  const scroll = document.querySelector("main")?.scrollTop ?? 0;
  const focus = captureFocus();

  for (const [id, html] of Object.entries(parts)) {
    if (lastRegionHtml.get(id) === html) continue;
    lastRegionHtml.set(id, html);
    const node = el("r-" + id);
    if (node) node.innerHTML = html;
  }

  const m = document.querySelector("main");
  if (m) m.scrollTop = scroll;
  restoreFocus(focus);

  if (state.tab === "quarantine" && !state.batches.length && !state.quarantineLoaded) {
    state.quarantineLoaded = true;
    actions.refreshQuarantine();
  }
}

/** The static skeleton, built once. Every region lives inside it. */
function shell() {
  return h`
    <div class="titlebar">
      <div class="brand"><div class="logo"></div></div>
    </div>
    <div id="r-elevbanner"></div>
    <div class="tabs" id="r-tabs"></div>
    <div id="r-progress"></div>
    <main id="r-main"></main>
    <div id="r-actionbar"></div>
    <div id="r-modal"></div>
    <div id="r-toast"></div>
  `;
}

/** The HTML for each region, given the current state. */
function layoutParts() {
  return {
    elevbanner: elevationBanner(),
    tabs: tabBar(),
    progress: state.scanning ? progressBlock() : "",
    main: state.tab === "overview" ? overviewView()
      : state.tab === "findings" ? findingsView()
      : state.tab === "settings" ? settingsView() : quarantineView(),
    actionbar: state.tab === "findings" && state.selected.size ? actionBar() : "",
    modal: state.preview ? previewModal() : "",
    toast: state.toast ? `<div class="toast ${esc(state.toast.kind)}">${esc(state.toast.msg)}</div>` : "",
  };
}

function tabBar() {
  return h`
    ${tabBtn("overview", T("tab.overview"))}
    ${tabBtn("findings", T("tab.findings"), state.findings.length || 0)}
    ${tabBtn("quarantine", T("tab.quarantine"), state.batches.length || 0, state.expiry?.expiredItems > 0)}
    ${tabBtn("settings", T("tab.settings"))}
  `;
}

/** Shown only when a cleanup was refused by the filesystem for permissions. */
function elevationBanner() {
  if (!state.elevateRequest) return "";
  return h`
    <div class="elevbanner">
      <div>
        <b>${T("elev.title")}</b>
        <div class="faint" style="font-size:11px;margin-top:3px">
          ${T("elev.desc")}
        </div>
      </div>
      <div style="display:flex;gap:8px;flex:0 0 auto">
        <button data-act="dismiss-elevate">${T("elev.later")}</button>
        <button class="primary" data-act="retry-elevated">${T("elev.retry")}</button>
      </div>
    </div>`;
}

function tabBtn(id, label, count, warn) {
  return `<button class="tab ${state.tab === id ? "active" : ""}" data-act="tab" data-tab="${id}">
    ${label}${count ? `<span class="badge ${warn ? "warn" : ""}">${count}</span>` : ""}
  </button>`;
}

// ---------------- Overview ----------------
function overviewView() {
  const totals = categoryTotals();
  const reclaimable = totals.reduce((s, t) => s + t.onDisk, 0);

  return h`
    <h2 class="section">${T("ov.disk")}</h2>
    <div class="vol-grid mb">
      ${state.volumes.map(volCard).join("") || `<div class="empty"><div class="big">◍</div><p>${T("ov.noDisk")}</p></div>`}
    </div>

    <h2 class="section">${T("ov.reclaimable")} <span class="hint">${state.findings.length ? T("ov.fromScan") : T("ov.notScanned")}</span></h2>
    <div class="stat-row">
      ${totals.length ? totals.map(t => `
        <div class="stat" data-act="goto-cat" data-cat="${t.category}">
          <div class="k"><span class="dot ${RISK_DOT[riskOf(t.category)]}"></span>${esc(categoryLabel(t.category))}</div>
          <div class="v">${fmtBytes(t.onDisk)}</div>
          <div class="n">${T("ov.itemCount", { count: t.count, logical: fmtBytes(t.logical) })}</div>
        </div>`).join("")
      : `<div class="empty" style="grid-column:1/-1"><div class="big">◎</div><p>${T("ov.startHint")}</p></div>`}
    </div>

    ${bigFileRollup()}

    <h2 class="section mt">${T("ov.cleanup")}</h2>
    <div class="card">
      <div class="toolbar" style="margin-bottom:0">
        <button class="primary" data-act="scan" data-cats="temp_junk,system_cache,dev_artifact,app_leftover" ${state.scanning ? "disabled" : ""}>${T("ov.quick")}</button>
        <button data-act="scan" data-cats="temp_junk,system_cache,dev_artifact,app_leftover,large_file" ${state.scanning ? "disabled" : ""}>${T("ov.deep")}</button>
        ${state.scanning ? `<div class="faint" style="font-size:11px;margin-top:8px">
          ${T("ov.scanning")}</div>` : ""}
      </div>
      <div class="faint" style="font-size:11px;margin-top:10px">
        ${T("ov.deepHint")}
      </div>
    </div>
  `;
}

function riskOf(cat) {
  return { temp_junk: "safe", system_cache: "safe", dev_artifact: "rebuildable", large_file: "personal", app_leftover: "userdata" }[cat];
}

/**
 * "Who is eating the disk" rollup.
 *
 * Only shown once a deep scan has actually found large files; before that it
 * would be an empty heading taking up space.
 */
function bigFileRollup() {
  const dirs = dirTotals();
  if (!dirs.length) return "";
  const total = dirs.reduce((s, d) => s + d.onDisk, 0);
  const top = dirs.slice(0, 12);
  const open = state.openDir;

  return h`
    <h2 class="section mt">${T("roll.title")} <span class="hint">${T("roll.total", { total: fmtBytes(total) })}</span></h2>
    <div class="card mb">
      <table>
        <thead><tr>
          <th style="width:26px"></th><th>${T("roll.folder")}</th>
          <th style="width:80px" class="right">${T("roll.fileCount")}</th>
          <th style="width:110px" class="right">${T("roll.used")}</th>
        </tr></thead>
        <tbody>
          ${top.map(d => h`<tr class="${open === d.dir ? "selected" : ""}">
            <td><span class="chev ${open === d.dir ? "" : "collapsed"}">▼</span></td>
            <td class="path" data-act="drill-dir" data-dir="${esc(d.dir)}" title="${esc(d.dir)}">${esc(d.dir)}</td>
            <td class="num">${d.count}</td>
            <td class="num">${fmtBytes(d.onDisk)}</td>
          </tr>`).join("")}
        </tbody>
      </table>
      ${dirs.length > top.length ? `<div class="faint" style="font-size:11px;padding:8px 12px">${T("roll.more", { n: dirs.length - top.length })}</div>` : ""}
      ${open ? drillTable(open) : `<div class="faint" style="font-size:11px;padding:8px 12px">${T("roll.hint")}</div>`}
    </div>`;
}

/** The files inside one rolled-up directory. */
function drillTable(dir) {
  const items = findingsInDir(dir);
  return h`
    <div class="drill">
      <div class="drill-head">
        <span class="mono faint" style="font-size:11px">${esc(dir)}</span>
        <button class="sm" data-act="drill-dir" data-dir="">${T("drill.collapse")}</button>
      </div>
      <table>
        <thead><tr>
          <th style="width:28px"></th><th>${T("drill.file")}</th>
          <th style="width:90px" class="right">${T("drill.logical")}</th>
          <th style="width:100px" class="right">${T("drill.onDisk")}</th>
          <th style="width:70px" class="right">${T("drill.age")}</th>
        </tr></thead>
        <tbody>
          ${items.map(f => h`<tr>
            <td><input type="checkbox" data-act="sel" data-id="${esc(f.id)}" ${state.selected.has(f.id) ? "checked" : ""} /></td>
            <td class="name" title="${esc(f.path)}" data-act="reveal" data-path="${esc(f.path)}">${esc(f.name)}</td>
            <td class="num">${fmtBytes(f.logicalSize)}</td>
            <td class="num">${fmtBytes(f.onDiskSize)}</td>
            <td class="num">${fmtAge(f.ageDays)}</td>
          </tr>`).join("")}
        </tbody>
      </table>
    </div>`;
}

function volCard(v) {
  const pct = Math.round((v.usedRatio || 0) * 100);
  const freePct = v.totalBytes ? (v.freeBytes / v.totalBytes) * 100 : 0;
  const cls = freePct < 10 ? "crit" : freePct < 20 ? "warn" : "";
  return h`
    <div class="vol">
      <div class="vol-head">
        <div class="vol-name">${esc(v.mount)}</div>
        <div class="vol-label">${esc(v.label)}</div>
      </div>
      <div class="vol-nums">
        <div class="vol-free ${cls}">${fmtBytes(v.freeBytes)}</div>
        <div class="vol-total">${T("vol.available", { total: fmtBytes(v.totalBytes) })}</div>
      </div>
      <div class="bar"><div class="used ${cls}" style="width:${pct}%"></div></div>
      <div class="vol-legend"><span>${T("vol.used", { used: fmtBytes(v.usedBytes) })}</span><span>${pct}%</span></div>
    </div>`;
}

// ---------------- Progress ----------------
function progressBlock() {
  const p = state.progress || {};
  return h`
    <div id="progress-live" class="progress-wrap" style="padding:12px 20px 0">
      <div class="progress-bar"><div class="fill"></div></div>
      <div class="progress-text">
        <span id="pg-label">${state.cancelling ? T("prog.cancelling") : T("prog.scanning")}</span> ${T("prog.line", {
          dirs: `<span id="pg-dirs">${p.dirsScanned || 0}</span>`,
          bytes: `<span id="pg-bytes">${fmtBytes(p.bytesScanned || 0)}</span>`,
          found: `<span id="pg-found">${p.findings || 0}</span>`,
        })}
        ${state.cancelling ? "" : `<button class="ghost sm" data-act="cancel" style="margin-left:10px">${T("prog.cancel")}</button>`}
      </div>
    </div>`;
}

// ---------------- Findings ----------------
function findingsView() {
  if (!state.findings.length) {
    // A running scan has no findings yet, but it is emphatically not "nothing
    // scanned" - showing the start button there is what made the tab look
    // like it had reset itself.
    if (state.scanning) {
      return h`<div class="empty">
        <div class="big">◍</div>
        <p>${T("find.scanning")}</p>
        <div class="faint" style="font-size:11px;margin-top:6px">${T("find.switchHint")}</div>
        <div class="mt">${state.cancelling ? `<span class="faint" style="font-size:11px">${T("prog.cancelling")}</span>` : `<button data-act="cancel">${T("find.cancelScan")}</button>`}</div>
      </div>`;
    }
    return h`<div class="empty"><div class="big">◍</div><p>${T("find.noResults")}</p>
      <div class="mt"><button class="primary" data-act="scan" data-cats="temp_junk,system_cache,dev_artifact,app_leftover">${T("find.start")}</button></div></div>`;
  }

  const cats = categoryTotals();
  const filtered = filteredFindings();

  // Group the visible findings by category in one pass. Doing it per category
  // meant a five-category table walked the whole finding list five times, and
  // this runs on every keystroke in the search box.
  const byCat = new Map();
  for (const f of filtered) {
    let bucket = byCat.get(f.category);
    if (!bucket) { bucket = []; byCat.set(f.category, bucket); }
    bucket.push(f);
  }

  return h`
    <div class="toolbar">
      <input type="search" placeholder="${T("find.search")}" value="${esc(state.query)}" data-act="search" />
      <select data-act="risk">
        <option value="all" ${state.riskFilter === "all" ? "selected" : ""}>${T("find.allRisk")}</option>
        <option value="safe" ${state.riskFilter === "safe" ? "selected" : ""}>${riskLabelOf("safe")}</option>
        <option value="rebuildable" ${state.riskFilter === "rebuildable" ? "selected" : ""}>${riskLabelOf("rebuildable")}</option>
        <option value="personal" ${state.riskFilter === "personal" ? "selected" : ""}>${riskLabelOf("personal")}</option>
        <option value="userdata" ${state.riskFilter === "userdata" ? "selected" : ""}>${riskLabelOf("userdata")}</option>
      </select>
      <button class="sm" data-act="selall" data-on="1">${T("find.selectAll")}</button>
      <button class="sm" data-act="selall" data-on="0">${T("find.selectNone")}</button>
      <div class="spacer"></div>
      <span class="faint" style="font-size:11px" data-role="shown-count">${T("find.shown", { shown: filtered.length, total: state.findings.length })}</span>
    </div>
    ${cats.map(c => catBlock(c, byCat.get(c.category) || [])).join("")}
  `;
}

/** Direction arrow shown beside the currently sorted column. */
function sortMark(key) {
  if (state.sortKey !== key) return "";
  return state.sortDir < 0 ? " ▲" : " ▼";
}

/** One category's table. `items` is already filtered and grouped. */
function catBlock(total, items) {
  if (!items.length) return "";
  const collapsed = state.collapsed.has(total.category);
  // A deep scan can surface hundreds of thousands of findings; render a
  // bounded window and page on demand. The header checkbox still operates on
  // the whole filtered set, and the aggregate counts stay exact.
  const cap = state.limit.get(total.category) || PAGE_SIZE;
  const visible = items.slice(0, cap);
  const allSel = items.every(f => state.selected.has(f.id));
  const someSel = items.some(f => state.selected.has(f.id));

  return h`
    <div class="cat-block">
      <div class="cat-head ${collapsed ? "collapsed" : ""}" data-act="collapse" data-cat="${total.category}">
        <span class="chev">▼</span>
        <input type="checkbox" data-act="cat-sel" data-cat="${total.category}" data-on="${allSel ? 0 : 1}" ${someSel ? 'checked' : ""} />
        <span class="dot ${RISK_DOT[riskOf(total.category)]}"></span>
        <span>${categoryLabel(total.category)}</span>
        <span class="faint" style="font-weight:400">${T("col.items", { n: total.count })}</span>
        <span class="size">${fmtBytes(total.onDisk)}</span>
      </div>
      ${collapsed ? "" : `<table>
        <thead><tr>
          <th style="width:28px"></th>
          <th data-act="sort" data-key="name" title="${T("col.sort")}">${T("col.name")}${sortMark("name")}</th>
          <th style="width:90px" data-act="sort" data-key="size" title="${T("col.sort")}">${T("col.size")}${sortMark("size")}</th>
          <th style="width:110px" data-act="sort" data-key="onDiskSize" title="${T("col.sort")}">${T("col.onDisk")}${sortMark("onDiskSize")}</th>
          <th style="width:70px" data-act="sort" data-key="ageDays" title="${T("col.sort")}">${T("col.age")}${sortMark("ageDays")}</th>
          <th style="width:90px" data-act="sort" data-key="lastAccessUnix" title="${T("col.sort")}">${T("col.lastAccess")}${sortMark("lastAccessUnix")}</th>
          <th style="width:110px">${T("col.disposition")}</th>
        </tr></thead>
        <tbody>${visible.map(f => row(f)).join("")}</tbody>
      </table>`}
      ${!collapsed && items.length > visible.length ? `<div class="more-row">
        <button class="sm" data-act="more" data-cat="${total.category}">${T("col.more", { shown: visible.length, total: items.length })}</button>
      </div>` : ""}
    </div>`;
}

function row(f) {
  const sel = state.selected.has(f.id);
  const d = effectiveDisposition(f);
  const overridden = state.overrides.has(f.id);
  return h`
    <tr class="${sel ? "selected" : ""}" data-id="${esc(f.id)}" data-cat="${esc(f.category)}">
      <td><input type="checkbox" data-act="sel" data-id="${esc(f.id)}" ${sel ? "checked" : ""} /></td>
      <td class="name" title="${esc(f.path)}" data-act="reveal" data-path="${esc(f.path)}">${esc(f.name)}</td>
      <td class="num">${fmtBytes(f.logicalSize)}</td>
      <td class="num">${fmtBytes(f.onDiskSize)}</td>
      <td class="num">${fmtAge(f.ageDays)}</td>
      <td class="num faint">${fmtWhen(f.lastAccessUnix)}</td>
      <td>
        <select data-act="dispo" data-id="${esc(f.id)}" class="dispo">
          ${f.category === "temp_junk" || f.category === "system_cache"
            ? `<option value="purge" ${d === "purge" ? "selected" : ""}>${T("disp.purge")}</option>`
            : `<option value="" ${!overridden ? "selected" : ""}>${T("disp.default")}</option>
               <option value="purge" ${d === "purge" ? "selected" : ""}>${T("disp.purge")}</option>
               <option value="quarantine" ${d === "quarantine" ? "selected" : ""}>${T("disp.quarantine")}</option>`}
        </select>
      </td>
    </tr>`;
}

function actionBar() {
  const t = selectionTotals();
  return h`
    <div class="actionbar">
      <div class="info">
        ${T("bar.selected", { n: `<b>${t.count}</b>`, logical: fmtBytes(t.logical) })}
        <span class="arrow">→</span>
        <span class="gain">${T("bar.reclaimable", { size: fmtBytes(t.onDisk) })}</span>
      </div>
      <div class="spacer"></div>
      <span class="faint" style="font-size:11px">${T("bar.disposition")}</span>
      <select data-act="batch-dispo">
        <option value="default" ${state.batchDisposition === "default" ? "selected" : ""}>${T("bar.byCategory")}</option>
        <option value="split" ${state.batchDisposition === "split" ? "selected" : ""}>${T("bar.split")}</option>
        <option value="purge" ${state.batchDisposition === "purge" ? "selected" : ""}>${T("bar.purgeAll")}</option>
        <option value="quarantine" ${state.batchDisposition === "quarantine" ? "selected" : ""}>${T("bar.quarantineAll")}</option>
      </select>
      <button class="primary" data-act="preview">${T("bar.clean")}</button>
    </div>`;
}

// ---------------- Preview modal ----------------
function previewModal() {
  const pv = state.preview;
  const hasPurge = pv.groups.some(g => g.disposition === "purge");
  return h`
    <div class="overlay" data-act="close-preview">
      <div class="modal">
        <div class="modal-head">
          <h3>${T("prev.title")}</h3>
          <p>${hasPurge ? T("prev.irreversible") : T("prev.restorable")}</p>
        </div>
        <div class="modal-body">
          ${pv.groups.map(g => previewGroup(g, pv.items)).join("")}
          ${pv.needsRed ? `<div class="checkline">
            <input type="checkbox" data-act="red-confirm" ${pv.confirmed ? "checked" : ""} />
            <label>${T("prev.redConfirm")}</label>
          </div>` : ""}
        </div>
        <div class="modal-foot">
          <div class="left faint" style="font-size:11px">
            ${hasPurge ? T("prev.noQuarantine") : ""}
          </div>
          <button data-act="export-preview" title="${T("prev.exportTitle", { n: pv.items.length })}">${T("prev.export")}</button>
          <button data-act="close-preview">${T("prev.cancel")}</button>
          <button class="${hasPurge ? "danger" : "primary"}" data-act="do-cleanup"
            ${pv.busy || (pv.needsRed && !pv.confirmed) ? "disabled" : ""}>
            ${pv.busy ? T("prev.busy") : hasPurge ? T("prev.purgeBtn") : T("prev.quarantineBtn")}
          </button>
        </div>
      </div>
    </div>`;
}

function previewGroup(g, items) {
  const isPurge = g.disposition === "purge";
  const groupItems = items.filter(i => i.disposition === g.disposition);
  const shown = groupItems.slice(0, 8);
  const catText = Object.entries(g.categoryCounts).map(([k, v]) => `${v} ${esc(categoryLabel(k))}`).join(" · ");
  return h`
    <div class="pgroup ${isPurge ? "purge" : ""}">
      <h4>
        ${isPurge ? T("prev.purge") : T("prev.quarantine")}
        <span class="dim" style="font-weight:400">${T("col.items", { n: g.itemCount })}</span>
        <span class="spacer" style="flex:1"></span>
        <span class="dim" style="font-weight:400">${fmtBytes(g.onDiskBytes)}</span>
      </h4>
      <div class="meta">${catText}${g.logicalBytes !== g.onDiskBytes ? T("prev.metaLogical", { size: fmtBytes(g.logicalBytes) }) : ""}</div>
      ${g.containsRedPurge ? `<div class="warn-note">${T("prev.warnRed")}</div>` : ""}
      <div class="list">
        ${shown.map(i => `<div title="${esc(i.finding.path)}">${esc(i.finding.path)}</div>`).join("")}
        ${groupItems.length > shown.length ? `<div class="faint">${T("prev.more", { n: groupItems.length - shown.length })}</div>` : ""}
      </div>
    </div>`;
}

// ---------------- Quarantine ----------------
function quarantineView() {
  const exp = state.expiry;
  return h`
    ${exp && exp.expiredItems > 0 ? h`
      <div class="banner">
        <span class="icon">⚠</span>
        <span>${T("q.banner", { n: `<b>${exp.expiredItems}</b>`, days: exp.expiryDays, size: `<b>${fmtBytes(exp.expiredOnDiskBytes)}</b>` })}</span>
        <div class="spacer" style="flex:1"></div>
        <button class="sm danger" data-act="purge-expired">${T("q.purgeExpired")}</button>
      </div>` : ""}

    ${state.batches.length ? state.batches.map(batchBlock).join("")
      : `<div class="empty"><div class="big">◌</div><p>${T("q.empty")}</p>
         <p style="margin-top:6px">${T("q.emptyHint")}</p></div>`}
  `;
}

function batchBlock(b) {
  const open = state.openBatch === b.dir;
  const expired = isExpired(b);
  return h`
    <div class="batch">
      <div class="batch-head" data-act="open-batch" data-dir="${esc(b.dir)}">
        <span class="chev">${open ? "▼" : "▶"}</span>
        <span class="id">${esc(b.batchId)}</span>
        <span class="dim">${T("col.items", { n: b.itemCount })}</span>
        <span class="dim">${T("age.ago", { n: fmtAge(Math.floor(ageDays(b))) })}</span>
        ${expired ? `<span class="tag personal">${T("q.expired")}</span>` : ""}
        <div class="spacer"></div>
        <span class="dim">${T("q.logical", { size: fmtBytes(b.logicalBytes) })}</span>
        <span style="color:var(--accent);font-weight:600">${T("q.used", { size: fmtBytes(b.onDiskBytes) })}</span>
      </div>
      ${open ? batchBody(b) : ""}
    </div>`;
}

function batchBody(b) {
  const items = state.batchItems || [];
  return h`
    <div class="batch-body">
      <div class="toolbar" style="margin-bottom:8px">
        <button class="sm" data-act="restore-batch" data-dir="${esc(b.dir)}" data-policy="skip">${T("q.restoreSkip")}</button>
        <button class="sm" data-act="restore-batch" data-dir="${esc(b.dir)}" data-policy="overwrite">${T("q.restoreOverwrite")}</button>
        <div class="spacer"></div>
        <button class="sm danger" data-act="purge-batch" data-dir="${esc(b.dir)}">${T("q.purgeBatch")}</button>
      </div>
      ${items.length ? `<table>
        <thead><tr><th>${T("q.colPath")}</th><th style="width:90px">${T("col.size")}</th><th style="width:90px">${T("q.colAction")}</th></tr></thead>
        <tbody>${items.map(e => h`<tr>
          <td class="path" title="${esc(e.originalPath)}">${esc(e.originalPath)}</td>
          <td class="num">${fmtBytes(e.logicalSize)}</td>
          <td class="right">
            <button class="sm" data-act="restore-item" data-dir="${esc(b.dir)}" data-path="${esc(e.originalPath)}" data-policy="skip">${T("q.restore")}</button>
          </td>
        </tr>`).join("")}</tbody>
      </table>` : `<div class="faint" style="font-size:11px;padding:6px 0">${T("q.loading")}</div>`}
    </div>`;
}

// ---------------- Settings ----------------
function settingsView() {
  const c = state.config || {};
  const log = state.auditLog || [];
  return h`
    <h2 class="section">${T("set.lang")}</h2>
    <div class="card mb">
      <div class="toolbar" style="margin-bottom:0">
        <select data-act="lang">
          <option value="zh" ${state.lang === "zh" ? "selected" : ""}>${T("lang.zh")}</option>
          <option value="en" ${state.lang === "en" ? "selected" : ""}>${T("lang.en")}</option>
        </select>
        <span class="faint" style="font-size:11px">${T("set.instant")}</span>
      </div>
    </div>

    <h2 class="section">${T("set.rules")}</h2>
    <div class="card mb">
      <div class="setrow">
        <div>
          <div class="setlabel">${T("set.largeAge")}</div>
          <div class="faint" style="font-size:11px">${T("set.largeAgeHint")}</div>
        </div>
        <input type="number" min="0" max="3650" value="${c.largeFileAgeDays ?? 30}" data-act="set" data-key="largeFileAgeDays" />
      </div>
      <div class="setrow">
        <div>
          <div class="setlabel">${T("set.leftoverAge")}</div>
          <div class="faint" style="font-size:11px">${T("set.leftoverAgeHint")}</div>
        </div>
        <input type="number" min="0" max="3650" value="${c.leftoverAgeDays ?? 30}" data-act="set" data-key="leftoverAgeDays" />
      </div>
      <div class="setrow">
        <div>
          <div class="setlabel">${T("set.largeMin")}</div>
          <div class="faint" style="font-size:11px">${T("set.largeMinHint")}</div>
        </div>
        <input type="number" min="1" max="102400" value="${Math.round((c.largeFileMinBytes ?? 524288000) / 1048576)}" data-act="setmb" data-key="largeFileMinBytes" />
      </div>
      <div class="setrow">
        <div>
          <div class="setlabel">${T("set.expiry")}</div>
          <div class="faint" style="font-size:11px">${T("set.expiryHint")}</div>
        </div>
        <input type="number" min="1" max="3650" value="${c.quarantineExpiryDays ?? 7}" data-act="set" data-key="quarantineExpiryDays" />
      </div>
      <div class="toolbar mt" style="margin-bottom:0">
        <button class="primary" data-act="save-settings">${T("set.save")}</button>
        <span class="faint" style="font-size:11px">${T("set.instant")}</span>
      </div>
    </div>

    <h2 class="section">${T("set.location")}</h2>
    <div class="card mb">
      <div class="mono faint" style="font-size:11px;word-break:break-all">${esc(c.quarantineDir || "")}</div>
      <div class="faint mt" style="font-size:11px">
        ${T("set.locationHint")}
      </div>
    </div>

    <h2 class="section">${T("set.log")} <span class="hint">${T("set.logHint")}</span></h2>
    <div class="card">
      ${log.length ? `<table>
        <thead><tr><th style="width:70px">${T("set.colTime")}</th><th style="width:110px">${T("set.colAction")}</th><th>${T("set.colPath")}</th><th style="width:80px" class="right">${T("set.colSize")}</th><th style="width:150px">${T("set.colResult")}</th></tr></thead>
        <tbody>${log.slice(0, 100).map(e => h`<tr>
          <td class="faint nowrap">${fmtWhen(e.unix)}</td>
          <td class="nowrap">${esc(T("log." + e.action))}</td>
          <td class="path" title="${esc(e.path)}">${esc(e.path)}</td>
          <td class="num">${fmtBytes(e.bytes)}</td>
          <td class="${e.outcome === "成功" ? "" : "faint"}">${esc(T(e.outcome))}</td>
        </tr>`).join("")}</tbody>
      </table>` : `<div class="empty" style="padding:24px"><p>${T("set.logEmpty")}</p></div>`}
      ${state.auditPath ? `<div class="faint mono mt" style="font-size:11px;word-break:break-all">${T("set.logFile", { path: state.auditPath })}</div>` : ""}
    </div>
  `;
}

// ---------------- Event binding ----------------
let bound = false;

/**
 * Attach the delegated listeners exactly once.
 *
 * render() rewrites the whole tree on every state change, and re-binding each
 * time stacked a fresh copy of every handler on the same root element. After a
 * few hundred renders a single click ran hundreds of actions, which is what
 * made the window appear to freeze during a scan.
 */
function bind() {
  if (bound) return;
  bound = true;
  const root = el("app");

  root.addEventListener("click", (e) => {
    const t = e.target.closest("[data-act]");
    if (!t) return;
    const act = t.dataset.act;
    if (act === "tab") actions.setTab(t.dataset.tab);
    else if (act === "scan") {
      if (t.disabled) return;
      actions.startScan(t.dataset.cats.split(","));
    }
    else if (act === "cancel") actions.cancelScan();
    else if (act === "sel") actions.toggleSel(t.dataset.id);
    else if (act === "selall") actions.selAll(t.dataset.on === "1");
    else if (act === "collapse") actions.toggleCollapse(t.dataset.cat);
    else if (act === "more") actions.moreRows(t.dataset.cat);
    else if (act === "sort") actions.sortBy(t.dataset.key);
    else if (act === "reveal") actions.reveal(t.dataset.path);
    else if (act === "cat-sel") actions.toggleCat(t.dataset.cat, t.dataset.on === "1");
    else if (act === "drill-dir") actions.toggleDir(t.dataset.dir || null);
    else if (act === "export-preview") actions.copyPreview();
    else if (act === "retry-elevated") actions.retryElevated();
    else if (act === "dismiss-elevate") actions.dismissElevate();
    else if (act === "preview") actions.openPreview();
    else if (act === "close-preview") {
      // Two different things share this action: the overlay (click-away) and
      // the footer's 取消 button. Only the overlay needs the target check,
      // because the dialog is its child - a click that lands on the dialog,
      // or on the button, must not be read as a click outside.
      //
      // The overlay used to carry an inline onclick with stopPropagation,
      // which killed every button in the dialog along with it: the app could
      // not confirm, cancel, or export anything.
      const isOverlay = t.classList.contains("overlay");
      if (!isOverlay || e.target === t) actions.closePreview();
    }
    else if (act === "do-cleanup") actions.doCleanup();
    else if (act === "open-batch") actions.loadBatchItems(t.dataset.dir);
    else if (act === "restore-batch") actions.restoreBatch(t.dataset.dir, t.dataset.policy);
    else if (act === "restore-item") actions.restoreItem(t.dataset.dir, t.dataset.path, t.dataset.policy);
    else if (act === "purge-batch") {
      if (confirm(T("q.purgeBatchConfirm"))) actions.purgeBatch(t.dataset.dir);
    }
    else if (act === "purge-expired") {
      if (confirm(T("q.purgeExpiredConfirm"))) actions.purgeExpired();
    }
    else if (act === "save-settings") actions.saveSettings(collectSettings());
    else if (act === "goto-cat") {
      // Jump to the findings tab, filtered to this one category's risk tier.
      // The two branches used to be identical ("all" either way), which
      // made every category card land on the same unfiltered list.
      state.riskFilter = riskOf(t.dataset.cat);
      state.query = "";
      actions.setTab("findings");
    }
  });

  root.addEventListener("change", (e) => {
    const t = e.target.closest("[data-act]");
    if (!t) return;
    const act = t.dataset.act;
    if (act === "search") actions.setQuery(t.value);
    else if (act === "risk") actions.setRiskFilter(t.value);
    else if (act === "batch-dispo") actions.setBatchDisposition(t.value);
    else if (act === "dispo") actions.setOverride(t.dataset.id, t.value || null);
    else if (act === "red-confirm") actions.setRedConfirm(t.checked);
    else if (act === "lang") actions.setLang(t.value);
    // "sel" and "cat-sel" are deliberately absent: they are checkboxes, and
    // the click listener already handles them. Handling both made every tick
    // fire twice - on, then off again - so a row could not be selected at all.
  });

  root.addEventListener("input", (e) => {
    const t = e.target.closest("[data-act='search']");
    if (t) actions.setQuery(t.value);
  });
}

/** Gather the settings form into a patch object. */
function collectSettings() {
  const patch = {};
  for (const el of document.querySelectorAll("[data-act='set']")) {
    patch[el.dataset.key] = parseInt(el.value, 10) || 0;
  }
  for (const el of document.querySelectorAll("[data-act='setmb']")) {
    patch[el.dataset.key] = (parseInt(el.value, 10) || 0) * 1048576;
  }
  return patch;
}

export async function boot() {
  // Held rather than discarded: if boot() is ever called twice (a reload
  // racing the first one), the old listener would double every progress
  // event instead of being replaced.
  unlistenProgress = await listen("scan-progress", (e) => actions.onProgress(e.payload));
  await render();
  await actions.loadOverview();
}

/** Detach the scan-progress listener. Used by tests and on teardown. */
export function dispose() {
  if (unlistenProgress) { unlistenProgress(); unlistenProgress = null; }
}
let unlistenProgress = null;
