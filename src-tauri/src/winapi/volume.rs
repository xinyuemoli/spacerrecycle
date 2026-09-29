use crate::model::VolumeInfo;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
};

/// `GetDriveTypeW` return codes. The `windows` crate does not re-export the
/// DRIVE_* constants for this feature set, so the two we branch on are spelled
/// out here. See: winbase.h.
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;

/// Enumerate local volumes that can actually be reclaimed from.
///
/// `GetLogicalDriveStringsW` is the right source here: it yields plain drive
/// letters (C:\\, D:\\, ...). `FindFirstVolumeW` would instead hand back
/// volume-GUID paths, which are awkward to display and require a second lookup
/// to resolve.
///
/// Removable drives are included: an often-overlooked disk filling up is
/// exactly the problem this tool exists to solve. Network drives are skipped.
pub fn list_volumes() -> Vec<VolumeInfo> {
    let mut out = Vec::new();

    unsafe {
        // First call sizes the buffer, second call fills it. The wrapper
        // takes a single slice and derives the length itself.
        let mut probe: Vec<u16> = vec![0u16; 512];
        let needed = GetLogicalDriveStringsW(Some(&mut probe));
        if needed == 0 {
            return out;
        }
        let mut buf = vec![0u16; needed as usize + 2];
        let written = GetLogicalDriveStringsW(Some(&mut buf));
        if written == 0 {
            return out;
        }

        // The result is a run of NUL-terminated strings ending with an empty one.
        let mut start = 0usize;
        while start < buf.len() {
            let end = match buf[start..].iter().position(|&c| c == 0) {
                Some(e) => start + e,
                None => break,
            };
            if end == start {
                break;
            }
            let mount = String::from_utf16_lossy(&buf[start..end]);
            start = end + 1;

            if let Some(info) = query_volume(&mount) {
                out.push(info);
            }
        }
    }
    out
}

fn nul_terminated(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn is_drive_letter_mount(mount: &str) -> bool {
    let b = mount.as_bytes();
    b.len() == 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

fn query_volume(mount: &str) -> Option<VolumeInfo> {
    if !is_drive_letter_mount(mount) {
        return None;
    }
    unsafe {
        // Only local fixed/removable media. Network (DRIVE_REMOTE) and
        // CD-ROM are excluded: neither is meaningfully reclaimable here.
        let drive_type = GetDriveTypeW(PCWSTR(mount_to_wide(mount).as_ptr()));
        if drive_type != DRIVE_FIXED && drive_type != DRIVE_REMOVABLE {
            return None;
        }

        let wide = mount_to_wide(mount);
        let mut free_to_caller: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            Some(&mut free_to_caller),
            Some(&mut total),
            Some(&mut total_free),
        )
        .ok()?;

        // arg2: volume label, arg6: filesystem name.
        let mut vol_label = [0u16; 261];
        let mut fs_name_buf = [0u16; 261];
        let mut serial: u32 = 0;
        let mut max_comp: u32 = 0;
        let mut flags: u32 = 0;
        let fs_ok = GetVolumeInformationW(
            PCWSTR(wide.as_ptr()),
            Some(&mut vol_label),
            Some(&mut serial),
            Some(&mut max_comp),
            Some(&mut flags),
            Some(&mut fs_name_buf),
        )
        .is_ok();

        let fs_name = if fs_ok {
            nul_terminated(&fs_name_buf)
        } else {
            String::new()
        };
        let label = if fs_ok { nul_terminated(&vol_label) } else { String::new() };

        let used = total.saturating_sub(total_free);
        Some(VolumeInfo {
            label: if label.is_empty() {
                mount.to_string()
            } else {
                format!("{} ({})", label, fs_name)
            },
            mount: mount.to_string(),
            total_bytes: total,
            free_bytes: total_free,
            used_bytes: used,
            used_ratio: if total > 0 {
                used as f64 / total as f64
            } else {
                0.0
            },
        })
    }
}

fn mount_to_wide(mount: &str) -> Vec<u16> {
    let mut v: Vec<u16> = mount.encode_utf16().collect();
    v.push(0);
    v
}


/// Volumes worth walking end to end: local fixed disks and removable media.
///
/// The large-file scanner used to carry its own hardcoded C:/D:/E:/F: list,
/// which silently skipped any volume past F while the UI went on displaying
/// it as full. Sharing this with list_volumes keeps the two in step, and the
/// drive-type filter is what keeps optical and network drives out: a full walk
/// of either is slow, and neither is meaningfully reclaimable here.
pub fn scannable_roots() -> Vec<std::path::PathBuf> {
    list_volumes()
        .into_iter()
        .map(|v| std::path::PathBuf::from(v.mount))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_system_drive() {
        let vols = list_volumes();
        println!("found {} volumes", vols.len());
        for v in &vols {
            println!("  {} {} free={}", v.mount, v.label, v.free_bytes);
        }
        assert!(!vols.is_empty(), "expected at least one local volume");
        assert!(
            vols.iter().any(|v| v.mount.to_uppercase().starts_with("C:")),
            "expected the system drive C: among {:?}",
            vols.iter().map(|v| &v.mount).collect::<Vec<_>>()
        );
    }

    #[test]
    fn reported_capacity_is_plausible() {
        for v in list_volumes() {
            assert!(v.total_bytes > 0, "{} reported zero capacity", v.mount);
            assert!(v.free_bytes <= v.total_bytes, "{} free exceeds total", v.mount);
            assert!((0.0..=1.0).contains(&v.used_ratio), "{} ratio out of range", v.mount);
        }
    }

    #[test]
    fn drive_letter_mount_detection() {
        assert!(is_drive_letter_mount("C:\\"));
        assert!(!is_drive_letter_mount("\\\\?\\Volume{12345678-1234-1234-1234-123456789abc}\\"));
        assert!(!is_drive_letter_mount("\\\\server\\share"));
    }
}
