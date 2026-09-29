//! NTFS-compressed quarantine store.
//!
//! Items are moved into batch directories. The batch directory carries the
//! NTFS compression attribute, so children are compressed by the filesystem
//! itself: no CPU cost, no archive format, and restore is a plain move that
//! the user never has to decompress.

pub mod manifest;
pub mod ops;

#[allow(unused_imports)]
pub use manifest::{list_batches, BatchMeta, Manifest, ManifestEntry};
#[allow(unused_imports)]
pub use ops::{
    purge, purge_batch, quarantine_items, restore_batch, restore_item, ConflictPolicy,
    PurgeReport, QuarantineReport, RestoreReport,
};
