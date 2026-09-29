use super::{finding_for_dir, ScanCtx, Scanner};
use crate::model::{Category, Finding};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use windows::core::PWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER,
    HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ,
};

pub struct LeftoverScanner;

impl Scanner for LeftoverScanner {
    fn name(&self) -> &'static str {
        "App leftovers"
    }

    fn scan(&self, ctx: &ScanCtx) -> Vec<Finding> {
        let cfg: &crate::config::Config = ctx.config.as_ref();
        let mut out = Vec::new();
        // Tokenised once, then reused for every folder.
        let live = LiveIndex::from_entries(&installed_app_names());

        let mut roots: Vec<(PathBuf, &str)> = Vec::new();
        if let Some(local) = dirs::data_local_dir() {
            roots.push((local, "leftover.local"));
        }
        if let Some(roam) = dirs::data_dir() {
            roots.push((roam, "leftover.roaming"));
        }
        if let Some(pd) = std::env::var_os("ProgramData") {
            roots.push((PathBuf::from(pd), "leftover.programb"));
        }

        for (root, rule) in roots {
            if ctx.counters.cancelled() {
                break;
            }
            let entries = match std::fs::read_dir(&root) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                if ctx.counters.cancelled() {
                    break;
                }
                let p = entry.path();
                if !p.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                // Vendor and OS component folders, plus the installer caches
                // that Windows and Visual Studio need in order to repair or
                // uninstall. "Package Cache" in particular is not owned by any
                // app named "Package" - it only escaped the leftover list by
                // accident, because unrelated tools like Application Verifier
                // have the word in their display name.
                if is_reserved_folder(&name) {
                    continue;
                }
                // Rebuildable developer stores belong to the dev scanner.
                // Reporting them here as well would show the same bytes twice
                // under two risk levels, and the red "may contain user data"
                // tag is simply wrong for a package cache.
                if is_dev_store(&name) {
                    continue;
                }
                if is_live_app(&name, &live) {
                    continue;
                }
                if !crate::safety::is_allowed(&p, cfg) {
                    continue;
                }
                let (la, cr) = super::walk::timestamps(&p);
                if let Some(f) =
                    finding_for_dir(ctx, &p, Category::AppLeftover, rule, la, cr)
                {
                    out.push(f);
                }
            }
        }

        // Stale installers in Downloads.
        if let Some(home) = dirs::home_dir() {
            let downloads = home.join("Downloads");
            if downloads.is_dir() {
                for entry in std::fs::read_dir(&downloads).into_iter().flatten().flatten() {
                    let p = entry.path();
                    let ext = p
                        .extension()
                        .map(|e| e.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    if !matches!(ext.as_str(), "msi" | "exe" | "msix" | "appx" | "iso" | "zip" | "7z") {
                        continue;
                    }
                    let md = match std::fs::metadata(&p) {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    use std::os::windows::fs::MetadataExt;
                    let la = super::walk::filetime_to_unix(md.last_access_time() as u64);
                    let cr = super::walk::filetime_to_unix(md.creation_time() as u64);
                    let (age_days, age_basis) = super::walk::age_of(la, cr);
                    if age_days < ctx.leftover_age_days || !crate::safety::is_allowed(&p, cfg) {
                        continue;
                    }
                    let logical = md.len();
                    let on_disk = crate::winapi::ntfs::on_disk_size(&p).unwrap_or(logical);
                    ctx.counters.findings.fetch_add(1, Ordering::Relaxed);
                    out.push(Finding {
                        id: uuid::Uuid::new_v4().to_string(),
                        rule_id: "leftover.installer".into(),
                        name: entry.file_name().to_string_lossy().to_string(),
                        path: p.to_string_lossy().to_string(),
                        is_dir: false,
                        logical_size: logical,
                        on_disk_size: on_disk,
                        last_access_unix: la,
                        created_unix: cr,
                        age_days,
                        age_basis,
                        category: Category::AppLeftover,
                        risk: Category::AppLeftover.risk(),
                        disposition: Category::AppLeftover.default_disposition(),
                        child_count: 0,
                    });
                }
            }
        }

        out
    }
}

/// Read every installed app's display name from the registry Uninstall keys.
fn installed_app_names() -> HashSet<String> {
    let mut names = HashSet::new();
    for (root, sub) in [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall",
        ),
    ] {
        collect_uninstall_names(root, sub, &mut names);
    }
    collect_uninstall_names(
        HKEY_CURRENT_USER,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall",
        &mut names,
    );
    names
}

fn collect_uninstall_names(root: HKEY, sub: &str, out: &mut HashSet<String>) {
    let mut sub_w: Vec<u16> = sub.encode_utf16().chain(std::iter::once(0)).collect();
    let mut key = HKEY(std::ptr::null_mut());
    unsafe {
        if RegOpenKeyExW(root, PWSTR(sub_w.as_mut_ptr()), None, KEY_READ, &mut key)
            != ERROR_SUCCESS
        {
            return;
        }
        let mut index = 0u32;
        loop {
            let mut name_buf = [0u16; 256];
            let mut len = name_buf.len() as u32;
            let res = RegEnumKeyExW(
                key,
                index,
                Some(PWSTR(name_buf.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            );
            if res != ERROR_SUCCESS {
                break;
            }
            index += 1;
            let subkey = String::from_utf16_lossy(&name_buf[..len as usize]);
            if let Some(display) = query_string(key, &subkey, "DisplayName") {
                out.insert(display);
            }
            // The display name is marketing text and often shares no substring
            // with the folder it owns ("Ollama version 0.30.10" vs
            // "ollama app.exe"). The install path is the reliable link, so
            // index the product directory names it points at too.
            if let Some(loc) = query_install_location(key, &subkey) {
                out.insert(loc);
            }
        }
        let _ = RegCloseKey(key);
    }
}

/// Read an uninstall entry's InstallLocation, and the names of the product
/// directories directly beneath it.
fn query_install_location(parent: HKEY, subkey: &str) -> Option<String> {
    let raw = query_string(parent, subkey, "InstallLocation")?;
    let trimmed = raw.trim().trim_end_matches('\\').to_string();
    if trimmed.is_empty() {
        return None;
    }
    let mut names = vec![trimmed.clone()];
    // Also index the leaf folder, e.g. "Programs\\Ollama" -> "ollama".
    if let Some(leaf) = PathBuf::from(&trimmed)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
    {
        if !leaf.is_empty() {
            names.push(leaf);
        }
    }
    Some(names.join("\u{1}"))
}

fn query_string(parent: HKEY, subkey: &str, value_name: &str) -> Option<String> {
    let mut sub_w: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
    let mut val_w: Vec<u16> = value_name.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        // Ask for the size first. A fixed buffer silently dropped any value
        // that did not fit, and a dropped InstallLocation means is_live_app
        // cannot recognise a still-installed app - so its live AppData folder
        // gets reported as a leftover, and the user is offered a red-tagged
        // "may contain your data" item that is very much in use.
        let mut needed: u32 = 0;
        let size_res = RegGetValueW(
            parent,
            PWSTR(sub_w.as_mut_ptr()),
            PWSTR(val_w.as_mut_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut needed),
        );
        if size_res != ERROR_SUCCESS || needed == 0 {
            return None;
        }

        // Include the terminating NUL that RegGetValueW counts.
        let mut buf = vec![0u16; (needed as usize / 2) + 1];
        let mut size: u32 = (buf.len() * 2) as u32;
        let res = RegGetValueW(
            parent,
            PWSTR(sub_w.as_mut_ptr()),
            PWSTR(val_w.as_mut_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            Some(&mut size),
        );
        if res != ERROR_SUCCESS {
            return None;
        }

        let chars = (size as usize / 2).saturating_sub(1);
        let s = String::from_utf16_lossy(&buf[..chars]);
        if s.trim().is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

/// Rebuildable developer stores, claimed by `dev.rs`. Keeping them out
/// of the leftover list avoids double-reporting one path under two categories.
fn is_dev_store(name: &str) -> bool {
    matches!(
        name,
        "uv"
            | "vcpkg"
            | "NuGet"
            | "ms-playwright"
            | "Yarn"
            | "node_modules"
            | ".cargo"
            | ".gradle"
            | "npm-cache"
            | "pnpm"
            | "pip"
    )
}

/// Folders under AppData / ProgramData that are never application leftovers.
fn is_reserved_folder(name: &str) -> bool {
    matches!(
        name,
        "Microsoft"
            | "Packages"
            | "Temp"
            | "MicrosoftEdge"
            | "NVIDIA"
            | "Google"
            | "Package Cache"
            | "Package Cache Extras"
            | "Microsoft Visual Studio"
            | "Microsoft OfficePLUS"
    )
}

/// Pre-tokenised view of the registry's uninstall entries.
///
/// Built once per scan, then reused for every AppData folder. Tokenising on the
/// fly instead cost ~130k redundant tokenisations on a machine with 400-odd
/// installed apps, because each folder re-tokenised every candidate entry.
pub struct LiveIndex {
    /// Every significant token belonging to any installed app.
    tokens: HashSet<String>,
}

impl LiveIndex {
    pub fn from_entries(entries: &HashSet<String>) -> LiveIndex {
        let mut tokens = HashSet::new();
        for entry in entries {
            for candidate in entry.split('\u{1}') {
                for t in tokens_of(candidate) {
                    if is_significant(&t) {
                        tokens.insert(t);
                    }
                }
            }
        }
        LiveIndex { tokens }
    }

    /// True when a folder name shares a meaningful token with an installed app.
    ///
    /// Matching is per token, never on concatenated strings: "Ollama version
    /// 0.30.10" and the install path "...\\Programs\\Ollama" only ever agree
    /// on the token "ollama". A token has to be meaningful on its own, which
    /// stops "dist" being claimed by "WPT Redistributables" and "cad" by
    /// "Cadence Design Systems", while still letting "Foo" match a folder
    /// called Foo.
    pub fn matches(&self, folder: &str) -> bool {
        tokens_of(folder)
            .iter()
            .any(|t| is_significant(t) && self.tokens.contains(t))
    }
}

/// Heuristic: does an AppData folder name correspond to a still-installed app?
fn is_live_app(folder: &str, live: &LiveIndex) -> bool {
    live.matches(folder)
}

/// Short tokens carry too little meaning to tie a folder to an app on their
/// own: "go", "id" and "sdk" appear inside dozens of unrelated display names.
///
/// Three characters is the floor, and only because the install-location index
/// deliberately records leaf directory names like "Foo" verbatim. Matching is
/// whole-token in both directions, so a three-letter token still has to be the
/// entire folder name and the entire candidate token to count.
fn is_significant(token: &str) -> bool {
    token.len() >= 3
}

/// Strip a trailing file extension, then split into lowercase alphanumeric runs.
fn tokens_of(s: &str) -> Vec<String> {
    let base = match s.rsplit_once('.') {
        Some((head, ext))
            if !head.is_empty()
                && ext.len() <= 4
                && ext.chars().all(|c| c.is_alphanumeric()) =>
        {
            head
        }
        _ => s,
    };
    base.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(pairs: &[&str]) -> LiveIndex {
        LiveIndex::from_entries(&pairs.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn installer_caches_are_never_leftovers() {
        // "Package Cache" is the Visual Studio / Windows Installer repair
        // store. Deleting it breaks uninstall and repair of installed apps.
        assert!(is_reserved_folder("Package Cache"));
        assert!(is_reserved_folder("Microsoft"));
        assert!(!is_reserved_folder("Dbg"));
    }

    #[test]
    fn developer_stores_are_left_to_the_dev_scanner() {
        assert!(is_dev_store("uv"));
        assert!(is_dev_store("NuGet"));
        assert!(!is_dev_store("Dbg"));
    }

    #[test]
    fn matches_a_folder_named_after_the_app() {
        let l = live(&["Notepad++"]);
        assert!(is_live_app("Notepad++", &l));
    }

    #[test]
    fn matches_through_the_install_location_not_the_display_name() {
        // "Ollama version 0.30.10" shares no whole token with
        // "ollama app.exe"; the install path is what links them.
        let l = live(&[
            "Ollama version 0.30.10",
            "C:\\Users\\u\\AppData\\Local\\Programs\\Ollama",
        ]);
        assert!(is_live_app("ollama app.exe", &l));
    }

    #[test]
    fn does_not_match_an_unrelated_vendor_with_a_shared_word() {
        // "dist" is a VS redistributable folder, not owned by WPT.
        let l = live(&["WPT Redistributables"]);
        assert!(!is_live_app("dist", &l));
    }

    #[test]
    fn an_uninstalled_app_is_reported_as_a_leftover() {
        let l = live(&["Google Chrome", "Mozilla Firefox"]);
        assert!(!is_live_app("NetLimiter", &l));
    }

    #[test]
    fn joined_multi_value_entries_are_all_considered() {
        let l = live(&["C:\\Program Files\\Foo\\u{1}foo"]);
        assert!(is_live_app("Foo", &l));
    }

    #[test]
    fn a_bare_substring_inside_a_longer_word_is_not_a_match() {
        // "cad" must not claim a "Cadence" folder, nor the reverse.
        let l = live(&["Cadence Design Systems"]);
        assert!(!is_live_app("cad", &l));
    }

    #[test]
    fn a_versioned_display_name_matches_through_its_product_token() {
        let l = live(&["Ollama version 0.30.10"]);
        assert!(is_live_app("ollama app.exe", &l));
    }

    #[test]
    fn an_install_path_matches_on_its_leaf_token() {
        let l = live(&["C:\\Users\\u\\AppData\\Local\\Programs\\Ollama"]);
        assert!(is_live_app("ollama app.exe", &l));
    }

    #[test]
    fn short_tokens_never_match_on_their_own() {
        let l = live(&["Go Programming Language"]);
        assert!(!is_live_app("go", &l));
    }

    #[test]
    fn significant_tokens_do_match() {
        let l = live(&["Go Programming Language"]);
        assert!(is_live_app("golang", &l) == false, "golang is a different token");
        assert!(is_live_app("programming", &l));
    }

    #[test]
    fn tokens_drops_the_extension_and_splits_on_punctuation() {
        assert_eq!(tokens_of("ollama app.exe"), vec!["ollama", "app"]);
        assert_eq!(tokens_of("Visual-Studio"), vec!["visual", "studio"]);
    }
}
