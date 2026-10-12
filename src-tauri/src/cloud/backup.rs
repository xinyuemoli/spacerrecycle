//! Uploading cleared items to OneDrive before their local copies are removed.
//!
//! This is the one disposition that frees the disk *and* keeps a copy, so it
//! carries the strictest invariant in the codebase:
//!
//! > **The local copy is removed only after the upload has been confirmed.**
//!
//! Every failure path here returns `Err` and leaves the file exactly where it
//! was, for the engine to fall back to a local quarantine. A backup feature
//! that can lose the thing it was backing up is worse than no feature.
//!
//! Files are streamed one chunk at a time rather than read into memory: a
//! multi-gigabyte file would otherwise be buffered whole, and the machine this
//! runs on is by definition short of space.

use crate::cloud::account::{self, AccessCache, AuthStore};
use crate::cloud::graph::{self, Transport};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Folder inside the app's OneDrive folder that backups live under.
pub const BACKUP_ROOT: &str = "SpaceRecycle Backup";

/// Somewhere an item can be uploaded to.
///
/// Injected so the engine's rules - upload first, delete only on success, fall
/// back on failure - can be tested without a Microsoft account, a network, or a
/// packaged build.
pub trait BackupSink: Send + Sync {
    /// Upload `path` under `batch_id`, preserving its structure.
    ///
    /// Returns the number of bytes uploaded. `Err` means nothing was removed
    /// and nothing may be removed.
    fn upload(&self, path: &Path, is_dir: bool, batch_id: &str) -> Result<u64, String>;

    /// Human-readable name for the destination, for logs and the preview.
    fn describe(&self) -> String;
}

/// One file to upload: where it is, and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    pub source: PathBuf,
    /// Path inside the batch folder, always with forward slashes.
    pub relative: String,
    pub size: u64,
}

/// Work out exactly which files an upload would move.
///
/// Split out from the upload so the listing can be tested - and eventually
/// shown to the user - without touching the network. Symlinks are skipped for
/// the same reason the scanners skip them: a link pointing outside the tree
/// would upload something the user never selected.
pub fn plan_transfer(path: &Path, is_dir: bool) -> Result<Vec<Transfer>, String> {
    if !path.exists() {
        return Err(format!("path no longer exists: {}", path.display()));
    }
    if !is_dir {
        let size = std::fs::metadata(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?
            .len();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| format!("path has no file name: {}", path.display()))?;
        return Ok(vec![Transfer { source: path.to_path_buf(), relative: name, size }]);
    }

    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(path) else { continue };
        let relative = rel.to_string_lossy().replace('\\', "/");
        let Ok(md) = entry.metadata() else { continue };
        out.push(Transfer {
            source: entry.path().to_path_buf(),
            relative,
            size: md.len(),
        });
    }
    // Deterministic order, so a partly-completed upload resumes predictably and
    // the log reads the same way twice.
    out.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok(out)
}

/// The real sink: the linked OneDrive account, over Microsoft Graph.
pub struct OneDriveSink<'a> {
    pub transport: &'a dyn Transport,
    pub store: &'a dyn AuthStore,
    pub cache: &'a AccessCache,
    /// Root-first so expiry cannot be observed between two files of one batch.
    pub now: i64,
}

impl BackupSink for OneDriveSink<'_> {
    fn describe(&self) -> String {
        BACKUP_ROOT.to_string()
    }

    fn upload(&self, path: &Path, is_dir: bool, batch_id: &str) -> Result<u64, String> {
        let transfers = plan_transfer(path, is_dir)?;
        if transfers.is_empty() {
            return Err("nothing to upload".to_string());
        }

        // Resolved once for the whole batch rather than per file: a mid-batch
        // 401 would otherwise strand a half-uploaded folder.
        let token = account::access_token(self.transport, self.store, self.cache, self.now)?;

        let mut uploaded = 0u64;
        for t in &transfers {
            let remote = format!("{BACKUP_ROOT}/{batch_id}/{}", t.relative);
            upload_file(self.transport, &token, &remote, &t.source)?;
            uploaded += t.size;
        }
        Ok(uploaded)
    }
}

/// Upload one file via a resumable session, streaming chunk by chunk.
pub fn upload_file(
    t: &dyn Transport,
    bearer: &str,
    remote_name: &str,
    source: &Path,
) -> Result<(), String> {
    let mut file = std::fs::File::open(source)
        .map_err(|e| format!("cannot open {}: {e}", source.display()))?;
    let size = file
        .metadata()
        .map_err(|e| format!("cannot stat {}: {e}", source.display()))?
        .len();

    // An empty file still has to be created remotely; the session API accepts
    // a single zero-length chunk.
    if size == 0 {
        let session = graph::create_upload_session(t, bearer, remote_name, "replace")?;
        let (status, body) = graph::put_chunk(t, &session, &[], 0, 0, 0)?;
        return check_chunk(status, &body, remote_name, 0);
    }

    let session = graph::create_upload_session(t, bearer, remote_name, "replace")?;
    for (start, end) in graph::plan_chunks(size) {
        let len = (end - start + 1) as usize;
        let mut buf = vec![0u8; len];
        file.seek(SeekFrom::Start(start))
            .map_err(|e| format!("cannot seek in {}: {e}", source.display()))?;
        file.read_exact(&mut buf)
            .map_err(|e| format!("cannot read {}: {e}", source.display()))?;

        let (status, body) = graph::put_chunk(t, &session, &buf, start, end, size)?;
        check_chunk(status, &body, remote_name, start)?;
    }
    Ok(())
}

/// Accept the statuses Graph uses for "chunk stored" / "upload complete".
///
/// Deliberately strict: anything else - including a 2xx we did not expect -
/// counts as failure, because the caller deletes the local file on `Ok`.
fn check_chunk(status: u16, body: &str, remote: &str, start: u64) -> Result<(), String> {
    const ACCEPTED: [u16; 4] = [200, 201, 202, 204];
    if ACCEPTED.contains(&status) {
        return Ok(());
    }
    let detail = body.trim().chars().take(200).collect::<String>();
    Err(format!(
        "upload of {remote} failed at byte {start} (HTTP {status}): {detail}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::account::AuthStore;
    use crate::cloud::graph::StoredAuth;
    use parking_lot::Mutex as PLMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    // ------------------------------------------------------------- fake

    struct MemAuth(StdMutex<Option<StoredAuth>>);
    impl AuthStore for MemAuth {
        fn load(&self) -> Option<StoredAuth> {
            self.0.lock().unwrap().clone()
        }
        fn save(&self, a: &StoredAuth) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(a.clone());
            Ok(())
        }
        fn clear(&self) {
            *self.0.lock().unwrap() = None;
        }
    }

    /// Records every chunk so the test can check the bytes actually sent.
    #[derive(Default)]
    struct Recorder {
        sessions: StdMutex<Vec<String>>,
        chunks: StdMutex<Vec<(String, usize, String)>>,
        /// Statuses to return for successive chunk PUTs.
        chunk_status: PLMutex<Vec<u16>>,
        session_ok: PLMutex<bool>,
    }

    impl Transport for Recorder {
        fn post_form(&self, _u: &str, _f: &[(&str, &str)]) -> Result<(u16, String), String> {
            Ok((400, r#"{"error":"invalid_grant"}"#.into()))
        }
        fn get_json(&self, _u: &str, _b: &str) -> Result<(u16, String), String> {
            Ok((200, "{}".into()))
        }
        fn post_json(&self, u: &str, _b: &str, body: &[u8]) -> Result<(u16, String), String> {
            if !*self.session_ok.lock() {
                return Ok((507, r#"{"error":{"message":"insufficient storage"}}"#.into()));
            }
            self.sessions.lock().unwrap().push(u.to_string());
            let _ = body;
            Ok((200, r#"{"uploadUrl":"https://upload.example/session"}"#.into()))
        }
        fn put(&self, u: &str, headers: &[(&str, &str)], body: &[u8]) -> Result<(u16, String), String> {
            let range = headers
                .iter()
                .find(|(k, _)| *k == "Content-Range")
                .map(|(_, v)| v.to_string())
                .unwrap_or_default();
            self.chunks.lock().unwrap().push((u.to_string(), body.len(), range));
            let mut statuses = self.chunk_status.lock();
            let status = if statuses.is_empty() { 202 } else { statuses.remove(0) };
            Ok((status, "{}".into()))
        }
    }

    fn recorder() -> Arc<Recorder> {
        Arc::new(Recorder { session_ok: PLMutex::new(true), ..Default::default() })
    }

    fn sink<'a>(t: &'a Recorder, store: &'a MemAuth, cache: &'a AccessCache, now: i64) -> OneDriveSink<'a> {
        OneDriveSink { transport: t, store, cache, now }
    }

    fn connected() -> (MemAuth, AccessCache) {
        (
            MemAuth(StdMutex::new(Some(StoredAuth {
                client_id: "c".into(),
                refresh_token: "r".into(),
            }))),
            Arc::new(PLMutex::new(Some(("token".into(), i64::MAX)))),
        )
    }

    // ------------------------------------------------------- planning

    #[test]
    fn a_single_file_plans_one_transfer() {
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("notes.txt");
        std::fs::write(&f, b"hello").unwrap();

        let plan = plan_transfer(&f, false).unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].relative, "notes.txt");
        assert_eq!(plan[0].size, 5);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The folder structure has to survive the round trip, or a restored
    /// backup is a heap of same-named files.
    #[test]
    fn a_directory_keeps_its_structure_with_forward_slashes() {
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("a").join("b")).unwrap();
        std::fs::write(dir.join("top.txt"), b"1").unwrap();
        std::fs::write(dir.join("a").join("mid.txt"), b"22").unwrap();
        std::fs::write(dir.join("a").join("b").join("deep.txt"), b"333").unwrap();

        let plan = plan_transfer(&dir, true).unwrap();
        let names: Vec<&str> = plan.iter().map(|t| t.relative.as_str()).collect();
        assert_eq!(names, vec!["a/b/deep.txt", "a/mid.txt", "top.txt"]);
        assert!(!names.iter().any(|n| n.contains('\\')), "a backslash leaked into a cloud path");

        let total: u64 = plan.iter().map(|t| t.size).sum();
        assert_eq!(total, 6);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn planning_a_missing_path_is_an_error() {
        let missing = std::env::temp_dir().join("sr-bk-definitely-missing");
        assert!(plan_transfer(&missing, false).is_err());
    }

    // ------------------------------------------------------- uploading

    #[test]
    fn a_small_file_uploads_in_one_chunk_with_the_whole_body() {
        let t = recorder();
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.bin");
        std::fs::write(&f, vec![7u8; 1000]).unwrap();

        let uploaded = sink(&t, &store, &cache, 0).upload(&f, false, "batch-1").unwrap();
        assert_eq!(uploaded, 1000);

        let chunks = t.chunks.lock().unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].1, 1000);
        assert_eq!(chunks[0].2, "bytes 0-999/1000");

        let sessions = t.sessions.lock().unwrap();
        assert!(
            sessions[0].contains("SpaceRecycle%20Backup/batch-1/a.bin"),
            "unexpected remote path: {}",
            sessions[0]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A large file must be streamed in Graph-sized pieces, not sent whole.
    #[test]
    fn a_large_file_is_split_into_legal_chunks() {
        let t = recorder();
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("big.bin");
        let size = 25 * 1024 * 1024;
        std::fs::write(&f, vec![1u8; size]).unwrap();

        let uploaded = sink(&t, &store, &cache, 0).upload(&f, false, "b").unwrap();
        assert_eq!(uploaded, size as u64);

        let chunks = t.chunks.lock().unwrap();
        assert!(chunks.len() >= 3, "expected several chunks, got {}", chunks.len());
        for (_, len, _) in chunks.iter().take(chunks.len() - 1) {
            assert_eq!(len % (320 * 1024), 0, "a mid-file chunk is not a multiple of 320 KiB");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The upload of a directory must send the real bytes of every file.
    #[test]
    fn a_directory_uploads_every_file() {
        let t = recorder();
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("one.bin"), vec![1u8; 10]).unwrap();
        std::fs::write(dir.join("sub").join("two.bin"), vec![2u8; 20]).unwrap();

        let uploaded = sink(&t, &store, &cache, 0).upload(&dir, true, "b").unwrap();
        assert_eq!(uploaded, 30);
        assert_eq!(t.sessions.lock().unwrap().len(), 2);
        assert_eq!(t.chunks.lock().unwrap().len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failed chunk has to surface as an error: the engine removes the local
    /// file only on `Ok`.
    #[test]
    fn a_failed_chunk_is_reported_as_an_error() {
        let t = recorder();
        *t.chunk_status.lock() = vec![202, 500, 202];
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("big.bin");
        // Three chunks: the second one fails.
        std::fs::write(&f, vec![1u8; 25 * 1024 * 1024]).unwrap();

        let err = sink(&t, &store, &cache, 0).upload(&f, false, "b").unwrap_err();
        assert!(err.contains("failed at byte"), "{err}");
        assert!(err.contains("500"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A full cloud drive must stop the upload before anything is removed.
    #[test]
    fn a_rejected_session_stops_the_upload() {
        let t = recorder();
        *t.session_ok.lock() = false;
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.bin");
        std::fs::write(&f, vec![1u8; 10]).unwrap();

        let err = sink(&t, &store, &cache, 0).upload(&f, false, "b").unwrap_err();
        assert!(err.contains("insufficient storage"), "{err}");
        assert!(f.exists(), "the file was touched despite a failed upload");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_file_still_creates_a_remote_entry() {
        let t = recorder();
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("empty.txt");
        std::fs::write(&f, b"").unwrap();

        let uploaded = sink(&t, &store, &cache, 0).upload(&f, false, "b").unwrap();
        assert_eq!(uploaded, 0);
        assert_eq!(t.sessions.lock().unwrap().len(), 1);
        assert_eq!(t.chunks.lock().unwrap().len(), 1, "an empty file still needs one PUT");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An unlinked account must fail before any chunk is sent.
    #[test]
    fn an_unlinked_account_fails_before_uploading() {
        let t = recorder();
        let store = MemAuth(StdMutex::new(None));
        let cache: AccessCache = Arc::new(PLMutex::new(None));
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.bin");
        std::fs::write(&f, vec![1u8; 10]).unwrap();

        let err = sink(&t, &store, &cache, 0).upload(&f, false, "b").unwrap_err();
        assert!(err.contains("no OneDrive account"), "{err}");
        assert!(t.sessions.lock().unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_directory_is_not_silently_reported_as_success() {
        let t = recorder();
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        assert!(sink(&t, &store, &cache, 0).upload(&dir, true, "b").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_accepted_statuses_are_exactly_what_graph_uses() {
        for ok in [200u16, 201, 202, 204] {
            assert!(check_chunk(ok, "{}", "x", 0).is_ok(), "{ok} should be accepted");
        }
        for bad in [400u16, 401, 403, 404, 409, 500, 507] {
            assert!(check_chunk(bad, "{\"error\":\"x\"}", "x", 0).is_err(), "{bad} must not be accepted");
        }
    }

    /// A `Recorder` counting sessions is only meaningful if a failure does not
    /// send the remaining files.
    #[test]
    fn a_failure_does_not_continue_with_the_rest_of_the_batch() {
        let t = recorder();
        *t.chunk_status.lock() = vec![500];
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.bin"), vec![1u8; 10]).unwrap();
        std::fs::write(dir.join("b.bin"), vec![2u8; 10]).unwrap();

        assert!(sink(&t, &store, &cache, 0).upload(&dir, true, "b").is_err());
        // "a.bin" sorts first, so it is the one that failed; "b.bin" must not
        // have been attempted.
        assert_eq!(t.sessions.lock().unwrap().len(), 1, "the batch continued past a failure");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Guards the property the whole module exists for.
    #[test]
    fn nothing_is_removed_by_a_successful_upload() {
        let t = recorder();
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("keep.bin");
        std::fs::write(&f, vec![1u8; 10]).unwrap();

        sink(&t, &store, &cache, 0).upload(&f, false, "b").unwrap();
        assert!(f.exists(), "the sink deleted the file; only the engine may do that");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_sink_counts_what_it_uploaded() {
        let t = recorder();
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), vec![0u8; 3]).unwrap();
        std::fs::write(dir.join("b"), vec![0u8; 4]).unwrap();

        assert_eq!(sink(&t, &store, &cache, 0).upload(&dir, true, "b").unwrap(), 7);
        assert_eq!(t.sessions.lock().unwrap().len(), 2);
        assert_eq!(t.chunks.lock().unwrap().len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_partial_batch_reports_the_failure_rather_than_a_count() {
        // Counter-example to the previous test: the same shape, one bad chunk.
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        CALLS.store(0, Ordering::Relaxed);
        let t = recorder();
        *t.chunk_status.lock() = vec![202, 507];
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a"), vec![0u8; 3]).unwrap();
        std::fs::write(dir.join("b"), vec![0u8; 4]).unwrap();

        let err = sink(&t, &store, &cache, 0).upload(&dir, true, "b").unwrap_err();
        assert!(err.contains("507"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}