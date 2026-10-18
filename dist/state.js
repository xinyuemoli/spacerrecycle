import { invoke, listen, hasBackend } from "./api.js";
import { render, renderSelectionOnly } from "./ui.js";
import { t, catLabel, riskLabel, detectLang } from "./i18n.js";

const state = {
  tab: "overview",
  volumes: [],
  findings: [],
  selected: new Set(),
  overrides: new Map(),   // findingId -> "purge" | "quarantine"
  batchDisposition: "default",
  elevateRequest: null,   // cleanup that hit a permission wall
  query: "",
  riskFilter: "all",
  sortKey: null,          // active sort column; null = backend order
  sortDir: -1,            // -1 = descending, 1 = ascending
  collapsed: new Set(),
  limit: new Map(),       // category -> how many rows to render (pagination)
  openDir: null,          // large-file directory currently drilled into
  batches: [],
  expiry: null,
  scanning: false,
  cancelling: false,
  progress: null,
  toast: null,
  config: null,
  lang: detectLang(),       // "zh" | "en"
  /**
   * User-assigned importance marks, keyed by normalized path.
   *
   * Kept beside the findings rather than on them: the backend owns the store
   * (so the cleanup gate can enforce it server-side), and a re-scan rebuilds
   * every finding from scratch. Re-reading the map after a scan is what makes
   * a mark survive one.
   */
  marks: new Map(),         // normalized path -> "important" | "disposable"
  importanceFilter: "all",  // "all" | level, for the findings toolbar
  menu: null,               // { path, name, level, x, y } right-click menu anchor
  entitlement: null,        // subscription state, loaded when Settings opens
  guard: null,              // delete-guard disposition + whether it is running
  /**
   * OneDrive account + cloud quota.
   *
   * `null` until asked; `{ connected, quota, detail }` afterwards. This is the
   * account the app itself is linked to, and it is what the OneDrive
   * quarantine target needs: nothing is uploaded anywhere the user has not
   * signed in to.
   */
  onedriveAccount: null,
  /** A sign-in in progress: { userCode, verificationUri, intervalSecs, status, detail }. */
  connect: null,
  /**
   * Cloud backup batches, newest first.
   *
   * `null` until asked. An empty array is the normal state for a machine with
   * no linked account, so it is not treated as an error.
   */
  cloudBackups: null,
  /** Batch currently being restored or deleted, so its buttons can disable. */
  cloudBusy: null,
};

export { state };

/** Minimum gap between progress repaints, in ms. */
const PROGRESS_THROTTLE_MS = 120;

/**
 * How many rows of one category the findings table renders before it pages.
 *
 * A deep scan can surface hundreds of thousands of findings; building one DOM
 * node per row (and re-building on every keystroke) would OOM or stall the
 * window. Rendering a bounded window keeps the table responsive while the
 * aggregate counts stay exact because they are computed over the full list.
 */
export const PAGE_SIZE = 400;
let progressTimer = null;

/**
 * The device-code poll timer.
 *
 * Owned here rather than by the view: the view is rebuilt with innerHTML on
 * every state change, so a timer held by it would be orphaned and keep polling
 * forever.
 */
let pollTimer = null;

function clearPoll() {
  if (pollTimer !== null) {
    clearTimeout(pollTimer);
    pollTimer = null;
  }
}

function schedulePoll(seconds) {
  clearPoll();
  pollTimer = setTimeout(() => {
    pollTimer = null;
    actions.pollConnect();
  }, Math.max(1, seconds) * 1000);
}

/** Stop any in-flight device-code polling. Used on teardown. */
export function stopPolling() {
  clearPoll();
}

/**
 * Update only the progress readout.
 *
 * Falls back to a full render when the panel is not on screen, so switching
 * to the tab mid-scan still shows correct numbers.
 */
function paintProgress() {
  const node = document.getElementById("progress-live");
  if (!node || !state.progress) {
    // The progress bar is not on screen: rebuild so the panel appears (or
    // disappears) with the right numbers for the tab we just switched to.
    if (state.scanning && (state.tab === "findings" || state.tab === "overview")) render();
    return;
  }
  const p = state.progress;
  const set = (id, v) => {
    const el = document.getElementById(id);
    if (el && el.textContent !== v) el.textContent = v;
  };
  set("pg-dirs", String(p.dirsScanned ?? 0));
  set("pg-bytes", fmtBytes(p.bytesScanned ?? 0));
  set("pg-found", String(p.findings ?? 0));
  // The label lives inside the same block as the cancel button, so it has to
  // follow the cancelling flag as well. Without this the progress bar keeps
  // saying "正在扫描" after the user hit cancel, with no button left to press.
  const label = document.getElementById("pg-label");
  const want = state.cancelling ? t(state.lang, "prog.cancelling") : t(state.lang, "prog.scanning");
  if (label && label.textContent !== want) label.textContent = want;
}
export function getState() { return state; }

// ---------- formatting ----------
//
// These three run several times per table row, so at a few thousand rows they
// are a measurable share of a render. All are pure functions of a number, and
// the set of distinct values in one scan is small (sizes cluster, dates repeat
// heavily), so a bounded cache turns most calls into a Map lookup.
const fmtCache = new Map();

function cached(key, compute) {
  const hit = fmtCache.get(key);
  if (hit !== undefined) return hit;
  const value = compute();
  // Bounded so a long session cannot grow this without limit.
  if (fmtCache.size > 20000) fmtCache.clear();
  fmtCache.set(key, value);
  return value;
}

export function fmtBytes(n) {
  if (n === 0 || n == null) return "0 B";
  return cached("b" + n, () => {
    const u = ["B", "KB", "MB", "GB", "TB"];
    const i = Math.min(Math.floor(Math.log(Math.abs(n)) / Math.log(1024)), u.length - 1);
    const v = n / Math.pow(1024, i);
    return (i === 0 ? v : v.toFixed(v >= 100 ? 0 : v >= 10 ? 1 : 2)) + " " + u[i];
  });
}

export function fmtAge(days) {
  if (days == null) return "-";
  // Defensive: a "no gate / unknown" sentinel must never surface as a giant
  // number of years (u32::MAX/365 ≈ 11767033.7y).
  if (days >= 4294967295) return "-";
  return cached("a" + state.lang + days, () => {
    if (days < 1) return t(state.lang, "age.today");
    if (days < 30) return t(state.lang, "age.days", { n: days });
    if (days < 365) return t(state.lang, "age.months", { n: Math.floor(days / 30) });
    return t(state.lang, "age.years", { n: (days / 365).toFixed(1) });
  });
}

export function fmtWhen(unix) {
  if (!unix) return "-";
  // Bucketed by day: the rendered value only changes at midnight, and a table
  // full of rows would otherwise construct a Date per cell.
  const day = Math.floor(unix / 86400);
  const locale = state.lang === "zh" ? "zh-CN" : "en-US";
  return cached("w" + state.lang + day, () => new Date(unix * 1000).toLocaleDateString(locale));
}

export function toast(msg, kind = "") {
  state.toast = { msg, kind, at: Date.now() };
  render();
  setTimeout(() => {
    if (state.toast && Date.now() - state.toast.at >= 3400) {
      state.toast = null;
      render();
    }
  }, 3500);
}

// ---------- derived ----------

/** The importance levels, most important first. Drives the menu and the filter. */
export const IMPORTANCE_LEVELS = ["important", "disposable"];

/**
 * Normalize a path into the same key form the backend store uses.
 *
 * Windows paths are case-insensitive and the same file reaches the UI with
 * forward slashes from one place and backslashes from another. If these keys
 * did not match the ones the Rust side writes, a mark would appear to save and
 * then silently fail to apply.
 */
export function normPath(p) {
  const s = String(p == null ? "" : p).replace(/\//g, "\\").replace(/\\+$/, "");
  return (s || String(p == null ? "" : p)).toLowerCase();
}

/** The importance level of a finding, or null when unmarked. */
export function importanceOf(f) {
  return (f && state.marks.get(normPath(f.path))) || null;
}

/** Sort rank, most important first. Unmarked sorts last. */
export function importanceRank(level) {
  const i = IMPORTANCE_LEVELS.indexOf(level);
  return i < 0 ? IMPORTANCE_LEVELS.length : i;
}

/**
 * A mark of important takes the irreversible option off the table.
 *
 * The backend re-checks this and refuses regardless, but offering a button that
 * is guaranteed to be refused is worse than not offering it: the user would
 * confirm an irreversible action and get a "blocked" row instead.
 *
 * Quarantine and cloud backup are deliberately *not* clamped - the right-click
 * menu promises both for an important file ("cleanup may quarantine or back it
 * up, but never delete it"), so removing them here would contradict the menu.
 */
export function clampDisposition(f, d) {
  const level = importanceOf(f);
  if (d === "purge" && level === "important") return "quarantine";
  return d;
}

/**
 * Whether "back this up, then delete it" can be offered for this row.
 *
 * Mirrors the backend's rule: the subscription must be active, and the
 * category must be one where a backup means something - uploading a temp file
 * would spend the user's cloud quota preserving rubbish, and a OneDrive
 * placeholder is already in the cloud.
 */
export function canCloudBackup(f) {
  if (!state.entitlement || !state.entitlement.subscriber) return false;
  return !["temp_junk", "system_cache", "cloud_placeholder"].includes(f.category);
}

export function effectiveDisposition(f) {
  // Cloud-backed content has exactly one meaningful action. Deleting a
  // placeholder inside a sync root deletes the cloud file, and quarantining it
  // would take it out of the sync folder entirely, so neither is offered -
  // and the backend refuses both regardless of what is chosen here.
  if (f.category === "cloud_placeholder") return "dehydrate";
  const o = state.overrides.get(f.id);
  if (o) return clampDisposition(f, o);
  if (state.batchDisposition === "backup" && canCloudBackup(f)) return "cloud_backup";
  if (state.batchDisposition === "purge" && f.category !== "temp_junk" && f.category !== "system_cache") return clampDisposition(f, "purge");
  if (state.batchDisposition === "quarantine") return "quarantine";
  if (state.batchDisposition === "split") {
    return (f.category === "temp_junk" || f.category === "system_cache") ? "purge" : "quarantine";
  }
  return clampDisposition(f, f.disposition);
}

export function selectedItems() {
  return state.findings.filter(f => state.selected.has(f.id));
}

export function selectionTotals() {
  let logical = 0, onDisk = 0;
  for (const f of selectedItems()) { logical += f.logicalSize; onDisk += f.onDiskSize; }
  return { count: state.selected.size, logical, onDisk };
}

export function filteredFindings() {
  const q = state.query.trim().toLowerCase();
  const list = state.findings.filter(f => {
    if (state.riskFilter !== "all" && f.risk !== state.riskFilter) return false;
    if (state.importanceFilter !== "all" && importanceOf(f) !== state.importanceFilter) return false;
    if (q && !(f.name.toLowerCase().includes(q) || f.path.toLowerCase().includes(q))) return false;
    return true;
  });
  return sortFindings(list);
}

/**
 * Rows the user is allowed to select.
 *
 * Every row is selectable now, which was not true when there were four levels.
 * A `critical` mark used to mean "leave this alone" and the backend refused
 * every disposition for it, so the checkbox was withheld to avoid a preview
 * full of rows that could only be blocked.
 *
 * The two-level model has no such level: 重要 refuses the irreversible action
 * and allows the two reversible ones. The row is therefore selectable, the
 * irreversible option is clamped to quarantine by {@link clampDisposition}, and
 * the menu's promise holds. The function is kept rather than inlined at its call
 * sites because it is the single place that decision is written down.
 */
export function selectable(f) {
  return Boolean(f);
}

/**
 * Sort a findings array by the current sort column. Descending by default so a
 * freshly clicked column shows the biggest / oldest first. Returns a new array;
 * `state.findings` is never reordered (stable `id` selection does not depend on
 * display order, but keeping the source list intact keeps sorting purely a
 * presentation concern).
 */
export function sortFindings(arr) {
  if (!state.sortKey) return arr;
  const k = state.sortKey, d = state.sortDir;
  const value = (f) => {
    switch (k) {
      case "name": return f.name || "";
      case "size": return f.logicalSize || 0;
      case "onDiskSize": return f.onDiskSize || 0;
      case "ageDays": return f.ageDays ?? 0;
      case "lastAccessUnix": return f.lastAccessUnix || 0;
      default: return 0;
    }
  };
  return arr.slice().sort((a, b) => {
    const av = value(a), bv = value(b);
    if (typeof av === "string") return d * av.localeCompare(bv);
    return d * (av - bv);
  });
}

export function categoryTotals() {
  const map = new Map();
  for (const f of state.findings) {
    let e = map.get(f.category);
    if (!e) { e = { category: f.category, count: 0, logical: 0, onDisk: 0 }; map.set(f.category, e); }
    e.count++; e.logical += f.logicalSize; e.onDisk += f.onDiskSize;
  }
  return [...map.values()].sort((a, b) => b.onDisk - a.onDisk);
}

/**
 * Roll large files up into their parent directory.
 *
 * A volume scan can surface hundreds of individual files, which answers
 * "what can I delete" but not "who is eating my disk". Grouping by the folder
 * that holds the file answers the second question, and each group is
 * drillable back down to the files themselves.
 */
export function dirTotals() {
  const map = new Map();
  for (const f of state.findings) {
    if (f.category !== "large_file" || f.isDir) continue;
    // Immediate parent only: a deeper rollup would hide which folder to open.
    const parent = f.path.replace(/[\\/][^\\/]*$/, "") || f.path;
    let e = map.get(parent);
    if (!e) { e = { dir: parent, count: 0, onDisk: 0 }; map.set(parent, e); }
    e.count++; e.onDisk += f.onDiskSize;
  }
  return [...map.values()].sort((a, b) => b.onDisk - a.onDisk);
}

/** Findings inside one directory, for the drill-down. */
export function findingsInDir(dir) {
  return state.findings.filter(f => f.category === "large_file" && !f.isDir &&
    (f.path.replace(/[\\/][^\\/]*$/, "") || f.path) === dir);
}

const RISK_DOT = { safe: "green", rebuildable: "blue", personal: "yellow", userdata: "red", cloud: "purple" };

export { RISK_DOT };

/** Localised category name for the current language. */
export function categoryLabel(cat) {
  return catLabel(state.lang, cat);
}

/** Localised risk-tier name for the current language. */
export function riskLabelOf(risk) {
  return riskLabel(state.lang, risk);
}

// ---------- actions ----------
export const actions = {
  setTab(tab) {
    state.tab = tab;
    render();
    if (tab === "quarantine") { actions.refreshQuarantine(); actions.loadCloudBackups(); }
    if (tab === "settings") actions.openSettings();
  },

  async loadOverview() {
    try {
      state.volumes = await invoke("get_volumes");
      state.config = await invoke("get_config");
      state.expiry = await invoke("quarantine_expiry");
    } catch (e) {
      toast(t(state.lang, "toast.overviewFail", { e }), "err");
    }
    // Marks are loaded alongside the overview because the findings tab can be
    // reached without ever opening it, and a badge that only appears after a
    // second visit would look like the save had failed.
    // Loaded here rather than only when Settings opens, because the tier
    // decides whether the right-click importance menu is offered at all - and
    // that menu lives on a different tab. The answer is cached for a day, so
    // this is one Store call on the first launch of the day, not one per view.
    await actions.loadEntitlement();
    await actions.loadImportance();
    render();
  },

  /**
   * Start a scan.
   *
   * The previous results are cleared as soon as a new scan starts, so
   * everything on screen belongs to the scan currently running. Keeping them
   * until the new one succeeded was tidier on paper - a failed scan left the
   * last good results in place - but it meant that during a re-scan the list
   * showed items that were not in it, and a cancelled scan left the previous
   * run's rows sitting there looking current.
   *
   * The trade-off is real: a scan that fails now leaves an empty list, and the
   * user has to scan again. That is the honest state, and the error toast says
   * so.
   */
  async startScan(categories) {
    if (state.scanning) return;
    state.scanning = true;
    state.cancelling = false;
    state.progress = { scanner: "", phase: "starting", dirsScanned: 0, bytesScanned: 0, findings: 0 };
    // Clear up front, and drop the selection with it: a selected row that no
    // longer exists would otherwise still count towards the action bar.
    state.findings = [];
    state.selected.clear();
    state.overrides.clear();
    state.limit.clear();
    state.openDir = null;
    render();

    // Findings arrive as "findings-chunk" events ending in {done:true}; the
    // invoke returns only a count. This keeps a deep scan from shipping one
    // giant array (hundreds of thousands of items) back through IPC.
    const pending = [];
    let finishResolve;
    const finished = new Promise((r) => { finishResolve = r; });
    const unlisten = hasBackend
      ? await listen("findings-chunk", (ev) => {
          const p = ev && ev.payload;
          if (!p) return;
          if (Array.isArray(p.findings)) pending.push(...p.findings);
          if (p.done) finishResolve();
        })
      : () => {};

    try {
      const res = await invoke("start_scan", { options: { categories } });
      let findings;
      if (Array.isArray(res)) {
        // Legacy array contract (older backend or test stubs).
        findings = res;
      } else if (typeof res === "number") {
        await finished;  // the terminal chunk has been received
        findings = pending;
      } else {
        throw new Error(t(state.lang, "toast.badResult"));
      }
      const wasCancelled = state.cancelling;
      state.findings = findings;
      state.scanning = false;
      state.cancelling = false;
      state.progress = null;
      state.tab = "findings";
      // A cancelled scan still returns whatever it had already collected, so
      // reporting "扫描完成，发现 N 项" after the user pressed cancel reads as
      // a success. Say what actually happened instead.
      if (wasCancelled) {
        toast(findings.length
          ? t(state.lang, "toast.cancelled", { n: findings.length })
          : t(state.lang, "toast.cancelledNone"), "");
      } else {
        toast(t(state.lang, "toast.done", { n: findings.length }), "ok");
      }
    } catch (e) {
      state.scanning = false;
      state.cancelling = false;
      state.progress = null;
      toast(t(state.lang, "toast.scanFail", { e }), "err");
    } finally {
      unlisten();
    }
    render();
  },

  /**
   * Ask the backend to stop.
   *
   * The scan is not over the moment this is called - the current directory
   * walk still has to unwind - so the UI says so and refuses to start a new
   * scan until the pending one settles. Without the flag the progress bar
   * kept animating as if nothing had happened and a second click did nothing
   * visible.
   */
  cancelScan() {
    if (!state.scanning || state.cancelling) return;
    state.cancelling = true;
    invoke("cancel_scan").catch(() => { /* the scan will report the outcome */ });
    render();
  },

  /**
   * Progress arrives every 400ms from the backend. Re-rendering the whole tree
   * on each one made tab switching unresponsive during a scan, because every
   * paint also rebuilt the DOM and re-attached the root listeners.
   *
   * The numbers are cheap to update in place, so only the progress readout is
   * touched, and only a few times a second. A trailing update is scheduled so
   * the final numbers always land.
   */
  onProgress(p) {
    state.progress = p;
    paintProgress();
    if (progressTimer) return;
    progressTimer = setTimeout(() => {
      progressTimer = null;
      paintProgress();
    }, PROGRESS_THROTTLE_MS);
  },

  toggleSel(id) {
    const f = state.findings.find(x => x.id === id);
    if (f && !selectable(f)) return;
    if (state.selected.has(id)) state.selected.delete(id); else state.selected.add(id);
    // Ticking a row cannot change the table itself, only checkbox and
    // highlight state, so it skips the full re-render.
    if (state.tab === "findings") renderSelectionOnly(); else render();
  },

  toggleCat(cat, on) {
    for (const f of state.findings) {
      if (f.category !== cat) continue;
      if (on && !selectable(f)) continue;
      if (on) state.selected.add(f.id); else state.selected.delete(f.id);
    }
    if (state.tab === "findings") renderSelectionOnly(); else render();
  },

  selAll(on) {
    if (on) for (const f of filteredFindings()) { if (selectable(f)) state.selected.add(f.id); }
    else for (const f of filteredFindings()) state.selected.delete(f.id);
    // Select-all can cross the filter boundary, so the visible set itself
    // may have changed: this one does need the full path.
    render();
  },

  // ---------- importance marks ----------

  /** Reload every mark from the backend into the path-keyed map. */
  async loadImportance() {
    try {
      const entries = await invoke("list_importance");
      const next = new Map();
      for (const e of entries || []) next.set(normPath(e.path), e.level);
      state.marks = next;
    } catch (e) {
      // A failed load must not wipe what is on screen; the gate still holds
      // server-side, so the worst case is a badge that is out of date.
      toast(t(state.lang, "imp.loadFail", { e }), "err");
    }
  },

  /** Mark one path, then re-render so the badge and the filter update. */
  async setImportance(path, level) {
    try {
      await invoke("set_importance", { path, level });
      state.marks.set(normPath(path), level);
      // A path that is no longer selectable must not stay selected.
      const stillOk = new Set();
      for (const id of state.selected) {
        const f = state.findings.find(x => x.id === id);
        if (f && selectable(f)) stillOk.add(id);
      }
      state.selected = stillOk;
      state.menu = null;
      render();
    } catch (e) {
      state.menu = null;
      toast(t(state.lang, "imp.saveFail", { e }), "err");
      render();
    }
  },

  async clearImportance(path) {
    try {
      await invoke("clear_importance", { path });
      state.marks.delete(normPath(path));
      state.menu = null;
      render();
    } catch (e) {
      state.menu = null;
      toast(t(state.lang, "imp.saveFail", { e }), "err");
      render();
    }
  },

  setImportanceFilter(level) { state.importanceFilter = level; render(); },

  /** Open the right-click importance menu for one finding. */
  openMenu(id, x, y) {
    const f = state.findings.find(v => v.id === id);
    if (!f) return;
    state.menu = {
      path: f.path,
      name: f.name,
      level: importanceOf(f),
      x: Number(x) || 0,
      y: Number(y) || 0,
    };
    render();
  },

  closeMenu() {
    if (!state.menu) return;
    state.menu = null;
    render();
  },

  setOverride(id, d) {
    if (d === f_default(state, id)) state.overrides.delete(id);
    else state.overrides.set(id, d);
    render();
  },

  setBatchDisposition(d) { state.batchDisposition = d; render(); },
  /** Reveal another page of rows for one category. */
  moreRows(cat) {
    const cur = state.limit.get(cat) || PAGE_SIZE;
    state.limit.set(cat, cur + PAGE_SIZE);
    render();
  },

  /** Sort the findings table by a column; re-clicking flips direction. */
  sortBy(key) {
    if (state.sortKey === key) state.sortDir = -state.sortDir;
    else { state.sortKey = key; state.sortDir = -1; }
    render();
  },
  setQuery(q) { state.query = q; render(); },
  setRiskFilter(r) { state.riskFilter = r; render(); },
  toggleCollapse(cat) {
    if (state.collapsed.has(cat)) state.collapsed.delete(cat); else state.collapsed.add(cat);
    render();
  },

  async openPreview() {
    const items = selectedItems().map(f => ({ finding: f, disposition: effectiveDisposition(f) }));
    if (!items.length) return;
    try {
      const groups = await invoke("build_preview", { items });
      const needsRed = await invoke("needs_red_confirmation", { items });
      state.preview = { items, groups, needsRed, confirmed: false, busy: false };
    } catch (e) {
      toast(t(state.lang, "toast.previewFail", { e }), "err");
    }
    render();
  },

  closePreview() { state.preview = null; render(); },
  setRedConfirm(v) { if (state.preview) { state.preview.confirmed = v; render(); } },

  async doCleanup() {
    const pv = state.preview;
    if (!pv) return;
    pv.busy = true; render();
    try {
      const request = { items: pv.items, confirmedRedPurge: pv.confirmed };
      const report = await invoke("run_cleanup", { request });
      pv.busy = false;
      state.preview = null;
      const released = (report.dehydrated || []).length;
      const backedUp = (report.backedUp || []).length;
      const fallbacks = (report.backupFallbacks || []).length;
      const done = report.purged.length + report.quarantined.length + released + backedUp;
      // Only mention the destructive counts when something was actually
      // destroyed. "Permanently deleted 0 items" in front of a release-only
      // cleanup reads as if a deletion happened and is worth checking.
      let msg = (report.purged.length || report.quarantined.length)
        ? t(state.lang, "toast.cleanup", {
            purged: report.purged.length,
            quarantined: report.quarantined.length,
          })
        : t(state.lang, "toast.cleanupNone");
      if (released) {
        // Deliberately not folded into "cleaned": nothing was removed, and
        // OneDrive finishes the dehydration in the background.
        msg += t(state.lang, "toast.released", {
          n: released,
          size: fmtBytes(report.dehydrateRequestedBytes || 0),
        });
      }
      if (backedUp) {
        msg += t(state.lang, "toast.backedUp", {
          n: backedUp,
          size: fmtBytes(report.backupBytes || 0),
        });
      }
      // A fallback is not a failure: the space was reclaimed. But the user
      // asked for a cloud copy and did not get one, so it is said out loud.
      if (fallbacks) {
        msg += t(state.lang, "toast.backupFallback", { n: fallbacks });
      }
      if (report.blocked.length) msg += t(state.lang, "toast.blocked", { n: report.blocked.length });
      if (report.failed.length) msg += t(state.lang, "toast.failed", { n: report.failed.length });
      toast(msg, done ? "ok" : "err");

      // The only reason to ask for administrator rights is a permission wall.
      if (report.needsElevation) {
        state.elevateRequest = request;
      }
      // Drop cleaned items from the list. Released cloud findings go too: the
      // folder is still there, but the row's byte count is now stale until the
      // next scan, and a row that claims space it no longer holds is worse
      // than no row.
      const cleaned = new Set([...report.purged, ...report.quarantined, ...(report.dehydrated || []), ...(report.backedUp || [])]);
      state.findings = state.findings.filter(f => !cleaned.has(f.path));
      state.selected.clear();
      // The drill-down is derived from state.findings; a stale openDir would
      // keep rendering the (now deleted) files of a cleaned-up folder.
      state.openDir = null;
      try {
        state.expiry = await invoke("quarantine_expiry");
      } catch (_) { /* expiry refresh is non-critical */ }
    } catch (e) {
      // A rejected cleanup must not leave the dialog stuck on "处理中…" with a
      // disabled button and no explanation.
      pv.busy = false;
      state.preview = null;
      toast(t(state.lang, "toast.cleanupFail", { e }), "err");
    }
    render();
  },

  /**
   * Render the current preview as plain text.
   *
   * The design requires every deletion to be previewable as text, so the user
   * can read the full list rather than the eight rows the dialog shows, and
   * keep a copy of exactly what they agreed to.
   */
  previewAsText() {
    const pv = state.preview;
    if (!pv) return "";
    const lang = state.lang;
    const lines = [];
    lines.push(t(lang, "exp.title"));
    lines.push(t(lang, "exp.generated", { time: new Date().toLocaleString() }));
    lines.push("");
    for (const g of pv.groups) {
      const label = g.disposition === "purge" ? t(lang, "exp.purge")
        : g.disposition === "dehydrate" ? t(lang, "exp.dehydrate")
        : g.disposition === "cloud_backup" ? t(lang, "exp.cloudBackup")
        : t(lang, "exp.quarantine");
      lines.push(t(lang, "exp.group", { label, count: g.itemCount, size: fmtBytes(g.onDiskBytes) }));
      const items = pv.items.filter(i => i.disposition === g.disposition);
      for (const it of items) {
        const f = it.finding;
        lines.push(`  [${categoryLabel(f.category)}] ${f.path}`);
        lines.push(
          t(lang, "exp.itemLine", {
            logical: fmtBytes(f.logicalSize),
            onDisk: fmtBytes(f.onDiskSize),
            age: fmtAge(f.ageDays),
            rule: f.ruleId,
          })
        );
      }
      if (g.containsRedPurge) {
        lines.push(t(lang, "exp.redNote"));
      }
      lines.push("");
    }
    if (pv.needsRed) {
      lines.push(t(lang, "exp.redConfirmNote"));
    }
    lines.push(t(lang, "exp.footer"));
    return lines.join("\n");
  },

  /** Copy the preview to the clipboard, falling back to a download. */
  async copyPreview() {
    const text = this.previewAsText();
    if (!text) return;
    try {
      if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(text);
        toast(t(state.lang, "toast.copied"), "ok");
        return;
      }
    } catch (_) { /* fall through to the download path */ }
    this.downloadPreview(text);
    toast(t(state.lang, "toast.exported"), "ok");
  },

  downloadPreview(text) {
    const blob = new Blob([text], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `spacerrecycle-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, "")}.txt`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  },

  toggleDir(dir) { state.openDir = state.openDir === dir ? null : dir; render(); },

  /** Open the item's containing folder in Explorer, with the item selected. */
  reveal(path) {
    invoke("reveal_in_explorer", { path }).catch((e) => toast(t(state.lang, "toast.revealFail", { e }), "err"));
  },

  async retryElevated() {
    const request = state.elevateRequest;
    if (!request) return;
    state.elevateRequest = null;
    try {
      await invoke("retry_cleanup_elevated", { request });
      toast(t(state.lang, "toast.elevated"), "ok");
    } catch (e) {
      toast(String(e), "err");
    }
    render();
  },

  dismissElevate() { state.elevateRequest = null; render(); },

  async refreshQuarantine() {
    try {
      state.batches = await invoke("list_quarantine_batches");
      state.expiry = await invoke("quarantine_expiry");
    } catch (e) {
      toast(t(state.lang, "toast.qFail", { e }), "err");
    }
    render();
  },

  async restoreBatch(dir, policy) {
    try {
      const r = await invoke("restore_batch", { batchDir: dir, policy });
      let msg = t(state.lang, "toast.restored", { n: r.restored.length });
      if (r.skipped.length) msg += t(state.lang, "toast.restoredSkip", { n: r.skipped.length });
      toast(msg, "ok");
    } catch (e) {
      toast(t(state.lang, "toast.restoreFail", { e }), "err");
    }
    await actions.refreshQuarantine();
  },

  async restoreItem(dir, path, policy) {
    try {
      await invoke("restore_item", { batchDir: dir, originalPath: path, policy });
      toast(t(state.lang, "toast.restoreDone"), "ok");
    } catch (e) { toast(t(state.lang, "toast.restoreFail", { e }), "err"); }
    await actions.refreshQuarantine();
  },

  async purgeBatch(dir) {
    try {
      const r = await invoke("purge_batch", { batchDir: dir });
      toast(t(state.lang, "toast.purged", { n: r.purged.length }), "ok");
    } catch (e) {
      toast(t(state.lang, "toast.purgeFail", { e }), "err");
    }
    await actions.refreshQuarantine();
  },

  async openSettings() {
    state.config = await invoke("get_config");
    state.auditLog = await invoke("read_audit_log", { limit: 200 });
    state.auditPath = await invoke("audit_log_path");
    state.showSettings = true;
    // The guard's disposition lives outside this app's config, in the file the
    // resident service reads, so it is fetched rather than read from state.
    // Its status comes along because the screen has to be able to say that
    // nothing is being protected right now, which is invisible otherwise.
    // A failure here must not take the whole settings tab with it: every other
    // control on it still works, and the card degrades to the plain choices.
    try {
      state.guard = await invoke("guard_status");
    } catch (e) {
      state.guard = null;
    }
    // Which mechanism is actually protecting files, and how many it is holding.
    //
    // Fetched separately from guard_status because they answer different
    // questions. guard_status is about the resident driver service; protection
    // is about whether anything is protected, which is now also achievable
    // without a service at all. The card used to read the service state and
    // conclude "nothing is protected", which stopped being true the moment the
    // hold backend existed - and reading the wrong one told the user they were
    // unprotected while their files were held open.
    try {
      state.protection = await invoke("protection_status");
    } catch (e) {
      state.protection = null;
    }
    // Subscription state is only shown here, so it is fetched when this tab
    // opens rather than on every launch: an entitlement check on a packaged
    // build is a Store round trip.
    await actions.loadEntitlement();
    await actions.loadAccount();
    render();
  },
  closeSettings() {
    state.showSettings = false;
    // A sign-in left half-finished would keep polling in the background and
    // start firing toasts at the user from another tab.
    actions.cancelConnect();
    render();
  },

  // ---------- OneDrive account ----------

  /** Read the linked-account state and cloud quota. */
  async loadAccount() {
    try {
      state.onedriveAccount = await invoke("graph_account_status");
    } catch (_) {
      // A failed status read is not worth a toast on every Settings visit;
      // the card simply reports that nothing is linked.
      state.onedriveAccount = { connected: false, quota: null, detail: "" };
    }
  },

  /** Begin a device-code sign-in and start polling. */
  async startConnect() {
    if (state.connect) return;
    try {
      const info = await invoke("graph_connect_start");
      state.connect = {
        userCode: info.userCode,
        verificationUri: info.verificationUri,
        intervalSecs: Math.max(1, info.intervalSecs || 5),
        status: "pending",
        detail: "",
      };
      render();
      schedulePoll(state.connect.intervalSecs);
    } catch (e) {
      // The backend rejects this when no Azure client ID is configured, and
      // that message is the only thing telling the user what to fix.
      toast(String(e), "err");
    }
  },

  /** One poll of a sign-in in progress. */
  async pollConnect() {
    if (!state.connect) return;
    let r;
    try {
      r = await invoke("graph_connect_poll");
    } catch (e) {
      state.connect = null;
      clearPoll();
      toast(t(state.lang, "acc.fail", { e }), "err");
      render();
      return;
    }
    // The user may have closed Settings while the request was in flight.
    if (!state.connect) return;

    if (r.status === "pending" || r.status === "slow_down" || r.status === "error") {
      state.connect.status = r.status;
      state.connect.detail = r.detail || "";
      if (r.intervalSecs) state.connect.intervalSecs = r.intervalSecs;
      render();
      schedulePoll(state.connect.intervalSecs);
      return;
    }

    clearPoll();
    state.connect = null;
    if (r.status === "connected") {
      state.onedriveAccount = { connected: true, quota: r.quota || null, detail: "" };
      toast(t(state.lang, "acc.connected"), "ok");
    } else {
      toast(t(state.lang, "acc.denied", { reason: r.detail || r.status }), "err");
    }
    render();
  },

  /** Abandon a sign-in in progress. */
  cancelConnect() {
    if (!state.connect) return;
    clearPoll();
    state.connect = null;
    invoke("graph_connect_cancel").catch(() => { /* nothing to clean up */ });
  },

  async signOut() {
    clearPoll();
    state.connect = null;
    try {
      await invoke("graph_sign_out");
    } catch (e) {
      toast(t(state.lang, "acc.fail", { e }), "err");
    }
    state.onedriveAccount = { connected: false, quota: null, detail: "" };
    render();
  },

  async copyConnectLink() {
    const url = state.connect && state.connect.verificationUri;
    if (!url) return;
    try {
      await navigator.clipboard.writeText(url);
      toast(t(state.lang, "acc.copied"), "ok");
    } catch (_) {
      // Clipboard access can be denied; the URL is on screen either way.
      toast(t(state.lang, "acc.copyFail"), "err");
    }
  },

  // ---------- cloud backup batches ----------

  /**
   * Load the batches stored in the cloud.
   *
   * A failure here means "no cloud backups to show", not an error worth a
   * toast on every visit to the tab: the overwhelmingly common reason is that
   * no OneDrive account is linked.
   */
  async loadCloudBackups() {
    try {
      state.cloudBackups = await invoke("list_cloud_backups");
    } catch (_) {
      state.cloudBackups = [];
    }
  },

  async restoreCloudBatch(batchId, policy) {
    state.cloudBusy = batchId;
    render();
    try {
      const r = await invoke("restore_cloud_backup", { batchId, policy });
      let msg = t(state.lang, "cb.restored", {
        n: r.restoredFiles,
        size: fmtBytes(r.restoredBytes),
      });
      if (r.skipped.length) msg += t(state.lang, "cb.skipped", { n: r.skipped.length });
      if (r.failed.length) msg += t(state.lang, "cb.failed", { n: r.failed.length });
      toast(msg, r.failed.length ? "err" : "ok");
    } catch (e) {
      toast(t(state.lang, "cb.restoreFail", { e }), "err");
    }
    state.cloudBusy = null;
    render();
  },

  async deleteCloudBatch(batchId) {
    state.cloudBusy = batchId;
    render();
    try {
      await invoke("delete_cloud_backup", { batchId });
      toast(t(state.lang, "cb.deleted"), "ok");
      await actions.loadCloudBackups();
    } catch (e) {
      toast(t(state.lang, "cb.deleteFail", { e }), "err");
    }
    state.cloudBusy = null;
    render();
  },

  /**
   * Read the current subscription state.
   *
   * A missing or failed answer leaves `entitlement` null rather than guessing:
   * the card is hidden, and the backend still refuses cloud backup on its own.
   */
  async loadEntitlement() {
    try {
      state.entitlement = await invoke("get_entitlement");
    } catch (_) {
      state.entitlement = null;
    }
  },

  /** Re-check the subscription, ignoring a fresh cached answer. */
  async refreshEntitlement() {
    try {
      state.entitlement = await invoke("refresh_entitlement");
      toast(
        state.entitlement && state.entitlement.subscriber
          ? t(state.lang, "sub.toastActive")
          : t(state.lang, "sub.toastInactive"),
        state.entitlement && state.entitlement.subscriber ? "ok" : ""
      );
      return;
    } catch (e) {
      toast(t(state.lang, "sub.toastFail", { e }), "err");
    }
    render();
  },

  /** Switch the interface language and persist the choice. */
  setLang(lang) {
    if (lang !== "zh" && lang !== "en") return;
    state.lang = lang;
    try { localStorage.setItem("spacerrecycle.lang", lang); } catch (_) { /* storage unavailable */ }
    // Rebuild the whole tree: cached region HTML and the formatting cache are
    // keyed by language, but the shell itself may contain language text.
    render();
  },

  async saveSettings(patch) {
    const next = Object.assign({}, state.config, patch);
    try {
      await invoke("save_config", { cfg: next });
      state.config = next;
      // Must go through toast(): assigning state.toast directly skipped the
      // auto-dismiss timer, so the confirmation never went away.
      toast(t(state.lang, "toast.saved"), "ok");
    } catch (e) {
      toast(t(state.lang, "toast.saveFail", { e }), "err");
    }
  },

  /// Choose whether quarantined items stay on this disk or go to OneDrive.
  ///
  /// Saved on change rather than through the Save button, for the same reason
  /// as the guard disposition: this decides where the *next* cleanup puts an
  /// item, and a choice that only takes effect after a second button is a
  /// choice a user can walk away from without having made.
  async setQuarantineTarget(target) {
    const previous = state.config;
    if (previous) state.config = Object.assign({}, previous, { quarantineTarget: target });
    render();
    try {
      await invoke("save_config", { cfg: state.config });
      // Re-read rather than trusting the local copy: serde fills in any field
      // the frontend did not know about, and a stale local object would drop
      // it on the next save.
      state.config = await invoke("get_config");
      render();
      toast(t(state.lang, "set.q.saved"), "ok");
    } catch (e) {
      state.config = previous;
      render();
      toast(t(state.lang, "toast.saveFail", { e }), "err");
    }
  },

  /// Change what the guard does about a marked file on delete.
  ///
  /// The value goes into the file the resident service watches, so the screen
  /// updates at once and the kernel follows a couple of seconds later. There is
  /// no synchronisation to wait on here and pretending otherwise would misstate
  /// what is actually immediate: this app cannot see the driver's state.
  async setGuardDisposition(disposition) {
    const previous = state.guard;
    if (previous) state.guard = Object.assign({}, previous, { disposition });
    render();
    try {
      await invoke("set_guard_disposition", { disposition });
      state.guard = await invoke("guard_status");
      render();
      toast(t(state.lang, "set.guard.saved"), "ok");
    } catch (e) {
      state.guard = previous;
      render();
      toast(t(state.lang, "toast.saveFail", { e }), "err");
    }
  },

  async loadBatchItems(dir) {    try {
      const m = await invoke("list_quarantine_entries", { batchDir: dir });
      state.openBatch = state.openBatch === dir ? null : dir;
      state.batchItems = m ? m.entries : [];
    } catch (e) {
      toast(t(state.lang, "toast.batchFail", { e }), "err");
    }
    render();
  },

  async purgeExpired() {
    const batches = state.batches.filter(b => isExpired(b));
    try {
      for (const b of batches) await invoke("purge_batch", { batchDir: b.dir });
      toast(t(state.lang, "toast.expired", { n: batches.length }), "ok");
    } catch (e) {
      toast(t(state.lang, "toast.expiredFail", { e }), "err");
    }
    await actions.refreshQuarantine();
  },
};

function f_default(st, id) {
  const f = st.findings.find(x => x.id === id);
  return f ? f.disposition : "quarantine";
}

export function isExpired(batch) {
  if (!state.expiry) return false;
  const ageDays = (Date.now() / 1000 - batch.createdUnix) / 86400;
  return ageDays >= state.expiry.expiryDays;
}

export function ageDays(batch) {
  return (Date.now() / 1000 - batch.createdUnix) / 86400;
}
