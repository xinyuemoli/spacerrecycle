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

/// The roots the temp and cache scanners treat as junk wholesale.
///
/// Kept next to the scanners' own list in spirit; a root added there and not
/// here makes the server refuse to vouch for a path it in fact classified,
/// which costs a confirmation box rather than a file.
fn junk_roots() -> Vec<PathBuf> {
    let mut roots = vec![std::env::temp_dir()];
    if let Some(windir) = std::env::var_os("SystemRoot") {
        roots.push(PathBuf::from(&windir).join("Temp"));
        roots.push(PathBuf::from(&windir).join("Prefetch"));
        roots.push(PathBuf::from(&windir).join("Logs").join("CBS"));
    }
    if let Some(local) = dirs::data_local_dir() {
        roots.push(local.join("Microsoft").join("Windows").join("WER"));
        roots.push(local.join("CrashDumps"));
    }
    if let Some(pd) = std::env::var_os("ProgramData") {
        roots.push(PathBuf::from(&pd).join("Microsoft").join("Windows").join("WER"));
    }
    roots
}

/// Whether the server will vouch that this path is junk, on its own evidence.
///
/// # Why a claim of "safe" has to be earned
///
/// The risk tier decides whether a permanent delete needs a confirmation box,
/// and the tier came from the request. A request that called an app-leftover
/// directory a temp file therefore removed the one step standing between the
/// user and an irreversible mistake - not through a bug in the gate, which
/// derives the tier from the category, but because the category itself was
/// whatever the request said.
///
/// A claim of *danger* is never worth checking: nobody attacks this by asking
/// for more confirmation. Only `Safe` is verified, and what counts as evidence
/// differs for a file and a directory - which is the distinction that matters
/// here and is easy to miss.
///
/// **A file** is vouched for when it lies under a junk root. That is exactly
/// where the temp and cache scanners look for individual junk files, so it is
/// the server's own claim rather than the request's.
///
/// **A directory** is vouched for only when it *is* a junk root. Deleting one
/// permanently destroys everything inside it, and the request carries no
/// inventory of that - a directory finding is a statement about a tree, and the
/// tree is not in the request. The scanner makes the same distinction: it emits
/// the junk root itself as a whole-directory finding, and says nothing about an
/// arbitrary directory that happens to sit inside one.
///
/// The cost of a wrong refusal is a confirmation box. The cost of a wrong
/// acceptance is somebody's only copy of something, so the refusal is the
/// direction to err in.
///
/// `is_dir` is asked of the filesystem rather than read from the finding, since
/// the finding is the thing under suspicion.
pub fn vouched_safe(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    // Through the same normalization the rest of the app uses, so a trailing
    // separator or a forward slash cannot be used to dodge the comparison.
    let p = crate::importance::normalize_path(path);

    if path.is_dir() {
        return junk_roots().iter().any(|root| {
            root.is_absolute() && crate::importance::normalize_path(root) == p
        });
    }

    let target = std::path::Path::new(&p);
    junk_roots().iter().any(|root| {
        root.is_absolute() && target.starts_with(&crate::importance::normalize_path(root))
    })
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

    // ---------- vouching for a "safe" claim ----------

    /// A file under a junk root is vouched for; a directory merely *inside* one
    /// is not.
    ///
    /// This is the distinction the whole check turns on, and it is worth stating
    /// why it is not arbitrary. A permanent delete of a directory destroys
    /// everything inside it, and the request carries no inventory of that - so
    /// the server has no evidence of its own about a tree it did not enumerate.
    /// The scanner draws the same line: it emits the junk root itself as a
    /// whole-directory finding and says nothing about a directory that happens
    /// to sit inside one.
    #[test]
    fn a_file_under_a_junk_root_is_vouched_and_a_nested_directory_is_not() {
        let temp = std::env::temp_dir();

        // A real file, really under the temp root.
        let file = temp.join("sr-vouch-probe.tmp");
        std::fs::write(&file, b"x").unwrap();
        assert!(
            vouched_safe(&file),
            "a temp file is what the temp scanner looks for"
        );

        // A real directory, really under the temp root but not one of the roots.
        let dir = temp.join(format!("sr-vouch-probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            !vouched_safe(&dir),
            "a nested directory's contents are not in the request"
        );

        // A file inside that directory is still a file under a junk root, which
        // is the case that keeps ordinary temp cleanup unconfirmed.
        let nested_file = dir.join("a.tmp");
        std::fs::write(&nested_file, b"x").unwrap();
        assert!(vouched_safe(&nested_file));

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&file);
    }

    /// The junk root itself is vouched for, because that is exactly what the
    /// scanner emits as a finding.
    #[test]
    fn a_junk_root_itself_is_vouched() {
        assert!(vouched_safe(&std::env::temp_dir()));
    }

    /// Somewhere the server has no claim about is never vouched for, whatever
    /// the request calls it.
    #[test]
    fn an_unrelated_directory_is_not_vouched() {
        let home = dirs::home_dir().unwrap();
        assert!(!vouched_safe(&home.join("Documents")));
        assert!(!vouched_safe(&home.join("Documents").join("thesis.docx")));
    }

    /// A relative path is a bug, not a user choice, and must never be vouched.
    #[test]
    fn a_relative_path_is_never_vouched() {
        assert!(!vouched_safe(Path::new("some\\relative\\file.tmp")));
    }
}
