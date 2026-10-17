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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Importance {
    /// Do not touch. The cleanup pipeline refuses these outright, for every
    /// disposition: the user said this matters, and an irreversible action is
    /// not worth a second guess.
    Critical,
    /// Restorable actions only. Quarantine and cloud backup are allowed;
    /// permanent purge is not.
    High,
    /// No opinion. The category's own default applies.
    #[default]
    Normal,
    /// Explicitly disposable. The category may still refuse, but the user's
    /// own judgement does not stand in the way.
    Low,
}

impl Importance {
    /// Sort rank, most important first.
    pub fn rank(&self) -> u8 {
        match self {
            Importance::Critical => 0,
            Importance::High => 1,
            Importance::Normal => 2,
            Importance::Low => 3,
        }
    }

    /// Stable machine key, matching the serde form the frontend sees.
    pub fn key(&self) -> &'static str {
        match self {
            Importance::Critical => "critical",
            Importance::High => "high",
            Importance::Normal => "normal",
            Importance::Low => "low",
        }
    }

    /// Whether a permanent purge may proceed at this level. Only the
    /// irreversible action is gated; quarantine stays available at every
    /// level because it can be undone.
    pub fn allows_purge(&self) -> bool {
        !matches!(self, Importance::Critical | Importance::High)
    }

    /// Whether a mark at this level should make the file undeletable for
    /// everything on the machine, not only for this app's own cleanup.
    ///
    /// Only `Critical`, and the choice is deliberate rather than an oversight.
    /// `Critical` already refuses every action this program can take, so
    /// arming it changes nothing about what the app does - it only adds
    /// protection against everything else. `High` still permits quarantine and
    /// cloud backup, and a deny entry blocks the move into the quarantine store
    /// as well, so arming it would quietly remove a choice the user has today.
    /// Widening this is a product decision, not a bug fix.
    ///
    /// When the four levels collapse to the two the product actually wants -
    /// 重要 and 可清理 - this is the 重要 side.
    pub fn protects_externally(&self) -> bool {
        matches!(self, Importance::Critical)
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
    /// empty store rather than an error: losing annotations must never make
    /// the app unusable, and the app never deletes anything on its own, so an
    /// unreadable file is left exactly where it is.
    pub fn load() -> Store {
        match store_path() {
            Some(p) => Store::load_from(&p),
            None => Store::default(),
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
        store.set(Path::new("C:\\Users\\me\\Documents\\thesis.docx"), Importance::Critical);
        store.set(Path::new("C:\\Temp\\junk.log"), Importance::Low);
        store.save_to(&file).unwrap();

        let reloaded = Store::load_from(&file);
        assert_eq!(
            reloaded.get(Path::new("C:\\Users\\me\\Documents\\thesis.docx")),
            Some(Importance::Critical)
        );
        assert_eq!(
            reloaded.get(Path::new("C:\\Temp\\junk.log")),
            Some(Importance::Low)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The frontend sends forward slashes and the scanners produce
    /// backslashes; both spellings must hit one mark.
    #[test]
    fn path_spelling_does_not_change_the_key() {
        let mut store = Store::default();
        store.set(Path::new("C:/Temp/Logs/app.log"), Importance::High);
        assert_eq!(
            store.get(Path::new("C:\\Temp\\Logs\\app.log")),
            Some(Importance::High)
        );
        assert_eq!(
            store.get(Path::new("C:\\TEMP\\LOGS\\APP.LOG")),
            Some(Importance::High)
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
    fn critical_and_high_refuse_permanent_purge() {
        assert!(!Importance::Critical.allows_purge());
        assert!(!Importance::High.allows_purge());
        assert!(Importance::Normal.allows_purge());
        assert!(Importance::Low.allows_purge());
    }

    #[test]
    fn a_missing_file_yields_an_empty_store() {
        let missing = std::env::temp_dir().join("sr-imp-does-not-exist.json");
        let store = Store::load_from(&missing);
        assert!(store.records.is_empty());
    }

    #[test]
    fn removal_clears_the_mark() {
        let mut store = Store::default();
        let p = Path::new("C:\\Temp\\a.log");
        store.set(p, Importance::Low);
        assert!(store.remove(p));
        assert_eq!(store.get(p), None);
        assert!(!store.remove(p), "removing twice reports nothing to remove");
    }

    #[test]
    fn entries_are_ordered_most_important_first() {
        let mut store = Store::default();
        store.set(Path::new("C:\\a"), Importance::Low);
        store.set(Path::new("C:\\b"), Importance::Critical);
        store.set(Path::new("C:\\c"), Importance::Normal);
        let levels: Vec<Importance> = store.entries().iter().map(|e| e.level).collect();
        assert_eq!(
            levels,
            vec![Importance::Critical, Importance::Normal, Importance::Low]
        );
    }
}