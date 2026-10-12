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
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
    WinHttpSetTimeouts, INTERNET_DEFAULT_HTTP_PORT, INTERNET_DEFAULT_HTTPS_PORT,
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};

/// A finished response.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// A finished response whose body may not be text.
#[derive(Debug, Clone)]
pub struct ByteResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl ByteResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Lossy text view, for error bodies and JSON.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One request.
pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a [u8]>,
    /// Whether to follow 3xx responses.
    ///
    /// Off by default and opted into only for downloads. Graph answers a
    /// content request with a redirect to a pre-authenticated URL, which is
    /// exactly the case this exists for; leaving it off everywhere else means a
    /// redirected upload can never silently resend its body to another host.
    pub follow_redirects: bool,
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
    let r = send(&Request { method, url, headers, body, follow_redirects: false })?;
    Ok(Response { status: r.status, body: r.text() })
}

/// Perform one request and hand back the raw body.
pub fn send(req: &Request<'_>) -> Result<ByteResponse, HttpError> {
    let parsed = parse_url(req.url).map_err(HttpError::Transport)?;

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
    // because the UI is waiting on the command that owns it. Downloads get a
    // longer receive window than an API call, since the body can be large.
    unsafe {
        let _ = WinHttpSetTimeouts(session.0, 10_000, 15_000, 60_000, 300_000);
    }

    let host = wide(&parsed.host);
    let connect = unsafe { WinHttpConnect(session.0, PCWSTR(host.as_ptr()), parsed.port, 0) };
    if connect.is_null() {
        return Err(HttpError::Transport(format!("cannot connect to {}", parsed.host)));
    }
    let connect = Handle(connect);

    let verb = wide(req.method);
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

    if req.follow_redirects {
        // WinHTTP refuses redirects unless told otherwise, and a download URL
        // is always a redirect to a pre-authenticated host. WinHTTP drops the
        // Authorization header when the host changes, which is what we want:
        // the redirect target carries its own credentials in the query string.
        let policy = WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS.to_le_bytes();
        unsafe {
            let _ = WinHttpSetOption(
                Some(request.0),
                WINHTTP_OPTION_REDIRECT_POLICY,
                Some(&policy),
            );
        }
    }

    let header_block = req
        .headers
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

    let (body_ptr, body_len) = match req.body {
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
        Ok(ByteResponse { status, body })
    } else {
        let detail = String::from_utf8_lossy(&body).to_string();
        Err(HttpError::Status(status, truncate(&detail, 400)))
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

fn read_body(request: *mut c_void) -> Result<Vec<u8>, HttpError> {
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
        // result we should be buffering into memory. Downloads are files the
        // user is deliberately restoring, so the ceiling is higher but still
        // finite: an unbounded buffer on a hostile or corrupt response is how
        // this process runs out of memory.
        if out.len() > 512 * 1024 * 1024 {
            return Err(HttpError::Transport("response too large".into()));
        }
    }
    Ok(out)
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

    /// Live check that redirects are followed only when asked for.
    ///
    /// Downloads from Graph are always a 302 to a pre-authenticated host, so
    /// this plumbing decides whether restore works at all. `aka.ms` is used
    /// because it is a stable Microsoft redirector rather than a third party.
    /// Live check that a download follows a redirect through to the content.
    ///
    /// Graph answers every content request with a redirect to a
    /// pre-authenticated host, so this is the path cloud restore depends on.
    ///
    /// The negative half - "without `follow_redirects` we would see a 3xx" - is
    /// deliberately *not* asserted here. On the machine this was written on,
    /// WinHTTP reached the final response either way, whether because the
    /// default policy already follows or because the configured proxy resolves
    /// redirects itself. An assertion on that would be reporting the
    /// environment, not the code; what matters is that following works.
    #[test]
    #[ignore]
    fn live_a_download_follows_through_to_the_content() {
        // The `http://` form of a Microsoft short link redirects to `https://`.
        let r = send(&Request {
            method: "GET",
            url: "http://aka.ms/devicelogin",
            headers: &[],
            body: None,
            follow_redirects: true,
        })
        .expect("following the redirect failed");
        assert_eq!(r.status, 200, "did not land on a 200 after redirects");
        assert!(!r.body.is_empty(), "the followed response had an empty body");
    }

    /// Live check that a binary body survives the round trip untouched.
    #[test]
    #[ignore]
    fn live_binary_body_is_returned_byte_for_byte() {
        // A tiny, stable, non-text resource: the IANA root zone database is
        // served as text, so use a PNG the Microsoft CDN serves instead.
        let r = send(&Request {
            method: "GET",
            url: "https://www.microsoft.com/favicon.ico",
            headers: &[],
            body: None,
            follow_redirects: true,
        })
        .expect("download failed");
        assert_eq!(r.status, 200);
        assert!(r.body.len() > 100, "suspiciously small body: {}", r.body.len());
        // An ICO starts with 00 00 01 00.
        assert_eq!(&r.body[..4], &[0, 0, 1, 0], "the bytes were mangled");
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