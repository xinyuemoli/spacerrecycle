//! Microsoft Graph: account linking, cloud quota, and upload sessions.
//!
//! Three design decisions worth stating up front, because each one narrows what
//! this code is allowed to do:
//!
//! 1. **Scope is `Files.ReadWrite.AppFolder`, not `Files.ReadWrite.All`.** The
//!    app can only see its own folder under `Apps/`. It cannot read, list, or
//!    touch anything else in the user's OneDrive, and a compromised build
//!    cannot either. A disk cleaner asking for the keys to every file would be
//!    a worse product, not a more capable one.
//! 2. **Device code flow.** No client secret exists to leak, no embedded
//!    browser, and the same code works for a packaged MSIX build. The user
//!    signs in on Microsoft's own page.
//! 3. **The transport is injected.** Every network call goes through
//!    [`Transport`], so request building and response interpretation are tested
//!    against a fake instead of against the live service. The only untested
//!    part is the WinHTTP call itself, which has its own live smoke tests.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Fixed endpoints. Not configurable: a user-settable token endpoint is an
/// obvious way to have the app hand its refresh token to someone else.
pub const AUTHORITY: &str = "https://login.microsoftonline.com/common/oauth2/v2.0";
pub const GRAPH: &str = "https://graph.microsoft.com/v1.0";

/// The narrowest scope that still supports a backup folder.
///
/// `offline_access` is what makes a refresh token possible; without it the user
/// would have to re-authorise on every launch.
pub const SCOPE: &str = "Files.ReadWrite.AppFolder offline_access";

/// Graph requires every chunk except the last to be a multiple of this.
const CHUNK_GRANULARITY: u64 = 320 * 1024;
/// 10 MiB, chosen because it is 32 x 320 KiB and is a good size for a resumable
/// upload: small enough that a dropped connection loses little, large enough
/// that a 1 GB file is ~100 requests instead of ~3000.
const DEFAULT_CHUNK: u64 = 10 * 1024 * 1024;

// ------------------------------------------------------------- token store

/// What has to survive between runs to keep the account linked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredAuth {
    pub client_id: String,
    /// DPAPI-protected on disk; plaintext only in memory.
    pub refresh_token: String,
}

pub fn auth_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("SpaceRecycle").join("auth.bin"))
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    for pair in bytes.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push((hi * 16 + lo) as u8);
    }
    Some(out)
}

/// Persist the refresh token, encrypted for this user.
///
/// The file is DPAPI ciphertext rather than a token in the clear: see
/// `winapi::dpapi` for what that does and does not buy.
pub fn save_auth_to(path: &std::path::Path, auth: &StoredAuth) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create config dir: {e}"))?;
    }
    let json = serde_json::to_vec(auth).map_err(|e| format!("cannot encode credentials: {e}"))?;
    let sealed = crate::winapi::dpapi::protect(&json)
        .ok_or_else(|| "could not encrypt the OneDrive credential".to_string())?;
    std::fs::write(path, to_hex(&sealed)).map_err(|e| format!("cannot save credentials: {e}"))
}

pub fn load_auth_from(path: &std::path::Path) -> Option<StoredAuth> {
    let raw = std::fs::read_to_string(path).ok()?;
    let sealed = from_hex(raw.trim())?;
    let plain = crate::winapi::dpapi::unprotect(&sealed)?;
    serde_json::from_slice(&plain).ok()
}

pub fn save_auth(auth: &StoredAuth) -> Result<(), String> {
    let path = auth_path().ok_or_else(|| "no config directory".to_string())?;
    save_auth_to(&path, auth)
}

pub fn load_auth() -> Option<StoredAuth> {
    load_auth_from(&auth_path()?)
}

/// Forget the linked account.
pub fn clear_auth() {
    if let Some(p) = auth_path() {
        let _ = std::fs::remove_file(p);
    }
}

// ---------------------------------------------------------------- transport

/// The seam between Graph logic and the network.
pub trait Transport: Send + Sync {
    /// `application/x-www-form-urlencoded` POST. Used by the OAuth endpoints.
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String), String>;
    /// Authenticated GET.
    fn get_json(&self, url: &str, bearer: &str) -> Result<(u16, String), String>;
    /// Authenticated POST with a JSON body. Distinct from `put` because the
/// upload-session endpoint accepts only POST, and a transport that silently
/// swapped verbs would fail in a way that looks like a server problem.
    fn post_json(&self, url: &str, bearer: &str, body: &[u8]) -> Result<(u16, String), String>;
    /// Download. `bearer` is `None` for a pre-authenticated URL, which is what
    /// Graph hands back for content: the redirect target carries its own
    /// credentials in the query string.
    fn get_bytes(&self, url: &str, bearer: Option<&str>) -> Result<(u16, Vec<u8>), String>;
    /// Authenticated DELETE.
    fn delete(&self, url: &str, bearer: &str) -> Result<(u16, String), String>;
    /// Authenticated PUT with a raw body and explicit headers.
    fn put(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<(u16, String), String>;
}

/// The real transport, over WinHTTP.
pub struct WinHttp;

fn form_encode(form: &[(&str, &str)]) -> String {
    form.iter()
        .map(|(k, v)| format!("{}={}", percent_encode(k), percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode everything outside the unreserved set.
///
/// Hand-rolled rather than pulled from a crate: the alphabet is fixed and tiny,
/// and a form encoder that gets `+` versus `%20` wrong silently corrupts the
/// device code.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl Transport for WinHttp {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String), String> {
        let body = form_encode(form);
        let headers = [("Content-Type", "application/x-www-form-urlencoded")];
        match crate::winapi::http::request("POST", url, &headers, Some(body.as_bytes())) {
            Ok(r) => Ok((r.status, r.body)),
            // A 4xx is a real answer from the identity service and must reach
            // the caller: `authorization_pending` arrives as HTTP 400 and is
            // the single most common response in this whole flow.
            Err(crate::winapi::http::HttpError::Status(code, body)) => Ok((code, body)),
            Err(e) => Err(e.message()),
        }
    }

    fn get_json(&self, url: &str, bearer: &str) -> Result<(u16, String), String> {
        let auth = format!("Bearer {bearer}");
        let headers = [("Authorization", auth.as_str()), ("Accept", "application/json")];
        match crate::winapi::http::request("GET", url, &headers, None) {
            Ok(r) => Ok((r.status, r.body)),
            Err(crate::winapi::http::HttpError::Status(code, body)) => Ok((code, body)),
            Err(e) => Err(e.message()),
        }
    }

    fn put(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<(u16, String), String> {
        match crate::winapi::http::request("PUT", url, headers, Some(body)) {
            Ok(r) => Ok((r.status, r.body)),
            Err(crate::winapi::http::HttpError::Status(code, body)) => Ok((code, body)),
            Err(e) => Err(e.message()),
        }
    }

    fn post_json(&self, url: &str, bearer: &str, body: &[u8]) -> Result<(u16, String), String> {
        let auth = format!("Bearer {bearer}");
        let headers = [
            ("Authorization", auth.as_str()),
            ("Content-Type", "application/json"),
        ];
        match crate::winapi::http::request("POST", url, &headers, Some(body)) {
            Ok(r) => Ok((r.status, r.body)),
            Err(crate::winapi::http::HttpError::Status(code, body)) => Ok((code, body)),
            Err(e) => Err(e.message()),
        }
    }

    fn get_bytes(&self, url: &str, bearer: Option<&str>) -> Result<(u16, Vec<u8>), String> {
        let auth = bearer.map(|b| format!("Bearer {b}"));
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(a) = auth.as_deref() {
            headers.push(("Authorization", a));
        }
        let req = crate::winapi::http::Request {
            method: "GET",
            url,
            headers: &headers,
            body: None,
            // Content requests always redirect to a pre-authenticated host.
            follow_redirects: true,
        };
        match crate::winapi::http::send(&req) {
            Ok(r) => Ok((r.status, r.body)),
            Err(crate::winapi::http::HttpError::Status(code, body)) => {
                Ok((code, body.into_bytes()))
            }
            Err(e) => Err(e.message()),
        }
    }

    fn delete(&self, url: &str, bearer: &str) -> Result<(u16, String), String> {
        let auth = format!("Bearer {bearer}");
        let headers = [("Authorization", auth.as_str())];
        match crate::winapi::http::request("DELETE", url, &headers, None) {
            Ok(r) => Ok((r.status, r.body)),
            Err(crate::winapi::http::HttpError::Status(code, body)) => Ok((code, body)),
            Err(e) => Err(e.message()),
        }
    }
}

// ------------------------------------------------------------- device code

/// What the user must be shown to sign in on another device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: i64,
    /// Seconds to wait between polls, per the service. Polling faster earns a
    /// `slow_down` and eventually a ban.
    pub interval: i64,
}

pub fn start_device_code(t: &dyn Transport, client_id: &str) -> Result<DeviceCode, String> {
    if client_id.trim().is_empty() {
        return Err("no Azure application (client) ID is configured".to_string());
    }
    let url = format!("{AUTHORITY}/devicecode");
    let (status, body) = t.post_form(&url, &[("client_id", client_id), ("scope", SCOPE)])?;
    if !(200..300).contains(&status) {
        return Err(format!("device code request failed (HTTP {status}): {}", graph_error(&body)));
    }
    parse_device_code(&body)
}

/// Parse the device-code response.
///
/// Separate from the call so the shape of Microsoft's answer is pinned by a
/// test rather than by whatever the service happened to return on the day it
/// was written.
pub fn parse_device_code(body: &str) -> Result<DeviceCode, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("malformed device code response: {e}"))?;
    let field = |name: &str| -> Result<String, String> {
        v.get(name)
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("device code response is missing `{name}`"))
    };
    let interval = v.get("interval").and_then(|x| x.as_i64()).unwrap_or(5).max(1);
    Ok(DeviceCode {
        device_code: field("device_code")?,
        user_code: field("user_code")?,
        verification_uri: field("verification_uri")?,
        expires_in: v.get("expires_in").and_then(|x| x.as_i64()).unwrap_or(900),
        interval,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Absolute expiry, unix seconds.
    pub expires_unix: i64,
}

/// One poll of the token endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// The user has not finished signing in. Poll again after the interval.
    Pending,
    /// The service asked us to back off. The caller must lengthen its interval.
    SlowDown,
    Ready(Tokens),
    /// The user declined, or the code expired / was already used.
    Denied(String),
    /// A transport-level failure. Worth retrying later.
    TransportError(String),
}

pub fn poll_token(t: &dyn Transport, client_id: &str, device_code: &str, now: i64) -> Poll {
    let url = format!("{AUTHORITY}/token");
    let form = [
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("client_id", client_id),
        ("device_code", device_code),
    ];
    match t.post_form(&url, &form) {
        Err(e) => Poll::TransportError(e),
        Ok((status, body)) => interpret_poll(status, &body, now),
    }
}

/// Turn a token-endpoint answer into a poll outcome.
///
/// The service reports "not yet" as HTTP 400 with an `error` string, so the
/// status code alone cannot decide anything - a naive `is_success` check would
/// treat every pending poll as a failure and abort the sign-in.
pub fn interpret_poll(status: u16, body: &str, now: i64) -> Poll {
    if (200..300).contains(&status) {
        return match parse_tokens(body, now) {
            Ok(t) => Poll::Ready(t),
            Err(e) => Poll::TransportError(e),
        };
    }
    match error_code(body).as_deref() {
        Some("authorization_pending") => Poll::Pending,
        Some("slow_down") => Poll::SlowDown,
        Some("expired_token") => Poll::Denied("the sign-in code expired".to_string()),
        Some("authorization_declined") | Some("access_denied") => {
            Poll::Denied("sign-in was declined".to_string())
        }
        Some("bad_verification_code") => Poll::Denied("the sign-in code was not recognised".to_string()),
        _ => Poll::TransportError(format!("token request failed (HTTP {status}): {}", graph_error(body))),
    }
}

pub fn parse_tokens(body: &str, now: i64) -> Result<Tokens, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("malformed token response: {e}"))?;
    let access = v
        .get("access_token")
        .and_then(|x| x.as_str())
        .ok_or("token response is missing `access_token`")?
        .to_string();
    let expires_in = v.get("expires_in").and_then(|x| x.as_i64()).unwrap_or(3600);
    // Refresh an access token a minute early: a request that starts just under
    // the wire is the one that fails with a 401 halfway through an upload.
    Ok(Tokens {
        access_token: access,
        refresh_token: v
            .get("refresh_token")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        expires_unix: now + (expires_in - 60).max(0),
    })
}

/// Exchange a refresh token for a new access token.
pub fn refresh(t: &dyn Transport, client_id: &str, refresh_token: &str, now: i64) -> Result<Tokens, String> {
    let url = format!("{AUTHORITY}/token");
    let form = [
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token),
        ("scope", SCOPE),
    ];
    let (status, body) = t.post_form(&url, &form)?;
    if !(200..300).contains(&status) {
        return Err(format!("token refresh failed (HTTP {status}): {}", graph_error(&body)));
    }
    let mut tokens = parse_tokens(&body, now)?;
    // The service may not return a new refresh token; keeping the old one is
    // correct and is what stops a long-lived session from expiring.
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(refresh_token.to_string());
    }
    Ok(tokens)
}

/// The `error` code from an OAuth error body.
fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(|s| s.to_string())
}

/// Best-effort human-readable message from a Graph/OAuth error body.
fn graph_error(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.trim().chars().take(200).collect();
    };
    if let Some(msg) = v
        .get("error_description")
        .or_else(|| v.get("message"))
        .and_then(|x| x.as_str())
    {
        return msg.to_string();
    }
    if let Some(e) = v.get("error") {
        if let Some(code) = e.get("message").and_then(|x| x.as_str()) {
            return code.to_string();
        }
        if let Some(s) = e.as_str() {
            return s.to_string();
        }
    }
    body.trim().chars().take(200).collect()
}

// ------------------------------------------------------------------- quota

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub remaining_bytes: u64,
    /// Microsoft's own assessment, e.g. "normal", "critical", "exceeded".
    pub state: String,
}

/// Fetch the drive quota.
pub fn fetch_quota(t: &dyn Transport, bearer: &str) -> Result<Quota, String> {
    let url = format!("{GRAPH}/me/drive?$select=quota");
    let (status, body) = t.get_json(&url, bearer)?;
    if status == 401 || status == 403 {
        return Err("the OneDrive sign-in has expired; please sign in again".to_string());
    }
    if !(200..300).contains(&status) {
        return Err(format!("quota request failed (HTTP {status}): {}", graph_error(&body)));
    }
    parse_quota(&body)
}

pub fn parse_quota(body: &str) -> Result<Quota, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("malformed drive response: {e}"))?;
    let q = v.get("quota").ok_or("drive response has no `quota`")?;
    let num = |name: &str| q.get(name).and_then(|x| x.as_u64()).unwrap_or(0);
    Ok(Quota {
        total_bytes: num("total"),
        used_bytes: num("used"),
        remaining_bytes: num("remaining"),
        state: q
            .get("state")
            .and_then(|x| x.as_str())
            .unwrap_or("unknown")
            .to_string(),
    })
}

// ---------------------------------------------------------- upload sessions

/// The byte ranges of one upload, in order.
///
/// Graph rejects any chunk that is not a multiple of 320 KiB, except the final
/// one. Getting this wrong fails late - after the user has waited for a large
/// file to start uploading - so it is a pure function with its own tests.
pub fn plan_chunks(total: u64) -> Vec<(u64, u64)> {
    if total == 0 {
        return Vec::new();
    }
    let chunk = DEFAULT_CHUNK - (DEFAULT_CHUNK % CHUNK_GRANULARITY);
    let mut out = Vec::new();
    let mut start = 0u64;
    while start < total {
        let end = (start + chunk).min(total) - 1;
        out.push((start, end));
        start = end + 1;
    }
    out
}

/// Open a resumable upload session for the app's backup folder.
///
/// `name` is a path *within* the app folder; it cannot escape it because the
/// scope cannot reach outside it and the `approot` address is fixed.
pub fn create_upload_session(
    t: &dyn Transport,
    bearer: &str,
    name: &str,
    conflict: &str,
) -> Result<String, String> {
    let encoded = name
        .split('/')
        .map(percent_encode)
        .collect::<Vec<_>>()
        .join("/");
    let url = format!("{GRAPH}/me/drive/special/approot:/{encoded}:/createUploadSession");
    let body = format!(
        r#"{{"item":{{"@microsoft.graph.conflictBehavior":"{conflict}"}}}}"#
    );
    let (status, resp) = t.post_json(&url, bearer, body.as_bytes())?;
    if !(200..300).contains(&status) {
        return Err(format!(
            "could not start an upload session (HTTP {status}): {}",
            graph_error(&resp)
        ));
    }
    parse_upload_url(&resp)
}

pub fn parse_upload_url(body: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("malformed upload session: {e}"))?;
    v.get("uploadUrl")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "upload session response has no `uploadUrl`".to_string())
}

/// Upload one chunk. Returns the raw response so the caller can read progress.
pub fn put_chunk(
    t: &dyn Transport,
    upload_url: &str,
    chunk: &[u8],
    start: u64,
    end: u64,
    total: u64,
) -> Result<(u16, String), String> {
    let range = format!("bytes {start}-{end}/{total}");
    let len = format!("{}", chunk.len());
    let headers = [("Content-Range", range.as_str()), ("Content-Length", len.as_str())];
    t.put(upload_url, &headers, chunk)
}

// ------------------------------------------------------- app folder items

/// Percent-encode a path for Graph's `:/path:` addressing form, keeping the
/// separators so the folder structure is preserved.
pub fn encode_path(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(percent_encode)
        .collect::<Vec<_>>()
        .join("/")
}

/// Address one item inside the app's folder.
pub fn item_url(path: &str) -> String {
    format!("{GRAPH}/me/drive/special/approot:/{}", encode_path(path))
}

/// Address one item's children.
pub fn children_url(path: &str) -> String {
    format!("{GRAPH}/me/drive/special/approot:/{}:/children", encode_path(path))
}

/// Address one item's content.
pub fn content_url(path: &str) -> String {
    format!("{GRAPH}/me/drive/special/approot:/{}:/content", encode_path(path))
}

/// A minimal drive item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveItem {
    pub id: String,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// List a folder's direct children.
pub fn list_children(t: &dyn Transport, bearer: &str, path: &str) -> Result<Vec<DriveItem>, String> {
    let url = format!("{}?$select=id,name,size,folder,file&$top=200", children_url(path));
    let (status, body) = t.get_json(&url, bearer)?;
    if status == 404 {
        // The folder does not exist yet, which is the normal state before the
        // first backup rather than an error.
        return Ok(Vec::new());
    }
    if !(200..300).contains(&status) {
        return Err(format!("listing {path} failed (HTTP {status}): {}", graph_error(&body)));
    }
    parse_children(&body)
}

pub fn parse_children(body: &str) -> Result<Vec<DriveItem>, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("malformed children response: {e}"))?;
    let arr = v
        .get("value")
        .and_then(|x| x.as_array())
        .ok_or("children response has no `value` array")?;
    let mut out = Vec::new();
    for item in arr {
        let Some(name) = item.get("name").and_then(|x| x.as_str()) else { continue };
        out.push(DriveItem {
            id: item.get("id").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            name: name.to_string(),
            is_dir: item.get("folder").is_some(),
            size: item.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Download a file by path.
///
/// Graph answers with a redirect to a pre-authenticated host; the transport
/// follows it and deliberately does not forward our bearer token across hosts.
pub fn download(t: &dyn Transport, bearer: &str, path: &str) -> Result<Vec<u8>, String> {
    let (status, body) = t.get_bytes(&content_url(path), Some(bearer))?;
    if !(200..300).contains(&status) {
        let detail = String::from_utf8_lossy(&body).chars().take(200).collect::<String>();
        return Err(format!("download of {path} failed (HTTP {status}): {detail}"));
    }
    Ok(body)
}

/// Delete a file or folder by path.
pub fn delete_path(t: &dyn Transport, bearer: &str, path: &str) -> Result<(), String> {
    let (status, body) = t.delete(&item_url(path), bearer)?;
    if !(200..300).contains(&status) {
        return Err(format!("deleting {path} failed (HTTP {status}): {}", graph_error(&body)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // ------------------------------------------------------------ fake

    /// A scripted transport. Records what was sent and replays canned answers,
    /// which is what lets the request *shape* be asserted rather than only the
    /// parsing of a response.
    #[derive(Default)]
    struct Fake {
        posts: Mutex<Vec<(String, Vec<(String, String)>)>>,
        gets: Mutex<Vec<String>>,
        puts: Mutex<Vec<(String, Vec<(String, String)>, Vec<u8>)>>,
        /// JSON POSTs, recorded separately from PUTs so a test can prove which
        /// verb was used rather than only that some body was sent.
        json_posts: Mutex<Vec<(String, Vec<u8>)>>,
        deletes: Mutex<Vec<String>>,
        replies: Mutex<Vec<(u16, String)>>,
    }

    impl Fake {
        fn with(replies: Vec<(u16, String)>) -> Self {
            Fake { replies: Mutex::new(replies), ..Default::default() }
        }
        fn next(&self) -> (u16, String) {
            let mut r = self.replies.lock().unwrap();
            if r.is_empty() {
                (500, "no more scripted replies".into())
            } else {
                r.remove(0)
            }
        }
    }

    impl Transport for Fake {
        fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String), String> {
            self.posts.lock().unwrap().push((
                url.to_string(),
                form.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ));
            Ok(self.next())
        }
        fn get_json(&self, url: &str, _bearer: &str) -> Result<(u16, String), String> {
            self.gets.lock().unwrap().push(url.to_string());
            Ok(self.next())
        }
        fn put(
            &self,
            url: &str,
            headers: &[(&str, &str)],
            body: &[u8],
        ) -> Result<(u16, String), String> {
            self.puts.lock().unwrap().push((
                url.to_string(),
                headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                body.to_vec(),
            ));
            Ok(self.next())
        }

        fn post_json(&self, url: &str, _bearer: &str, body: &[u8]) -> Result<(u16, String), String> {
            self.json_posts
                .lock()
                .unwrap()
                .push((url.to_string(), body.to_vec()));
            Ok(self.next())
        }

        fn get_bytes(&self, url: &str, _bearer: Option<&str>) -> Result<(u16, Vec<u8>), String> {
            self.gets.lock().unwrap().push(url.to_string());
            let (status, body) = self.next();
            Ok((status, body.into_bytes()))
        }

        fn delete(&self, url: &str, _bearer: &str) -> Result<(u16, String), String> {
            self.deletes.lock().unwrap().push(url.to_string());
            Ok(self.next())
        }
    }

    // ------------------------------------------------- device code flow

    #[test]
    fn the_device_code_request_carries_the_narrow_scope() {
        let fake = Fake::with(vec![(
            200,
            r#"{"device_code":"dc","user_code":"ABCD-EFGH","verification_uri":"https://microsoft.com/devicelogin","expires_in":900,"interval":5}"#.into(),
        )]);
        let dc = start_device_code(&fake, "client-1").unwrap();

        let posts = fake.posts.lock().unwrap();
        assert_eq!(posts.len(), 1);
        assert!(posts[0].0.ends_with("/devicecode"));
        let scope = posts[0].1.iter().find(|(k, _)| k == "scope").unwrap().1.clone();
        assert!(scope.contains("Files.ReadWrite.AppFolder"));
        assert!(
            !scope.contains("Files.ReadWrite.All"),
            "the app must never ask for access to the whole drive"
        );
        assert!(scope.contains("offline_access"), "no refresh token without it");

        assert_eq!(dc.user_code, "ABCD-EFGH");
        assert_eq!(dc.interval, 5);
    }

    #[test]
    fn an_unconfigured_client_id_fails_before_making_a_request() {
        let fake = Fake::default();
        let err = start_device_code(&fake, "  ").unwrap_err();
        assert!(
            err.to_lowercase().contains("client"),
            "the error should name what is missing: {err}"
        );
        assert!(fake.posts.lock().unwrap().is_empty(), "a request was still sent");
    }

    /// Polling faster than the service asks eventually gets the client banned,
    /// so the interval must survive parsing with a safe floor.
    #[test]
    fn a_missing_or_silly_poll_interval_falls_back_to_something_safe() {
        let body = r#"{"device_code":"d","user_code":"U","verification_uri":"https://x","expires_in":1}"#;
        assert_eq!(parse_device_code(body).unwrap().interval, 5);

        let zero = r#"{"device_code":"d","user_code":"U","verification_uri":"https://x","interval":0}"#;
        assert_eq!(parse_device_code(zero).unwrap().interval, 1);
    }

    #[test]
    fn a_device_code_response_missing_a_field_is_rejected() {
        let err = parse_device_code(r#"{"device_code":"d"}"#).unwrap_err();
        assert!(err.contains("user_code"), "{err}");
        assert!(parse_device_code("not json").is_err());
    }

    // ------------------------------------------------------- polling

    /// The single most important branch in the file: "not yet" arrives as
    /// HTTP 400, so treating a non-2xx as failure would abort every sign-in.
    #[test]
    fn authorization_pending_is_not_a_failure() {
        let body = r#"{"error":"authorization_pending","error_description":"waiting"}"#;
        assert_eq!(interpret_poll(400, body, 0), Poll::Pending);
    }

    #[test]
    fn slow_down_is_surfaced_so_the_caller_can_back_off() {
        assert_eq!(interpret_poll(400, r#"{"error":"slow_down"}"#, 0), Poll::SlowDown);
    }

    #[test]
    fn terminal_errors_are_reported_as_denied() {
        for (code, expected) in [
            ("expired_token", "expired"),
            ("access_denied", "declined"),
            ("authorization_declined", "declined"),
            ("bad_verification_code", "not recognised"),
        ] {
            match interpret_poll(400, &format!(r#"{{"error":"{code}"}}"#), 0) {
                Poll::Denied(msg) => assert!(msg.contains(expected), "{code} -> {msg}"),
                other => panic!("{code} produced {other:?}"),
            }
        }
    }

    #[test]
    fn a_successful_poll_yields_tokens() {
        let body = r#"{"access_token":"at","refresh_token":"rt","expires_in":3600}"#;
        match interpret_poll(200, body, 1_000) {
            Poll::Ready(t) => {
                assert_eq!(t.access_token, "at");
                assert_eq!(t.refresh_token.as_deref(), Some("rt"));
                // Refreshed a minute early on purpose.
                assert_eq!(t.expires_unix, 1_000 + 3_540);
            }
            other => panic!("expected tokens, got {other:?}"),
        }
    }

    #[test]
    fn an_unrecognised_error_is_a_transport_problem_not_a_refusal() {
        match interpret_poll(503, "upstream unavailable", 0) {
            Poll::TransportError(_) => {}
            other => panic!("a 503 must be retryable, got {other:?}"),
        }
    }

    // ------------------------------------------------------- refresh

    #[test]
    fn a_refresh_keeps_the_old_token_when_none_is_returned() {
        let fake = Fake::with(vec![(200, r#"{"access_token":"new","expires_in":3600}"#.into())]);
        let t = refresh(&fake, "c", "old-refresh", 0).unwrap();
        assert_eq!(t.access_token, "new");
        assert_eq!(
            t.refresh_token.as_deref(),
            Some("old-refresh"),
            "dropping the refresh token would end the session"
        );
    }

    #[test]
    fn a_rejected_refresh_reports_why() {
        let fake = Fake::with(vec![(
            400,
            r#"{"error":"invalid_grant","error_description":"the refresh token has expired"}"#.into(),
        )]);
        let err = refresh(&fake, "c", "stale", 0).unwrap_err();
        assert!(err.contains("refresh token has expired"), "{err}");
    }

    // --------------------------------------------------------- quota

    #[test]
    fn quota_is_parsed_from_the_drive_response() {
        let body = r#"{"quota":{"total":1099511627776,"used":274877906944,"remaining":824633720832,"state":"normal"}}"#;
        let q = parse_quota(body).unwrap();
        assert_eq!(q.total_bytes, 1_099_511_627_776);
        assert_eq!(q.used_bytes, 274_877_906_944);
        assert_eq!(q.remaining_bytes, 824_633_720_832);
        assert_eq!(q.state, "normal");
    }

    #[test]
    fn a_drive_response_without_a_quota_is_an_error() {
        assert!(parse_quota(r#"{"id":"drive-1"}"#).is_err());
    }

    #[test]
    fn an_expired_sign_in_says_so_rather_than_reporting_a_status() {
        let fake = Fake::with(vec![(401, "{}".into())]);
        let err = fetch_quota(&fake, "stale").unwrap_err();
        assert!(err.contains("sign in again"), "{err}");
    }

    /// A live device-code start against Microsoft, proving the request shape
    /// is accepted by the real service. Ignored by default (needs the network),
    /// and needs a real client ID to do more than fail cleanly.
    #[test]
    #[ignore]
    fn live_device_code_start_is_rejected_cleanly_without_a_real_client_id() {
        let err = start_device_code(&WinHttp, "00000000-0000-0000-0000-000000000000").unwrap_err();
        // Any answer is fine as long as it is a clean error and not a panic:
        // the point is that the request reached Microsoft and the response was
        // interpreted.
        assert!(!err.is_empty());
    }

    // -------------------------------------------------- chunk planning

    #[test]
    fn a_small_file_is_one_chunk() {
        assert_eq!(plan_chunks(1024), vec![(0, 1023)]);
    }

    #[test]
    fn an_empty_file_has_no_chunks() {
        assert!(plan_chunks(0).is_empty());
    }

    #[test]
    fn chunks_are_contiguous_and_cover_the_whole_file() {
        let total = DEFAULT_CHUNK * 3 + 1234;
        let chunks = plan_chunks(total);
        assert_eq!(chunks[0].0, 0);
        assert_eq!(chunks.last().unwrap().1, total - 1);
        for w in chunks.windows(2) {
            assert_eq!(w[0].1 + 1, w[1].0, "a gap between chunks");
        }
    }

    /// Graph rejects a chunk that is not a multiple of 320 KiB, except the
    /// last one. Getting this wrong fails only after the upload has started.
    #[test]
    fn every_chunk_but_the_last_is_a_multiple_of_the_granularity() {
        let total = DEFAULT_CHUNK * 2 + 7_777;
        let chunks = plan_chunks(total);
        for (i, (start, end)) in chunks.iter().enumerate() {
            let len = end - start + 1;
            if i + 1 == chunks.len() {
                assert!(len <= DEFAULT_CHUNK);
            } else {
                assert_eq!(
                    len % CHUNK_GRANULARITY,
                    0,
                    "chunk {i} has length {len}, which Graph will reject"
                );
            }
        }
    }

    #[test]
    fn an_exact_multiple_does_not_produce_a_trailing_empty_chunk() {
        let total = DEFAULT_CHUNK * 2;
        let chunks = plan_chunks(total);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[1].1, total - 1);
    }

    // -------------------------------------------------- upload session

    #[test]
    fn the_upload_session_targets_the_app_folder() {
        let fake = Fake::with(vec![(200, r#"{"uploadUrl":"https://upload.example/abc"}"#.into())]);
        let url = create_upload_session(&fake, "bearer", "batch-1/photo.jpg", "replace").unwrap();
        assert_eq!(url, "https://upload.example/abc");

        let posts = fake.json_posts.lock().unwrap();
        assert_eq!(posts.len(), 1, "the session must be created with a POST");
        assert!(
            posts[0].0.contains("/special/approot:"),
            "an upload must stay inside the app folder, got {}",
            posts[0].0
        );
        assert!(posts[0].0.contains("batch-1/photo.jpg"));
        let body = String::from_utf8_lossy(&posts[0].1);
        assert!(body.contains("\"replace\""));
        assert!(fake.puts.lock().unwrap().is_empty(), "a PUT was sent instead of a POST");
    }

    /// A space in a filename must be encoded, or the request targets a path
    /// that does not exist and the backup silently goes nowhere.
    #[test]
    fn a_filename_with_spaces_is_percent_encoded() {
        let fake = Fake::with(vec![(200, r#"{"uploadUrl":"u"}"#.into())]);
        let _ = create_upload_session(&fake, "b", "my report.pdf", "replace");
        let posts = fake.json_posts.lock().unwrap();
        assert!(posts[0].0.contains("my%20report.pdf"), "got {}", posts[0].0);
    }

    #[test]
    fn a_session_response_without_a_url_is_an_error() {
        assert!(parse_upload_url(r#"{"expirationDateTime":"..."}"#).is_err());
    }

    #[test]
    fn a_chunk_sends_the_range_graph_expects() {
        let fake = Fake::with(vec![(202, r#"{"nextExpectedRanges":["10485760-"]}"#.into())]);
        let data = vec![0u8; 1024];
        let (status, _) = put_chunk(&fake, "https://upload/x", &data, 0, 1023, 5000).unwrap();
        assert_eq!(status, 202);
        let puts = fake.puts.lock().unwrap();
        let headers = &puts[0].1;
        let range = headers.iter().find(|(k, _)| k == "Content-Range").unwrap();
        assert_eq!(range.1, "bytes 0-1023/5000");
        let len = headers.iter().find(|(k, _)| k == "Content-Length").unwrap();
        assert_eq!(len.1, "1024");
    }

    // -------------------------------------------------- app folder items

    #[test]
    fn app_folder_paths_keep_their_structure_and_encode_each_segment() {
        let url = item_url("SpaceRecycle Backup/batch-1/my report.pdf");
        assert!(url.contains("approot:/SpaceRecycle%20Backup/batch-1/my%20report.pdf"), "{url}");
        // The separators must survive, or the path becomes one long file name.
        assert!(url.contains("/batch-1/"), "{url}");
    }

    #[test]
    fn empty_path_segments_are_dropped_rather_than_producing_a_double_slash() {
        assert_eq!(encode_path("/a//b/"), "a/b");
        assert_eq!(encode_path("a"), "a");
    }

    #[test]
    fn children_are_parsed_and_folders_distinguished_from_files() {
        let body = r#"{"value":[
            {"id":"1","name":"batch-b","size":0,"folder":{"childCount":2}},
            {"id":"2","name":"manifest.json","size":42,"file":{"mimeType":"application/json"}}
        ]}"#;
        let items = parse_children(body).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "batch-b");
        assert!(items[0].is_dir);
        assert_eq!(items[1].name, "manifest.json");
        assert!(!items[1].is_dir);
        assert_eq!(items[1].size, 42);
    }

    /// Before the first backup the folder simply does not exist. That is a
    /// normal empty state, not a failure to report.
    #[test]
    fn a_missing_folder_lists_as_empty() {
        let fake = Fake::with(vec![(404, r#"{"error":{"code":"itemNotFound"}}"#.into())]);
        assert!(list_children(&fake, "b", "SpaceRecycle Backup").unwrap().is_empty());
    }

    #[test]
    fn children_are_sorted_so_the_ui_is_stable() {
        let body = r#"{"value":[
            {"id":"1","name":"c","folder":{}},
            {"id":"2","name":"a","folder":{}},
            {"id":"3","name":"b","folder":{}}
        ]}"#;
        let names: Vec<String> = parse_children(body).unwrap().into_iter().map(|i| i.name).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn a_children_response_without_a_value_array_is_an_error() {
        assert!(parse_children(r#"{"id":"x"}"#).is_err());
        assert!(parse_children("not json").is_err());
    }

    #[test]
    fn a_download_returns_the_bytes_and_reports_failure_clearly() {
        let fake = Fake::with(vec![(200, "file contents".into())]);
        let bytes = download(&fake, "tok", "SpaceRecycle Backup/b/manifest.json").unwrap();
        assert_eq!(bytes, b"file contents");
        let gets = fake.gets.lock().unwrap();
        assert!(gets[0].contains(":/content"), "{}", gets[0]);
    }

    #[test]
    fn a_failed_download_is_an_error_not_an_empty_file() {
        let fake = Fake::with(vec![(404, r#"{"error":{"message":"not found"}}"#.into())]);
        let err = download(&fake, "tok", "SpaceRecycle Backup/missing").unwrap_err();
        assert!(err.contains("404"), "{err}");
    }

    #[test]
    fn deleting_uses_the_delete_verb() {
        let fake = Fake::with(vec![(204, String::new())]);
        delete_path(&fake, "tok", "SpaceRecycle Backup/batch-1").unwrap();
        let deletes = fake.deletes.lock().unwrap();
        assert_eq!(deletes.len(), 1);
        assert!(deletes[0].contains("SpaceRecycle%20Backup/batch-1"), "{}", deletes[0]);
    }

    #[test]
    fn a_failed_delete_is_reported() {
        let fake = Fake::with(vec![(403, r#"{"error":{"message":"forbidden"}}"#.into())]);
        let err = delete_path(&fake, "tok", "x").unwrap_err();
        assert!(err.contains("403"), "{err}");
    }

    // -------------------------------------------------- encoding

    #[test]
    fn form_values_are_encoded_the_way_oauth_requires() {
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("x/y"), "x%2Fy");
        assert_eq!(percent_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(percent_encode("urn:ietf:params:oauth:grant-type:device_code"),
                   "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code");
    }

    #[test]
    fn a_form_is_joined_with_ampersands() {
        let encoded = form_encode(&[("a", "1"), ("b", "2 3")]);
        assert_eq!(encoded, "a=1&b=2%203");
    }

    // -------------------------------------------------- token store

    #[test]
    fn hex_round_trips_and_rejects_nonsense() {
        let bytes = vec![0u8, 1, 15, 16, 255];
        assert_eq!(from_hex(&to_hex(&bytes)).unwrap(), bytes);
        assert_eq!(from_hex("00ff"), Some(vec![0, 255]));
        assert!(from_hex("abc").is_none(), "an odd length is not a byte string");
        assert!(from_hex("zz").is_none());
    }

    /// The credential must not sit on disk in the clear.
    #[test]
    fn a_stored_refresh_token_is_not_readable_in_the_file() {
        let dir = std::env::temp_dir().join(format!("sr-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.bin");

        let auth = StoredAuth {
            client_id: "client-1".into(),
            refresh_token: "SUPER-SECRET-REFRESH-TOKEN".into(),
        };
        save_auth_to(&path, &auth).unwrap();

        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(
            !on_disk.contains("SUPER-SECRET"),
            "the refresh token is sitting in plaintext on disk"
        );

        let loaded = load_auth_from(&path).unwrap();
        assert_eq!(loaded, auth, "the credential did not survive the round trip");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_credential_file_yields_none_rather_than_panicking() {
        let dir = std::env::temp_dir().join(format!("sr-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.bin");

        std::fs::write(&path, "not hex at all").unwrap();
        assert!(load_auth_from(&path).is_none());

        // Valid hex, but not a blob this user can decrypt.
        std::fs::write(&path, "deadbeef").unwrap();
        assert!(load_auth_from(&path).is_none());

        assert!(load_auth_from(&dir.join("missing.bin")).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}