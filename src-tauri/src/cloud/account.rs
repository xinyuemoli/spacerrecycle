//! Linking a OneDrive account and keeping its access token fresh.
//!
//! Two seams are injected rather than reached for directly, for the same reason
//! as everywhere else in this codebase: `cargo test` runs unpackaged, has no
//! linked account, and must not touch the developer's real `%APPDATA%`.
//!
//! * [`Transport`] — the network.
//! * [`AuthStore`] — where the refresh token lives.
//!
//! Both have a real implementation and an in-memory one, so the whole sign-in
//! and refresh flow is exercised end to end in tests without a Microsoft
//! account, a network, or a packaged build.

use crate::cloud::graph::{self, Poll, Quota, StoredAuth, Tokens, Transport};
use parking_lot::Mutex;
use std::sync::Arc;

/// The refresh token's home.
pub trait AuthStore: Send + Sync {
    fn load(&self) -> Option<StoredAuth>;
    fn save(&self, auth: &StoredAuth) -> Result<(), String>;
    fn clear(&self);
}

/// The real store: DPAPI-protected file under `%APPDATA%`.
pub struct DiskAuth;

impl AuthStore for DiskAuth {
    fn load(&self) -> Option<StoredAuth> {
        graph::load_auth()
    }
    fn save(&self, auth: &StoredAuth) -> Result<(), String> {
        graph::save_auth(auth)
    }
    fn clear(&self) {
        graph::clear_auth();
    }
}

/// Cached access token: `(token, expires_unix)`.
///
/// Kept in memory only. An access token is short-lived, and writing it to disk
/// would add a second credential to protect for no benefit.
pub type AccessCache = Arc<Mutex<Option<(String, i64)>>>;

/// A sign-in the user has started but not finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval_secs: i64,
    pub expires_unix: i64,
    pub client_id: String,
}

impl Session {
    pub fn is_expired(&self, now: i64) -> bool {
        now >= self.expires_unix
    }
}

/// What the UI needs to show about the account.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub connected: bool,
    pub quota: Option<Quota>,
    /// Why the quota is missing, when it is.
    pub detail: String,
}

/// Start a sign-in and return the code the user must enter.
pub fn begin(t: &dyn Transport, client_id: &str, now: i64) -> Result<Session, String> {
    let dc = graph::start_device_code(t, client_id)?;
    Ok(Session {
        device_code: dc.device_code,
        user_code: dc.user_code,
        verification_uri: dc.verification_uri,
        interval_secs: dc.interval,
        expires_unix: now + dc.expires_in,
        client_id: client_id.to_string(),
    })
}

/// One poll of a sign-in in progress.
///
/// Deliberately one poll per call rather than a loop: the caller is an IPC
/// command, and a command that blocks for the two minutes a user might take to
/// sign in is a frozen window.
pub fn poll(t: &dyn Transport, session: &Session, now: i64) -> Poll {
    if session.is_expired(now) {
        return Poll::Denied("the sign-in code expired".to_string());
    }
    graph::poll_token(t, &session.client_id, &session.device_code, now)
}

/// Persist a completed sign-in and prime the access cache.
pub fn accept(tokens: &Tokens, client_id: &str, store: &dyn AuthStore, cache: &AccessCache) -> Result<(), String> {
    let refresh = tokens
        .refresh_token
        .clone()
        .ok_or_else(|| "the service did not return a refresh token".to_string())?;
    store.save(&StoredAuth { client_id: client_id.to_string(), refresh_token: refresh })?;
    *cache.lock() = Some((tokens.access_token.clone(), tokens.expires_unix));
    Ok(())
}

/// A usable access token, refreshing if the cached one has expired.
pub fn access_token(
    t: &dyn Transport,
    store: &dyn AuthStore,
    cache: &AccessCache,
    now: i64,
) -> Result<String, String> {
    if let Some((token, exp)) = cache.lock().clone() {
        if now < exp {
            return Ok(token);
        }
    }

    let auth = store
        .load()
        .ok_or_else(|| "no OneDrive account is linked".to_string())?;

    match graph::refresh(t, &auth.client_id, &auth.refresh_token, now) {
        Ok(tokens) => {
            // The service sometimes issues a rotated refresh token. Dropping it
            // would end the session at the next expiry.
            if let Some(rotated) = &tokens.refresh_token {
                if rotated != &auth.refresh_token {
                    let _ = store.save(&StoredAuth {
                        client_id: auth.client_id.clone(),
                        refresh_token: rotated.clone(),
                    });
                }
            }
            *cache.lock() = Some((tokens.access_token.clone(), tokens.expires_unix));
            Ok(tokens.access_token)
        }
        Err(e) => {
            // A revoked or expired refresh token is permanent: retrying it on
            // every request would leave the UI claiming to be connected while
            // nothing works. Forget it so the app asks the user to sign in.
            if is_permanent_auth_failure(&e) {
                store.clear();
                *cache.lock() = None;
            }
            Err(e)
        }
    }
}

/// Whether re-trying this refresh could ever succeed.
fn is_permanent_auth_failure(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("invalid_grant")
        || e.contains("refresh token has expired")
        || e.contains("revoked")
        || e.contains("aadsts700082")
        || e.contains("aadsts50173")
}

/// Current account state, including the cloud quota.
///
/// A failure to read the quota does not unlink the account: the user is still
/// connected, the number is just unavailable right now, and the reason is
/// carried back so the UI can say which of the two it is.
pub fn status(t: &dyn Transport, store: &dyn AuthStore, cache: &AccessCache, now: i64) -> AccountStatus {
    if store.load().is_none() {
        return AccountStatus {
            connected: false,
            quota: None,
            detail: "not connected".to_string(),
        };
    }
    match access_token(t, store, cache, now).and_then(|at| graph::fetch_quota(t, &at)) {
        Ok(q) => AccountStatus { connected: true, quota: Some(q), detail: "connected".to_string() },
        Err(e) => AccountStatus {
            // The refresh may have just been rejected and the credential
            // dropped; report what is true *now* rather than what was true
            // when the call started.
            connected: store.load().is_some(),
            quota: None,
            detail: e,
        },
    }
}

/// Unlink the account.
pub fn sign_out(store: &dyn AuthStore, cache: &AccessCache) {
    store.clear();
    *cache.lock() = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    struct MemAuth {
        inner: StdMutex<Option<StoredAuth>>,
        /// Counts saves, so a test can prove a rotated token was persisted.
        saves: StdMutex<usize>,
    }

    impl MemAuth {
        fn with(auth: Option<StoredAuth>) -> Self {
            MemAuth { inner: StdMutex::new(auth), saves: StdMutex::new(0) }
        }
        fn current(&self) -> Option<StoredAuth> {
            self.inner.lock().unwrap().clone()
        }
        fn save_count(&self) -> usize {
            *self.saves.lock().unwrap()
        }
    }

    impl AuthStore for MemAuth {
        fn load(&self) -> Option<StoredAuth> {
            self.inner.lock().unwrap().clone()
        }
        fn save(&self, auth: &StoredAuth) -> Result<(), String> {
            *self.inner.lock().unwrap() = Some(auth.clone());
            *self.saves.lock().unwrap() += 1;
            Ok(())
        }
        fn clear(&self) {
            *self.inner.lock().unwrap() = None;
        }
    }

    /// Scripted transport, keyed by the form/url it receives.
    struct Fake {
        posts: StdMutex<Vec<(String, Vec<(String, String)>)>>,
        gets: StdMutex<Vec<String>>,
        replies: StdMutex<Vec<(u16, String)>>,
    }

    impl Fake {
        fn with(replies: Vec<(u16, String)>) -> Self {
            Fake {
                posts: StdMutex::new(Vec::new()),
                gets: StdMutex::new(Vec::new()),
                replies: StdMutex::new(replies),
            }
        }
        fn next(&self) -> (u16, String) {
            let mut r = self.replies.lock().unwrap();
            if r.is_empty() {
                (500, "{}".into())
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
        fn get_json(&self, url: &str, _b: &str) -> Result<(u16, String), String> {
            self.gets.lock().unwrap().push(url.to_string());
            Ok(self.next())
        }
        fn post_json(&self, _u: &str, _b: &str, _body: &[u8]) -> Result<(u16, String), String> {
            Ok(self.next())
        }
        fn put(&self, _u: &str, _h: &[(&str, &str)], _b: &[u8]) -> Result<(u16, String), String> {
            Ok(self.next())
        }
    }

    fn cache() -> AccessCache {
        Arc::new(Mutex::new(None))
    }

    fn stored() -> StoredAuth {
        StoredAuth { client_id: "client-1".into(), refresh_token: "refresh-1".into() }
    }

    // ------------------------------------------------------- sign-in

    #[test]
    fn begin_turns_a_device_code_into_a_usable_session() {
        let fake = Fake::with(vec![(
            200,
            r#"{"device_code":"dc","user_code":"WXYZ-1234","verification_uri":"https://microsoft.com/devicelogin","expires_in":900,"interval":5}"#.into(),
        )]);
        let s = begin(&fake, "client-1", 1_000).unwrap();
        assert_eq!(s.user_code, "WXYZ-1234");
        assert_eq!(s.interval_secs, 5);
        assert_eq!(s.expires_unix, 1_900);
        assert!(!s.is_expired(1_000));
        assert!(s.is_expired(1_900));
    }

    /// An expired code must not be sent to the service at all: it wastes a
    /// round trip and reports a less specific error.
    #[test]
    fn an_expired_session_is_refused_locally() {
        let fake = Fake::with(vec![]);
        let s = Session {
            device_code: "dc".into(),
            user_code: "U".into(),
            verification_uri: "https://x".into(),
            interval_secs: 5,
            expires_unix: 100,
            client_id: "c".into(),
        };
        match poll(&fake, &s, 200) {
            Poll::Denied(m) => assert!(m.contains("expired"), "{m}"),
            other => panic!("expected an expiry, got {other:?}"),
        }
        assert!(fake.posts.lock().unwrap().is_empty(), "a request was still sent");
    }

    #[test]
    fn a_completed_sign_in_is_persisted_and_cached() {
        let store = MemAuth::with(None);
        let c = cache();
        let tokens = Tokens {
            access_token: "access".into(),
            refresh_token: Some("refresh".into()),
            expires_unix: 5_000,
        };
        accept(&tokens, "client-1", &store, &c).unwrap();

        assert_eq!(store.current().unwrap().refresh_token, "refresh");
        assert_eq!(c.lock().clone().unwrap().0, "access");
    }

    /// Without a refresh token the link would silently die within the hour.
    #[test]
    fn a_sign_in_without_a_refresh_token_is_rejected() {
        let store = MemAuth::with(None);
        let tokens = Tokens { access_token: "a".into(), refresh_token: None, expires_unix: 1 };
        assert!(accept(&tokens, "c", &store, &cache()).is_err());
        assert!(store.current().is_none(), "a dead credential was stored anyway");
    }

    // ------------------------------------------------------ tokens

    #[test]
    fn a_fresh_cached_token_is_used_without_touching_the_network() {
        let fake = Fake::with(vec![]);
        let store = MemAuth::with(Some(stored()));
        let c = cache();
        *c.lock() = Some(("cached".into(), 5_000));

        let t = access_token(&fake, &store, &c, 1_000).unwrap();
        assert_eq!(t, "cached");
        assert!(fake.posts.lock().unwrap().is_empty(), "the token was refreshed early");
    }

    #[test]
    fn an_expired_cached_token_is_refreshed() {
        let fake = Fake::with(vec![(200, r#"{"access_token":"fresh","expires_in":3600}"#.into())]);
        let store = MemAuth::with(Some(stored()));
        let c = cache();
        *c.lock() = Some(("stale".into(), 500));

        let t = access_token(&fake, &store, &c, 1_000).unwrap();
        assert_eq!(t, "fresh");
        assert_eq!(c.lock().clone().unwrap().0, "fresh");
    }

    /// The service may rotate the refresh token; dropping the new one would end
    /// the session at the next expiry.
    #[test]
    fn a_rotated_refresh_token_is_persisted() {
        let fake = Fake::with(vec![(
            200,
            r#"{"access_token":"a","refresh_token":"rotated","expires_in":3600}"#.into(),
        )]);
        let store = MemAuth::with(Some(stored()));
        access_token(&fake, &store, &cache(), 1_000).unwrap();
        assert_eq!(store.current().unwrap().refresh_token, "rotated");
    }

    #[test]
    fn an_unchanged_refresh_token_is_not_rewritten() {
        let fake = Fake::with(vec![(200, r#"{"access_token":"a","expires_in":3600}"#.into())]);
        let store = MemAuth::with(Some(stored()));
        access_token(&fake, &store, &cache(), 1_000).unwrap();
        assert_eq!(store.save_count(), 0, "an unnecessary write to disk");
    }

    #[test]
    fn no_linked_account_reports_that_rather_than_a_network_error() {
        let fake = Fake::with(vec![]);
        let err = access_token(&fake, &MemAuth::with(None), &cache(), 0).unwrap_err();
        assert!(err.contains("no OneDrive account"), "{err}");
    }

    /// A revoked credential is permanent. Keeping it would leave the UI
    /// claiming to be connected while nothing works.
    #[test]
    fn a_revoked_refresh_token_is_forgotten() {
        let fake = Fake::with(vec![(
            400,
            r#"{"error":"invalid_grant","error_description":"AADSTS700082: the refresh token has expired"}"#.into(),
        )]);
        let store = MemAuth::with(Some(stored()));
        let c = cache();
        *c.lock() = Some(("stale".into(), 1));

        let err = access_token(&fake, &store, &c, 1_000).unwrap_err();
        assert!(err.to_lowercase().contains("expired"), "{err}");
        assert!(store.current().is_none(), "a dead credential was kept");
        assert!(c.lock().is_none(), "a dead access token stayed cached");
    }

    /// A transient network failure must NOT unlink the account: that would make
    /// a flaky connection look like a revoked subscription.
    #[test]
    fn a_transient_failure_keeps_the_account_linked() {
        let fake = Fake::with(vec![(503, "service unavailable".into())]);
        let store = MemAuth::with(Some(stored()));
        assert!(access_token(&fake, &store, &cache(), 1_000).is_err());
        assert!(store.current().is_some(), "a transient error unlinked the account");
    }

    #[test]
    fn permanent_failure_detection_matches_what_microsoft_returns() {
        assert!(is_permanent_auth_failure("error: invalid_grant"));
        assert!(is_permanent_auth_failure("AADSTS700082: The refresh token has expired"));
        assert!(is_permanent_auth_failure("the grant was revoked"));
        assert!(!is_permanent_auth_failure("service unavailable"));
        assert!(!is_permanent_auth_failure("HTTP 503: try again"));
    }

    // ------------------------------------------------------- status

    #[test]
    fn status_reports_a_quota_when_connected() {
        let fake = Fake::with(vec![(
            200,
            r#"{"quota":{"total":100,"used":40,"remaining":60,"state":"normal"}}"#.into(),
        )]);
        let store = MemAuth::with(Some(stored()));
        let c = cache();
        *c.lock() = Some(("token".into(), 9_999));

        let s = status(&fake, &store, &c, 1_000);
        assert!(s.connected);
        let q = s.quota.expect("no quota");
        assert_eq!(q.total_bytes, 100);
        assert_eq!(q.remaining_bytes, 60);
    }

    #[test]
    fn status_says_not_connected_when_nothing_is_linked() {
        let s = status(&Fake::with(vec![]), &MemAuth::with(None), &cache(), 0);
        assert!(!s.connected);
        assert!(s.quota.is_none());
        assert_eq!(s.detail, "not connected");
    }

    /// Being unable to read the quota is not the same as being signed out.
    #[test]
    fn a_quota_failure_still_reports_the_account_as_connected() {
        let fake = Fake::with(vec![(500, "graph is down".into())]);
        let store = MemAuth::with(Some(stored()));
        let c = cache();
        *c.lock() = Some(("token".into(), 9_999));

        let s = status(&fake, &store, &c, 1_000);
        assert!(s.connected, "a transient Graph error unlinked the account");
        assert!(s.quota.is_none());
        assert!(!s.detail.is_empty(), "no reason given for the missing quota");
    }

    #[test]
    fn signing_out_forgets_both_the_credential_and_the_cache() {
        let store = MemAuth::with(Some(stored()));
        let c = cache();
        *c.lock() = Some(("token".into(), 9_999));

        sign_out(&store, &c);
        assert!(store.current().is_none());
        assert!(c.lock().is_none());
    }
}