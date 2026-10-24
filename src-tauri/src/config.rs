use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Where a quarantined item is put.
///
/// This is a user setting rather than a per-item choice because it answers
/// "where do I keep the things I might want back", and that is a property of
/// the person, not of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineTarget {
    /// The local quarantine folder. The default, and the only option that works
    /// with no account, no subscription and no network.
    #[default]
    Local,
    /// The user's own OneDrive, through the linked account.
    ///
    /// The file is uploaded first and the local copy is removed only once the
    /// upload is confirmed, so this frees local space while keeping the item
    /// restorable. It is a subscriber feature and is refused in the engine, not
    /// only in the UI.
    OneDrive,
}

impl QuarantineTarget {
    pub fn key(&self) -> &'static str {
        match self {
            QuarantineTarget::Local => "local",
            QuarantineTarget::OneDrive => "onedrive",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Age gate for the large-file scanner, in days.
    pub large_file_age_days: u32,
    /// Age gate for the app-leftover scanner, in days.
    pub leftover_age_days: u32,
    /// Minimum size for a file to count as "large".
    pub large_file_min_bytes: u64,
    /// Age at which quarantined items are flagged as expired.
    pub quarantine_expiry_days: u32,
    /// Where quarantined data lives, when the target is local.
    pub quarantine_dir: PathBuf,
    /// Whether quarantined items stay on this disk or go to the user's
    /// OneDrive. `quarantine_dir` is used for the local case and ignored for
    /// the cloud one.
    pub quarantine_target: QuarantineTarget,
    /// Roots to exclude from every scan, in addition to the built-in denylist.
    pub extra_excluded_roots: Vec<PathBuf>,
    /// Store ID of the subscription add-on that unlocks the paid features.
    ///
    /// An override, not the normal source. Release builds compile the real ID
    /// in with `DEFAULT_SUBSCRIPTION_ADDON_STORE_ID`, because asking a customer
    /// to hand-edit a JSON file to make their purchase work is not a thing that
    /// can ship. Empty here means "use whatever was compiled in".
    ///
    /// Read it through [`Config::subscription_addon_store_id`] rather than
    /// directly: an empty value stored in a config file would otherwise
    /// silently override the built-in one.
    pub subscription_addon_store_id: String,
    /// Development only: treat this build as subscribed without asking the
    /// Store, so the subscriber path can be exercised on a machine with no
    /// Store identity.
    ///
    /// **Compiled out unless the `dev-entitlement` feature is on**, which is why
    /// the field itself is behind a `cfg`. It used to be a plain config key that
    /// worked in every build, and that made it a licensing hole rather than a
    /// development aid: one line added to `%APPDATA%\SpaceRecycle\config.json`
    /// with Notepad, in the Store build, unlocked every paid feature with
    /// nothing on screen to say so.
    ///
    /// With the feature off the field does not exist, so a config file
    /// containing the key loads with the key ignored - serde skips unknown
    /// fields - and saving the config drops it. That is the intended behaviour:
    /// a release build should not carry a switch it will never honour.
    #[cfg(feature = "dev-entitlement")]
    pub dev_subscriber_override: bool,
    /// Azure application (client) ID used for the OneDrive device-code flow.
    ///
    /// Same arrangement as the add-on ID: compiled into release builds, this
    /// field is the override. Empty disables account linking entirely, which
    /// is correct for a build with no registered application - a sign-in
    /// button that cannot work is worse than no button.
    ///
    /// Read it through [`Config::graph_client_id`].
    pub graph_client_id: String,
}

/// Store ID of the subscription add-on, compiled into the binary.
///
/// This is the real Product ID from Partner Center, and it is the default
/// because a build that has to be told its own add-on ID at compile time is a
/// build that will eventually ship without one. The failure mode is quiet and
/// expensive: an unconfigured build resolves to `NotConfigured`, which means
/// "there is nothing on sale here", and the app correctly decides there is
/// nothing to gate - so every paid feature goes out free to everyone.
///
/// The authentication model makes publishing it harmless. The add-on can only
/// be acquired through the Store, against a package whose identity matches this
/// product, and the Store checks that itself; knowing the string buys nothing.
/// It is also visible in the binary regardless.
///
/// Overridable at build time, which is what the tests for the override path use:
///
/// ```text
/// $env:SPACERECYCLE_ADDON_ID="SomethingElse"; cargo build --release
/// ```
///
/// `option_env!` rather than a runtime lookup so that a shipped binary cannot be
/// repointed at a different add-on by setting a variable, and so that cargo
/// rebuilds when the value changes.
pub const DEFAULT_SUBSCRIPTION_ADDON_STORE_ID: &str = match option_env!("SPACERECYCLE_ADDON_ID") {
    Some(id) => id,
    // Empty: nothing is on sale.
    //
    // The subscription tier was dropped before the add-on was ever submitted to
    // the Store, so every feature ships open. The gating machinery stays, because
    // it is correct and cheap and the decision may come back - but an empty
    // add-on ID is what "no tier" looks like to it: `EntitlementSource::
    // NotConfigured`, which `paid_features_open()` already treats as "nothing to
    // gate, let everything through".
    //
    // Passing -AddonStoreId to the build restores the tier without touching this.
    None => "",
};

/// Azure application (client) ID for the OneDrive device-code flow, compiled
/// into the binary. Set with `SPACERECYCLE_GRAPH_CLIENT_ID` at build time.
///
/// This is not a secret: the device-code flow uses a public client, so the ID
/// is visible in the binary and in every authorization request regardless.
pub const DEFAULT_GRAPH_CLIENT_ID: &str = match option_env!("SPACERECYCLE_GRAPH_CLIENT_ID") {
    Some(id) => id,
    None => "",
};

impl Default for Config {
    fn default() -> Self {
        Config {
            large_file_age_days: 30,
            leftover_age_days: 30,
            large_file_min_bytes: 500 * 1024 * 1024,
            quarantine_expiry_days: 7,
            quarantine_dir: default_quarantine_dir(),
            quarantine_target: QuarantineTarget::Local,
            extra_excluded_roots: Vec::new(),
            subscription_addon_store_id: String::new(),
            #[cfg(feature = "dev-entitlement")]
            dev_subscriber_override: false,
            graph_client_id: String::new(),
        }
    }
}

impl Config {
    /// The add-on Store ID to check, preferring an explicit override.
    ///
    /// The fallback matters on upgrade. A config file written by an earlier
    /// build contains `"subscriptionAddonStoreId": ""`, and because `Config`
    /// derives `Deserialize` that empty string would otherwise win over the ID
    /// compiled into the new binary - so a customer who had already configured
    /// nothing would keep getting nothing after the update that introduced the
    /// built-in value. Treating empty as "not set" rather than "set to empty"
    /// is what makes shipping the ID possible at all.
    pub fn subscription_addon_store_id(&self) -> &str {
        non_empty_or(&self.subscription_addon_store_id, DEFAULT_SUBSCRIPTION_ADDON_STORE_ID)
    }

    /// The Azure client ID to use, preferring an explicit override. Empty means
    /// account linking is disabled.
    pub fn graph_client_id(&self) -> &str {
        non_empty_or(&self.graph_client_id, DEFAULT_GRAPH_CLIENT_ID)
    }

    pub fn load() -> Config {
        let path = match config_path() {
            Some(p) => p,
            None => return Config::default(),
        };
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "no config directory")
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }
}

fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("SpaceRecycle").join("config.json"))
}

/// `value` if it has content, otherwise `fallback`, with surrounding
/// whitespace discounted so that a value of `" "` counts as unset.
fn non_empty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value.trim()
    }
}

fn default_quarantine_dir() -> PathBuf {
    std::env::var("ProgramData")
        .map(|p| PathBuf::from(p).join("SpaceRecycle").join("Quarantine"))
        .unwrap_or_else(|_| PathBuf::from("C:/ProgramData/SpaceRecycle/Quarantine"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config as an older build would have written it: the field present and
    /// empty, because the older build had no value to put there.
    fn legacy_config_json() -> &'static str {
        r#"{
            "largeFileAgeDays": 30,
            "leftoverAgeDays": 30,
            "largeFileMinBytes": 524288000,
            "quarantineExpiryDays": 7,
            "quarantineDir": "C:\\ProgramData\\SpaceRecycle\\Quarantine",
            "quarantineTarget": "local",
            "extraExcludedRoots": [],
            "subscriptionAddonStoreId": "",
            "devSubscriberOverride": false,
            "graphClientId": ""
        }"#
    }

    #[test]
    fn an_explicit_addon_id_wins_over_the_built_in_one() {
        let mut cfg = Config::default();
        cfg.subscription_addon_store_id = "SpaceRecyclePro".to_string();
        assert_eq!(cfg.subscription_addon_store_id(), "SpaceRecyclePro");
    }

    #[test]
    fn an_empty_addon_id_falls_back_to_the_built_in_one() {
        let mut cfg = Config::default();
        cfg.subscription_addon_store_id = String::new();
        assert_eq!(
            cfg.subscription_addon_store_id(),
            DEFAULT_SUBSCRIPTION_ADDON_STORE_ID
        );
    }

    /// The case that makes shipping a compiled-in ID possible at all.
    ///
    /// Every customer upgrading from a build that predates the built-in value
    /// already has `"subscriptionAddonStoreId": ""` on disk. Because `Config`
    /// derives `Deserialize`, that empty string is a *present* value and would
    /// silently beat the compiled one, leaving a paying customer gated out of
    /// what they paid for. Round-tripping the old file through serde is what
    /// reproduces it; setting the field by hand in `Default` does not.
    #[test]
    fn a_config_written_before_the_built_in_id_existed_still_resolves() {
        let cfg: Config = serde_json::from_str(legacy_config_json()).expect("legacy config parses");
        assert!(
            cfg.subscription_addon_store_id.is_empty(),
            "the fixture must actually carry the empty string, or this proves nothing"
        );
        assert_eq!(
            cfg.subscription_addon_store_id(),
            DEFAULT_SUBSCRIPTION_ADDON_STORE_ID
        );
        assert_eq!(cfg.graph_client_id(), DEFAULT_GRAPH_CLIENT_ID);
    }

    #[test]
    fn the_built_in_values_come_from_the_build_environment() {
        // Not a tautology: it fails if the constants stop being wired to
        // option_env!, which is the only thing that makes a build able to
        // override them at all.
        assert_eq!(
            DEFAULT_SUBSCRIPTION_ADDON_STORE_ID,
            option_env!("SPACERECYCLE_ADDON_ID").unwrap_or("")
        );
        assert_eq!(
            DEFAULT_GRAPH_CLIENT_ID,
            option_env!("SPACERECYCLE_GRAPH_CLIENT_ID").unwrap_or("")
        );
    }

    /// A build with no add-on has nothing on sale, so nothing is gated.
    ///
    /// This replaces a test that asserted the opposite default. The subscription
    /// tier was dropped before the add-on was ever submitted, so the shipped
    /// product is free; keeping the published Store product ID as the default
    /// would now lock every paid feature behind a purchase that cannot be made.
    ///
    /// The trade-off is worth stating because it is a real one: a release that
    /// forgets to pass -AddonStoreId now ships free rather than shipping broken.
    /// For a product whose paid features are the delete guard and the importance
    /// marks, broken is worse than free.
    #[test]
    fn an_unconfigured_build_gates_nothing() {
        if option_env!("SPACERECYCLE_ADDON_ID").is_some() {
            return;
        }
        assert_eq!(DEFAULT_SUBSCRIPTION_ADDON_STORE_ID, "");

        let cfg = Config::default();
        assert!(cfg.subscription_addon_store_id().is_empty());

        // The gate itself: no add-on means NotConfigured, and NotConfigured
        // means open. The source is asserted separately from the permission
        // because "nothing to gate" and "the user paid" are different claims -
        // Settings shows the second one and must not claim the first.
        let e = crate::license::entitlement_with(&cfg, 1_000_000, |_| {
            crate::license::StoreAnswer::Unavailable("must not be asked".to_string())
        });
        assert_eq!(e.source, crate::license::EntitlementSource::NotConfigured);
        assert!(e.paid_features_open());
        assert!(e.guard_allowed());
        assert!(e.cloud_allowed());
        assert!(e.importance_allowed());
        assert!(!e.is_subscriber());
    }

    #[test]
    fn whitespace_counts_as_unset() {
        let mut cfg = Config::default();
        cfg.subscription_addon_store_id = "   ".to_string();
        cfg.graph_client_id = "\t".to_string();
        assert_eq!(
            cfg.subscription_addon_store_id(),
            DEFAULT_SUBSCRIPTION_ADDON_STORE_ID
        );
        assert_eq!(cfg.graph_client_id(), DEFAULT_GRAPH_CLIENT_ID);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed_off_a_real_value() {
        let mut cfg = Config::default();
        cfg.subscription_addon_store_id = "  SpaceRecyclePro\n".to_string();
        assert_eq!(cfg.subscription_addon_store_id(), "SpaceRecyclePro");
    }

    #[test]
    fn a_config_round_trips_through_json_unchanged() {
        // The config is written by the app and read by the app, so a field that
        // serializes but does not deserialize would lose a setting silently on
        // the next launch.
        let mut cfg = Config::default();
        cfg.quarantine_target = QuarantineTarget::OneDrive;
        cfg.subscription_addon_store_id = "SpaceRecyclePro".to_string();
        let json = serde_json::to_string(&cfg).expect("serializes");
        let back: Config = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back.quarantine_target, cfg.quarantine_target);
        assert_eq!(back.subscription_addon_store_id(), "SpaceRecyclePro");
    }

    /// The development override round-trips when it exists.
    ///
    /// Gated, because with the feature off the field is not in the struct at
    /// all - which is the point of the gate, and would otherwise make this test
    /// stop compiling rather than stop being relevant.
    #[cfg(feature = "dev-entitlement")]
    #[test]
    fn the_dev_override_round_trips_when_it_exists() {
        let mut cfg = Config::default();
        cfg.dev_subscriber_override = true;
        let json = serde_json::to_string(&cfg).expect("serializes");
        let back: Config = serde_json::from_str(&json).expect("deserializes");
        assert!(back.dev_subscriber_override);
    }

    /// A release build ignores the key rather than carrying a switch it will
    /// never honour.
    ///
    /// This is the test for the hole being closed: a config file that somebody
    /// edited with Notepad to unlock the paid features has to load, and has to
    /// leave the app unsubscribed. Silently ignoring is the intended outcome -
    /// serde skips unknown fields, so the file stays valid and the app starts.
    #[cfg(not(feature = "dev-entitlement"))]
    #[test]
    fn a_config_claiming_the_dev_override_is_ignored_without_the_feature() {
        let json = r#"{"devSubscriberOverride":true}"#;
        let cfg: Config = serde_json::from_str(json).expect("an unknown key must not break loading");

        // The field does not exist, so the only way to assert it did nothing is
        // through what it would have changed. With no add-on configured this is
        // `NotConfigured`, and with one it is the Store answer - never the
        // override.
        let e = crate::license::entitlement_with_addon(
            "SpaceRecyclePro",
            &cfg,
            1,
            |_| crate::license::StoreAnswer::Inactive,
            false,
            true,
        );
        assert_ne!(
            e.source,
            crate::license::EntitlementSource::DevOverride,
            "a config key unlocked the paid features in a build without the feature"
        );
        assert!(!e.subscriber);
    }
}
