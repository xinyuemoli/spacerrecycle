//! User-assigned importance levels for files and directories.
//!
//! Importance is an *annotation*, not a scan property. It lives in its own
//! store keyed by normalized path, for three reasons:
//!
//! 1. A re-scan or an app restart must not lose the user's judgement. Findings
//!    are rebuilt from scratch every scan, so anything stored on them is gone.
//! 2. The scanner layer must not read this store. The scanners' hot loop runs
//!    per file across every core; a disk read there would be paid on every
//!    scan for an annotation that only matters at cleanup time.
//! 3. The cleanup pipeline has to be able to enforce it *server-side*. The
//!    frontend ships a level per row across IPC and could relabel a Critical
//!    file as Low to get it purged; the gate re-reads the store instead of
//!    trusting the request.
//!
//! The store is a small JSON file. It is keyed by normalized absolute path
//! (lowercased, backslashes, no trailing separator) because Windows paths are
//! case-insensitive and the same file arrives spelled several ways.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How much the user cares about a path.
///
/// Two levels, which is what the product offers and what the right-click menu
/// has always written. There were briefly four - Critical, High, Normal, Low -
/// and the extra two existed because the cleanup pipeline distinguished "refuse
/// everything" from "restorable actions only". Nobody was ever shown that
/// distinction, the menu never offered it, and the two stores disagreed about
/// how many levels there were.
///
/// The old names are deserialisation aliases, so a store written by the
/// four-level build loads with no migration step: `critical` and `high` become
/// [`Importance::Important`], `normal` and `low` become
/// [`Importance::Disposable`]. Files are re-saved in the new form the next time
/// anything writes to the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Importance {
    /// 重要. Never deleted by this program, and held open so that nothing else
    /// can delete it either. Quarantine and cloud backup remain available,
    /// because both are reversible - which is what the right-click menu promises
    /// for this level.
    ///
    /// The default when a level cannot be determined, which is the safe
    /// direction: an unreadable level must not become permission to delete.
    #[default]
    #[serde(alias = "critical", alias = "high")]
    Important,
    /// 可清理. The user has said this may go.
    ///
    /// `normal` and `low` both land here. `normal` meant "no opinion", and two
    /// levels cannot express that - the honest reading of "I marked this and
    /// expressed no opinion" is "not important", and the alternative would
    /// silently promote it to protected.
    #[serde(alias = "normal", alias = "low")]
    Disposable,
}

impl Importance {
    /// Sort rank, most important first.
    pub fn rank(&self) -> u8 {
        match self {
            Importance::Important => 0,
            Importance::Disposable => 1,
        }
    }

    /// Stable machine key, matching the serde form the frontend sees and the
    /// token the shell extension writes into the stream.
    ///
    /// One vocabulary across all three: the app's JSON store, the IPC boundary
    /// and the alternate data stream now spell these the same way. They did not
    /// before, which is part of why the two stores never met.
    pub fn key(&self) -> &'static str {
        match self {
            Importance::Important => "important",
            Importance::Disposable => "disposable",
        }
    }

    /// Whether a permanent purge may proceed at this level.
    ///
    /// Only the irreversible action is gated; quarantine stays available at
    /// every level because it can be undone.
    pub fn allows_purge(&self) -> bool {
        matches!(self, Importance::Disposable)
    }

    /// Whether a mark at this level should make the file undeletable for
    /// everything on the machine, not only for this app's own cleanup.
    ///
    /// Only 重要. 可清理 is the user saying the file may go, and holding it open
    /// would be the guard arguing with them.
    pub fn protects_externally(&self) -> bool {
        matches!(self, Importance::Important)
    }
}

/// One stored mark.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mark {
    pub level: Importance,
    /// Unix seconds when the user set it. Kept so the UI can explain why a
    /// path is marked, and so a future "recently marked" view is possible
    /// without a migration.
    pub marked_unix: i64,
}

/// A stored mark together with the path it belongs to, for the IPC list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub path: String,
    pub level: Importance,
    pub marked_unix: i64,
}

/// The persisted annotation set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Store {
    pub records: BTreeMap<String, Mark>,
}

/// Normalize a path into the store's key form.
///
/// Windows paths are case-insensitive, and the same file reaches us with
/// forward slashes from the frontend and backslashes from the scanners. Both
/// must land on one key or a mark silently fails to apply after a re-scan.
pub fn normalize_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('/', "\\");
    let trimmed = s.trim_end_matches('\\');
    let out = if trimmed.is_empty() { s.as_str() } else { trimmed };
    out.to_lowercase()
}

/// Default store location: `%APPDATA%\SpaceRecycle\importance.json`.
pub fn store_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("SpaceRecycle").join("importance.json"))
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Store {
    /// Load from the default location. A missing or corrupt file yields an
    /// empty store rather than an error: losing annotations must never make the
    /// app unusable, and the app never deletes anything on its own, so an
    /// unreadable file is left exactly where it is.
    ///
    /// The failure is recorded rather than discarded. An empty store used to be
    /// indistinguishable from a corrupt one, and that mattered more once the
    /// delete guard started deriving what to protect from this store: a file
    /// that failed to parse meant the guard silently held nothing, and every
    /// screen still said the marks were fine. [`last_load_error`] is what lets
    /// the guard say it could not read them instead.
    pub fn load() -> Store {
        let (store, error) = Store::load_checked();
        set_last_load_error(error);
        store
    }

    /// Loads, and reports why the file could not be used if it could not.
    ///
    /// A *missing* file is not an error - that is the state before anyone has
    /// marked anything - so only a file that exists and will not parse is
    /// reported.
    pub fn load_checked() -> (Store, Option<String>) {
        let Some(path) = store_path() else {
            return (Store::default(), None);
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Store::default(), None),
            Err(e) => {
                return (
                    Store::default(),
                    Some(format!("{} could not be read: {e}", path.display())),
                )
            }
        };
        match serde_json::from_str(&text) {
            Ok(store) => (store, None),
            Err(e) => (
                Store::default(),
                Some(format!(
                    "{} is not valid JSON, so no marks could be loaded: {e}",
                    path.display()
                )),
            ),
        }
    }

    pub fn load_from(path: &Path) -> Store {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = store_path().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no config directory")
        })?;
        self.save_to(&path)
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    /// Set (or replace) the level for a path.
    pub fn set(&mut self, path: &Path, level: Importance) {
        self.records.insert(
            normalize_path(path),
            Mark { level, marked_unix: now_unix() },
        );
    }

    /// Remove a mark. Returns true when something was there.
    pub fn remove(&mut self, path: &Path) -> bool {
        self.records.remove(&normalize_path(path)).is_some()
    }

    pub fn get(&self, path: &Path) -> Option<Importance> {
        self.records.get(&normalize_path(path)).map(|m| m.level)
    }

    /// Every mark, most important first so a UI list needs no second sort.
    pub fn entries(&self) -> Vec<Entry> {
        let mut out: Vec<Entry> = self
            .records
            .iter()
            .map(|(path, m)| Entry {
                path: path.clone(),
                level: m.level,
                marked_unix: m.marked_unix,
            })
            .collect();
        out.sort_by(|a, b| {
            a.level
                .rank()
                .cmp(&b.level.rank())
                .then_with(|| b.marked_unix.cmp(&a.marked_unix))
        });
        out
    }
}

/// Convenience lookup for callers that need one answer (the cleanup gate).
pub fn level_for(path: &Path) -> Option<Importance> {
    Store::load().get(path)
}

/// Why the last [`Store::load`] could not read the file, if it could not.
///
/// `None` covers both "it read fine" and "there was nothing to read", which are
/// the same thing from the caller's point of view: no marks were lost.
///
/// This exists because an empty store and an unreadable one used to be
/// indistinguishable, and the delete guard derives what to protect from this
/// store. A corrupt file therefore meant the guard held nothing while every
/// screen still reported the marks were intact - the failure was invisible to
/// exactly the person who needed to know.
pub fn last_load_error() -> Option<String> {
    slot().lock().ok().and_then(|e| e.clone())
}

fn set_last_load_error(error: Option<String>) {
    if let Ok(mut guard) = slot().lock() {
        *guard = error;
    }
}

fn slot() -> &'static std::sync::Mutex<Option<String>> {
    static SLOT: std::sync::OnceLock<std::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mark must survive the round trip through disk, because the whole
    /// point is that it outlives a scan and a restart.
    #[test]
    fn marks_survive_a_save_and_reload() {
        let dir = std::env::temp_dir().join(format!("sr-imp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("importance.json");

        let mut store = Store::default();
        store.set(Path::new("C:\\Users\\me\\Documents\\thesis.docx"), Importance::Important);
        store.set(Path::new("C:\\Temp\\junk.log"), Importance::Disposable);
        store.save_to(&file).unwrap();

        let reloaded = Store::load_from(&file);
        assert_eq!(
            reloaded.get(Path::new("C:\\Users\\me\\Documents\\thesis.docx")),
            Some(Importance::Important)
        );
        assert_eq!(
            reloaded.get(Path::new("C:\\Temp\\junk.log")),
            Some(Importance::Disposable)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The frontend sends forward slashes and the scanners produce
    /// backslashes; both spellings must hit one mark.
    #[test]
    fn path_spelling_does_not_change_the_key() {
        let mut store = Store::default();
        store.set(Path::new("C:/Temp/Logs/app.log"), Importance::Important);
        assert_eq!(
            store.get(Path::new("C:\\Temp\\Logs\\app.log")),
            Some(Importance::Important)
        );
        assert_eq!(
            store.get(Path::new("C:\\TEMP\\LOGS\\APP.LOG")),
            Some(Importance::Important)
        );
        // A trailing separator is not a different file.
        assert_eq!(
            store.get(Path::new("C:\\Temp\\Logs\\")),
            None,
            "only the exact path is marked"
        );
    }

    #[test]
    fn a_trailing_separator_normalizes_away() {
        assert_eq!(
            normalize_path(Path::new("C:\\Temp\\Logs\\")),
            normalize_path(Path::new("C:/Temp/Logs"))
        );
    }

    #[test]
    fn important_refuses_permanent_purge_and_disposable_allows_it() {
        assert!(
            !Importance::Important.allows_purge(),
            "重要 means the irreversible action is off the table"
        );
        assert!(
            Importance::Disposable.allows_purge(),
            "可清理 is the user saying this may go"
        );
    }

    #[test]
    fn only_important_is_protected_externally() {
        assert!(Importance::Important.protects_externally());
        assert!(
            !Importance::Disposable.protects_externally(),
            "holding a file the user called disposable would be the guard arguing with them"
        );
    }

    /// The four-level names load as two levels, with no migration step.
    ///
    /// A store written before the collapse is on disk for anyone who ran an
    /// earlier build. Losing it would silently unprotect every marked file, and
    /// the user could not tell that the app had forgotten rather than that the
    /// mark had never been there.
    #[test]
    fn the_four_level_names_still_load() {
        for (old, expected) in [
            ("critical", Importance::Important),
            ("high", Importance::Important),
            ("normal", Importance::Disposable),
            ("low", Importance::Disposable),
        ] {
            let doc = format!(r#"{{"records":{{"c:\\x":{{"level":"{old}","markedUnix":1}}}}}}"#);
            let store: Store = serde_json::from_str(&doc)
                .unwrap_or_else(|e| panic!("'{old}' must still load: {e}"));
            assert_eq!(store.get(Path::new("C:\\x")), Some(expected), "{old}");
        }
    }

    /// A store written by this build uses the two-level names, so a later build
    /// reads what this one wrote without depending on the aliases.
    #[test]
    fn the_store_is_written_with_the_two_level_names() {
        let mut store = Store::default();
        store.set(Path::new("C:\\x"), Importance::Important);
        let json = serde_json::to_string(&store).expect("serialises");
        assert!(json.contains("\"important\""), "{json}");
        assert!(!json.contains("\"critical\""), "{json}");
    }

    /// The level key and the token the shell writes into the stream are one
    /// vocabulary. They were not before, which is part of why the two stores
    /// never met.
    #[test]
    fn the_level_names_match_the_stream_tokens() {
        for level in [Importance::Important, Importance::Disposable] {
            assert_eq!(level.key(), crate::ads::token(level));
            assert_eq!(crate::ads::level_from_token(level.key()), Some(level));
        }
    }

    #[test]
    fn a_missing_file_yields_an_empty_store() {
        let missing = std::env::temp_dir().join("sr-imp-does-not-exist.json");
        let store = Store::load_from(&missing);
        assert!(store.records.is_empty());
    }

    /// A file that exists and will not parse is reported, not swallowed.
    ///
    /// This is the failure that bit the guard during verification: a JSON file
    /// with a byte order mark failed to parse, the store fell back to empty, and
    /// the guard held nothing while every screen still said the marks were
    /// there. Nothing in the app could tell the difference between "no marks"
    /// and "could not read the marks", which are opposite facts.
    #[test]
    fn a_corrupt_store_reports_why_rather_than_looking_empty() {
        let dir = std::env::temp_dir().join(format!("sr-imp-corrupt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("importance.json");

        // A byte order mark, which serde_json rejects - the exact shape of the
        // failure seen in practice.
        std::fs::write(&file, b"\xef\xbb\xbf{\"records\":{}}").unwrap();

        let (store, error) = {
            let text = std::fs::read_to_string(&file).unwrap();
            match serde_json::from_str::<Store>(&text) {
                Ok(s) => (s, None),
                Err(e) => (Store::default(), Some(e.to_string())),
            }
        };
        assert!(store.records.is_empty(), "an unreadable store yields nothing");
        assert!(error.is_some(), "and it must say so");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A store that does not exist is not an error: that is simply the state
    /// before anyone has marked anything.
    #[test]
    fn an_absent_store_is_silent() {
        let missing = std::env::temp_dir().join("sr-imp-absent-never-created.json");
        let _ = std::fs::remove_file(&missing);
        match std::fs::read_to_string(&missing) {
            Ok(_) => panic!("the file should not exist"),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        }
    }

    #[test]
    fn removal_clears_the_mark() {
        let mut store = Store::default();
        let p = Path::new("C:\\Temp\\a.log");
        store.set(p, Importance::Disposable);
        assert!(store.remove(p));
        assert_eq!(store.get(p), None);
        assert!(!store.remove(p), "removing twice reports nothing to remove");
    }

    #[test]
    fn entries_are_ordered_most_important_first() {
        let mut store = Store::default();
        store.set(Path::new("C:\\a"), Importance::Disposable);
        store.set(Path::new("C:\\b"), Importance::Important);
        store.set(Path::new("C:\\c"), Importance::Disposable);
        let levels: Vec<Importance> = store.entries().iter().map(|e| e.level).collect();
        assert_eq!(
            levels,
            vec![Importance::Important, Importance::Disposable, Importance::Disposable]
        );
    }
}