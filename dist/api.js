// Tauri IPC bridge.
//
// We use Tauri's withGlobalTauri mode so the frontend needs no bundler and no
// npm packages: the app ships as plain ES modules. Outside Tauri (e.g. opening
// dist/index.html in a browser) every call degrades to an inert stub so the
// UI still renders instead of throwing.
const T = typeof window !== "undefined" ? window.__TAURI__ : undefined;

export const hasBackend = !!(T && T.core && T.core.invoke);

export async function invoke(cmd, args) {
  if (hasBackend) return T.core.invoke(cmd, args);
  return stub(cmd, args);
}

export async function listen(event, handler) {
  if (hasBackend && T.event && T.event.listen) return T.event.listen(event, handler);
  return () => {};
}

// The guard's disposition lives outside this app, in the file the resident
// service reads. The stub keeps it in memory so the settings screen still
// behaves coherently when the UI is opened without a backend - a stub that
// always answered the same thing would make "changing the setting" look broken.
const stubGuard = {
  serviceInstalled: false,
  serviceRunning: false,
  policyPath: "",
  disposition: "ask",
  configured: false,
};

// Settings are kept the same way as the guard disposition above: the stub
// remembers what was saved so the quarantine-target choice survives a re-render
// when the UI is opened without a backend.
let stubConfig = {
  largeFileAgeDays: 30, leftoverAgeDays: 30,
  largeFileMinBytes: 524288000, quarantineExpiryDays: 7,
  quarantineDir: "C:\\ProgramData\\SpaceRecycle\\Quarantine",
  quarantineTarget: "local",
  extraExcludedRoots: [],
};

function stub(cmd, args) {
  switch (cmd) {
    case "get_volumes": return [];
    case "get_config": return Object.assign({}, stubConfig);
    case "save_config":
      stubConfig = Object.assign({}, stubConfig, (args && args.cfg) || {});
      return null;
    case "quarantine_expiry":
      return { expiryDays: 7, expiredBatches: [], expiredItems: 0, expiredOnDiskBytes: 0 };
    case "list_quarantine_batches": return [];
    case "list_importance": return [];
    case "guard_status": return Object.assign({}, stubGuard);
    case "set_guard_disposition":
      // Rejecting an unknown value is what the real backend does too - the
      // Rust side deserialises into a three-variant enum - so having the stub
      // do the same keeps the failure path testable.
      if (!args || !["ask", "deny", "allow"].includes(args.disposition)) {
        throw new Error("unknown disposition: " + (args && args.disposition));
      }
      stubGuard.disposition = args.disposition;
      stubGuard.configured = true;
      return null;
    case "onedrive_status": return { available: false, roots: [] };
    // Mirrors EntitlementView: the backend sends the three resolved flags so
    // the frontend does not re-derive the tier rule. The stub reports the
    // unpackaged case, which is what a browser or unpackaged run actually is -
    // nothing is on sale, so the paid features are open while subscriber stays
    // false.
    case "get_entitlement":
    case "refresh_entitlement":
      return {
        subscriber: false, source: "unpackaged", checkedUnix: 0, expiresUnix: null,
        detail: "this build was not installed from the Microsoft Store",
        cloudAllowed: true, importanceAllowed: true, guardAllowed: true,
      };
    case "graph_account_status": return { connected: false, quota: null, detail: "" };
    case "graph_connect_start": return { userCode: "SKIP-INERT", verificationUri: "", intervalSecs: 5, expiresIn: 0 };
    case "list_cloud_backups": return [];
    case "build_preview": return [];
    case "needs_red_confirmation": return false;
    default: return null;
  }
}
