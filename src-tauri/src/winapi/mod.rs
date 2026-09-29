//! Thin wrappers over the Win32 APIs we need. No cross-platform abstraction
//! layer: the product is Windows-only by design.

pub mod elevate;
pub mod ntfs;
pub mod volume;

pub use elevate::{is_elevated, is_permission_denied, relaunch_elevated};
pub use ntfs::{backdate, is_ntfs, on_disk_size, set_directory_compression, set_times};
pub use volume::list_volumes;

/// Re-exported so tests can build FILETIME values without reaching into the
/// scanner internals.
pub use crate::scanner::walk;
