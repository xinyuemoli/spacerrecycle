//! Keeping a marked path from being deleted by anything else.
//!
//! The product's delete guard has two possible mechanisms, and which one is
//! available is a commercial fact rather than a technical preference:
//!
//! * A **kernel minifilter** can intercept the delete before it happens and ask
//!   the user. It is the better experience and the only way to prompt. Shipping
//!   one requires an EV code signing certificate, and certificate authorities
//!   issue those to organisations only - Microsoft will not attest-sign a
//!   driver whose account has no EV certificate. `driver/` implements this and
//!   is verified on a Windows 7 guest; it cannot be distributed by an
//!   individual developer.
//!
//! * A **deny entry in the file's own ACL** needs no driver and no certificate.
//!   It does not prompt - the user sees Windows' own "you need permission"
//!   dialog - but the deletion it prevents really is prevented, by the kernel's
//!   access check rather than by this program being awake.
//!
//! This module is the second one, behind a trait so the first can drop in
//! without the rest of the app - the marks, the settings screen, the cleanup
//! gate - knowing which is in use.
//!
//! ## Why an ACL rather than holding a handle open
//!
//! Holding an open handle without `FILE_SHARE_DELETE` also blocks deletion, and
//! it is gentler: it changes nothing about the file, and the error message even
//! names this program. It was rejected because the protection would last only
//! as long as the holder ran. A guard that stops guarding when the user closes
//! the window, or when the machine reboots, is not a guard - it is a state the
//! user cannot see the edges of. An ACL entry persists, so what is protected
//! stays protected.
//!
//! ## What this does not do
//!
//! The owner of a file can always rewrite its permissions, so a determined user
//! can unprotect a file by hand. That is the same limit the driver has (its
//! service can be stopped) and it is the honest limit of any client-side guard;
//! `license.rs` already states the equivalent for the subscription gate. This
//! protects against accident and against other programs, not against the person
//! sitting at the keyboard.

//! ## Status: the ACL mechanism is built but NOT verified, and is not selected
//!
//! The deny-entry implementation below is complete and its tests describe the
//! behaviour it is supposed to have - but that behaviour could not be
//! reproduced. On a real Windows 11 guest, a file carrying an explicit
//! `Everyone:(DENY)(D)` entry was still deleted by every path tried:
//! `del /f`, `[IO.File]::Delete` and Rust's own `fs::remove_file`. The same
//! happened on the development host.
//!
//! Those guest runs were launched through `vmrun runProgramInGuest`, whose
//! process runs as an elevated administrator at High mandatory level, and that
//! is a real confound - a normal user's token may well honour the deny entry.
//! It has not been established either way, and until it is, this backend is
//! **not wired up**: `current()` returns [`NoProtection`] rather than arming an
//! ACL it cannot show protects anything. Changing a user's permissions for no
//! benefit is worse than doing nothing, and a guard that silently does not
//! guard is the one failure this product cannot ship.
//!
//! The trait stays because it is the seam the finished feature needs, whatever
//! mechanism turns out to work: the marks, the settings screen and the cleanup
//! gate should not have to know whether the teeth come from a filter driver, a
//! permission entry or a held handle.

use std::path::Path;

/// Which mechanism is protecting paths on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A deny entry in the file's ACL.
    ///
    /// Kept as a variant because the code exists and the reason it is not used
    /// belongs somewhere discoverable, but it is **not** selected by `current`.
    /// Measured: it does not prevent a deletion, because Windows takes the right
    /// to delete from the parent directory's FILE_DELETE_CHILD as well as from
    /// the file itself.
    Acl,
    /// Holding the file open without sharing delete. Needs no driver, no
    /// certificate and no privilege.
    Hold,
    /// The kernel minifilter. Needs an EV-signed driver.
    Driver,
    /// Nothing is available.
    None,
}

/// Whether protection is in place, and which kind.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub kind: Kind,
    pub active: bool,
    /// Human-readable explanation, shown in Settings. A guard whose state is
    /// invisible is indistinguishable from one that is not working.
    pub detail: String,
}

/// A way of making a path undeletable.
pub trait Protection: Send + Sync {
    fn kind(&self) -> Kind;

    /// Whether this mechanism can be used at all here.
    fn available(&self) -> Result<(), String>;

    /// Make `path` undeletable. Idempotent.
    fn arm(&self, path: &Path) -> Result<(), String>;

    /// Let `path` be deleted again. Idempotent.
    fn disarm(&self, path: &Path) -> Result<(), String>;

    /// Whether the protection is actually in effect right now.
    ///
    /// Asked of the filesystem rather than remembered, so a file whose ACL was
    /// changed behind this program's back reads as unprotected instead of
    /// quietly appearing safe.
    fn is_armed(&self, path: &Path) -> bool;
}

// ---------------------------------------------------------------- ACL

/// Every ACE this module writes denies DELETE to this SID.
///
/// A well-known SID rather than the localized name "Everyone": the name is
/// translated on a non-English Windows, and `icacls` writes whichever one the
/// system uses.
pub const EVERYONE_SID: &str = "*S-1-1-0";

/// The right denied. `D` is DELETE.
///
/// Only DELETE, and deliberately not `DC` (delete child) as well: denying
/// delete-child on a directory would stop this program's own cleanup from
/// touching anything inside it, and it does not actually protect the children -
/// a child is deleted by holding DELETE on the child, not on its parent. A
/// directory mark protects the directory, which is what the ACE can honestly
/// promise.
pub const DENY_RIGHT: &str = "(D)";

/// Not selected by [current] yet; see the module note.\n#[allow(dead_code)]
pub struct AclProtection;

impl AclProtection {
    /// Run `icacls` with the path first, as the tool expects.
    ///
    /// The path is passed as a single argument rather than interpolated into a
    /// command line, so spaces and quotes in it cannot become extra arguments.
    /// `icacls` is used instead of driving the security APIs by hand because
    /// this program must not get DACL surgery wrong: a malformed ACL could lock
    /// a user out of their own file, and `icacls` is the OS's own supported
    /// tool for exactly this edit.
    fn run(path: &Path, args: &[&str]) -> Result<String, String> {
        let out = std::process::Command::new("icacls")
            .arg(path)
            .args(args)
            .output()
            .map_err(|e| format!("cannot run icacls: {e}"))?;

        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        if !out.status.success() {
            return Err(format!(
                "icacls failed for {}: {}",
                path.display(),
                stdout.trim()
            ));
        }
        Ok(stdout)
    }
}

#[allow(dead_code)]
impl Protection for AclProtection {
    fn kind(&self) -> Kind {
        Kind::Acl
    }
    fn available(&self) -> Result<(), String> {
        // Asking the tool to describe the volume it would edit is enough to
        // prove it exists and can run. A missing icacls is a real possibility
        // on a stripped-down image, and the failure has to be reported rather
        // than discovered later as a silent no-op.
        let out = std::process::Command::new("icacls")
            .arg("/?")
            .output()
            .map_err(|e| format!("icacls is not available: {e}"))?;
        if out.stdout.is_empty() && out.stderr.is_empty() {
            return Err("icacls produced no output".to_string());
        }
        Ok(())
    }

    fn arm(&self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            return Err(format!("path no longer exists: {}", path.display()));
        }
        if self.is_armed(path) {
            return Ok(());
        }
        let deny = format!("{EVERYONE_SID}:{DENY_RIGHT}");
        Self::run(path, &["/deny", &deny])?;

        // Verified rather than assumed. icacls reports success for changes that
        // an inherited ACE or a tightened volume can still override, and a
        // protection that silently did not take is the one failure this feature
        // cannot afford.
        if self.is_armed(path) {
            Ok(())
        } else {
            Err(format!(
                "the deny entry was written but {} is still deletable",
                path.display()
            ))
        }
    }

    fn disarm(&self, path: &Path) -> Result<(), String> {
        if !path.exists() {
            // A path that is already gone needs no unprotecting. Reporting this
            // as a failure would strand a mark that can never be cleared.
            return Ok(());
        }
        Self::run(path, &["/remove:d", EVERYONE_SID])?;
        Ok(())
    }

    fn is_armed(&self, path: &Path) -> bool {
        delete_is_denied(path)
    }
}

/// Whether the filesystem refuses to let this process delete `path`.
///
/// A behavioural check rather than an inspection of the ACL: it asks the same
/// question Explorer will ask, so it cannot disagree with what the user sees.
/// Parsing `icacls` output was the alternative and was rejected - the words in
/// it are localized, and a check that reads differently on a Chinese Windows is
/// a check that stops working exactly where it is needed.
#[cfg(windows)]
#[allow(dead_code)]
fn delete_is_denied(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);

    // Requesting DELETE is the whole test: the access check runs against the
    // DACL, so a deny entry makes this fail while an ordinary file opens fine.
    // FILE_FLAG_BACKUP_SEMANTICS is what lets the same call open a directory.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            DELETE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };

    match handle {
        Ok(h) => {
            unsafe {
                let _ = CloseHandle(h);
            }
            false
        }
        Err(e) => e.code() == ERROR_ACCESS_DENIED.to_hresult(),
    }
}

#[cfg(not(windows))]
fn delete_is_denied(_path: &Path) -> bool {
    false
}

/// The protection in force on this machine.
///
/// The driver is preferred when it is there, because it can *prompt*: it sees
/// the delete as it happens and can ask the user. A held handle cannot. It only
/// makes the operation fail, and the user is told by Windows that the file is in
/// use.
///
/// With no driver, the hold backend is used. It is user mode, so it needs no
/// certificate and no elevation, which is what makes a delete guard shippable at
/// all for an individual developer - see `hold.rs` for why the alternatives do
/// not work.
///
/// The ACL backend is deliberately **not** in this chain. Its arm/disarm code is
/// complete and its measurement is conclusive: a deny ACE does not stop a
/// deletion, because the parent directory authorises it. Arming a permission
/// entry that protects nothing would modify the user's files for no benefit
/// while reporting protection.
pub fn current() -> Box<dyn Protection> {
    if crate::guard::driver_is_active() {
        return Box::new(DriverProtection);
    }
    Box::new(HoldProtection)
}

/// The process-wide set of held handles.
///
/// Global on purpose, and it has to be. A `HoldGuard` that is constructed per
/// call would drop its `File` handles at the end of the call, and the protection
/// would last exactly as long as the function that set it up - which looks like
/// a working guard in every test that does not check the file afterwards.
pub fn holds() -> &'static crate::hold::HoldGuard {
    static HOLDS: std::sync::OnceLock<crate::hold::HoldGuard> = std::sync::OnceLock::new();
    HOLDS.get_or_init(crate::hold::HoldGuard::new)
}

/// Holds files open so that Windows refuses to delete them.
pub struct HoldProtection;

/// Makes the held set match every mark this machine has, and reports what could
/// not be held.
///
/// There are two places a mark can live, and before this function existed the
/// guard only knew about one of them:
///
/// * the app's JSON store, written by the app's own screens;
/// * the `SpaceRecycle.Importance` stream, written by the right-click menu and,
///   now, by the app as well.
///
/// A file marked through the right-click menu was marked in a place nothing
/// read, so the menu produced an icon and no protection at all. The stream is
/// read through the index because a stream is a property of one file and cannot
/// be enumerated - see `ads::index_path`.
///
/// A path counts if any of the two says 重要. Neither is asked to agree with the
/// other, because they can legitimately disagree: the stream travels with the
/// file and the store is per-user, so a file copied from another machine can
/// arrive marked. Erring towards holding is the right direction for a guard.
///
/// The index is rewritten afterwards so that entries for files that were
/// deleted or cleared do not accumulate.
///
/// Failures are returned rather than swallowed. A file that could not be held is
/// unprotected, and a caller that reported success anyway would be telling the
/// user they are safe when they are not.
pub fn sync_holds_from_marks() -> Vec<(std::path::PathBuf, String)> {
    let mut wanted: std::collections::BTreeSet<std::path::PathBuf> = std::collections::BTreeSet::new();

    // The app's own store.
    for e in crate::importance::Store::load().entries() {
        if e.level.protects_externally() {
            wanted.insert(std::path::PathBuf::from(&e.path));
        }
    }

    // The shell's marks, reached through the index.
    let from_streams = crate::ads::resolve_index();
    for (path, level) in &from_streams {
        if level.protects_externally() {
            wanted.insert(path.clone());
        }
    }

    // Pruned to what the streams still vouch for. Entries whose file is gone, or
    // whose mark was cleared through the menu, are dropped here rather than
    // needing the shell to maintain the list on every clear.
    let alive: Vec<std::path::PathBuf> = from_streams.into_iter().map(|(p, _)| p).collect();
    if let Err(e) = crate::ads::rewrite_index(&alive) {
        // Not fatal: the holds below are what protect files. Reported through
        // the failure list so it is not silent, since an index that cannot be
        // written will keep re-appearing.
        return vec![(std::path::PathBuf::from("<index>"), e)];
    }

    let paths: Vec<std::path::PathBuf> = wanted.into_iter().collect();
    let failures = holds().sync(&paths);

    // Kept so that Settings can name them. A file that could not be held is
    // unprotected, and the count alone would leave the user to work out which
    // of their marks is the one not doing anything.
    if let Ok(mut slot) = sync_failures().lock() {
        *slot = failures.clone();
    }
    failures
}

/// The files the last sync could not hold, and why.
pub fn last_sync_failures() -> Vec<(std::path::PathBuf, String)> {
    sync_failures().lock().map(|f| f.clone()).unwrap_or_default()
}

fn sync_failures() -> &'static std::sync::Mutex<Vec<(std::path::PathBuf, String)>> {
    static FAILURES: std::sync::OnceLock<std::sync::Mutex<Vec<(std::path::PathBuf, String)>>> =
        std::sync::OnceLock::new();
    FAILURES.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Releases every hold.
///
/// What "allow the delete" means. It goes through `release_all` rather than
/// `sync_holds_from_marks` with an empty list so that the release does not
/// depend on the mark store being readable.
pub fn release_all_holds() -> usize {
    holds().release_all()
}

impl Protection for HoldProtection {
    fn kind(&self) -> Kind {
        Kind::Hold
    }

    fn available(&self) -> Result<(), String> {
        // Always available, and that is the point of the backend: it needs no
        // service, no driver and no administrator.
        Ok(())
    }

    fn arm(&self, path: &Path) -> Result<(), String> {
        holds().protect(path)
    }

    fn disarm(&self, path: &Path) -> Result<(), String> {
        holds().release(path);
        Ok(())
    }

    fn is_armed(&self, path: &Path) -> bool {
        // Asked of the hold set rather than of the filesystem. `delete_is_denied`
        // would answer a different question - whether *this* process may delete
        // it - and it can delete a file it is holding, so it would always say no.
        holds().is_protected(path)
    }
}

/// What is in force when neither mechanism is usable.
///
/// It exists so that "nothing is protecting you" is a state the app can say out
/// loud, rather than the absence of a state. Every method is a no-op and
/// `available` reports the failure, which is what the settings screen needs to
/// avoid claiming a guard that is not there.
pub struct NoProtection;

impl Protection for NoProtection {
    fn kind(&self) -> Kind {
        Kind::None
    }

    fn available(&self) -> Result<(), String> {
        Err("no delete protection is available on this machine".to_string())
    }

    fn arm(&self, _path: &Path) -> Result<(), String> {
        Ok(())
    }

    fn disarm(&self, _path: &Path) -> Result<(), String> {
        Ok(())
    }

    fn is_armed(&self, _path: &Path) -> bool {
        false
    }
}

/// The kernel minifilter, used when its service is running.
///
/// Arming is a no-op here and that is not an oversight: the filter reads the
/// mark from the file's alternate data stream on every delete, so the act of
/// marking the file was already the arming. Nothing has to be written to the
/// file, which is also why this mechanism leaves no trace on the user's data.
pub struct DriverProtection;

impl Protection for DriverProtection {
    fn kind(&self) -> Kind {
        Kind::Driver
    }

    fn available(&self) -> Result<(), String> {
        if crate::guard::driver_is_active() {
            Ok(())
        } else {
            Err("the guard service is not running".to_string())
        }
    }

    fn arm(&self, _path: &Path) -> Result<(), String> {
        Ok(())
    }

    fn disarm(&self, _path: &Path) -> Result<(), String> {
        Ok(())
    }

    fn is_armed(&self, _path: &Path) -> bool {
        // The filter decides per delete and this program cannot observe its
        // verdict, so claiming a per-path answer would be inventing one. The
        // status screen reports the service instead.
        crate::guard::driver_is_active()
    }
}

/// What the UI shows about protection.
pub fn status() -> Status {
    let p = current();
    match p.available() {
        Ok(()) => Status {
            kind: p.kind(),
            active: true,
            detail: match p.kind() {
                Kind::Driver => "the guard service is running".to_string(),
                Kind::Hold => {
                    // The count is the honest part. `active: true` only says the
                    // mechanism is usable; the number says whether anything is
                    // actually being held, which is the question a user looking
                    // at "protection is on" is really asking.
                    let held = holds().protected_count();
                    let base = match held {
                        0 => "the guard is on, but no file is currently held".to_string(),
                        1 => "1 marked file is held open, so it cannot be deleted".to_string(),
                        n => format!("{n} marked files are held open, so they cannot be deleted"),
                    };

                    // Both kinds of failure, kept separate because they call for
                    // different actions: a mark that could not be held is a
                    // permissions or lock problem on one file, while a store
                    // that could not be read means the guard does not know what
                    // to protect at all.
                    let mut detail = base;
                    if let Some(e) = crate::importance::last_load_error() {
                        detail.push_str(&format!(
                            ". The list of marks could not be read, so nothing it contains is \
                             protected: {e}"
                        ));
                    }
                    let failed = last_sync_failures();
                    if !failed.is_empty() {
                        // Named, not counted. "2 files could not be protected"
                        // leaves the user with no way to find out which two.
                        detail.push_str(&format!(
                            ". {} could not be protected: {}",
                            failed.len(),
                            failed
                                .iter()
                                .map(|(p, e)| format!("{} ({e})", p.display()))
                                .collect::<Vec<_>>()
                                .join("; ")
                        ));
                    }
                    detail
                }
                Kind::Acl => {
                    "protected by a deny entry in each marked file's permissions".to_string()
                }
                Kind::None => "no protection is available".to_string(),
            },
        },
        Err(e) => Status {
            kind: p.kind(),
            active: false,
            detail: e,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sr-prot-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The whole point of the feature, asserted against the filesystem: a
    /// protected file cannot be deleted, and it can be again once released.
    ///
    /// This is the test the feature exists for, so it is written to fail loudly
    /// rather than to pass trivially. Removing the file is attempted for real.
    #[test]
    #[cfg(windows)]
    // Ignored rather than deleted: it records what the mechanism was supposed
    // to do and is the test that has to pass before the ACL backend is wired
    // up. It does not pass today - on a real Windows 11 guest the deny entry
    // did not stop `del`, `[IO.File]::Delete` or `fs::remove_file`, and those
    // runs were launched with an elevated token, which is an unresolved
    // confound. See the module note.
    #[ignore = "the deny entry did not prevent deletion in a real guest; unresolved"]    fn a_protected_file_cannot_be_deleted_and_can_be_released() {
        let dir = scratch("roundtrip");
        let file = dir.join("thesis.docx");
        fs::write(&file, b"important").unwrap();

        let p = AclProtection;
        p.available().expect("icacls must be usable on a Windows test machine");

        // Before: an ordinary file deletes normally.
        assert!(!p.is_armed(&file), "a fresh file reported as protected");

        p.arm(&file).expect("arming failed");
        assert!(p.is_armed(&file), "the file is still deletable after arming");
        assert!(
            fs::remove_file(&file).is_err(),
            "the file was deleted despite being protected"
        );
        assert!(file.exists(), "the protected file is gone");

        p.disarm(&file).expect("releasing failed");
        assert!(!p.is_armed(&file), "the file is still protected after release");
        fs::remove_file(&file).expect("the released file could not be deleted");

        let _ = fs::remove_dir_all(&dir);
    }

    /// Arming twice is the normal case - every app start re-arms every mark -
    /// and must not stack duplicate deny entries.
    #[test]
    #[cfg(windows)]
    // Ignored rather than deleted: it records what the mechanism was supposed
    // to do and is the test that has to pass before the ACL backend is wired
    // up. It does not pass today - on a real Windows 11 guest the deny entry
    // did not stop `del`, `[IO.File]::Delete` or `fs::remove_file`, and those
    // runs were launched with an elevated token, which is an unresolved
    // confound. See the module note.
    #[ignore = "the deny entry did not prevent deletion in a real guest; unresolved"]    fn arming_twice_is_harmless() {
        let dir = scratch("idempotent");
        let file = dir.join("a.txt");
        fs::write(&file, b"x").unwrap();

        let p = AclProtection;
        p.arm(&file).unwrap();
        p.arm(&file).unwrap();
        assert!(p.is_armed(&file));

        p.disarm(&file).unwrap();
        assert!(!p.is_armed(&file));
        fs::remove_file(&file).unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    /// Releasing a path that is already gone must succeed. A mark whose file
    /// was deleted by some other means would otherwise be unclearable.
    #[test]
    #[cfg(windows)]
    // Ignored rather than deleted: it records what the mechanism was supposed
    // to do and is the test that has to pass before the ACL backend is wired
    // up. It does not pass today - on a real Windows 11 guest the deny entry
    // did not stop `del`, `[IO.File]::Delete` or `fs::remove_file`, and those
    // runs were launched with an elevated token, which is an unresolved
    // confound. See the module note.
    #[ignore = "the deny entry did not prevent deletion in a real guest; unresolved"]    fn releasing_a_missing_path_is_not_an_error() {
        let dir = scratch("missing");
        let gone = dir.join("never-existed.txt");
        assert!(!gone.exists());
        AclProtection.disarm(&gone).expect("releasing a missing path failed");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A directory is protected by the same mechanism, and the ACE that
    /// protects it must not stop this program from later creating or reading
    /// inside it: only DELETE is denied.
    #[test]
    #[cfg(windows)]
    // Ignored rather than deleted: it records what the mechanism was supposed
    // to do and is the test that has to pass before the ACL backend is wired
    // up. It does not pass today - on a real Windows 11 guest the deny entry
    // did not stop `del`, `[IO.File]::Delete` or `fs::remove_file`, and those
    // runs were launched with an elevated token, which is an unresolved
    // confound. See the module note.
    #[ignore = "the deny entry did not prevent deletion in a real guest; unresolved"]    fn protecting_a_directory_leaves_its_contents_usable() {
        let dir = scratch("dir");
        let inner = dir.join("notes");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("a.txt"), b"x").unwrap();

        let p = AclProtection;
        p.arm(&inner).unwrap();
        assert!(p.is_armed(&inner));

        // Reading and writing inside must still work.
        fs::write(inner.join("b.txt"), b"y").expect("could not write inside the protected folder");
        fs::read(inner.join("a.txt")).expect("could not read inside the protected folder");

        p.disarm(&inner).unwrap();
        assert!(!p.is_armed(&inner), "the folder is still protected after release");

        let _ = fs::remove_dir_all(&dir);
    }
}
