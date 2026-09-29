use serde::{Deserialize, Serialize};

/// Risk tier, drives both UI coloring and the extra confirmation gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskTier {
    /// Temp files, caches. Safe to purge outright.
    Safe,
    /// Rebuildable dev artifacts such as node_modules or target.
    Rebuildable,
    /// Large files that may be personal data.
    Personal,
    /// App leftovers that may contain user data.
    UserData,
}

impl RiskTier {
    pub fn label(&self) -> &'static str {
        match self {
            RiskTier::Safe => "Temp/cache",
            RiskTier::Rebuildable => "Dev artifact",
            RiskTier::Personal => "Large file",
            RiskTier::UserData => "App leftover",
        }
    }

    /// Stable machine key, matching the serde snake_case form the frontend
    /// sees. Used instead of the human label for UI-facing maps so the UI can
    /// localise the display itself.
    pub fn key(&self) -> &'static str {
        match self {
            RiskTier::Safe => "safe",
            RiskTier::Rebuildable => "rebuildable",
            RiskTier::Personal => "personal",
            RiskTier::UserData => "userdata",
        }
    }
}

/// Scanner category. Groups findings and drives default disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    #[default]
    TempJunk,
    SystemCache,
    DevArtifact,
    LargeFile,
    AppLeftover,
}

impl Category {
    pub fn label(&self) -> &'static str {
        match self {
            Category::TempJunk => "Temp junk",
            Category::SystemCache => "System cache",
            Category::DevArtifact => "Dev artifacts",
            Category::LargeFile => "Large files",
            Category::AppLeftover => "App leftovers",
        }
    }

    /// Stable machine key, matching the serde snake_case form the frontend
    /// sees. Used instead of the human label for UI-facing maps so the UI can
    /// localise the display itself.
    pub fn key(&self) -> &'static str {
        match self {
            Category::TempJunk => "temp_junk",
            Category::SystemCache => "system_cache",
            Category::DevArtifact => "dev_artifact",
            Category::LargeFile => "large_file",
            Category::AppLeftover => "app_leftover",
        }
    }

    pub fn risk(&self) -> RiskTier {
        match self {
            Category::TempJunk | Category::SystemCache => RiskTier::Safe,
            Category::DevArtifact => RiskTier::Rebuildable,
            Category::LargeFile => RiskTier::Personal,
            Category::AppLeftover => RiskTier::UserData,
        }
    }

    /// Temp junk and cache are garbage by definition, so permanent purge is
    /// the only meaningful disposition. Everything else defaults to quarantine.
    pub fn default_disposition(&self) -> Disposition {
        match self {
            Category::TempJunk | Category::SystemCache => Disposition::Purge,
            _ => Disposition::Quarantine,
        }
    }

    pub fn allows_purge(&self) -> bool {
        !matches!(self, Category::TempJunk | Category::SystemCache)
    }

    /// Age gating. Dev artifacts are always listed; anything that might be
    /// personal data needs a minimum untouched period.
    pub fn min_age_days(&self, large_file_age_days: u32, leftover_age_days: u32) -> u32 {
        match self {
            Category::TempJunk | Category::SystemCache | Category::DevArtifact => 0,
            Category::LargeFile => large_file_age_days,
            Category::AppLeftover => leftover_age_days,
        }
    }
}

/// How a finding will be cleaned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// Delete immediately, nothing retained.
    Purge,
    /// Move to the NTFS-compressed quarantine, restorable.
    Quarantine,
}

/// A single cleanup candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub id: String,
    pub rule_id: String,
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// Sum of file logical sizes (or the file's own size).
    pub logical_size: u64,
    /// Actual disk footprint, which is lower for compressed/sparse files.
    pub on_disk_size: u64,
    pub last_access_unix: i64,
    pub created_unix: i64,
    /// Age derived from last access, falling back to creation time.
    pub age_days: u32,
    /// Which timestamp the age was derived from.
    pub age_basis: AgeBasis,
    pub category: Category,
    pub risk: RiskTier,
    pub disposition: Disposition,
    pub child_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgeBasis {
    LastAccess,
    Creation,
}

impl Finding {
    /// Space actually reclaimed, which is the on-disk footprint.
    pub fn reclaimable(&self) -> u64 {
        match self.disposition {
            Disposition::Purge => self.on_disk_size,
            Disposition::Quarantine => self.on_disk_size,
        }
    }
}

/// A user selection handed to the cleanup pipeline.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub finding_id: String,
    pub disposition: Disposition,
}

/// Aggregate numbers for the overview page.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    pub label: String,
    pub mount: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
    pub used_ratio: f64,
}

/// Per-category totals for the overview summary.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryTotal {
    pub category: Category,
    pub count: u64,
    pub logical_bytes: u64,
    pub on_disk_bytes: u64,
}

/// Live scan progress streamed to the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgress {
    pub scanner: String,
    pub phase: String,
    pub dirs_scanned: u64,
    pub bytes_scanned: u64,
    pub findings: u64,
    pub skipped: u64,
    pub current_path: String,
    pub done: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_junk_defaults_to_purge_and_offers_no_alternative() {
        assert_eq!(Category::TempJunk.default_disposition(), Disposition::Purge);
        assert_eq!(Category::SystemCache.default_disposition(), Disposition::Purge);
        // "Purge" is already the default, so offering it again is meaningless.
        assert!(!Category::TempJunk.allows_purge());
        assert!(!Category::SystemCache.allows_purge());
    }

    #[test]
    fn data_categories_default_to_quarantine_but_allow_purge() {
        for cat in [Category::DevArtifact, Category::LargeFile, Category::AppLeftover] {
            assert_eq!(cat.default_disposition(), Disposition::Quarantine);
            assert!(cat.allows_purge());
        }
    }

    #[test]
    fn risk_tiers_map_to_categories() {
        assert_eq!(Category::TempJunk.risk(), RiskTier::Safe);
        assert_eq!(Category::SystemCache.risk(), RiskTier::Safe);
        assert_eq!(Category::DevArtifact.risk(), RiskTier::Rebuildable);
        assert_eq!(Category::LargeFile.risk(), RiskTier::Personal);
        assert_eq!(Category::AppLeftover.risk(), RiskTier::UserData);
    }

    #[test]
    fn dev_artifacts_skip_the_age_gate() {
        assert_eq!(Category::DevArtifact.min_age_days(30, 30), 0);
        assert_eq!(Category::TempJunk.min_age_days(30, 30), 0);
    }

    #[test]
    fn data_categories_honor_the_age_gate() {
        assert_eq!(Category::LargeFile.min_age_days(45, 30), 45);
        assert_eq!(Category::AppLeftover.min_age_days(45, 30), 30);
    }
}
