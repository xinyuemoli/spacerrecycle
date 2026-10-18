//! Subscription entitlement for the cloud-backup feature.
//!
//! The split in this module is deliberate and is what makes it testable:
//!
//! * [`evaluate`] and [`entitlement_with`] are pure policy - cache windows,
//!   offline grace, the development override, expiry handling. Every branch is
//!   covered by unit tests.
//! * [`ask_store`] is the only part that touches the Microsoft Store, and it
//!   cannot be exercised here: it needs a packaged (MSIX) build and a real
//!   subscription add-on to answer anything but "unavailable". It is isolated
//!   precisely so that its untestability cannot hide bugs in the policy.
//!
//! A caveat worth stating plainly, because it shapes what this gate can do:
//! the client is open source, and the feature uploads to the *user's own*
//! OneDrive, so it costs us nothing to serve. This gate therefore stops
//! accidental use and keeps the honest path honest - it is not, and cannot be,
//! an anti-tamper measure. Anyone who can build the project can remove it.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How long a previously successful check keeps working while the Store
/// cannot be reached.
///
/// The Store itself grants offline licences for a period; being stricter than
/// the thing we are checking would lock out paying users on a plane, which is
/// the one situation where a backup feature is most wanted.
pub const GRACE_DAYS: i64 = 7;

/// How long a positive result is trusted before asking the Store again.
///
/// Keeps app startup from making a Store call on every launch while still
/// noticing a cancelled subscription within a day.
pub const RECHECK_HOURS: i64 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntitlementSource {
    /// A live answer from the Microsoft Store.
    Store,
    /// A remembered answer, still inside its trust window.
    Cache,
    /// The development override in the config file.
    DevOverride,
    /// No package identity: this build was not distributed through the Store.
    ///
    /// A GitHub download or a hand-built binary. There is no subscription to
    /// buy on this copy and nothing to gate, so the paid features are open -
    /// and saying so explicitly is the point, because the alternative is a
    /// free build that spends its life reporting "not subscribed".
    Unpackaged,
    /// No add-on Store ID is configured, so the feature is not on sale yet.
    NotConfigured,
    /// The Store could not be asked and no usable cached answer exists.
    Unavailable,
}

impl EntitlementSource {
    pub fn key(&self) -> &'static str {
        match self {
            EntitlementSource::Store => "store",
            EntitlementSource::Cache => "cache",
            EntitlementSource::DevOverride => "dev_override",
            EntitlementSource::Unpackaged => "unpackaged",
            EntitlementSource::NotConfigured => "not_configured",
            EntitlementSource::Unavailable => "unavailable",
        }
    }

    /// Reasons that mean "there is nothing to buy here", not "you have not
    /// paid". Both leave the paid features open, and neither is a failure.
    fn nothing_on_sale(&self) -> bool {
        matches!(
            self,
            EntitlementSource::Unpackaged | EntitlementSource::NotConfigured
        )
    }
}

/// What the app is allowed to do for this user right now.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entitlement {
    pub subscriber: bool,
    pub source: EntitlementSource,
    pub checked_unix: i64,
    /// When the subscription lapses, if the Store reported a date.
    pub expires_unix: Option<i64>,
    /// Human-readable explanation. Surfaced in Settings so a user who has paid
    /// and is being refused can see *why* rather than guessing.
    pub detail: String,
}

impl Entitlement {
    /// Whether the paid feature set is open.
    ///
    /// **One subscription opens all of it.** The tier is not sold per feature,
    /// so there is one answer here and three names below rather than three
    /// independent flags. The names exist for the call sites: they read as
    /// "may I do this thing", and if the tier is ever split, each one can gain
    /// its own source without any call site changing.
    ///
    /// `Unpackaged` and `NotConfigured` both count as open, and that is
    /// deliberate. They mean nothing is on sale on this copy - either because
    /// the build did not come from the Store or because no add-on has been
    /// published yet - so there is nothing to gate. Treating either as closed
    /// ships a build whose advertised paid features are dead for everyone,
    /// including people who would have paid.
    ///
    /// The distinction between them is kept anyway, because they read
    /// differently to a user: "this copy is not from the Store" and "the
    /// subscription is not on sale yet" are different sentences, and only one
    /// of them is about them.
    pub fn paid_features_open(&self) -> bool {
        self.subscriber || self.source.nothing_on_sale()
    }

    /// Cloud backup and OneDrive quarantine, plus the quota display.
    pub fn cloud_allowed(&self) -> bool {
        self.paid_features_open()
    }

    /// Right-click importance marks and the icon overlays that show them.
    pub fn importance_allowed(&self) -> bool {
        self.paid_features_open()
    }

    /// The delete-guard: the minifilter and its resident service.
    pub fn guard_allowed(&self) -> bool {
        self.paid_features_open()
    }

    /// Whether the user is actually paying.
    ///
    /// Distinct from the three above, and the difference is the whole point:
    /// "the feature is open" and "the user bought it" are not the same claim.
    /// Settings shows this one, so a build with nothing on sale does not tell
    /// the user they are subscribed.
    pub fn is_subscriber(&self) -> bool {
        self.subscriber
    }

    /// The form the frontend receives.
    ///
    /// A separate type rather than three fields on `Entitlement`, because they
    /// are derived: storing them would be a second place the rule is written
    /// down, which is the failure mode the rest of this change exists to
    /// remove. The frontend cannot call a method across IPC, so the answer has
    /// to be computed here and sent.
    pub fn view(&self) -> EntitlementView {
        EntitlementView {
            entitlement: self.clone(),
            cloud_allowed: self.cloud_allowed(),
            importance_allowed: self.importance_allowed(),
            guard_allowed: self.guard_allowed(),
        }
    }
}

/// [`Entitlement`] plus the resolved feature flags, for the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementView {
    #[serde(flatten)]
    pub entitlement: Entitlement,
    /// One subscription opens all three; they are separate names for one fact,
    /// and sending all three keeps the frontend from re-deriving the rule.
    pub cloud_allowed: bool,
    pub importance_allowed: bool,
    pub guard_allowed: bool,
}

/// The Store's answer, normalised away from WinRT types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreAnswer {
    /// The add-on is active.
    Active {
        expires_unix: Option<i64>,
    },
    /// The Store answered, and the user does not have this add-on.
    Inactive,
    /// The Store could not be asked at all. The reason is kept verbatim: it is
    /// the only clue a user gets when a packaged build misbehaves.
    Unavailable(String),
}

/// The remembered answer, persisted between runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cached {
    pub subscriber: bool,
    pub checked_unix: i64,
    pub expires_unix: Option<i64>,
    pub detail: String,
}

pub fn cache_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("SpaceRecycle").join("entitlement.json"))
}

pub fn load_cache() -> Option<Cached> {
    let p = cache_path()?;
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

pub fn save_cache(c: &Cached) {
    let Some(p) = cache_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(c) {
        let _ = std::fs::write(p, json);
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Turn a Store answer plus whatever we remembered into a decision.
///
/// Pure: no clock, no disk, no WinRT. `now` is passed in so the grace window
/// can be tested without waiting a week.
pub fn evaluate(
    now: i64,
    answer: StoreAnswer,
    cached: Option<&Cached>,
) -> Entitlement {
    match answer {
        StoreAnswer::Active { expires_unix } => {
            // An active licence whose date has already passed is not active.
            // The Store occasionally reports a lapsed subscription as active
            // until the next sync, and granting it would let a cancelled
            // subscription keep working indefinitely on a machine that never
            // goes online again.
            if let Some(exp) = expires_unix {
                if exp <= now {
                    return Entitlement {
                        subscriber: false,
                        source: EntitlementSource::Store,
                        checked_unix: now,
                        expires_unix,
                        detail: "subscription expired".to_string(),
                    };
                }
            }
            Entitlement {
                subscriber: true,
                source: EntitlementSource::Store,
                checked_unix: now,
                expires_unix,
                detail: "active subscription".to_string(),
            }
        }
        StoreAnswer::Inactive => Entitlement {
            subscriber: false,
            source: EntitlementSource::Store,
            checked_unix: now,
            expires_unix: None,
            detail: "no active subscription for this account".to_string(),
        },
        StoreAnswer::Unavailable(reason) => {
            // Offline, unpackaged, or the Store API failed. Fall back to the
            // last real answer for a bounded time.
            match cached {
                Some(c) if c.subscriber && now.saturating_sub(c.checked_unix) <= GRACE_DAYS * 86_400 => {
                    Entitlement {
                        subscriber: true,
                        source: EntitlementSource::Cache,
                        checked_unix: c.checked_unix,
                        expires_unix: c.expires_unix,
                        detail: format!("using the last verified licence ({reason})"),
                    }
                }
                _ => Entitlement {
                    subscriber: false,
                    source: EntitlementSource::Unavailable,
                    checked_unix: now,
                    expires_unix: None,
                    detail: reason,
                },
            }
        }
    }
}

/// Whether a positive cached answer is fresh enough to skip the Store call.
fn cache_is_fresh(cached: Option<&Cached>, now: i64) -> bool {
    match cached {
        Some(c) => c.subscriber && now.saturating_sub(c.checked_unix) < RECHECK_HOURS * 3_600,
        None => false,
    }
}

/// Resolve the entitlement, with the Store probe injected.
///
/// The injection point exists because `ask_store` cannot run in this project's
/// test environment: it requires package identity, which an unpackaged `cargo
/// test` binary does not have. Passing the probe in lets every policy branch be
/// tested against a real store-shaped answer.
///
/// This variant assumes a packaged build, because that is the case every policy
/// test is about. The unpackaged branch has its own test and its own entry point.
pub fn entitlement_with<F>(cfg: &crate::config::Config, now: i64, ask: F) -> Entitlement
where
    F: FnOnce(&str) -> StoreAnswer,
{
    entitlement_with_opts(cfg, now, ask, true, true)
}

/// As [`entitlement_with`], but able to ignore a fresh positive cache.
///
/// `use_fresh_cache: false` is what a user's explicit "check again" uses: the
/// whole point of pressing it is to not be told what we already remembered.
///
/// `packaged` is injected for the same reason the probe is: whether this
/// process carries package identity is a fact about the running binary, and the
/// test binary has none. Reading it directly here would send every Store test
/// down the unpackaged branch.
pub fn entitlement_with_opts<F>(
    cfg: &crate::config::Config,
    now: i64,
    ask: F,
    use_fresh_cache: bool,
    packaged: bool,
) -> Entitlement
where
    F: FnOnce(&str) -> StoreAnswer,
{
    // The development override wins over everything, including an expired
    // subscription: its entire purpose is to unlock the feature on a machine
    // that has no Store identity at all.
    if cfg.dev_subscriber_override {
        return Entitlement {
            subscriber: true,
            source: EntitlementSource::DevOverride,
            checked_unix: now,
            expires_unix: None,
            detail: "development override is enabled in the config file".to_string(),
        };
    }

    let addon = cfg.subscription_addon_store_id();
    if addon.is_empty() {
        return Entitlement {
            subscriber: false,
            source: EntitlementSource::NotConfigured,
            checked_unix: now,
            expires_unix: None,
            detail: "no subscription add-on is configured".to_string(),
        };
    }

    // Checked before the Store is asked, not after it fails: the two reasons a
    // Store call cannot succeed are structurally different, and only one of
    // them is worth retrying. This build simply is not a Store build.
    if !packaged {
        return Entitlement {
            subscriber: false,
            source: EntitlementSource::Unpackaged,
            checked_unix: now,
            expires_unix: None,
            detail: "this build was not installed from the Microsoft Store".to_string(),
        };
    }

    let cached = load_cache();
    if use_fresh_cache && cache_is_fresh(cached.as_ref(), now) {
        let c = cached.expect("cache_is_fresh implies a cached value");
        return Entitlement {
            subscriber: true,
            source: EntitlementSource::Cache,
            checked_unix: c.checked_unix,
            expires_unix: c.expires_unix,
            detail: "recently verified".to_string(),
        };
    }

    let answer = ask(addon);
    let result = evaluate(now, answer, cached.as_ref());

    // Only a real Store answer is worth remembering. Persisting an
    // "unavailable" as a negative would extend an outage into a lockout.
    if matches!(result.source, EntitlementSource::Store) {
        save_cache(&Cached {
            subscriber: result.subscriber,
            checked_unix: result.checked_unix,
            expires_unix: result.expires_unix,
            detail: result.detail.clone(),
        });
    }

    result
}

/// Whether this process is running with package identity.
///
/// This is how "this build did not come from the Store" is told apart from "the
/// Store could not be reached". Identity is a property of how the build was
/// installed - a Store MSIX has it, an MSI or a hand-built binary does not - and
/// it cannot change while the app runs, so it is a structural fact rather than a
/// transient one.
///
/// The two situations call for opposite behaviour, which is why the distinction
/// has to be made at all. A build with no identity has no subscription to buy
/// and nothing to gate, so its features are open. A packaged build whose Store
/// check failed is a paying user on a plane, and the grace period applies.
#[cfg(windows)]
pub fn has_package_identity() -> bool {
    use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

    let mut length: u32 = 0;
    // The call is *expected* to fail here: with a null buffer and a zero length
    // it reports how much room the name needs, and that particular failure is
    // the proof that a package exists. Any other error means there is none.
    let rc = unsafe { GetCurrentPackageFullName(&mut length, None) };
    rc == ERROR_INSUFFICIENT_BUFFER
}

/// Non-Windows builds have no package identity by definition. The app is
/// Windows-only; this exists so the crate still builds elsewhere.
#[cfg(not(windows))]
pub fn has_package_identity() -> bool {
    false
}

/// Resolve the entitlement against the real Store.
pub fn entitlement() -> Entitlement {
    let cfg = crate::config::Config::load();
    entitlement_with_opts(&cfg, now_unix(), ask_store, true, has_package_identity())
}

/// Resolve the entitlement, ignoring a fresh positive cache.
///
/// Backs the Settings tab's explicit re-check.
pub fn entitlement_forced() -> Entitlement {
    let cfg = crate::config::Config::load();
    entitlement_with_opts(&cfg, now_unix(), ask_store, false, has_package_identity())
}

/// Ask the Microsoft Store whether the subscription add-on is active.
///
/// Requires package identity: `StoreContext::GetDefault` fails in an
/// unpackaged build, and that failure is reported as `Unavailable` rather than
/// as "not subscribed" so the offline grace period can apply.
#[cfg(windows)]
pub fn ask_store(addon_store_id: &str) -> StoreAnswer {
    use windows::Services::Store::StoreContext;

    let context = match StoreContext::GetDefault() {
        Ok(c) => c,
        Err(e) => {
            return StoreAnswer::Unavailable(format!(
                "the Microsoft Store is unavailable (needs a packaged build): {e}"
            ))
        }
    };

    let license = match context.GetAppLicenseAsync() {
        // `join` is windows-future's blocking wait (renamed from `get` in
        // 0.3). This module runs on a blocking thread via `offload`, so
        // waiting here cannot stall the webview.
        Ok(op) => match op.join() {
            Ok(l) => l,
            Err(e) => return StoreAnswer::Unavailable(format!("license query failed: {e}")),
        },
        Err(e) => return StoreAnswer::Unavailable(format!("license query failed: {e}")),
    };

    let addons = match license.AddOnLicenses() {
        Ok(a) => a,
        Err(e) => return StoreAnswer::Unavailable(format!("add-on list unavailable: {e}")),
    };

    // Iterate rather than index by key: the map's key is not reliably the add-on
    // Store ID across Partner Center configurations, so every identifier the
    // licensing API exposes is compared instead. Being generous here cannot
    // grant a subscription that is not present - each candidate still has to
    // report itself active.
    let mut expires: Option<i64> = None;
    if let Ok(it) = addons.First() {
        loop {
            let Ok(has) = it.HasCurrent() else { break };
            if !has {
                break;
            }
            let pair = match it.Current() {
                Ok(p) => p,
                Err(_) => break,
            };
            let key = pair.Key().map(|k| k.to_string()).unwrap_or_default();
            let Ok(lic) = pair.Value() else {
                let _ = it.MoveNext();
                continue;
            };
            let sku = lic.SkuStoreId().map(|s| s.to_string()).unwrap_or_default();
            let token = lic.InAppOfferToken().map(|s| s.to_string()).unwrap_or_default();

            if matches_identifier(addon_store_id, [key.as_str(), sku.as_str(), token.as_str()]) {
                if lic.IsActive().unwrap_or(false) {
                    if let Ok(dt) = lic.ExpirationDate() {
                        expires = filetime_to_unix(dt.UniversalTime);
                    }
                    return StoreAnswer::Active { expires_unix: expires };
                }
            }
            let _ = it.MoveNext();
        }
    }

    // Either the add-on is absent from the licence list or it is present and
    // inactive. Both mean the same thing to a caller, and neither is an error:
    // most installations simply have not bought it.
    StoreAnswer::Inactive
}

#[cfg(not(windows))]
pub fn ask_store(_addon_store_id: &str) -> StoreAnswer {
    StoreAnswer::Unavailable("the Microsoft Store is Windows-only".to_string())
}

/// Case-insensitive comparison against any of the identifiers a licence may
/// carry for the add-on.
fn matches_identifier<const N: usize>(wanted: &str, candidates: [&str; N]) -> bool {
    candidates
        .iter()
        .any(|c| !c.is_empty() && c.eq_ignore_ascii_case(wanted))
}

/// `DateTime::UniversalTime` is 100-nanosecond ticks since 1601-01-01.
fn filetime_to_unix(ticks: i64) -> Option<i64> {
    const EPOCH_DIFF: i64 = 116_444_736_000_000_000;
    if ticks <= 0 {
        return None;
    }
    Some((ticks - EPOCH_DIFF) / 10_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_addon() -> crate::config::Config {
        crate::config::Config {
            subscription_addon_store_id: "9ABCDEF12345".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn an_unconfigured_addon_is_not_a_subscription() {
        let cfg = crate::config::Config::default();
        assert!(cfg.subscription_addon_store_id.is_empty());
        let e = entitlement_with(&cfg, 1_000, |_| panic!("must not ask the Store when unconfigured"));
        assert!(!e.subscriber);
        assert_eq!(e.source, EntitlementSource::NotConfigured);
    }

    #[test]
    fn the_dev_override_short_circuits_everything() {
        let cfg = crate::config::Config {
            dev_subscriber_override: true,
            ..Default::default()
        };
        let e = entitlement_with(&cfg, 1_000, |_| panic!("must not ask the Store with the override on"));
        assert!(e.subscriber);
        assert_eq!(e.source, EntitlementSource::DevOverride);
    }

    #[test]
    fn an_active_licence_grants_the_feature() {
        let e = evaluate(1_000, StoreAnswer::Active { expires_unix: None }, None);
        assert!(e.subscriber);
        assert_eq!(e.source, EntitlementSource::Store);
        assert!(e.cloud_allowed());
    }

    // -------------------------------------------- the tier

    /// One subscription opens every paid feature. The three checks are one
    /// fact with three names, and a build where they disagree would be a build
    /// where the tier had been split without anyone saying so.
    #[test]
    fn one_subscription_opens_all_of_the_paid_features() {
        let e = evaluate(1_000, StoreAnswer::Active { expires_unix: None }, None);
        assert!(e.cloud_allowed());
        assert!(e.importance_allowed());
        assert!(e.guard_allowed());

        let off = evaluate(1_000, StoreAnswer::Inactive, None);
        assert!(!off.cloud_allowed());
        assert!(!off.importance_allowed());
        assert!(!off.guard_allowed());
    }

    /// A build with no add-on configured has nothing on sale, so nothing is
    /// gated. The alternative - what this code did - ships a build whose
    /// advertised paid features are dead for everyone, including people who
    /// would have paid.
    #[test]
    fn an_unconfigured_build_leaves_the_paid_features_open() {
        let cfg = crate::config::Config::default();
        assert!(cfg.subscription_addon_store_id.is_empty());
        let e = entitlement_with(&cfg, 1_000, |_| panic!("must not ask the Store when unconfigured"));

        assert!(!e.subscriber, "nobody is paying before anything is on sale");
        assert_eq!(e.source, EntitlementSource::NotConfigured);
        assert!(e.cloud_allowed(), "a feature that cannot be bought was locked");
        assert!(e.importance_allowed());
        assert!(e.guard_allowed());
    }

    /// "The feature is open" and "the user bought it" are different claims, and
    /// Settings must not conflate them.
    #[test]
    fn being_open_is_not_the_same_as_being_subscribed() {
        let cfg = crate::config::Config::default();
        let e = entitlement_with(&cfg, 1_000, |_| panic!("unconfigured"));
        assert!(e.paid_features_open());
        assert!(
            !e.is_subscriber(),
            "a build with nothing on sale claimed the user was subscribed"
        );
    }

    /// An expired subscription closes the gate. It must not be mistaken for the
    /// unconfigured case, which is open.
    #[test]
    fn an_expired_subscription_closes_the_paid_features() {
        let cfg = crate::config::Config {
            subscription_addon_store_id: "9ABCDEF12345".to_string(),
            ..Default::default()
        };
        let e = entitlement_with(&cfg, 2_000, |_| {
            StoreAnswer::Active { expires_unix: Some(1_500) }
        });
        assert_eq!(e.source, EntitlementSource::Store);
        assert!(!e.subscriber);
        assert!(!e.cloud_allowed(), "an expired subscription kept the gate open");
        assert!(!e.importance_allowed());
        assert!(!e.guard_allowed());
    }

    /// An unreachable Store with no cache is the case that locks out the
    /// GitHub/MSI build, which has no package identity and so can never obtain
    /// a licence. Recorded as a test because it is a real limitation of the
    /// distribution model, not an accident.
    #[test]
    fn an_unpackaged_build_cannot_hold_a_licence() {
        let cfg = crate::config::Config {
            subscription_addon_store_id: "9ABCDEF12345".to_string(),
            ..Default::default()
        };
        let e = entitlement_with(&cfg, 5_000, |_| {
            StoreAnswer::Unavailable("needs a packaged build".to_string())
        });
        assert_eq!(e.source, EntitlementSource::Unavailable);
        assert!(!e.subscriber);
        assert!(!e.cloud_allowed());
    }

    // ---------------------------------------- distribution

    /// A build that did not come from the Store has nothing to sell and
    /// therefore nothing to gate. The distinction from a packaged build whose
    /// Store check failed is the whole reason this branch exists: that one is a
    /// paying user offline, and this one is a free copy.
    #[test]
    fn a_build_that_is_not_from_the_store_gates_nothing() {
        let cfg = crate::config::Config {
            subscription_addon_store_id: "9ABCDEF12345".to_string(),
            ..Default::default()
        };
        let e = entitlement_with_opts(
            &cfg,
            1_000,
            |_| panic!("an unpackaged build must not call the Store at all"),
            true,
            false, // not packaged
        );

        assert_eq!(e.source, EntitlementSource::Unpackaged);
        assert!(!e.subscriber, "a free copy claimed the user was subscribed");
        assert!(e.cloud_allowed(), "a free copy locked a paid feature");
        assert!(e.importance_allowed());
        assert!(e.guard_allowed());
        assert!(e.paid_features_open());
    }

    /// The check is ordered before the Store call rather than after it fails,
    /// so a free copy never spends a round trip or a grace-period timer on a
    /// licence it could not hold.
    #[test]
    fn the_distribution_is_settled_before_the_store_is_consulted() {
        let cfg = crate::config::Config {
            subscription_addon_store_id: "9ABCDEF12345".to_string(),
            ..Default::default()
        };
        // The panic in the probe is the assertion: reaching it fails the test.
        let e = entitlement_with_opts(&cfg, 1_000, |_| panic!("asked the Store"), false, false);
        assert_eq!(e.source, EntitlementSource::Unpackaged);
    }

    /// The two open reasons are not the same sentence, and the UI shows which
    /// one applies: "this copy is not from the Store" is about the build, while
    /// "nothing is on sale yet" is about the product.
    #[test]
    fn the_two_nothing_on_sale_reasons_stay_distinguishable() {
        let packaged_but_unsold = crate::config::Config::default();
        let a = entitlement_with(&packaged_but_unsold, 1_000, |_| panic!("unconfigured"));
        assert_eq!(a.source, EntitlementSource::NotConfigured);

        let cfg = crate::config::Config {
            subscription_addon_store_id: "9ABCDEF12345".to_string(),
            ..Default::default()
        };
        let b = entitlement_with_opts(&cfg, 1_000, |_| panic!("unpackaged"), true, false);
        assert_eq!(b.source, EntitlementSource::Unpackaged);

        assert_ne!(a.source, b.source);
        assert_eq!(a.source.key(), "not_configured");
        assert_eq!(b.source.key(), "unpackaged");
    }

    /// The Store sometimes still reports a lapsed subscription as active. A
    /// date in the past must not be treated as a live licence.
    #[test]
    fn an_expired_licence_is_not_a_subscription() {
        let e = evaluate(2_000, StoreAnswer::Active { expires_unix: Some(1_500) }, None);
        assert!(!e.subscriber);
        assert_eq!(e.source, EntitlementSource::Store);
        assert!(e.detail.contains("expired"));
    }

    #[test]
    fn an_inactive_answer_is_definitive() {
        let e = evaluate(2_000, StoreAnswer::Inactive, None);
        assert!(!e.subscriber);
        assert_eq!(e.source, EntitlementSource::Store);
    }

    /// Offline inside the grace window keeps a paying user working.
    #[test]
    fn an_unavailable_store_falls_back_to_a_recent_cache() {
        let cached = Cached {
            subscriber: true,
            checked_unix: 1_000,
            expires_unix: None,
            detail: "active".into(),
        };
        let now = 1_000 + GRACE_DAYS * 86_400 - 1;
        let e = evaluate(now, StoreAnswer::Unavailable("offline".into()), Some(&cached));
        assert!(e.subscriber, "a paying user was locked out while offline");
        assert_eq!(e.source, EntitlementSource::Cache);
        assert!(e.detail.contains("offline"));
    }

    #[test]
    fn the_grace_window_actually_expires() {
        let cached = Cached {
            subscriber: true,
            checked_unix: 1_000,
            expires_unix: None,
            detail: "active".into(),
        };
        let now = 1_000 + GRACE_DAYS * 86_400 + 1;
        let e = evaluate(now, StoreAnswer::Unavailable("offline".into()), Some(&cached));
        assert!(!e.subscriber, "the grace window never closed");
        assert_eq!(e.source, EntitlementSource::Unavailable);
    }

    /// A cached *negative* must never grant anything, however fresh.
    #[test]
    fn a_cached_non_subscriber_stays_a_non_subscriber() {
        let cached = Cached {
            subscriber: false,
            checked_unix: 1_000,
            expires_unix: None,
            detail: "no subscription".into(),
        };
        let e = evaluate(1_001, StoreAnswer::Unavailable("offline".into()), Some(&cached));
        assert!(!e.subscriber);
        assert_eq!(e.source, EntitlementSource::Unavailable);
    }

    #[test]
    fn a_fresh_positive_cache_skips_the_store_call() {
        let mut cfg = cfg_with_addon();
        cfg.dev_subscriber_override = false;
        // A cache written by the live path is what makes this reachable; here
        // the check is exercised directly so the test does not depend on the
        // machine's real %APPDATA%.
        let cached = Cached {
            subscriber: true,
            checked_unix: now_unix(),
            expires_unix: None,
            detail: "active".into(),
        };
        assert!(cache_is_fresh(Some(&cached), now_unix()));
        let stale = Cached { checked_unix: now_unix() - RECHECK_HOURS * 3_600 - 1, ..cached };
        assert!(!cache_is_fresh(Some(&stale), now_unix()));
        assert!(!cache_is_fresh(None, now_unix()));
    }

    #[test]
    fn licence_identifiers_are_matched_case_insensitively() {
        assert!(matches_identifier("9abc", ["9ABC", "", ""]));
        assert!(matches_identifier("9abc", ["", "9AbC", ""]));
        assert!(matches_identifier("9abc", ["", "", "9ABC"]));
        assert!(!matches_identifier("9abc", ["", "", ""]));
        assert!(!matches_identifier("9abc", ["8xyz", "", ""]));
    }

    #[test]
    fn filetime_converts_to_unix_seconds() {
        // 1601 + 11644473600 s == the Unix epoch.
        assert_eq!(filetime_to_unix(116_444_736_000_000_000), Some(0));
        // One day later.
        assert_eq!(filetime_to_unix(116_444_736_000_000_000 + 86_400 * 10_000_000), Some(86_400));
        assert_eq!(filetime_to_unix(0), None);
        assert_eq!(filetime_to_unix(-5), None);
    }
}