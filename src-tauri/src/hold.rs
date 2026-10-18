//! The user-mode delete guard: hold a file open, and Windows refuses to delete it.
//!
//! # Why this exists
//!
//! The obvious way to stop a delete is a deny ACE, and it does not work. That
//! was measured, not assumed: with `DENY Everyone:(D)` on a file, an explicit
//! `CreateFile` asking for DELETE is refused, and the file is deleted anyway by
//! `del`. Windows takes deletion rights from two places - DELETE on the object,
//! **or `FILE_DELETE_CHILD` on its parent directory** - and a user's own folder
//! grants the latter by default, so the ACE is never the thing deciding.
//!
//! Denying `FILE_DELETE_CHILD` on the parent does block it, and is unusable:
//! it freezes every file in that folder, and marking one document important
//! cannot mean disabling deletion in `Documents`.
//!
//! A minifilter can intercept the operation properly, and that is what the
//! driver in `driver/` does. It also needs a kernel-signed driver, which needs
//! an EV code-signing certificate, which is issued only to organisations. For
//! an individual developer that path is closed.
//!
//! Holding a handle is not closed. `DeleteFile` fails with
//! `ERROR_SHARING_VIOLATION` when an open handle to the file was created without
//! `FILE_SHARE_DELETE`, and that is enforced by the I/O manager rather than by
//! an ACL - so it does not consult the parent directory, needs no driver, and
//! needs no privilege. Measured as a standard user: deleted with no handle,
//! refused while one was held, deleted again once it was released.
//!
//! # What it costs
//!
//! Three real limitations, all of them consequences of the mechanism rather than
//! of this implementation:
//!
//! * **Lifetime.** The handle lives in this process, so protection ends when the
//!   process does. The driver-based guard survives because a service holds it;
//!   this one is only as resident as the app is.
//! * **Renaming and moving are blocked too.** A rename needs DELETE on the
//!   source, so a held file cannot be moved or renamed either. An approved
//!   operation has to be performed with the handle released.
//! * **The error is a sharing violation**, not "access denied". Windows reports
//!   "the file is in use by another process", which is true and describes the
//!   mechanism rather than the reason.
//!
//! None of those change what the feature promises: the guard is there to stop an
//! accident, not a determined user, and a determined user can end the process.
//!
//! # Two invisible settings decide whether this works at all
//!
//! Both are wrong by default in different ways, and both fail silently - a
//! guard that protects nothing behaves exactly like one that works, right up
//! until something is deleted.
//!
//! **The share mode.** `std::fs::File::open` does not block deletion. Rust sets
//! the default share mode to `FILE_SHARE_READ | FILE_SHARE_WRITE |
//! FILE_SHARE_DELETE`, to match POSIX semantics where an open file can still be
//! unlinked. That default is precisely wrong here. The share mode is therefore
//! set explicitly, to read and write but not delete. Read and write are shared
//! so that the file stays usable: a held document can still be opened and
//! edited, which is the point of protecting it rather than locking it away.
//!
//! **The access mode.** Asking for `access_mode(0)` - no rights at all, since
//! the handle is wanted only for its sharing effect - protects nothing. A
//! handle with `dwDesiredAccess = 0` does not participate in the delete sharing
//! check, so the file deletes as if no handle existed. It was the first thing
//! tried here and it took a measurement to notice. `GENERIC_READ` is required,
//! and the cost is that a file has to be readable to be protected; an
//! unreadable one is reported as a failure rather than counted as protected.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use windows::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE};
use std::os::windows::fs::OpenOptionsExt;

/// Open reads and writes, but not deletes.
///
/// Not sharing delete is the entire mechanism. Sharing read and write keeps the
/// file usable while it is protected.
const SHARE_MODE: u32 = FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0;

/// A file held open so that it cannot be deleted.
///
/// The handle has to be kept alive; dropping it releases the protection. That is
/// why the set is a map of open handles rather than a set of paths.
struct Held {
    /// Kept only so the handle stays open. Never read from or written to.
    _handle: std::fs::File,
    /// Recorded separately because `File` does not carry the path it was opened
    /// from, and an error message that cannot name the file is not useful.
    path: PathBuf,
}

/// The set of paths currently held open.
///
/// One guard per process. The app holds the marked files for as long as it runs.
#[derive(Default)]
pub struct HoldGuard {
    held: Mutex<HashMap<PathBuf, Held>>,
}

impl HoldGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens `path` and keeps the handle, so that deleting it fails.
    ///
    /// Read access, deliberately, and **not** `access_mode(0)`.
    ///
    /// Asking for no access looks like the careful choice - a handle is wanted
    /// for its sharing effect, so why require the right to read anything? - and
    /// it silently protects nothing. A handle opened with `dwDesiredAccess = 0`
    /// does not take part in the delete sharing check, so the file deletes
    /// exactly as if no handle existed. Measured, four combinations, all
    /// consistent: access 0 never blocks whatever the share mode says, and
    /// GENERIC_READ always does.
    ///
    /// The cost is that the file must be readable by this process to be
    /// protected. That is the honest trade: an unreadable file is reported as a
    /// failure by [`HoldGuard::sync`] rather than appearing protected while
    /// being deletable.
    ///
    /// `FILE_FLAG_BACKUP_SEMANTICS` is what lets a directory be opened the same
    /// way. A held directory cannot be removed; the files inside it are
    /// unaffected, because they are deleted by holding DELETE on themselves.
    pub fn protect(&self, path: &Path) -> Result<(), String> {
        let mut held = self.held.lock().map_err(|_| "the hold set is poisoned".to_string())?;
        if held.contains_key(path) {
            return Ok(());
        }

        let handle = std::fs::OpenOptions::new()
            // Read, not zero: see the note above. This is the line the whole
            // mechanism rests on.
            .read(true)
            .share_mode(SHARE_MODE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(path)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => format!("{} no longer exists", path.display()),
                std::io::ErrorKind::PermissionDenied => {
                    format!("{} could not be opened to protect it: {e}", path.display())
                }
                _ => format!("{} could not be held open: {e}", path.display()),
            })?;

        held.insert(
            path.to_path_buf(),
            Held {
                _handle: handle,
                path: path.to_path_buf(),
            },
        );
        Ok(())
    }

    /// Releases the hold on one path, letting it be deleted again.
    ///
    /// Reports whether anything was held. A path that was not held is not an
    /// error: standing down a guard that is not up is the desired end state
    /// either way.
    pub fn release(&self, path: &Path) -> bool {
        match self.held.lock() {
            Ok(mut held) => held.remove(path).is_some(),
            Err(_) => false,
        }
    }

    /// Releases every hold. This is what the "allow the delete" disposition does.
    pub fn release_all(&self) -> usize {
        match self.held.lock() {
            Ok(mut held) => {
                let n = held.len();
                held.clear();
                n
            }
            Err(_) => 0,
        }
    }

    pub fn is_protected(&self, path: &Path) -> bool {
        self.held
            .lock()
            .map(|held| held.contains_key(path))
            .unwrap_or(false)
    }

    pub fn protected_count(&self) -> usize {
        self.held.lock().map(|held| held.len()).unwrap_or(0)
    }

    pub fn protected_paths(&self) -> Vec<PathBuf> {
        self.held
            .lock()
            .map(|held| {
                let mut v: Vec<PathBuf> = held.values().map(|h| h.path.clone()).collect();
                v.sort();
                v
            })
            .unwrap_or_default()
    }

    /// Makes the held set exactly `paths`: opens what is missing, releases what
    /// is no longer wanted.
    ///
    /// Used after a mark changes, where the desired set is known and diffing it
    /// here avoids the caller having to remember what it asked for last time.
    ///
    /// Returns the paths that could not be opened. A failure to protect one file
    /// does not abandon the rest - and it must not, because the alternative is
    /// that one unreadable file silently disables protection for every other.
    pub fn sync(&self, paths: &[PathBuf]) -> Vec<(PathBuf, String)> {
        // Owned copies, so that the lookup can take a `&Path`: `PathBuf`
        // implements `Borrow<Path>`, which is what lets `retain` be handed the
        // map's key directly instead of allocating one per entry.
        let wanted: std::collections::HashSet<PathBuf> = paths.iter().cloned().collect();

        self.retain_where(|p| wanted.contains(p));

        let mut failures = Vec::new();
        for path in paths {
            if let Err(e) = self.protect(path) {
                failures.push((path.clone(), e));
            }
        }
        failures
    }

    /// Drops every hold for which `keep` is false.
    fn retain_where<F: Fn(&Path) -> bool>(&self, keep: F) {
        if let Ok(mut held) = self.held.lock() {
            held.retain(|path, _| keep(path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both settings that the mechanism depends on, pinned by measurement.
    ///
    /// Two ways to build a guard that protects nothing, both of which look
    /// correct: share delete (Rust's default), and ask for no access. This
    /// asserts the working combination and both failure modes, so that neither
    /// can come back unnoticed.
    #[test]
    fn the_share_and_access_modes_are_both_load_bearing() {
        use windows::Win32::Storage::FileSystem::FILE_SHARE_DELETE;

        #[derive(Clone, Copy)]
        enum Case {
            /// What protect() uses.
            ReadSharingReadWrite,
            /// Rust's default share mode: opens fine, blocks nothing.
            ReadSharingEverything,
            /// No access rights: blocks nothing, whatever the share mode.
            NoAccessSharingReadWrite,
            /// No access and no delete sharing: still blocks nothing.
            NoAccessSharingReadWriteNoDelete,
        }

        for (i, case) in [
            Case::ReadSharingReadWrite,
            Case::ReadSharingEverything,
            Case::NoAccessSharingReadWrite,
            Case::NoAccessSharingReadWriteNoDelete,
        ]
        .into_iter()
        .enumerate()
        {
            let path = std::env::temp_dir().join(format!("sr-access-{}-{i}.txt", std::process::id()));
            std::fs::write(&path, b"x").expect("write");

            let mut opts = std::fs::OpenOptions::new();
            opts.read(true);
            match case {
                Case::ReadSharingReadWrite => {
                    opts.share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0);
                }
                Case::ReadSharingEverything => {
                    opts.share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
                }
                Case::NoAccessSharingReadWrite => {
                    opts.access_mode(0).share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0);
                }
                Case::NoAccessSharingReadWriteNoDelete => {
                    opts.access_mode(0).share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
                }
            }

            let handle = opts.open(&path).expect("the scratch file opens either way");
            let deleted = std::fs::remove_file(&path).is_ok();

            let expected_to_block = matches!(case, Case::ReadSharingReadWrite);
            assert_eq!(
                !deleted,
                expected_to_block,
                "{}: {}",
                match case {
                    Case::ReadSharingReadWrite => "read access with delete NOT shared must block",
                    Case::ReadSharingEverything => "the default share mode includes delete and must not block",
                    Case::NoAccessSharingReadWrite => "a zero-access handle does not join the sharing check",
                    Case::NoAccessSharingReadWriteNoDelete => "still zero access",
                },
                if deleted { "it was deleted" } else { "it survived" }
            );

            drop(handle);
            let _ = std::fs::remove_file(&path);
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sr-hold-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join(name);
        std::fs::write(&path, b"important").expect("write scratch file");
        path
    }

    /// The test that matters, and the reason the share mode is set explicitly.
    ///
    /// `File::open` shares delete, so a guard built on it would pass every other
    /// test in this module and protect nothing. This asks the filesystem the
    /// same question the delete path asks.
    #[test]
    fn a_held_file_cannot_be_deleted() {
        let guard = HoldGuard::new();
        let path = scratch("held.txt");

        std::fs::remove_file(&path).expect("deletable before it is held");

        std::fs::write(&path, b"important").expect("write again");
        guard.protect(&path).expect("protect");
        assert!(guard.is_protected(&path));

        let err = std::fs::remove_file(&path)
            .expect_err("a held file must not be deletable, and Rust shares delete by default");
        assert!(
            err.raw_os_error().is_some(),
            "expected a Windows sharing violation, got {err:?}"
        );
        assert!(path.exists(), "the file was deleted despite being held");

        guard.release(&path);
        std::fs::remove_file(&path).expect("released, so it deletes again");
    }

    #[test]
    fn releasing_lets_the_delete_through() {
        let guard = HoldGuard::new();
        let path = scratch("release.txt");
        guard.protect(&path).expect("protect");
        assert!(std::fs::remove_file(&path).is_err(), "held, so refused");

        assert!(guard.release(&path));
        std::fs::remove_file(&path).expect("released, so allowed");
        assert!(!guard.is_protected(&path));
    }

    #[test]
    fn releasing_something_that_was_never_held_is_not_an_error() {
        let guard = HoldGuard::new();
        assert!(!guard.release(Path::new("C:/does/not/exist")));
    }

    #[test]
    fn protecting_a_missing_file_reports_it_rather_than_panicking() {
        let guard = HoldGuard::new();
        let err = guard
            .protect(Path::new("C:/does/not/exist/at/all.txt"))
            .expect_err("must fail");
        assert!(err.contains("no longer exists"), "{err}");
        assert_eq!(guard.protected_count(), 0);
    }

    #[test]
    fn protecting_twice_keeps_one_handle() {
        let guard = HoldGuard::new();
        let path = scratch("twice.txt");
        guard.protect(&path).expect("first");
        guard.protect(&path).expect("second is a no-op");
        assert_eq!(guard.protected_count(), 1);
        guard.release_all();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn release_all_clears_everything() {
        let guard = HoldGuard::new();
        let a = scratch("all-a.txt");
        let b = scratch("all-b.txt");
        guard.protect(&a).expect("a");
        guard.protect(&b).expect("b");
        assert_eq!(guard.release_all(), 2);
        assert_eq!(guard.protected_count(), 0);
        std::fs::remove_file(&a).expect("a is free");
        std::fs::remove_file(&b).expect("b is free");
    }

    /// Standing down the guard has to make the files deletable, or "allow the
    /// delete" is a lie.
    #[test]
    fn the_allow_disposition_really_allows() {
        let guard = HoldGuard::new();
        let path = scratch("allow.txt");
        guard.protect(&path).expect("protect");
        assert!(std::fs::remove_file(&path).is_err());

        guard.release_all();
        std::fs::remove_file(&path).expect("allow must mean allow");
    }

    #[test]
    fn sync_opens_what_is_new_and_drops_what_is_gone() {
        let guard = HoldGuard::new();
        let a = scratch("sync-a.txt");
        let b = scratch("sync-b.txt");

        guard.sync(&[a.clone()]);
        assert!(guard.is_protected(&a));
        assert!(!guard.is_protected(&b));

        // b is added and a is dropped in one call.
        guard.sync(&[b.clone()]);
        assert!(!guard.is_protected(&a), "a was dropped from the wanted set");
        assert!(guard.is_protected(&b));

        std::fs::remove_file(&a).expect("a is no longer held");
        std::fs::remove_file(&b).expect_err("b is held");

        guard.release_all();
        let _ = std::fs::remove_file(&b);
    }

    /// One unreadable file must not cost every other file its protection.
    #[test]
    fn one_failure_does_not_abandon_the_rest() {
        let guard = HoldGuard::new();
        let good = scratch("good.txt");
        let missing = good.with_file_name("missing.txt");
        let _ = std::fs::remove_file(&missing);

        let failures = guard.sync(&[missing.clone(), good.clone()]);
        assert_eq!(failures.len(), 1, "exactly the missing one should be reported");
        assert_eq!(failures[0].0, missing);
        assert!(guard.is_protected(&good), "the readable file was given up on");

        guard.release_all();
        let _ = std::fs::remove_file(&good);
    }

    /// A directory can be held, which is what `FILE_FLAG_BACKUP_SEMANTICS` is
    /// for. Without it the open fails outright and a marked folder is silently
    /// unprotected.
    ///
    /// The assertion is on removal, not on the open succeeding: opening a
    /// directory is easy to get right and irrelevant if the handle does not
    /// actually block anything.
    #[test]
    fn a_held_directory_cannot_be_removed() {
        let guard = HoldGuard::new();
        let dir = std::env::temp_dir().join(format!("sr-hold-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");

        guard.protect(&dir).expect("a directory must be openable for protection");
        assert!(guard.is_protected(&dir));
        std::fs::remove_dir(&dir).expect_err("a held directory must not be removable");

        guard.release_all();
        std::fs::remove_dir(&dir).expect("released, so removable");
    }

    /// The protected file stays usable. A guard that also blocked reading or
    /// writing would be a lock, not protection.
    #[test]
    fn a_held_file_is_still_readable_and_writable() {
        let guard = HoldGuard::new();
        let path = scratch("usable.txt");
        guard.protect(&path).expect("protect");

        let contents = std::fs::read_to_string(&path).expect("still readable");
        assert_eq!(contents, "important");
        std::fs::write(&path, b"edited").expect("still writable");

        guard.release_all();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn held_paths_are_listed_in_a_stable_order() {
        let guard = HoldGuard::new();
        let a = scratch("order-a.txt");
        let b = scratch("order-b.txt");
        guard.protect(&b).expect("b");
        guard.protect(&a).expect("a");

        let listed = guard.protected_paths();
        assert_eq!(listed.len(), 2);
        let mut sorted = listed.clone();
        sorted.sort();
        assert_eq!(listed, sorted, "the listing must not depend on hash order");

        guard.release_all();
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }
}
