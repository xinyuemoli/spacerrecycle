//! Files On-Demand attribute handling.
//!
//! OneDrive keeps a *placeholder* on disk for every file in the sync root. The
//! file's local state is encoded entirely in its attributes, which is why this
//! module is the only place that needs to know the bit layout:
//!
//! | State | Attributes | Local footprint |
//! |---|---|---|
//! | online-only | `RECALL_ON_DATA_ACCESS` (or `RECALL_ON_OPEN` / `OFFLINE`), not `PINNED` | ~0 |
//! | pinned | `PINNED` | full |
//! | locally available | everything else | full, and free to release |
//!
//! "Releasing" is what Explorer's *Free up space* does: set `UNPINNED` and
//! clear `PINNED`. The sync engine performs the actual dehydration in the
//! background, so the operation is a *request*; it is verified separately
//! rather than assumed to have taken effect.

use serde::{Deserialize, Serialize};
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetFileAttributesW, SetFileAttributesW, FILE_FLAGS_AND_ATTRIBUTES,
};

/// The user asked for this file to stay on the device.
pub const FILE_ATTRIBUTE_PINNED: u32 = 0x0008_0000;
/// The user (or this tool) asked for the local copy to be released.
pub const FILE_ATTRIBUTE_UNPINNED: u32 = 0x0010_0000;
/// The content is not present locally; opening it triggers a download.
pub const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
/// The content is not present locally; reading it triggers a download.
pub const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
/// Legacy offline attribute, still set by some providers.
pub const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;

/// Every attribute that means "the content is not on this disk".
const NOT_LOCAL_MASK: u32 =
    FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_OFFLINE;

/// Where a cloud-backed file's content currently lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudState {
    /// Content is in the cloud only. Already costs no meaningful local space.
    OnlineOnly,
    /// Content is on disk because the user pinned it. Deliberately untouched:
    /// they asked for this file to be available offline.
    Pinned,
    /// Content is on disk with nothing asking it to stay. This is the state
    /// whose space can be reclaimed.
    LocallyAvailable,
}

impl CloudState {
    pub fn key(&self) -> &'static str {
        match self {
            CloudState::OnlineOnly => "online_only",
            CloudState::Pinned => "pinned",
            CloudState::LocallyAvailable => "locally_available",
        }
    }
}

/// Classify a raw attribute word.
///
/// Pinned wins over the recall bits: a file that is being kept on the device
/// can transiently still carry a recall attribute while it downloads, and
/// treating that as "online-only" would under-report the space the user chose
/// to spend.
pub fn classify(attrs: u32) -> CloudState {
    if attrs & FILE_ATTRIBUTE_PINNED != 0 {
        return CloudState::Pinned;
    }
    if attrs & NOT_LOCAL_MASK != 0 {
        return CloudState::OnlineOnly;
    }
    CloudState::LocallyAvailable
}

/// Read a path's attributes.
///
/// Goes through `std` rather than `GetFileAttributesW` because `MetadataExt`
/// already exposes the attribute word and handles the wide-string conversion.
/// Directories work the same way, which matters: a folder is dehydrated by
/// walking into it.
pub fn attributes(path: &Path) -> Option<u32> {
    std::fs::symlink_metadata(path).ok().map(|m| m.file_attributes())
}

/// The cloud state of a path, or `None` when it cannot be read.
pub fn state_of(path: &Path) -> Option<CloudState> {
    attributes(path).map(classify)
}

/// True when the path is a cloud placeholder at all.
///
/// A plain local file has none of the cloud bits and classifies as
/// `LocallyAvailable`, so callers that must not touch ordinary files need this
/// distinction: a non-OneDrive path is never a dehydration target.
pub fn is_placeholder(path: &Path) -> bool {
    match attributes(path) {
        Some(a) => {
            a & (FILE_ATTRIBUTE_PINNED
                | FILE_ATTRIBUTE_UNPINNED
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0
        }
        None => false,
    }
}

fn wide(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let mut v: Vec<u16> = path.as_os_str().encode_wide().collect();
    v.push(0);
    v
}

/// Write an attribute word back. Returns false on any failure.
pub fn set_attributes(path: &Path, attrs: u32) -> bool {
    let w = wide(path);
    unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(attrs)).is_ok() }
}

/// Ask the provider to release this path's local copy.
///
/// The full attribute word is preserved with only the pin bits changed, so
/// hidden / read-only / system survive the rewrite. Returns the state observed
/// *after* the call, which is how the caller learns whether the release has
/// already taken effect or is still pending in the sync engine.
pub fn unpin(path: &Path) -> Result<CloudState, String> {
    let attrs = attributes(path)
        .ok_or_else(|| format!("cannot read file attributes: {}", path.display()))?;
    let next = (attrs | FILE_ATTRIBUTE_UNPINNED) & !FILE_ATTRIBUTE_PINNED;
    if !set_attributes(path, next) {
        return Err(format!("cannot set file attributes: {}", path.display()));
    }
    Ok(state_of(path).unwrap_or(CloudState::LocallyAvailable))
}

/// Re-pin a path, keeping its content on the device. Used by tests and by the
/// undo path in the UI.
pub fn pin(path: &Path) -> Result<CloudState, String> {
    let attrs = attributes(path)
        .ok_or_else(|| format!("cannot read file attributes: {}", path.display()))?;
    let next = (attrs | FILE_ATTRIBUTE_PINNED) & !FILE_ATTRIBUTE_UNPINNED;
    if !set_attributes(path, next) {
        return Err(format!("cannot set file attributes: {}", path.display()));
    }
    Ok(state_of(path).unwrap_or(CloudState::Pinned))
}

/// The raw Win32 reader, kept for the one caller that needs to distinguish
/// "attributes are zero" from "the call failed".
pub fn raw_attributes(path: &Path) -> Option<u32> {
    let w = wide(path);
    let v = unsafe { GetFileAttributesW(PCWSTR(w.as_ptr())) };
    if v == u32::MAX || v == 0xFFFF_FFFF {
        None
    } else {
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_only_is_recognised() {
        assert_eq!(classify(FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS), CloudState::OnlineOnly);
        assert_eq!(classify(FILE_ATTRIBUTE_RECALL_ON_OPEN), CloudState::OnlineOnly);
        assert_eq!(classify(FILE_ATTRIBUTE_OFFLINE), CloudState::OnlineOnly);
        assert_eq!(
            classify(FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_UNPINNED),
            CloudState::OnlineOnly
        );
    }

    #[test]
    fn pinned_wins_over_a_transient_recall_bit() {
        assert_eq!(
            classify(FILE_ATTRIBUTE_PINNED | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS),
            CloudState::Pinned
        );
    }

    #[test]
    fn a_plain_file_is_locally_available() {
        // No cloud bits at all: an ordinary file, which is exactly the state
        // that must never be treated as a dehydration target on its own.
        assert_eq!(classify(0), CloudState::LocallyAvailable);
        assert_eq!(classify(0x20 /* ARCHIVE */), CloudState::LocallyAvailable);
    }

    #[test]
    fn a_plain_file_is_not_a_placeholder() {
        let p = std::env::temp_dir().join(format!("sr-cloud-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&p, b"ordinary").unwrap();
        assert!(
            !is_placeholder(&p),
            "an ordinary local file must not be mistaken for a cloud placeholder"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn unpinning_preserves_the_other_attributes() {
        let p = std::env::temp_dir().join(format!("sr-cloud-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&p, b"x").unwrap();
        let before = attributes(&p).unwrap();

        // Set PINNED by hand, then release it, and check that the unrelated
        // bits (ARCHIVE here) came through untouched.
        set_attributes(&p, before | FILE_ATTRIBUTE_PINNED);
        assert_eq!(state_of(&p), Some(CloudState::Pinned));

        let after_state = unpin(&p).unwrap();
        let after = attributes(&p).unwrap();

        assert_eq!(after_state, CloudState::LocallyAvailable);
        assert_eq!(after & FILE_ATTRIBUTE_PINNED, 0, "PINNED survived the release");
        assert_ne!(after & FILE_ATTRIBUTE_UNPINNED, 0, "UNPINNED was not set");
        assert_eq!(
            after & !(FILE_ATTRIBUTE_PINNED | FILE_ATTRIBUTE_UNPINNED),
            before & !(FILE_ATTRIBUTE_PINNED | FILE_ATTRIBUTE_UNPINNED),
            "an unrelated attribute was clobbered"
        );

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn pinning_and_unpinning_round_trip() {
        let p = std::env::temp_dir().join(format!("sr-cloud-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&p, b"x").unwrap();

        assert_eq!(pin(&p).unwrap(), CloudState::Pinned);
        assert_eq!(state_of(&p), Some(CloudState::Pinned));
        assert_eq!(unpin(&p).unwrap(), CloudState::LocallyAvailable);

        let _ = std::fs::remove_file(&p);
    }
}