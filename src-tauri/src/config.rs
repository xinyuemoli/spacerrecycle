use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// Where quarantined data lives.
    pub quarantine_dir: PathBuf,
    /// Roots to exclude from every scan, in addition to the built-in denylist.
    pub extra_excluded_roots: Vec<PathBuf>,
    /// Store ID of the subscription add-on that unlocks cloud backup.
    ///
    /// Empty means the feature is not on sale yet, which is the shipping
    /// default: an unconfigured build must not pretend to offer a paid tier.
    pub subscription_addon_store_id: String,
    /// Development only: treat this build as subscribed without asking the
    /// Store. Documented as such because it is also the documented way to
    /// exercise the subscriber path on a machine with no Store identity.
    pub dev_subscriber_override: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            large_file_age_days: 30,
            leftover_age_days: 30,
            large_file_min_bytes: 500 * 1024 * 1024,
            quarantine_expiry_days: 7,
            quarantine_dir: default_quarantine_dir(),
            extra_excluded_roots: Vec::new(),
            subscription_addon_store_id: String::new(),
            dev_subscriber_override: false,
        }
    }
}

impl Config {
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

fn default_quarantine_dir() -> PathBuf {
    std::env::var("ProgramData")
        .map(|p| PathBuf::from(p).join("SpaceRecycle").join("Quarantine"))
        .unwrap_or_else(|_| PathBuf::from("C:/ProgramData/SpaceRecycle/Quarantine"))
}
