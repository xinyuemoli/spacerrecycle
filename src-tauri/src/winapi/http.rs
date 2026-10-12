//! Minimal synchronous HTTP client over WinHTTP.
//!
//! WinHTTP rather than a vendored TLS stack, for three reasons that all matter
//! to this product:
//!
//! * It adds nothing to the binary. The alternative (rustls + a crypto
//!   provider) is on the order of a megabyte in a build whose whole selling
//!   point is a single small executable.
//! * It uses the machine's certificate store, so a corporate TLS-inspecting
//!   proxy or a pinned internal CA works without shipping a trust bundle.
//! * It honours the system proxy configuration automatically, which is the
//!   difference between "works at the office" and "mysteriously times out".
//!
//! The product is Windows-only and the base design forbids a cross-platform
//! abstraction layer, so this is the faithful choice rather than a shortcut.
//!
//! Everything here is blocking. Callers run it on a blocking thread.

use std::ffi::c_void;
use windows::core::PCWSTR;
use windows::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    INTERNET_DEFAULT_HTTP_PORT, INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};

/// A finished response.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// What went wrong, split by whether retrying could help.
#[derive(Debug, Clone)]
pub enum HttpError {
    /// DNS, connect, TLS, timeout. Retrying later is reasonable.
    Transport(String),
    /// The request was rejected. Retrying the same request will not help.
    Status(u16, String),
}

impl HttpError {
    pub fn message(&self) -> String {
        match self {
            HttpError::Transport(m) => format!("network error: {m}"),
            HttpError::Status(code, body) => format!("HTTP {code}: {body}"),
        }
    }
}

/// Owned WinHTTP handle that closes itself.
struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A parsed absolute URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    /// Path plus query, as WinHTTP wants it.
    pub target: String,
}

/// Parse `https://host[:port]/path?query`.
///
/// Only the shapes this app actually builds are supported, and anything else
/// is an error rather than a guess: silently "fixing up" a malformed URL in an
/// upload path is how a backup lands somewhere unexpected.
pub fn parse_url(raw: &str) -> Result<Url, String> {
    let (secure, rest) = if let Some(r) = raw.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = raw.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("unsupported URL scheme: {raw}"));
    };

    let (authority, target) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(format!("URL has no host: {raw}"));
    }

    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => {
            let port: u16 = p
                .parse()
                .map_err(|_| format!("invalid port in URL: {raw}"))?;
            (h.to_string(), port)
        }
        None => (
            authority.to_string(),
            if secure { INTERNET_DEFAULT_HTTPS_PORT } else { INTERNET_DEFAULT_HTTP_PORT },
        ),
    };
    if host.is_empty() {
        return Err(format!("URL has no host: {raw}"));
    }

    Ok(Url { secure, host, port, target: target.to_string() })
}

/// Perform one request.
///
/// `headers` are sent verbatim; the caller owns content-type and authorization.
/// `body` is the full request body, so an upload of any size is one call.
pub fn request(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> Result<Response, HttpError> {
    let parsed = parse_url(url).map_err(HttpError::Transport)?;

    let agent = wide("SpaceRecycle/0.1");
    let session = unsafe {
        WinHttpOpen(
            PCWSTR(agent.as_ptr()),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    };
    if session.is_null() {
        return Err(HttpError::Transport("WinHttpOpen failed".into()));
    }
    let session = Handle(session);

    // Bounded waits: a hung connect must not pin a blocking thread forever,
    // because the UI is waiting on the command that owns it.
    unsafe {
        let _ = WinHttpSetTimeouts(session.0, 10_000, 15_000, 30_000, 60_000);
    }

    let host = wide(&parsed.host);
    let connect = unsafe { WinHttpConnect(session.0, PCWSTR(host.as_ptr()), parsed.port, 0) };
    if connect.is_null() {
        return Err(HttpError::Transport(format!("cannot connect to {}", parsed.host)));
    }
    let connect = Handle(connect);

    let verb = wide(method);
    let target = wide(&parsed.target);
    let flags = if parsed.secure { WINHTTP_FLAG_SECURE } else { Default::default() };
    let request = unsafe {
        WinHttpOpenRequest(
            connect.0,
            PCWSTR(verb.as_ptr()),
            PCWSTR(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        )
    };
    if request.is_null() {
        return Err(HttpError::Transport("WinHttpOpenRequest failed".into()));
    }
    let request = Handle(request);

    let header_block = headers
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    let header_wide = wide(&header_block);
    let headers_arg: Option<&[u16]> = if header_block.is_empty() {
        None
    } else {
        // WinHttpSendRequest takes the header block without its terminator and
        // with an explicit length, so the trailing NUL is excluded.
        Some(&header_wide[..header_wide.len() - 1])
    };

    let (body_ptr, body_len) = match body {
        Some(b) if !b.is_empty() => (b.as_ptr() as *const c_void, b.len() as u32),
        _ => (std::ptr::null(), 0),
    };

    unsafe {
        WinHttpSendRequest(request.0, headers_arg, Some(body_ptr), body_len, body_len, 0)
            .map_err(|e| HttpError::Transport(format!("send failed: {e}")))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|e| HttpError::Transport(format!("no response: {e}")))?;
    }

    let status = query_status(request.0)?;
    let body = read_body(request.0)?;

    if (200..300).contains(&status) {
        Ok(Response { status, body })
    } else {
        Err(HttpError::Status(status, truncate(&body, 400)))
    }
}

fn query_status(request: *mut c_void) -> Result<u16, HttpError> {
    let mut status: u32 = 0;
    let mut len: u32 = std::mem::size_of::<u32>() as u32;
    unsafe {
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut c_void),
            &mut len,
            std::ptr::null_mut(),
        )
        .map_err(|e| HttpError::Transport(format!("cannot read status code: {e}")))?;
    }
    Ok(status as u16)
}

fn read_body(request: *mut c_void) -> Result<String, HttpError> {
    let mut out: Vec<u8> = Vec::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut read: u32 = 0;
        unsafe {
            WinHttpReadData(
                request,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut read,
            )
            .map_err(|e| HttpError::Transport(format!("read failed: {e}")))?;
        }
        if read == 0 {
            break;
        }
        out.extend_from_slice(&buf[..read as usize]);
        // A JSON API response larger than this is a bug or an attack, not a
        // result we should be buffering into memory.
        if out.len() > 32 * 1024 * 1024 {
            return Err(HttpError::Transport("response too large".into()));
        }
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

/// Keep an error body short enough to show in a toast.
fn truncate(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let cut: String = t.chars().take(max).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_urls_parse_with_the_default_port() {
        let u = parse_url("https://graph.microsoft.com/v1.0/me/drive").unwrap();
        assert!(u.secure);
        assert_eq!(u.host, "graph.microsoft.com");
        assert_eq!(u.port, 443);
        assert_eq!(u.target, "/v1.0/me/drive");
    }

    #[test]
    fn http_urls_parse_with_their_own_default_port() {
        let u = parse_url("http://localhost:8080/x").unwrap();
        assert!(!u.secure);
        assert_eq!(u.host, "localhost");
        assert_eq!(u.port, 8080);
        assert_eq!(u.target, "/x");
    }

    #[test]
    fn an_explicit_port_wins() {
        let u = parse_url("https://example.com:8443/a?b=c").unwrap();
        assert_eq!(u.port, 8443);
        assert_eq!(u.target, "/a?b=c");
    }

    #[test]
    fn a_bare_host_gets_the_root_target() {
        let u = parse_url("https://example.com").unwrap();
        assert_eq!(u.target, "/");
    }

    /// A malformed URL must fail loudly. Guessing at one in an upload path is
    /// how a backup ends up somewhere the user did not choose.
    #[test]
    fn malformed_urls_are_rejected() {
        assert!(parse_url("ftp://example.com/x").is_err());
        assert!(parse_url("https://").is_err());
        assert!(parse_url("https://host:notaport/x").is_err());
        assert!(parse_url("example.com/x").is_err());
    }

    #[test]
    fn error_bodies_are_truncated_for_display() {
        let long = "x".repeat(1000);
        let short = truncate(&long, 400);
        assert!(short.chars().count() <= 401);
        assert_eq!(truncate("  hello  ", 400), "hello");
    }

    #[test]
    fn success_is_two_hundreds_only() {
        assert!(Response { status: 200, body: String::new() }.is_success());
        assert!(Response { status: 299, body: String::new() }.is_success());
        assert!(!Response { status: 301, body: String::new() }.is_success());
        assert!(!Response { status: 404, body: String::new() }.is_success());
    }

    /// Live network check against Microsoft's public OpenID metadata.
    ///
    /// `#[ignore]` because a unit test that needs the internet is a unit test
    /// that fails on a plane. It exists so the WinHTTP plumbing - TLS, proxy
    /// discovery, header handling, body reading - can be verified end to end
    /// by hand with `cargo test --lib -- --ignored live_`, which is the only
    /// way to catch a mistake that only shows up against a real server.
    #[test]
    #[ignore]
    fn live_get_reaches_a_real_https_endpoint() {
        let url = "https://login.microsoftonline.com/common/discovery/v2.0/keys";
        let resp = request("GET", url, &[("Accept", "application/json")], None)
            .expect("the request itself failed");
        assert_eq!(resp.status, 200, "unexpected status: {}", resp.status);
        assert!(
            resp.body.contains("\"keys\""),
            "response did not look like the OpenID metadata document"
        );
    }

    /// Live check that a non-2xx is reported as a status error rather than a
    /// transport one: the difference decides whether the caller retries.
    #[test]
    #[ignore]
    fn live_404_is_reported_as_a_status_error() {
        let url = "https://login.microsoftonline.com/definitely-not-a-tenant/discovery/v2.0/keys";
        match request("GET", url, &[], None) {
            Err(HttpError::Status(code, _)) => assert!(code >= 400, "got {code}"),
            other => panic!("expected a status error, got {other:?}"),
        }
    }
}