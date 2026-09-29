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

function stub(cmd) {
  switch (cmd) {
    case "get_volumes": return [];
    case "get_config": return {
      largeFileAgeDays: 30, leftoverAgeDays: 30,
      largeFileMinBytes: 524288000, quarantineExpiryDays: 7,
      quarantineDir: "C:\\ProgramData\\SpaceRecycle\\Quarantine",
      extraExcludedRoots: [],
    };
    case "quarantine_expiry":
      return { expiryDays: 7, expiredBatches: [], expiredItems: 0, expiredOnDiskBytes: 0 };
    case "list_quarantine_batches": return [];
    case "build_preview": return [];
    case "needs_red_confirmation": return false;
    default: return null;
  }
}
