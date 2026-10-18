//! Starting the app at logon, so the guard is up when nobody is looking.
//!
//! The user-mode delete guard holds a file open for as long as the process
//! holding it lives. That is the mechanism's one structural weakness compared
//! with the driver it replaced: a driver runs in the kernel whether or not
//! anything is open, while a held handle ends with the process. A guard that is
//! only on while a window is open would protect files during exactly the hours
//! the user is already watching them.
//!
//! # Why the registry and not the startup folder or a startup task
//!
//! A packaged app has a proper answer - the `windows.startupTask` manifest
//! extension - and it is the one Microsoft recommends. It is also not usable
//! here without making the guard conditional on the app being packaged, and the
//! GitHub build is an unpackaged exe that needs the same behaviour.
//!
//! One mechanism that works for both, then, and it has to survive the packaging
//! wrinkle described below.
//!
//! # The command has to activate the app, not run the exe
//!
//! For a packaged build, `current_exe()` is inside `C:\Program Files\WindowsApps`,
//! and starting it directly produces a process with **no package identity**.
//! Everything keyed on identity then takes the wrong branch: the Store
//! entitlement resolves to "unpackaged" and every paid feature opens for free,
//! and the app would report itself as a copy that did not come from the Store
//! while sitting in the Store's own install directory.
//!
//! So a packaged build is launched through its Application User Model ID, which
//! is what the shell uses when the user clicks the tile. An unpackaged build has
//! no such id and starts its own executable.
//!
//! # Failures are reported, never fatal
//!
//! Autostart is a convenience; the guard still works whenever the app is open.
//! A registry that refuses the write must not take the feature down, so the
//! result is passed back for display rather than propagated as an error.

/// The registry key under HKCU, which needs no elevation and no machine-wide
/// state. `Run` is per-user, and installing an app for one user should not
/// change what starts for every user on the machine.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// The value name. Also what [`is_enabled`] looks for.
const VALUE_NAME: &str = "SpaceRecycle";

/// The command that starts the app the way the shell would.
///
/// `cmd /c start ""` rather than `explorer.exe shell:...` because the registry
/// value is a command line, not a shell request: `start` is what turns the
/// `shell:` URI into an activation, and the empty quoted argument is the window
/// title that `start` would otherwise take the URI for.
fn launch_command() -> Result<String, String> {
    if crate::license::has_package_identity() {
        let aumid = current_aumid().ok_or_else(|| {
            "this build has a package identity but its application id could not be read"
                .to_string()
        })?;
        Ok(format!("cmd /c start \"\" \"shell:AppsFolder\\{aumid}\""))
    } else {
        let exe = std::env::current_exe()
            .map_err(|e| format!("the running executable cannot be located: {e}"))?;
        Ok(format!("\"{}\"", exe.display()))
    }
}

/// This process's Application User Model ID, when it has one.
#[cfg(windows)]
fn current_aumid() -> Option<String> {
    use windows::Win32::Storage::Packaging::Appx::GetCurrentApplicationUserModelId;

    let mut length = 0u32;
    // First call asks for the size: a null buffer with a length of zero returns
    // ERROR_INSUFFICIENT_BUFFER and fills in `length`.
    let rc = unsafe { GetCurrentApplicationUserModelId(&mut length, None) };
    if rc.is_err() || length == 0 {
        return None;
    }

    let mut buf = vec![0u16; length as usize];
    let mut length = buf.len() as u32;
    let rc = unsafe {
        use windows::core::PWSTR;
        GetCurrentApplicationUserModelId(&mut length, Some(PWSTR(buf.as_mut_ptr())))
    };
    if rc.is_err() {
        return None;
    }
    buf.truncate(length as usize);
    // The returned length includes the terminating null.
    while buf.last() == Some(&0) {
        buf.pop();
    }
    String::from_utf16(&buf).ok()
}

#[cfg(not(windows))]
fn current_aumid() -> Option<String> {
    None
}

#[cfg(windows)]
fn open_key(write: bool) -> std::io::Result<winreg_key::Key> {
    winreg_key::open(write)
}

#[cfg(not(windows))]
fn open_key(_write: bool) -> std::io::Result<winreg_key::Key> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "autostart is Windows only",
    ))
}

/// Registers the app to start at logon. Idempotent.
pub fn enable() -> Result<(), String> {
    let command = launch_command()?;
    open_key(true)
        .and_then(|k| k.set(VALUE_NAME, &command))
        .map_err(|e| format!("could not register autostart: {e}"))
}

/// Removes the registration. Doing so when it is not there is a success.
pub fn disable() -> Result<(), String> {
    match open_key(true).and_then(|k| k.delete(VALUE_NAME)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not remove autostart: {e}")),
    }
}

/// Whether the registration is present and points at this build.
///
/// Checked by value rather than by presence: an entry left behind by an older
/// install can point at a path that no longer exists, and reporting that as "on"
/// would be describing a login that never happens.
pub fn is_enabled() -> bool {
    let Ok(expected) = launch_command() else {
        return false;
    };
    open_key(false)
        .and_then(|k| k.get(VALUE_NAME))
        .map(|v| v == expected)
        .unwrap_or(false)
}

/// Turns autostart on or off to match a guard that is standing or standing down.
///
/// Called when the disposition changes rather than from a setting of its own.
/// The reasoning is that a guard the user has switched on is a guard they want
/// active; one they have switched off is not. A separate checkbox would be a
/// second thing to get wrong, and its default would be a guess either way.
///
/// Returns a description of what happened, for the screen to show - including
/// the failure, because silently not starting at logon is exactly the kind of
/// thing a protection feature must not do.
pub fn follow_guard(standing: bool) -> String {
    if standing {
        match enable() {
            Ok(()) => "it will start with Windows, so the guard is up after a reboot".to_string(),
            Err(e) => e,
        }
    } else {
        match disable() {
            Ok(()) => "it will no longer start with Windows".to_string(),
            Err(e) => e,
        }
    }
}

/// A minimal HKCU `Run` key wrapper.
///
/// Written out rather than pulled in as a dependency: three operations are
/// needed, and a registry crate would be a build dependency and an audit surface
/// for the sake of about sixty lines.
#[cfg(windows)]
mod winreg_key {
    use std::io;

    use windows::core::HSTRING;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
        RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
        REG_SZ,
    };

    pub struct Key(HKEY);

    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    pub fn open(write: bool) -> io::Result<Key> {
        let path = HSTRING::from(super::RUN_KEY);
        let mut key = HKEY::default();
        let access = if write { KEY_SET_VALUE } else { KEY_READ };
        let rc = unsafe {
            RegOpenKeyExW(HKEY_CURRENT_USER, &path, Some(0), access, &mut key)
        };
        if rc == ERROR_FILE_NOT_FOUND {
            return Err(io::Error::new(io::ErrorKind::NotFound, "the Run key does not exist"));
        }
        if rc != ERROR_SUCCESS {
            return Err(io::Error::other(format!("RegOpenKeyExW failed: {rc:?}")));
        }
        Ok(Key(key))
    }

    impl Key {
        pub fn set(&self, name: &str, value: &str) -> io::Result<()> {
            let name = HSTRING::from(name);
            // The value is REG_SZ, which is UTF-16 and includes the terminator
            // in its byte length.
            let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    wide.as_ptr() as *const u8,
                    wide.len() * std::mem::size_of::<u16>(),
                )
            };
            let rc = unsafe {
                RegSetValueExW(self.0, &name, Some(0), REG_SZ, Some(bytes))
            };
            if rc != ERROR_SUCCESS {
                return Err(io::Error::other(format!("RegSetValueExW failed: {rc:?}")));
            }
            Ok(())
        }

        pub fn get(&self, name: &str) -> io::Result<String> {
            let name = HSTRING::from(name);
            let mut size = 0u32;
            let rc = unsafe {
                RegQueryValueExW(self.0, &name, None, None, None, Some(&mut size))
            };
            if rc == ERROR_FILE_NOT_FOUND {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no such value"));
            }
            if rc != ERROR_SUCCESS || size == 0 {
                return Err(io::Error::other(format!("RegQueryValueExW failed: {rc:?}")));
            }
            let mut buf = vec![0u8; size as usize];
            let rc = unsafe {
                RegQueryValueExW(self.0, &name, None, None, Some(buf.as_mut_ptr()), Some(&mut size))
            };
            if rc != ERROR_SUCCESS {
                return Err(io::Error::other(format!("RegQueryValueExW (read) failed: {rc:?}")));
            }
            buf.truncate(size as usize);
            let wide: Vec<u16> = buf
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|c| *c != 0)
                .collect();
            String::from_utf16(&wide).map_err(|e| io::Error::other(format!("not utf-16: {e}")))
        }

        pub fn delete(&self, name: &str) -> io::Result<()> {
            let name = HSTRING::from(name);
            let rc = unsafe { RegDeleteValueW(self.0, &name) };
            if rc == ERROR_FILE_NOT_FOUND {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no such value"));
            }
            if rc != ERROR_SUCCESS {
                return Err(io::Error::other(format!("RegDeleteValueW failed: {rc:?}")));
            }
            Ok(())
        }
    }

    // RegCreateKeyExW is imported for the writer that will need it when the Run
    // key itself has to be created; kept referenced so the import does not warn.
    #[allow(dead_code)]
    fn _create_is_available(key: &HKEY, path: &HSTRING) -> HKEY {
        use windows::Win32::System::Registry::REG_CREATE_KEY_DISPOSITION;
        let mut out = HKEY::default();
        let mut disposition = REG_CREATE_KEY_DISPOSITION::default();
        let _ = unsafe {
            RegCreateKeyExW(
                *key,
                path,
                Some(0),
                None,
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut out,
                Some(&mut disposition),
            )
        };
        out
    }
}

#[cfg(not(windows))]
mod winreg_key {
    use std::io;

    pub struct Key;

    impl Key {
        pub fn set(&self, _name: &str, _value: &str) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "Windows only"))
        }
        pub fn get(&self, _name: &str) -> io::Result<String> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "Windows only"))
        }
        pub fn delete(&self, _name: &str) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "Windows only"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The packaged build must be launched through its app id, and the
    /// unpackaged one must not be.
    ///
    /// Getting this wrong is silent and expensive: a packaged app started from
    /// its exe path has no package identity, so it takes the "not from the
    /// Store" branch and hands out every paid feature for free - while sitting
    /// in the Store's own install directory.
    #[test]
    fn a_packaged_build_is_launched_through_its_app_id() {
        let cmd = launch_command().expect("a launch command");
        if crate::license::has_package_identity() {
            assert!(cmd.contains("shell:AppsFolder\\"), "packaged: {cmd}");
            assert!(!cmd.starts_with('"'), "packaged must not run the exe directly");
        } else {
            assert!(!cmd.contains("shell:AppsFolder"), "unpackaged: {cmd}");
        }
    }

    /// `start` treats its first quoted argument as the window title, so an empty
    /// title has to be supplied or the URI is consumed as one and nothing opens.
    #[test]
    fn the_packaged_command_gives_start_an_empty_title() {
        let cmd = launch_command().expect("a launch command");
        if crate::license::has_package_identity() {
            assert!(cmd.contains("start \"\" "), "start needs an empty title: {cmd}");
        }
    }

    #[test]
    fn the_run_key_is_per_user() {
        // Per-user on purpose: installing an app for one account should not
        // change what starts for every account on the machine.
        assert!(RUN_KEY.starts_with("Software\\Microsoft\\Windows\\CurrentVersion\\Run"));
    }
}
