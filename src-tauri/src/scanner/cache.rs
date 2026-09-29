use super::{finding_for_dir, ScanCtx, Scanner};
use crate::model::{Category, Finding};
use std::path::PathBuf;

pub struct CacheScanner;

impl Scanner for CacheScanner {
    fn name(&self) -> &'static str {
        "System cache"
    }

    fn scan(&self, ctx: &ScanCtx) -> Vec<Finding> {
        let mut out = Vec::new();
        let mut targets: Vec<(PathBuf, &str)> = Vec::new();

        if let Some(local) = dirs::data_local_dir() {
            let l = local.clone();

            // Browser caches. Profiles are nested one level under the vendor dir.
            for (vendor, rule) in [
                ("Google/Chrome/User Data", "cache.chrome"),
                ("Microsoft/Edge/User Data", "cache.edge"),
                ("BraveSoftware/Brave-Browser/User Data", "cache.brave"),
            ] {
                let base = l.join(vendor);
                if let Ok(entries) = std::fs::read_dir(&base) {
                    for e in entries.flatten() {
                        let p = e.path();
                        if p.is_dir() && is_profile_dir(&p) {
                            targets.push((p.join("Cache"), rule));
                            targets.push((p.join("Code Cache"), rule));
                            targets.push((p.join("GPUCache"), rule));
                            targets.push((p.join("Service Worker").join("CacheStorage"), rule));
                        }
                    }
                }
            }
            if let Some(mozilla) = dirs::data_dir() {
                let m = mozilla.join("Mozilla").join("Firefox").join("Profiles");
                if let Ok(entries) = std::fs::read_dir(&m) {
                    for e in entries.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            targets.push((p.join("cache2"), "cache.firefox"));
                        }
                    }
                }
            }

            // OS / shell caches.
            targets.push((l.join("Microsoft").join("Windows").join("Explorer"), "cache.thumbnails"));
            targets.push((l.join("D3DSCache"), "cache.shader"));
            targets.push((l.join("Microsoft").join("Windows").join("INetCache"), "cache.inet"));
            targets.push((l.join("Microsoft").join("Windows").join("WebCache"), "cache.webcache"));
            targets.push((l.join("NVIDIA").join("DXCache"), "cache.nv"));
            targets.push((l.join("AMD").join("DxCache"), "cache.amd"));
        }

        if let Some(pd) = std::env::var_os("ProgramData") {
            let p = PathBuf::from(pd);
            targets.push((p.join("Microsoft").join("Windows").join("DeliveryOptimization"), "cache.delivery-opt"));
        }

        for (dir, rule) in targets {
            if ctx.counters.cancelled() {
                break;
            }
            if !dir.is_dir() {
                continue;
            }
            let (la, cr) = super::walk::timestamps(&dir);
            if let Some(f) = finding_for_dir(ctx, &dir, Category::SystemCache, rule, la, cr) {
                out.push(f);
            }
        }
        out
    }
}

/// A browser profile directory is normally named "Default", "Profile 1", ...
fn is_profile_dir(p: &std::path::Path) -> bool {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    name == "Default" || name.starts_with("Profile ")
}
