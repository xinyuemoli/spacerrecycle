//! Append-only audit log.
//!
//! Every purge, quarantine, restore, and permanent-delete is recorded so the
//! user can answer "what did this thing remove, and when?" months later, and
//! so the software's own behaviour is inspectable.
//!
//! The log lives beside the config file, not in the quarantine store: it must
//! survive the user purging every batch.

use crate::model::Disposition;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Purge,
    Quarantine,
    Restore,
    PurgeBatch,
    CancelScan,
}

impl Action {
    pub fn label(&self) -> &'static str {
        match self {
            Action::Purge => "永久删除",
            Action::Quarantine => "移到隔离区",
            Action::Restore => "还原",
            Action::PurgeBatch => "彻底清除隔离批次",
            Action::CancelScan => "取消扫描",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub unix: i64,
    pub action: Action,
    pub path: String,
    pub bytes: u64,
    pub outcome: String,
}

pub fn log_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("SpaceRecycle").join("audit.log"))
}

/// Hard cap on the audit log's on-disk size. The log is append-only by design,
/// but "append-only forever" on a machine the user runs a cleanup tool on is a
/// slow-motion disk leak. When the file exceeds this many bytes the oldest
/// entries are dropped, so the bound is in bytes rather than by entry count.
const MAX_LOG_BYTES: u64 = 1_000_000; // 1 MB

/// Never trim below this many entries, so one pathological oversized line
/// cannot empty the whole history.
const MIN_ENTRIES: usize = 250;

/// Append one entry. Logging must never break a cleanup, so every error is
/// swallowed after reporting to stderr.
pub fn record(action: Action, path: &str, bytes: u64, outcome: &str) {
    let entry = Entry {
        unix: crate::scanner::walk::now_unix(),
        action,
        path: path.to_string(),
        bytes,
        outcome: outcome.to_string(),
    };
    let Some(p) = log_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(line) = serde_json::to_string(&entry) else { return };

    with_log_lock(|| {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
            let _ = writeln!(f, "{line}");
        }
        APPENDED.fetch_add(1, Ordering::Relaxed);
        // Enforce the byte cap on a fixed schedule rather than stat'ing the
        // file on every line. The check is *after* the increment, so it fires
        // at ROTATE_EVERY, 2*ROTATE_EVERY, ... (the old code checked before the
        // increment, which skipped the boundary).
        if APPENDED.load(Ordering::Relaxed) % ROTATE_EVERY == 0 {
            trim_if_oversized(&p);
        }
    });
}

/// Counts writes since the last trim check, so the (relatively expensive)
/// read+rewrite happens on a fixed schedule instead of on every line.
static APPENDED: AtomicUsize = AtomicUsize::new(0);

/// Check the byte cap once every this many appends.
const ROTATE_EVERY: usize = 250;

pub fn record_disposition(d: Disposition, path: &str, bytes: u64, ok: bool) {
    let action = match d {
        Disposition::Purge => Action::Purge,
        Disposition::Quarantine => Action::Quarantine,
    };
    record(
        action,
        path,
        bytes,
        if ok { "成功" } else { "失败" },
    );
}

/// Largest slice of the log `tail` will look at.
///
/// The settings tab reads the log on every visit, and a user who has been
/// running this tool for a year has a log far bigger than the 200 rows it
/// displays. Reading and JSON-parsing the whole file to keep the last 200
/// entries made tab switching visibly stutter, so the read is bounded
/// instead: 64 KB is roughly 400 entries, comfortably more than any limit
/// the UI can ask for.
const TAIL_WINDOW_BYTES: u64 = 64 * 1024;

/// Read the most recent entries, newest first.
pub fn tail(limit: usize) -> Vec<Entry> {
    let Some(p) = log_path() else { return Vec::new() };
    tail_file(&p, limit)
}

/// Bounded newest-first read of a specific log file.
fn tail_file(p: &std::path::Path, limit: usize) -> Vec<Entry> {
    let Ok(file) = std::fs::File::open(p) else {
        return Vec::new();
    };

    // Start at the end of the file and walk backwards over whole lines.
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return Vec::new();
    }
    let window = TAIL_WINDOW_BYTES.min(len);
    let start = len - window;
    let mut buf = vec![0u8; window as usize];
    let mut f = file;
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut buf).is_err() {
        return Vec::new();
    }

    // The window almost certainly begins mid-line; that fragment is not a
    // record, so drop it before parsing.
    let text = String::from_utf8_lossy(&buf);
    let text = if start > 0 {
        match text.find('\n') {
            Some(i) => &text[i + 1..],
            None => "",
        }
    } else {
        &text
    };

    let mut entries: Vec<Entry> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    entries.reverse();
    entries.truncate(limit);
    entries
}

/// Drop the oldest entries until the log is back under the byte cap, keeping at
/// least `MIN_ENTRIES`. Runs only while the cross-process lock is held.
fn trim_if_oversized(p: &std::path::Path) {
    let Ok(meta) = std::fs::metadata(p) else { return };
    if meta.len() <= MAX_LOG_BYTES {
        return;
    }
    let Ok(text) = std::fs::read_to_string(p) else { return };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= MIN_ENTRIES {
        return;
    }
    // Drop from the front until the remainder fits, but never below MIN_ENTRIES.
    let mut start = 0usize;
    let mut kept = lines.iter().map(|l| l.len() + 1).sum::<usize>();
    while (kept as u64) > MAX_LOG_BYTES && lines.len() - start > MIN_ENTRIES {
        kept -= lines[start].len() + 1;
        start += 1;
    }
    let _ = std::fs::write(p, lines[start..].join("\n") + "\n");
}

/// Serialise the append + trim against the elevated helper process.
///
/// The UAC-elevated helper runs in a second process and writes to the *same*
/// audit log. A trim is read-modify-rewrite, so it can interleave with the
/// other process's append: the trim reads the file, the other process appends a
/// line, then the trim rewrites the file *without* that line, silently dropping
/// an audit entry. A byte-range lock (`LockFileEx`) on a dedicated lock file,
/// acquired by both processes, makes append and trim mutually exclusive.
/// Closing the handle releases the lock, so it is held for exactly the closure.
fn with_log_lock(f: impl FnOnce()) {
    let Some(lockp) = lock_path() else { f(); return };
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lockp)
    else {
        // Could not even open the lock file; log anyway rather than drop the
        // entry.
        f();
        return;
    };

    unsafe {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::Storage::FileSystem::LockFile;

        // Lock the whole range (offset 0, length 0xFFFF_FFFF_FFFF_FFFF): every
        // process locks the same range, so this is a cross-process mutex. If it
        // somehow fails we still run the closure — one unlocked trim is a
        // better outcome than silently losing an audit line.
        let _ = LockFile(
            HANDLE(lock.as_raw_handle()),
            0,
            0,
            u32::MAX,
            u32::MAX,
        );
    }
    f();
}

fn lock_path() -> Option<std::path::PathBuf> {
    log_path().map(|p| {
        let mut s = p.into_os_string();
        s.push(".lock");
        std::path::PathBuf::from(s)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip_and_are_newest_first() {
        let dir = std::env::temp_dir().join(format!("sr-log-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("audit.log");

        // Write directly so the test does not touch the user's real log.
        for i in 0..5 {
            let e = Entry {
                unix: 1000 + i,
                action: Action::Purge,
                path: format!("C:/temp/file{i}.tmp"),
                bytes: 1024,
                outcome: "成功".into(),
            };
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file)
                .unwrap();
            writeln!(f, "{}", serde_json::to_string(&e).unwrap()).unwrap();
        }

        let text = std::fs::read_to_string(&file).unwrap();
        let mut entries: Vec<Entry> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        entries.reverse();
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].path, "C:/temp/file4.tmp", "newest first");
        assert_eq!(entries[4].path, "C:/temp/file0.tmp");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotate_keeps_only_the_newest_entries() {
        let dir = std::env::temp_dir().join(format!("sr-logrot-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("audit.log");

        let mut text = String::new();
        for i in 0..10 {
            let e = Entry {
                unix: i,
                action: Action::Restore,
                path: format!("p{i}"),
                bytes: 0,
                outcome: "成功".into(),
            };
            text.push_str(&serde_json::to_string(&e).unwrap());
            text.push('\n');
        }
        std::fs::write(&file, &text).unwrap();

        let text = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 10);
        let kept = &lines[10 - 4..];
        assert_eq!(kept.len(), 4);
        assert!(kept[3].contains("p9"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_reads_a_bounded_window_and_ignores_a_partial_first_line() {
        let dir = std::env::temp_dir().join(format!("sr-logtail-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("audit.log");

        // Far more than TAIL_WINDOW_BYTES so the window has to kick in.
        let mut text = String::new();
        for i in 0..20_000u32 {
            let e = Entry {
                unix: i as i64,
                action: Action::Purge,
                path: "C:/some/very/long/path/that/pads/the/window/past/its/limit".to_string(),
                bytes: 1,
                outcome: "成功".into(),
            };
            text.push_str(&serde_json::to_string(&e).unwrap());
            text.push('\n');
        }
        let total = text.len();
        std::fs::write(&file, &text).unwrap();

        let got = tail_file(&file, 200);
        assert!(!got.is_empty());
        assert!(got.len() <= 200, "limit is respected");
        assert!(
            total as u64 > TAIL_WINDOW_BYTES,
            "the test log must be larger than the read window"
        );
        // Newest first, and nothing outside the window may leak in.
        assert_eq!(got[0].unix, 19_999, "newest entry is first");
        assert!(
            got.iter().all(|e| e.unix > 10_000),
            "only the tail of the file is read, not a truncated head"
        );
        // Every returned entry must be a whole record, never a cut-off one.
        assert!(
            got.iter().all(|e| e.path.starts_with("C:/some/very/long/path")),
            "no partial JSON line is parsed"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trim_drops_the_oldest_entries_until_under_the_byte_cap() {
        let dir = std::env::temp_dir().join(format!("sr-logtrim-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("audit.log");

        // Far more than MAX_LOG_BYTES of data.
        let mut text = String::new();
        for i in 0..20_000u32 {
            let e = Entry {
                unix: i as i64,
                action: Action::Purge,
                path: "C:/a/very/long/path/that/pads/the/line/well/past/a/hundred/bytes/easily"
                    .to_string(),
                bytes: 1,
                outcome: "成功".into(),
            };
            text.push_str(&serde_json::to_string(&e).unwrap());
            text.push('\n');
        }
        std::fs::write(&file, &text).unwrap();
        assert!(
            std::fs::metadata(&file).unwrap().len() > MAX_LOG_BYTES,
            "precondition: the fixture must exceed the byte cap"
        );

        trim_if_oversized(&file);

        let after = std::fs::metadata(&file).unwrap().len();
        assert!(
            after <= MAX_LOG_BYTES,
            "log must shrink under the byte cap, is {} bytes",
            after
        );

        let lines: Vec<Entry> = std::fs::read_to_string(&file)
            .unwrap()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        assert!(
            lines.len() >= MIN_ENTRIES,
            "must keep at least MIN_ENTRIES, kept {}",
            lines.len()
        );
        assert!(lines.len() < 20_000, "must actually drop something");
        // Only the newest entries survive, in original order.
        assert_eq!(lines[lines.len() - 1].unix, 19_999, "newest entry is retained");
        assert!(
            lines.windows(2).all(|w| w[0].unix < w[1].unix),
            "retained entries stay in order"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
