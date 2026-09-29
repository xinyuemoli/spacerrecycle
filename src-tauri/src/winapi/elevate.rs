//! UAC elevation for the cleanup step.
//!
//! The product does not ask for administrator rights on launch. Scanning and
//! reviewing work fine as a standard user, and demanding elevation up front
//! trains people to click through the blue box without reading it. Instead the
//! cleanup step detects that it is blocked by permissions and re-runs itself
//! elevated, so the prompt only ever appears when it is actually needed.

use std::path::{Path, PathBuf};
use windows::core::PCWSTR;
use windows::Win32::Foundation::GetLastError;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{SHOW_WINDOW_CMD, SW_NORMAL, SW_SHOWNOACTIVATE};

/// True when the current process already holds administrator rights.
pub fn is_elevated() -> bool {
    // Opening a token with TOKEN_QUERY and reading TokenElevation is the
    // supported way; GetTokenInformation on a null token is not.
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut core::ffi::c_void),
            size,
            &mut size,
        )
        .is_ok();
        let _ = windows::Win32::Foundation::CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// Relaunch the app elevated, passing `args` through.
///
/// Returns the child process id, or an error string. The caller decides what
/// to do with a failure: the user declining the UAC prompt is a normal
/// outcome, not a crash.
pub fn relaunch_elevated(args: &str) -> Result<u32, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot locate the running executable: {e}"))?;

    let verb = wide("runas");
    let file = wide(&exe.to_string_lossy());
    let params = wide(args);

    unsafe {
        let result = ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(params.as_ptr()),
            None,
            SHOW_WINDOW_CMD(SW_NORMAL.0 | SW_SHOWNOACTIVATE.0),
        );
        // ShellExecuteW signals failure by returning a null HINSTANCE; the
        // reason is only available from GetLastError.
        if result.0.is_null() {
            let code = GetLastError();
            // ERROR_CANCELLED (1223) means the user clicked No. That is a
            // legitimate choice and deserves a plain-language message rather
            // than a raw Win32 code.
            return Err(if code.0 as u32 == 1223 {
                "已取消权限提升，未执行任何清理。".to_string()
            } else {
                format!("无法请求管理员权限（错误 {}）。", code.0)
            });
        }
        Ok(result.0 as u32)
    }
}

/// Serialise a cleanup request to a temp file for the elevated process.
///
/// Staging through a file is the only way to hand work to a process we do not
/// control: ShellExecuteW starts a brand-new instance that shares no memory
/// with this one.
pub fn stage_cleanup_request(
    request: &crate::engine::CleanupRequest,
) -> std::io::Result<PathBuf> {
    let json = serde_json::to_string(request)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let path = std::env::temp_dir().join(format!(
        "spacerrecycle-cleanup-{}.json",
        uuid::Uuid::new_v4()
    ));
    // Create exclusively: the UUID makes a name clash effectively impossible,
    // and refusing to overwrite means another process cannot pre-seed this
    // exact file before we write it. The real defence against same-user
    // tampering stays with the elevated run, which re-validates every path
    // through the denylist before deleting anything.
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    f.write_all(json.as_bytes())?;
    Ok(path)
}

/// Read and immediately delete a staged request.
///
/// Consume-once is deliberate: if the elevated process is killed after
/// deleting files but before finishing, a retry must not replay the deletion.
pub fn take_staged_cleanup(path: &Path) -> Option<crate::engine::CleanupRequest> {
    let text = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    serde_json::from_str(&text).ok()
}

fn wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    let mut v: Vec<u16> = Path::new(s).as_os_str().encode_wide().collect();
    v.push(0);
    v
}

/// Best-effort check for "you do not have permission" in a Win32 error text.
///
/// Callers pass the OS error code rather than a message so this stays a
/// property of the error, not of how it was formatted.
pub fn is_permission_denied(code: i32) -> bool {
    matches!(code, 5 | 32 | 33)
}

/// A human-readable explanation for a blocked path.
pub fn explain_denial(path: &PathBuf, code: i32) -> String {
    if is_permission_denied(code) {
        format!("权限不足，无法访问：{}", path.display())
    } else {
        format!("无法访问（错误 {}）：{}", code, path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_denied_and_sharing_violation_count_as_permission_problems() {
        assert!(is_permission_denied(5)); // ERROR_ACCESS_DENIED
        assert!(is_permission_denied(32)); // ERROR_SHARING_VIOLATION
        assert!(is_permission_denied(33)); // ERROR_LOCK_VIOLATION
    }

    #[test]
    fn unrelated_errors_are_not_treated_as_permission_problems() {
        assert!(!is_permission_denied(2)); // ERROR_FILE_NOT_FOUND
        assert!(!is_permission_denied(3)); // ERROR_PATH_NOT_FOUND
    }

    #[test]
    fn a_staged_request_round_trips_and_is_consumed_once() {
        use crate::model::{Category, Disposition, Finding};
        let dir = std::env::temp_dir().join(format!("sr-stage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("victim.bin");
        std::fs::write(&target, b"data").unwrap();

        let request = crate::engine::CleanupRequest {
            items: vec![crate::engine::SelectedItem {
                finding: Finding {
                    id: "abc".into(),
                    rule_id: "temp.user".into(),
                    name: "victim.bin".into(),
                    path: target.to_string_lossy().to_string(),
                    is_dir: false,
                    logical_size: 4,
                    on_disk_size: 4,
                    last_access_unix: 0,
                    created_unix: 0,
                    age_days: 0,
                    age_basis: crate::model::AgeBasis::Creation,
                    category: Category::TempJunk,
                    risk: Category::TempJunk.risk(),
                    disposition: Disposition::Purge,
                    child_count: 0,
                },
                disposition: Disposition::Purge,
            }],
            confirmed_red_purge: true,
        };

        let staged = stage_cleanup_request(&request).unwrap();
        assert!(staged.exists());

        let parsed = take_staged_cleanup(&staged).expect("staged request must be readable");
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.items[0].finding.path, target.to_string_lossy());
        assert!(parsed.confirmed_red_purge);

        // Consume-once: a second attempt must not replay the deletion.
        assert!(
            take_staged_cleanup(&staged).is_none(),
            "a staged request must not survive being read"
        );
        assert!(!staged.exists(), "the staged file must be deleted after use");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn this_test_process_is_not_elevated() {
        // cargo test runs unelevated; if this ever fails the machine is
        // running the suite as admin, which invalidates the assumption the
        // elevation path is built on.
        assert!(!is_elevated());
    }
}
