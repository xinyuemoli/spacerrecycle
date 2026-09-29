//! Last line of defense before anything is deleted or quarantined.
//!
//! Every rule in the scanner layer can be wrong. This module assumes the rules
//! are wrong and refuses anything that looks like it could destroy data the
//! user cares about.

use std::path::{Path, PathBuf};

/// How a protected root applies to paths beneath it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Only this exact path is protected. Its children are fair game.
    ///
    /// This matters for container directories: the user profile root itself
    /// must never be deleted, but everything under it (AppData, Temp, ...) is
    /// exactly what this tool exists to clean.
    ExactOnly,
    /// This path and every descendant is protected.
    Subtree,
}

struct Rule {
    path: PathBuf,
    scope: Scope,
}

fn builtin_rules() -> &'static [Rule] {
    // Built once per process: the rules depend only on environment variables
    // and the user's home directory, neither of which changes while the app
    // runs. Rebuilding on every `is_allowed` call meant a `dirs::home_dir()`
    // round trip (SHGetKnownFolderPath) plus several allocations per file in
    // the scanners' parallel hot loop.
    static RULES: std::sync::OnceLock<Vec<Rule>> = std::sync::OnceLock::new();
    RULES.get_or_init(|| {
    let mut rules: Vec<Rule> = Vec::new();
    let mut sub = |p: PathBuf, scope: Scope| rules.push(Rule { path: normalize(&p), scope });

    if let Some(windir) = std::env::var_os("SystemRoot") {
        // SystemRoot *is* the Windows directory (C:\Windows). The old rule
        // joined "Windows", producing C:\Windows\Windows and leaving the real
        // Windows dir unprotected. A whole-subtree rule is the opposite
        // mistake: the temp scanner's own targets (C:\Windows\Temp, Prefetch,
        // Logs\CBS, MEMORY.DMP) live under it, and blocking the whole tree
        // would make those findings fail the safety gate. Protect the Windows
        // directory itself plus the irreplaceable component stores, and leave
        // the temp subdirectories scannable.
        let w = PathBuf::from(&windir);
        sub(w.clone(), Scope::ExactOnly);
        for component in ["System32", "SysWOW64", "WinSxS"] {
            sub(w.join(component), Scope::Subtree);
        }
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(p) = std::env::var_os(var) {
            sub(PathBuf::from(p), Scope::Subtree);
        }
    }
    if let Some(pd) = std::env::var_os("ProgramData") {
        // Only the root itself: individual app subfolders are legitimate
        // cleanup targets, but the container must never vanish.
        sub(PathBuf::from(pd), Scope::ExactOnly);
    }
    if let Some(home) = dirs::home_dir() {
        // Same reasoning as ProgramData: protect the container, not the tree.
        sub(home.clone(), Scope::ExactOnly);
        for known in ["Desktop", "Documents", "Pictures", "Music", "Videos", "Downloads"] {
            sub(home.join(known), Scope::Subtree);
        }
    }
    rules
    })
}

/// The quarantine store itself must never be a scan target, or a scan could
/// quarantine the quarantine.
pub fn quarantine_root(cfg: &crate::config::Config) -> PathBuf {
    normalize(&cfg.quarantine_dir)
}

fn normalize(p: &Path) -> PathBuf {
    let s = p.to_string_lossy().replace('/', "\\");
    let trimmed = s.trim_end_matches('\\').to_string();
    PathBuf::from(trimmed)
}

fn is_under(child: &Path, root: &Path) -> bool {
    let c = normalize(child).to_string_lossy().to_lowercase();
    let r = normalize(root).to_string_lossy().to_lowercase();
    c == r || c.starts_with(&format!("{}\\", r))
}

fn violates(path: &Path, rule: &Rule) -> bool {
    match rule.scope {
        Scope::ExactOnly => normalize(path) == rule.path,
        Scope::Subtree => is_under(path, &rule.path),
    }
}

/// True when the path is safe to act on. Anything false here is skipped and
/// reported rather than deleted.
pub fn is_allowed(path: &Path, cfg: &crate::config::Config) -> bool {
    // A path must be absolute. Relative paths are a bug, not a user choice.
    if !path.is_absolute() {
        return false;
    }
    // Never touch our own quarantine store.
    if is_under(path, &quarantine_root(cfg)) {
        return false;
    }
    for rule in builtin_rules() {
        if violates(path, rule) {
            return false;
        }
    }
    for root in &cfg.extra_excluded_roots {
        if is_under(path, root) {
            return false;
        }
    }
    true
}

/// Re-verify the age gate at deletion time.
///
/// A finding that was old enough when it was scanned may have been touched in
/// the meantime; re-checking closes that window for the irreversible case
/// (permanent purge). Files age off their access/creation stamps. Directories
/// age off the newest *modification* time in their tree, because enumerating a
/// directory rewrites its own access stamp and would misleadingly read as
/// "brand new". A false negative only blocks a cleanup - the safe direction.
pub fn age_gate_passes(
    path: &Path,
    is_dir: bool,
    category: crate::model::Category,
    cfg: &crate::config::Config,
) -> bool {
    let min_age = category.min_age_days(cfg.large_file_age_days, cfg.leftover_age_days);
    if min_age == 0 {
        // Temp junk, caches and dev artifacts have no age gate.
        return true;
    }
    let age_days = if is_dir {
        match crate::scanner::walk::newest_mtime_in_tree(path, 20_000) {
            Some(newest) => crate::scanner::walk::age_of(0, newest).0,
            None => 0,
        }
    } else {
        let (la, cr) = crate::scanner::walk::timestamps(path);
        crate::scanner::walk::age_of(la, cr).0
    };
    age_days >= min_age
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> crate::config::Config {
        crate::config::Config::default()
    }

    #[test]
    fn relative_paths_are_never_allowed() {
        assert!(!is_allowed(Path::new("relative/path"), &cfg()));
    }

    #[test]
    fn windows_directory_is_denied() {
        let windir = std::env::var("SystemRoot").unwrap();
        // The Windows directory itself and its descendants are off limits.
        assert!(!is_allowed(Path::new(&windir), &cfg()));
        assert!(!is_allowed(&Path::new(&windir).join("System32"), &cfg()));
    }

    #[test]
    fn program_files_is_denied() {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            assert!(!is_allowed(&Path::new(&pf).join("SomeApp"), &cfg()));
        }
    }

    #[test]
    fn user_documents_are_denied() {
        let home = dirs::home_dir().unwrap();
        assert!(!is_allowed(&home.join("Documents"), &cfg()));
        assert!(!is_allowed(&home.join("Desktop"), &cfg()));
    }

    #[test]
    fn the_profile_root_itself_is_denied() {
        let home = dirs::home_dir().unwrap();
        assert!(!is_allowed(&home, &cfg()));
    }

    #[test]
    fn quarantine_store_is_never_a_target() {
        let c = cfg();
        let q = c.quarantine_dir.join("batch-1").join("stuff");
        assert!(!is_allowed(&q, &c));
    }

    #[test]
    fn temp_directory_is_allowed() {
        let temp = std::env::temp_dir().join("some-temp-file.tmp");
        assert!(
            is_allowed(&temp, &cfg()),
            "user temp files are the primary cleanup target and must be allowed"
        );
    }

    #[test]
    fn appdata_is_allowed_so_caches_can_be_cleaned() {
        let home = dirs::home_dir().unwrap();
        assert!(is_allowed(&home.join("AppData").join("Local").join("Temp"), &cfg()));
    }
}
