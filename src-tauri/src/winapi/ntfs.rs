use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::GetLastError;
use windows::Win32::Storage::FileSystem::{
    GetCompressedFileSizeW, GetVolumeInformationW, SetFileAttributesW,
    FILE_ATTRIBUTE_COMPRESSED,
};

/// Actual bytes a file occupies on disk, which accounts for NTFS compression
/// and sparse allocation. This is the number that matters for reclaiming space.
pub fn on_disk_size(path: &Path) -> Option<u64> {
    let wide = wide(path);
    let mut high: u32 = 0;
    unsafe {
        let low = GetCompressedFileSizeW(PCWSTR(wide.as_ptr()), Some(&mut high));
        // On failure the return value is INVALID_FILE_SIZE and GetLastError
        // carries the reason.
        if low == u32::MAX && GetLastError() != windows::Win32::Foundation::ERROR_SUCCESS {
            return None;
        }
        Some(((high as u64) << 32) | low as u64)
    }
}

/// True when the volume hosting `path` is NTFS. NTFS compression is only
/// available there, so callers must degrade gracefully otherwise.
pub fn is_ntfs(path: &Path) -> bool {
    let root = volume_root(path);
    let wide = wide(Path::new(&root));
    let mut vol_label = [0u16; 261];
    let mut fs_buf = [0u16; 261];
    let mut serial = 0u32;
    let mut max_comp = 0u32;
    let mut flags = 0u32;
    unsafe {
        if GetVolumeInformationW(
            PCWSTR(wide.as_ptr()),
            Some(&mut vol_label),
            Some(&mut serial),
            Some(&mut max_comp),
            Some(&mut flags),
            Some(&mut fs_buf),
        )
        .is_err()
        {
            return false;
        }
        let end = fs_buf.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf16_lossy(&fs_buf[..end]).eq_ignore_ascii_case("NTFS")
    }
}

/// Turn on NTFS compression for a directory. Returns false if the volume does
/// not support it; the caller then stores the data uncompressed.
///
/// NOTE: this only marks the directory so *newly created* files inherit the
/// compressed attribute. It does NOT compress files that already exist inside
/// it (including files moved in with `rename`). Use [`compress_tree`] to
/// actually compress existing content.
pub fn set_directory_compression(dir: &Path) -> bool {
    let wide = wide(dir);
    unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_COMPRESSED).is_ok() }
}

/// `FSCTL_SET_COMPRESSION` control code (winioctl.h). `compact /c` calls this.
const FSCTL_SET_COMPRESSION: u32 = 0x0009_C040;
/// LZNT1 is NTFS's standard per-file compression format.
const COMPRESSION_FORMAT_LZNT1: u16 = 0x0002;

/// Compress one file (or directory entry) in place via `FSCTL_SET_COMPRESSION`.
///
/// Returns false when the volume isn't NTFS or the file can't be opened for
/// write (e.g. still locked); callers degrade by keeping it uncompressed.
pub fn compress_path(path: &Path) -> bool {
    if !is_ntfs(path) {
        return false;
    }
    compress_path_unchecked(path)
}

/// Recursively compress a file or directory tree in place. Symlinks are not
/// followed: a link pointing out of a quarantined tree must not be walked.
///
/// The NTFS check happens once here rather than per file, so compressing a
/// tree of tens of thousands of files does not pay a `GetVolumeInformationW`
/// round trip for every one of them.
pub fn compress_tree(path: &Path) {
    if !is_ntfs(path) {
        return;
    }
    compress_tree_unchecked(path);
}

fn compress_tree_unchecked(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if meta.file_type().is_symlink() {
        return;
    }
    if meta.is_dir() {
        if let Ok(rd) = std::fs::read_dir(path) {
            for entry in rd.flatten() {
                compress_tree_unchecked(&entry.path());
            }
        }
    }
    let _ = compress_path_unchecked(path);
}

/// The actual `DeviceIoControl(FSCTL_SET_COMPRESSION, LZNT1)` call, without
/// the NTFS pre-check (callers gate on `is_ntfs` once for the whole tree).
fn compress_path_unchecked(path: &Path) -> bool {
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    let w = wide(path);
    let handle = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    };
    let Ok(handle) = handle else { return false };

    let format = COMPRESSION_FORMAT_LZNT1;
    let result = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_SET_COMPRESSION,
            Some(&format as *const u16 as *const core::ffi::c_void),
            std::mem::size_of::<u16>() as u32,
            None,
            0,
            None,
            None,
        )
    };
    unsafe {
        let _ = windows::Win32::Foundation::CloseHandle(handle);
    }
    result.is_ok()
}

fn volume_root(path: &Path) -> String {
    let s = path.to_string_lossy().to_string();
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        format!("{}\\0", &s[..2])
    } else {
        "C:\\0".to_string()
    }
}

pub fn wide(path: &Path) -> Vec<u16> {
    let mut v: Vec<u16> = path.as_os_str().encode_wide().collect();
    v.push(0);
    v
}

/// Overwrite a path's access, write, and creation timestamps.
///
/// Used to age a file backwards, which is how tests construct fixtures that
/// must clear (or fail) the age gate.
pub fn set_times(path: &Path, filetime: u64) -> std::io::Result<()> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, SetFileTime, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, OPEN_EXISTING,
    };

    let wide = wide(path);
    let ft = FILETIME {
        dwLowDateTime: (filetime & 0xFFFF_FFFF) as u32,
        dwHighDateTime: (filetime >> 32) as u32,
    };
    unsafe {
        let h = CreateFileW(
            PCWSTR(wide.as_ptr()),
            FILE_WRITE_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )?;
        let r = SetFileTime(h, Some(&ft), Some(&ft), Some(&ft));
        let _ = windows::Win32::Foundation::CloseHandle(h);
        r.map_err(|e| std::io::Error::from_raw_os_error(e.code().0 as i32))
    }
}

/// Move a path's timestamps `days` into the past.
pub fn backdate(path: &Path, days: u32) -> std::io::Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    set_times(path, crate::scanner::walk::unix_to_filetime(now - days as i64 * 86_400))
}


/// Open a file without FILE_SHARE_DELETE, so it cannot be deleted or renamed
/// while the returned handle is alive.
///
/// Only the test suite needs this: std::fs::File::open shares delete access on
/// Windows, so a plain open does not actually lock anything out.
pub fn open_exclusive(path: &Path) -> Option<std::fs::File> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, OPEN_EXISTING,
    };

    let w = wide(path);
    unsafe {
        let h = CreateFileW(
            PCWSTR(w.as_ptr()),
            windows::Win32::Foundation::GENERIC_READ.0,
            FILE_SHARE_READ,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        )
        .ok()?;
        // Hand the raw handle to an owned File so closing is automatic and
        // nothing leaks.
        let owned = OwnedHandle::from_raw_handle(h.0 as *mut _);
        Some(std::fs::File::from(owned))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backdate_moves_the_timestamps_back() {
        use std::os::windows::fs::MetadataExt;
        let p = std::env::temp_dir().join(format!("sr-time-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&p, b"x").unwrap();

        let before = std::fs::metadata(&p).unwrap().last_access_time() as i64;
        backdate(&p, 120).unwrap();
        let after = std::fs::metadata(&p).unwrap().last_access_time() as i64;

        assert!(
            after < before - 100 * 86_400,
            "backdate did not move the timestamp: {} -> {}",
            before,
            after
        );
        let _ = std::fs::remove_file(&p);
    }
}
