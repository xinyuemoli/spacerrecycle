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
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Folder inside the app's OneDrive folder that backups live under.
pub const BACKUP_ROOT: &str = "SpaceRecycle Backup";

/// Name of the per-batch manifest, written last.
pub const MANIFEST_NAME: &str = "manifest.json";

/// One uploaded file, as recorded in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFile {
    /// Path relative to the item that was uploaded.
    pub relative: String,
    pub size: u64,
}

/// One item that was uploaded, and where it came from.
///
/// The original path is recorded *in the cloud*, not only locally, so the
/// backup is restorable from another machine or after a reinstall. Without it
/// a batch is an anonymous pile of files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub original_path: String,
    /// Folder (or file) name inside the batch.
    pub name: String,
    pub is_dir: bool,
    pub bytes: u64,
    pub files: Vec<ManifestFile>,
    pub uploaded_unix: i64,
}

/// One batch's manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudManifest {
    pub version: u32,
    pub batch_id: String,
    pub created_unix: i64,
    pub items: Vec<ManifestEntry>,
}

impl CloudManifest {
    pub fn new(batch_id: &str, now: i64, items: Vec<ManifestEntry>) -> Self {
        CloudManifest { version: 1, batch_id: batch_id.to_string(), created_unix: now, items }
    }

    /// Total bytes the batch holds.
    pub fn total_bytes(&self) -> u64 {
        self.items.iter().map(|i| i.bytes).sum()
    }

    /// Total files the batch holds.
    pub fn total_files(&self) -> u64 {
        self.items.iter().map(|i| i.files.len() as u64).sum()
    }
}

/// What one upload put in the cloud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadOutcome {
    pub bytes: u64,
    pub files: Vec<ManifestFile>,
}

/// A batch as the UI sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudBatch {
    pub batch_id: String,
    /// `None` when the manifest is missing or unreadable, in which case the
    /// batch is listed but cannot be restored by this app.
    pub manifest: Option<CloudManifest>,
    pub error: Option<String>,
}

/// Somewhere an item can be uploaded to.
///
/// Injected so the engine's rules - upload first, delete only on success, fall
/// back on failure - can be tested without a Microsoft account, a network, or a
/// packaged build.
pub trait BackupSink: Send + Sync {
    /// Upload `path` under `batch_id`, preserving its structure.
    ///
    /// `Err` means nothing was removed and nothing may be removed.
    fn upload(&self, path: &Path, is_dir: bool, batch: &str) -> Result<UploadOutcome, String>;

    /// Write the batch manifest, once every upload has finished.
    ///
    /// Deliberately a separate call and deliberately last: a batch with files
    /// but no manifest is visibly incomplete, which is the honest state for an
    /// upload that was interrupted.
    fn finish_batch(&self, batch: &str, items: &[ManifestEntry]) -> Result<(), String>;

    /// Human-readable name for the destination, for logs and the preview.
    fn describe(&self) -> String;
}

/// One file that could not be restored.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreFailure {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudRestoreReport {
    pub batch_id: String,
    pub restored_files: u64,
    pub restored_bytes: u64,
    /// Files left alone because something is already there.
    pub skipped: Vec<String>,
    pub failed: Vec<RestoreFailure>,
}

/// Whether a path recorded in a manifest is somewhere we will write.
///
/// The manifest comes back from the cloud, so its `originalPath` is data, not
/// an instruction. Refusing relative paths and `..` keeps a tampered or
/// corrupted manifest from writing outside the tree the user backed up. It does
/// not make the path *trusted* - the user is shown the destination and the
/// batch is one this app wrote - but it removes the trivial escape.
fn safe_target(p: &Path) -> Result<(), String> {
    if !p.is_absolute() {
        return Err("recorded path is not absolute".to_string());
    }
    if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err("recorded path escapes its parent".to_string());
    }
    Ok(())
}

/// Download a batch back to the locations its manifest recorded.
pub fn restore_batch(
    t: &dyn Transport,
    store: &dyn AuthStore,
    cache: &AccessCache,
    now: i64,
    batch_id: &str,
    policy: crate::quarantine::ConflictPolicy,
) -> Result<CloudRestoreReport, String> {
    let token = account::access_token(t, store, cache, now)?;
    let manifest_path = format!("{BACKUP_ROOT}/{batch_id}/{MANIFEST_NAME}");
    let raw = graph::download(t, &token, &manifest_path)?;
    let manifest: CloudManifest = serde_json::from_slice(&raw)
        .map_err(|e| format!("this batch's manifest is unreadable, so it cannot be restored here: {e}"))?;

    let mut report = CloudRestoreReport { batch_id: batch_id.to_string(), ..Default::default() };

    for item in &manifest.items {
        let root = PathBuf::from(&item.original_path);
        if let Err(reason) = safe_target(&root) {
            report.failed.push(RestoreFailure { path: item.original_path.clone(), reason });
            continue;
        }

        for file in &item.files {
            let remote = if item.is_dir {
                format!("{BACKUP_ROOT}/{batch_id}/{}/{}", item.name, file.relative)
            } else {
                format!("{BACKUP_ROOT}/{batch_id}/{}", file.relative)
            };
            let local = if item.is_dir {
                root.join(file.relative.replace('/', "\\"))
            } else {
                root.clone()
            };

            if local.exists() {
                match policy {
                    crate::quarantine::ConflictPolicy::Skip => {
                        report.skipped.push(local.to_string_lossy().to_string());
                        continue;
                    }
                    crate::quarantine::ConflictPolicy::Overwrite => {}
                }
            }

            let data = match graph::download(t, &token, &remote) {
                Ok(d) => d,
                Err(reason) => {
                    report.failed.push(RestoreFailure {
                        path: local.to_string_lossy().to_string(),
                        reason,
                    });
                    continue;
                }
            };
            if let Some(parent) = local.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    report.failed.push(RestoreFailure {
                        path: local.to_string_lossy().to_string(),
                        reason: format!("cannot create {}: {e}", parent.display()),
                    });
                    continue;
                }
            }
            match std::fs::write(&local, &data) {
                Ok(()) => {
                    report.restored_files += 1;
                    report.restored_bytes += data.len() as u64;
                }
                Err(e) => report.failed.push(RestoreFailure {
                    path: local.to_string_lossy().to_string(),
                    reason: e.to_string(),
                }),
            }
        }
    }

    Ok(report)
}

/// Delete a whole batch from the cloud.
///
/// The only action in this module that destroys anything, and it only ever
/// touches this app's own backup folder.
pub fn delete_batch(
    t: &dyn Transport,
    store: &dyn AuthStore,
    cache: &AccessCache,
    now: i64,
    batch_id: &str,
) -> Result<(), String> {
    // Guard against a batch id that could address something else: the id comes
    // from a listing, but a slash or a parent reference would escape the
    // backup folder and delete part of the user's drive.
    if batch_id.is_empty() || batch_id.contains('/') || batch_id.contains('\\') || batch_id.contains("..") {
        return Err("refusing to delete a batch with an unexpected name".to_string());
    }
    let token = account::access_token(t, store, cache, now)?;
    graph::delete_path(t, &token, &format!("{BACKUP_ROOT}/{batch_id}"))
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

    fn upload(&self, path: &Path, is_dir: bool, batch: &str) -> Result<UploadOutcome, String> {
        let transfers = plan_transfer(path, is_dir)?;
        if transfers.is_empty() {
            return Err("nothing to upload".to_string());
        }

        // Resolved once for the whole batch rather than per file: a mid-batch
        // 401 would otherwise strand a half-uploaded folder.
        let token = account::access_token(self.transport, self.store, self.cache, self.now)?;

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .ok_or_else(|| format!("path has no file name: {}", path.display()))?;

        let mut files = Vec::new();
        let mut bytes = 0u64;
        for t in &transfers {
            // A directory is uploaded *under its own name*. Without that prefix
            // two folders in one batch would merge into each other, and a
            // single-folder batch would lose the folder the user recognised.
            let remote = if is_dir {
                format!("{BACKUP_ROOT}/{batch}/{name}/{}", t.relative)
            } else {
                format!("{BACKUP_ROOT}/{batch}/{}", t.relative)
            };
            upload_file(self.transport, &token, &remote, &t.source)?;
            bytes += t.size;
            files.push(ManifestFile { relative: t.relative.clone(), size: t.size });
        }
        Ok(UploadOutcome { bytes, files })
    }

    fn finish_batch(&self, batch: &str, items: &[ManifestEntry]) -> Result<(), String> {
        if items.is_empty() {
            return Ok(());
        }
        let token = account::access_token(self.transport, self.store, self.cache, self.now)?;
        let manifest = CloudManifest::new(batch, self.now, items.to_vec());
        let json = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| format!("cannot encode the backup manifest: {e}"))?;
        let remote = format!("{BACKUP_ROOT}/{batch}/{MANIFEST_NAME}");
        upload_bytes(self.transport, &token, &remote, &json)
    }
}

/// Upload an in-memory payload (the manifest).
pub fn upload_bytes(t: &dyn Transport, bearer: &str, remote: &str, data: &[u8]) -> Result<(), String> {
    let session = graph::create_upload_session(t, bearer, remote, "replace")?;
    let total = data.len() as u64;
    if total == 0 {
        let (status, body) = graph::put_chunk(t, &session, &[], 0, 0, 0)?;
        return check_chunk(status, &body, remote, 0);
    }
    for (start, end) in graph::plan_chunks(total) {
        let chunk = &data[start as usize..=end as usize];
        let (status, body) = graph::put_chunk(t, &session, chunk, start, end, total)?;
        check_chunk(status, &body, remote, start)?;
    }
    Ok(())
}

/// Every backup batch in the cloud, with its manifest where one is readable.
pub fn list_backups(
    t: &dyn Transport,
    store: &dyn AuthStore,
    cache: &AccessCache,
    now: i64,
) -> Result<Vec<CloudBatch>, String> {
    let token = account::access_token(t, store, cache, now)?;
    let children = graph::list_children(t, &token, BACKUP_ROOT)?;

    let mut out = Vec::new();
    for child in children.into_iter().filter(|c| c.is_dir) {
        let manifest_path = format!("{BACKUP_ROOT}/{}/{MANIFEST_NAME}", child.name);
        match graph::download(t, &token, &manifest_path)
            .and_then(|bytes| {
                serde_json::from_slice::<CloudManifest>(&bytes)
                    .map_err(|e| format!("manifest is unreadable: {e}"))
            }) {
            Ok(manifest) => out.push(CloudBatch {
                batch_id: child.name,
                manifest: Some(manifest),
                error: None,
            }),
            // A batch without a readable manifest is still listed: the files
            // are in the user's drive and they can reach them, so hiding the
            // batch would be worse than showing it as not restorable here.
            Err(e) => out.push(CloudBatch { batch_id: child.name, manifest: None, error: Some(e) }),
        }
    }
    // Newest first, using the manifest's timestamp where there is one.
    out.sort_by(|a, b| {
        let ta = a.manifest.as_ref().map(|m| m.created_unix).unwrap_or(0);
        let tb = b.manifest.as_ref().map(|m| m.created_unix).unwrap_or(0);
        tb.cmp(&ta).then_with(|| b.batch_id.cmp(&a.batch_id))
    });
    Ok(out)
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
        /// Download responses, consumed in order.
        downloads: StdMutex<Vec<String>>,
        download_replies: PLMutex<Vec<(u16, String)>>,
        /// Responses for authenticated GETs (folder listings).
        list_replies: PLMutex<Vec<(u16, String)>>,
        deletes: StdMutex<Vec<String>>,
    }

    impl Transport for Recorder {
        fn post_form(&self, _u: &str, _f: &[(&str, &str)]) -> Result<(u16, String), String> {
            Ok((400, r#"{"error":"invalid_grant"}"#.into()))
        }
        fn get_json(&self, _u: &str, _b: &str) -> Result<(u16, String), String> {
            let mut replies = self.list_replies.lock();
            if replies.is_empty() {
                return Ok((200, "{}".into()));
            }
            Ok(replies.remove(0))
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

        fn get_bytes(&self, url: &str, _b: Option<&str>) -> Result<(u16, Vec<u8>), String> {
            self.downloads.lock().unwrap().push(url.to_string());
            let mut replies = self.download_replies.lock();
            if replies.is_empty() {
                return Ok((404, Vec::new()));
            }
            let (status, body) = replies.remove(0);
            Ok((status, body.into_bytes()))
        }

        fn delete(&self, url: &str, _b: &str) -> Result<(u16, String), String> {
            self.deletes.lock().unwrap().push(url.to_string());
            Ok((204, String::new()))
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

        let uploaded = sink(&t, &store, &cache, 0).upload(&f, false, "batch-1").unwrap().bytes;
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

        let uploaded = sink(&t, &store, &cache, 0).upload(&f, false, "b").unwrap().bytes;
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

        let uploaded = sink(&t, &store, &cache, 0).upload(&dir, true, "b").unwrap().bytes;
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

        let uploaded = sink(&t, &store, &cache, 0).upload(&f, false, "b").unwrap().bytes;
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

        let outcome = sink(&t, &store, &cache, 0).upload(&dir, true, "b").unwrap();
        assert_eq!(outcome.bytes, 7);
        // The manifest gets the file list, so a restore knows what to fetch.
        assert_eq!(outcome.files.len(), 2);
        assert_eq!(t.sessions.lock().unwrap().len(), 2);
        assert_eq!(t.chunks.lock().unwrap().len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory must upload *under its own name*. Without that prefix two
    /// folders in one batch merge into each other, and a single-folder batch
    /// loses the folder the user actually recognised.
    #[test]
    fn a_directory_is_uploaded_under_its_own_name() {
        let t = recorder();
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        let inner = dir.join("photos");
        std::fs::create_dir_all(inner.join("2026")).unwrap();
        std::fs::write(inner.join("2026").join("a.jpg"), vec![1u8; 10]).unwrap();

        sink(&t, &store, &cache, 0).upload(&inner, true, "batch-1").unwrap();

        let sessions = t.sessions.lock().unwrap();
        assert!(
            sessions[0].contains("batch-1/photos/2026/a.jpg"),
            "the folder name was dropped: {}",
            sessions[0]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A plain file has no folder level, so it must not gain one.
    #[test]
    fn a_single_file_is_not_nested_under_a_redundant_folder() {
        let t = recorder();
        let (store, cache) = connected();
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("notes.txt");
        std::fs::write(&f, b"hello").unwrap();

        sink(&t, &store, &cache, 0).upload(&f, false, "batch-1").unwrap();

        let sessions = t.sessions.lock().unwrap();
        assert!(
            sessions[0].contains("batch-1/notes.txt:"),
            "unexpected remote path: {}",
            sessions[0]
        );
        assert!(
            !sessions[0].contains("notes.txt/notes.txt"),
            "a file was nested under itself: {}",
            sessions[0]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------- manifest

    #[test]
    fn a_manifest_round_trips_through_json() {
        let m = CloudManifest::new(
            "batch-1",
            1234,
            vec![ManifestEntry {
                original_path: "C:\\code\\app\\node_modules".into(),
                name: "node_modules".into(),
                is_dir: true,
                bytes: 4096,
                files: vec![ManifestFile { relative: "a/b.js".into(), size: 10 }],
                uploaded_unix: 1234,
            }],
        );
        let json = serde_json::to_string(&m).unwrap();
        let back: CloudManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.total_bytes(), 4096);
        assert_eq!(back.total_files(), 1);
    }

    /// The manifest is what makes a batch restorable from another machine, so
    /// the original path has to be in it.
    #[test]
    fn the_manifest_records_where_every_item_came_from() {
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, b"x").unwrap();

        let items = vec![ManifestEntry {
            original_path: f.to_string_lossy().to_string(),
            name: "a.txt".into(),
            is_dir: false,
            bytes: 1,
            files: vec![ManifestFile { relative: "a.txt".into(), size: 1 }],
            uploaded_unix: 1,
        }];
        let m = CloudManifest::new("b", 1, items);
        assert_eq!(m.items[0].original_path, f.to_string_lossy());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------- safety

    /// The manifest comes back from the cloud, so its path is data rather than
    /// an instruction. These are the shapes that must be refused.
    #[test]
    fn a_recorded_path_that_could_escape_is_refused() {
        assert!(safe_target(Path::new("C:\\Users\\me\\file.txt")).is_ok());
        assert!(safe_target(Path::new("relative\\file.txt")).is_err());
        assert!(safe_target(Path::new("C:\\Users\\me\\..\\..\\windows\\x")).is_err());
        assert!(safe_target(Path::new("..\\file.txt")).is_err());
    }

    #[test]
    fn deleting_a_batch_refuses_a_name_that_could_address_something_else() {
        let t = recorder();
        let (store, cache) = connected();
        for bad in ["", "..", "a/b", "a\\b", "SpaceRecycle Backup/..", "../other"] {
            let err = delete_batch(&*t, &store, &cache, 0, bad).unwrap_err();
            assert!(err.contains("unexpected name"), "{bad} -> {err}");
        }
        assert!(t.deletes.lock().unwrap().is_empty(), "a row was deleted anyway");
    }

    #[test]
    fn deleting_a_batch_addresses_the_app_folder() {
        let t = recorder();
        let (store, cache) = connected();
        delete_batch(&*t, &store, &cache, 0, "batch-1").unwrap();
        let deletes = t.deletes.lock().unwrap();
        assert_eq!(deletes.len(), 1);
        assert!(deletes[0].contains("SpaceRecycle%20Backup/batch-1"), "{}", deletes[0]);
    }

    // ------------------------------------------------------- listing

    #[test]
    fn listing_reads_each_batch_manifest_and_sorts_newest_first() {
        let older = serde_json::to_string(&CloudManifest::new("old-batch", 100, vec![])).unwrap();
        let newer = serde_json::to_string(&CloudManifest::new("new-batch", 900, vec![])).unwrap();
        let t = recorder();
        // First call: the folder listing. Then one download per batch.
        *t.list_replies.lock() = vec![(
            200,
            r#"{"value":[
                {"id":"1","name":"old-batch","folder":{}},
                {"id":"2","name":"new-batch","folder":{}}
            ]}"#
            .into(),
        )];
        // Children come back name-sorted, so "new-batch" is fetched first.
        *t.download_replies.lock() = vec![(200, newer), (200, older)];

        let (store, cache) = connected();
        let batches = list_backups(&*t, &store, &cache, 0).unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].batch_id, "new-batch", "not newest first");
        assert_eq!(batches[1].batch_id, "old-batch");
        assert!(batches.iter().all(|b| b.manifest.is_some()));
    }

    /// A batch whose manifest is missing is still listed: its files are in the
    /// user's drive and they can reach them, so hiding it would be worse.
    #[test]
    fn a_batch_without_a_readable_manifest_is_listed_with_a_reason() {
        let t = recorder();
        *t.list_replies.lock() = vec![(
            200,
            r#"{"value":[{"id":"1","name":"broken","folder":{}}]}"#.into(),
        )];
        *t.download_replies.lock() = vec![(404, String::new())];

        let (store, cache) = connected();
        let batches = list_backups(&*t, &store, &cache, 0).unwrap();
        assert_eq!(batches.len(), 1);
        assert!(batches[0].manifest.is_none());
        assert!(batches[0].error.is_some(), "the reason was not reported");
    }

    // ------------------------------------------------------- restoring

    #[test]
    fn restoring_writes_the_files_back_to_their_recorded_paths() {
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        let target = dir.join("restored.txt");
        std::fs::create_dir_all(&dir).unwrap();

        let manifest = CloudManifest::new(
            "batch-1",
            1,
            vec![ManifestEntry {
                original_path: target.to_string_lossy().to_string(),
                name: "restored.txt".into(),
                is_dir: false,
                bytes: 5,
                files: vec![ManifestFile { relative: "restored.txt".into(), size: 5 }],
                uploaded_unix: 1,
            }],
        );

        let t = recorder();
        *t.download_replies.lock() = vec![
            (200, serde_json::to_string(&manifest).unwrap()),
            (200, "hello".into()),
        ];
        let (store, cache) = connected();
        let report = restore_batch(
            &*t,
            &store,
            &cache,
            0,
            "batch-1",
            crate::quarantine::ConflictPolicy::Overwrite,
        )
        .unwrap();

        assert_eq!(report.restored_files, 1);
        assert_eq!(report.restored_bytes, 5);
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Restoring must not silently clobber something the user has since put
    /// back themselves.
    #[test]
    fn a_skip_policy_leaves_an_existing_file_alone() {
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        let target = dir.join("keep.txt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&target, "newer content").unwrap();

        let manifest = CloudManifest::new(
            "batch-1",
            1,
            vec![ManifestEntry {
                original_path: target.to_string_lossy().to_string(),
                name: "keep.txt".into(),
                is_dir: false,
                bytes: 5,
                files: vec![ManifestFile { relative: "keep.txt".into(), size: 5 }],
                uploaded_unix: 1,
            }],
        );
        let t = recorder();
        *t.download_replies.lock() = vec![(200, serde_json::to_string(&manifest).unwrap())];

        let (store, cache) = connected();
        let report = restore_batch(
            &*t,
            &store,
            &cache,
            0,
            "batch-1",
            crate::quarantine::ConflictPolicy::Skip,
        )
        .unwrap();

        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.restored_files, 0);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "newer content");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_item_restores_its_structure() {
        let dir = std::env::temp_dir().join(format!("sr-bk-{}", uuid::Uuid::new_v4()));
        let root = dir.join("photos");
        std::fs::create_dir_all(&dir).unwrap();

        let manifest = CloudManifest::new(
            "batch-1",
            1,
            vec![ManifestEntry {
                original_path: root.to_string_lossy().to_string(),
                name: "photos".into(),
                is_dir: true,
                bytes: 10,
                files: vec![
                    ManifestFile { relative: "2026/a.jpg".into(), size: 5 },
                    ManifestFile { relative: "2026/b.jpg".into(), size: 5 },
                ],
                uploaded_unix: 1,
            }],
        );
        let t = recorder();
        *t.download_replies.lock() = vec![
            (200, serde_json::to_string(&manifest).unwrap()),
            (200, "aaaaa".into()),
            (200, "bbbbb".into()),
        ];
        let (store, cache) = connected();
        let report = restore_batch(
            &*t,
            &store,
            &cache,
            0,
            "batch-1",
            crate::quarantine::ConflictPolicy::Overwrite,
        )
        .unwrap();

        assert_eq!(report.restored_files, 2);
        assert_eq!(std::fs::read_to_string(root.join("2026").join("a.jpg")).unwrap(), "aaaaa");
        assert_eq!(std::fs::read_to_string(root.join("2026").join("b.jpg")).unwrap(), "bbbbb");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hostile or corrupted manifest must not write outside the tree.
    #[test]
    fn restoring_refuses_an_escaping_path() {
        // Absolute, so the "not absolute" guard is not what stops it: this is
        // the `..` case specifically.
        let manifest = CloudManifest::new(
            "batch-1",
            1,
            vec![ManifestEntry {
                original_path: "C:\\Users\\me\\..\\..\\Windows\\System32\\evil.dll".into(),
                name: "evil.dll".into(),
                is_dir: false,
                bytes: 1,
                files: vec![ManifestFile { relative: "evil.dll".into(), size: 1 }],
                uploaded_unix: 1,
            }],
        );
        let t = recorder();
        *t.download_replies.lock() = vec![(200, serde_json::to_string(&manifest).unwrap())];
        let (store, cache) = connected();
        let report = restore_batch(
            &*t,
            &store,
            &cache,
            0,
            "batch-1",
            crate::quarantine::ConflictPolicy::Overwrite,
        )
        .unwrap();

        assert_eq!(report.restored_files, 0);
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].reason.contains("escapes"), "{:?}", report.failed[0].reason);
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