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
    /// Cloud-backed content whose only local copy is a cached download. The
    /// file itself is untouched by releasing it, which is why this tier never
    /// triggers the red-tag confirmation gate.
    CloudCopy,
}

impl RiskTier {
    pub fn label(&self) -> &'static str {
        match self {
            RiskTier::Safe => "Temp/cache",
            RiskTier::Rebuildable => "Dev artifact",
            RiskTier::Personal => "Large file",
            RiskTier::UserData => "App leftover",
            RiskTier::CloudCopy => "Cloud copy",
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
            RiskTier::CloudCopy => "cloud",
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
    /// OneDrive content whose local copy is a cached download that can be
    /// released. Not a cleanup target in the deletion sense: the file stays
    /// exactly where it is, in the cloud.
    CloudPlaceholder,
}

impl Category {
    pub fn label(&self) -> &'static str {
        match self {
            Category::TempJunk => "Temp junk",
            Category::SystemCache => "System cache",
            Category::DevArtifact => "Dev artifacts",
            Category::LargeFile => "Large files",
            Category::AppLeftover => "App leftovers",
            Category::CloudPlaceholder => "OneDrive local copies",
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
            Category::CloudPlaceholder => "cloud_placeholder",
        }
    }

    pub fn risk(&self) -> RiskTier {
        match self {
            Category::TempJunk | Category::SystemCache => RiskTier::Safe,
            Category::DevArtifact => RiskTier::Rebuildable,
            Category::LargeFile => RiskTier::Personal,
            Category::AppLeftover => RiskTier::UserData,
            Category::CloudPlaceholder => RiskTier::CloudCopy,
        }
    }

    /// Temp junk and cache are garbage by definition, so permanent purge is
    /// the only meaningful disposition. Everything else defaults to quarantine.
    /// A cloud placeholder is neither: its only meaningful action is releasing
    /// the download.
    pub fn default_disposition(&self) -> Disposition {
        match self {
            Category::TempJunk | Category::SystemCache => Disposition::Purge,
            Category::CloudPlaceholder => Disposition::Dehydrate,
            _ => Disposition::Quarantine,
        }
    }

    pub fn allows_purge(&self) -> bool {
        !matches!(
            self,
            Category::TempJunk | Category::SystemCache | Category::CloudPlaceholder
        )
    }

    /// Whether this category may ever be permanently deleted.
    ///
    /// Distinct from `allows_purge`, which asks whether the *alternative* to
    /// the default is worth offering. A cloud placeholder's content lives in
    /// OneDrive; deleting it locally is not this tool's call to make.
    pub fn is_cloud(&self) -> bool {
        matches!(self, Category::CloudPlaceholder)
    }

    /// Whether "back this up, then delete it" is a meaningful offer.
    ///
    /// No for temp junk and caches, which are garbage by definition: uploading
    /// a temp file to the cloud costs the user quota to preserve something they
    /// already decided to throw away. No for cloud placeholders either - they
    /// are already in OneDrive, so "back it up" is a no-op dressed as a
    /// feature.
    pub fn allows_cloud_backup(&self) -> bool {
        !matches!(
            self,
            Category::TempJunk | Category::SystemCache | Category::CloudPlaceholder
        )
    }

    /// Age gating. Dev artifacts are always listed; anything that might be
    /// personal data needs a minimum untouched period.
    pub fn min_age_days(&self, large_file_age_days: u32, leftover_age_days: u32) -> u32 {
        match self {
            Category::TempJunk
            | Category::SystemCache
            | Category::DevArtifact
            | Category::CloudPlaceholder => 0,
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
    /// Release the local copy of a cloud-backed file. The file is untouched;
    /// Windows re-downloads it on first access, so this is fully reversible by
    /// simply opening the file.
    Dehydrate,
    /// Upload to the user's OneDrive, then remove the local copy.
    ///
    /// The only disposition that frees the disk *and* keeps a copy, which is
    /// why it is the safest way to act on something the user is unsure about -
    /// and why it is the one behind a subscription. The invariant that matters:
    /// the local copy is removed only after the upload has been confirmed, so
    /// a failure here can never lose data.
    CloudBackup,
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
            // Releasing a download frees exactly its local footprint; the
            // logical size stays in the cloud and is not reclaimed.
            Disposition::Dehydrate => self.on_disk_size,
            // The local copy goes away, so the footprint is what comes back.
            Disposition::CloudBackup => self.on_disk_size,
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

    /// A cloud placeholder is never deleted, and never quarantined either:
    /// neither action has a meaning for a file that already lives in OneDrive.
    #[test]
    fn cloud_placeholders_are_only_released() {
        assert_eq!(
            Category::CloudPlaceholder.default_disposition(),
            Disposition::Dehydrate
        );
        assert!(!Category::CloudPlaceholder.allows_purge());
        assert!(Category::CloudPlaceholder.is_cloud());
        assert_eq!(Category::CloudPlaceholder.risk(), RiskTier::CloudCopy);
    }

    /// The cloud tier must stay out of the red confirmation gate. Releasing a
    /// local copy cannot destroy data, and demanding a checkbox for it would
    /// train the user to tick the one that matters without reading it.
    #[test]
    fn releasing_a_local_copy_is_not_a_destructive_action() {
        assert_ne!(Category::CloudPlaceholder.risk(), RiskTier::UserData);
        assert_eq!(Category::CloudPlaceholder.min_age_days(30, 30), 0);
    }

    #[test]
    fn every_category_key_is_unique_and_stable() {
        let all = [
            Category::TempJunk,
            Category::SystemCache,
            Category::DevArtifact,
            Category::LargeFile,
            Category::AppLeftover,
            Category::CloudPlaceholder,
        ];
        let keys: Vec<&str> = all.iter().map(|c| c.key()).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(keys.len(), sorted.len(), "two categories share a key");
        assert_eq!(Category::CloudPlaceholder.key(), "cloud_placeholder");
    }

    /// Offering to upload garbage to the user's cloud would spend their quota
    /// preserving something they already decided to throw away.
    #[test]
    fn backing_up_garbage_is_not_offered() {
        assert!(!Category::TempJunk.allows_cloud_backup());
        assert!(!Category::SystemCache.allows_cloud_backup());
        assert!(!Category::CloudPlaceholder.allows_cloud_backup());
        assert!(Category::DevArtifact.allows_cloud_backup());
        assert!(Category::LargeFile.allows_cloud_backup());
        assert!(Category::AppLeftover.allows_cloud_backup());
    }

    /// A cloud backup removes the local copy, so it must count the same space
    /// as a purge - a smaller number would understate what the user reclaims.
    #[test]
    fn a_cloud_backup_reclaims_the_local_footprint() {
        let f = Finding {
            id: "x".into(),
            rule_id: "t".into(),
            name: "n".into(),
            path: "C:\\x".into(),
            is_dir: false,
            logical_size: 100,
            on_disk_size: 60,
            last_access_unix: 0,
            created_unix: 0,
            age_days: 1,
            age_basis: AgeBasis::Creation,
            category: Category::LargeFile,
            risk: RiskTier::Personal,
            disposition: Disposition::CloudBackup,
            child_count: 0,
        };
        assert_eq!(f.reclaimable(), 60);
    }
}
