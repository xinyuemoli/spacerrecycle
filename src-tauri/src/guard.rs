//! The delete guard's user-facing half.
//!
//! A resident service answers the kernel filter's delete questions, and this is
//! the policy it acts on: what should happen when somebody tries to delete a
//! file marked 重要. The service owns the file and watches it, so a change made
//! here reaches the driver within a couple of seconds without restarting
//! anything.
//!
//! Why a file rather than a setting inside this app's own config: the policy has
//! to be readable by a service running as LocalSystem, which cannot be assumed
//! to share the interactive user's profile. `%ProgramData%` is the one place
//! both can see, and the service creates the file with a DACL that lets the
//! interactive user rewrite it - see the note on that tradeoff in
//! `driver/tools/srguard.cpp`.

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What the guard should do about a delete of a marked file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Disposition {
    /// Refuse the delete and ask the user. A yes performs the deletion.
    Ask,
    /// Refuse it outright, with nothing shown.
    Deny,
    /// Let it through. Marking still drives the icon, but nothing is blocked.
    Allow,
}

impl Disposition {
    /// The token the service writes into the file and the driver understands.
    pub fn as_token(self) -> &'static str {
        match self {
            Disposition::Ask => "ask",
            Disposition::Deny => "deny",
            Disposition::Allow => "allow",
        }
    }

    pub fn parse(token: &str) -> Option<Self> {
        match token.trim().to_ascii_lowercase().as_str() {
            "ask" => Some(Disposition::Ask),
            "deny" => Some(Disposition::Deny),
            "allow" => Some(Disposition::Allow),
            _ => None,
        }
    }

    /// The default when nothing has been configured.
    ///
    /// `Ask` is the only one that can surprise the user in a way they can undo:
    /// the other two either block a delete silently or permit it silently, and
    /// silently permitting is how a protection feature stops being one.
    pub fn default_disposition() -> Self {
        Disposition::Ask
    }
}

/// Where the service keeps its policy. Must match `srguard.cpp`.
pub fn policy_path() -> Option<PathBuf> {
    let base = std::env::var_os("ProgramData")?;
    let mut path = PathBuf::from(base);
    path.push("SpaceRecycle");
    path.push("policy.ini");
    Some(path)
}

/// Reads the configured disposition. An absent or unreadable file is not an
/// error: it is the state before anyone has chosen anything, and the default
/// applies.
pub fn read() -> Disposition {
    let Some(path) = policy_path() else {
        return Disposition::default_disposition();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Disposition::default_disposition();
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "action_marked" {
            if let Some(d) = Disposition::parse(value) {
                return d;
            }
        }
    }
    Disposition::default_disposition()
}

/// Writes the disposition, preserving the file's other keys.
///
/// Rewriting only the one line matters because the file is also where a future
/// setting will live, and a save from this screen should not silently drop it.
pub fn write(disposition: Disposition) -> io::Result<()> {
    let Some(path) = policy_path() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "ProgramData is not set",
        ));
    };
    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in existing.lines() {
        let trimmed = line.trim();
        let is_target = !trimmed.starts_with('#')
            && !trimmed.starts_with(';')
            && trimmed
                .split_once('=')
                .map(|(k, _)| k.trim() == "action_marked")
                .unwrap_or(false);
        if is_target {
            if !replaced {
                lines.push(format!("action_marked={}", disposition.as_token()));
                replaced = true;
            }
            continue;
        }
        lines.push(line.to_string());
    }
    if !replaced {
        lines.push(format!("action_marked={}", disposition.as_token()));
    }
    // The service only reads keys it knows, but keeping action_unmarked visible
    // makes the file self-describing when somebody opens it.
    if !existing.contains("action_unmarked") {
        lines.push("action_unmarked=allow".to_string());
    }
    lines.push(String::new());

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, lines.join("\r\n"))
}

/// Whether the guard is actually running.
///
/// This is not decoration. With no service attached the driver fails open by
/// design, so "protection is off" and "there was nothing to protect" look
/// identical from the outside - a marked file simply deletes. The settings
/// screen has to be able to say which one it is, or the only honest thing it
/// can show is a guess.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub service_installed: bool,
    pub service_running: bool,
    pub policy_path: String,
    pub disposition: Disposition,
    /// True when the file has never been written, so the screen can say the
    /// value shown is the default rather than something the user chose.
    pub configured: bool,
}

pub fn status() -> Status {
    let (installed, running) = service_state();
    let path = policy_path();
    let configured = path.as_ref().map(|p| p.exists()).unwrap_or(false);
    Status {
        service_installed: installed,
        service_running: running,
        policy_path: path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        disposition: read(),
        configured,
    }
}

#[cfg(windows)]
fn service_state() -> (bool, bool) {
    use windows::core::PCWSTR;
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
    };

    // The name must match SR_SERVICE_NAME in srguard.cpp.
    const SERVICE_NAME: &str = "SpaceRecycleGuard";

    let wide: Vec<u16> = SERVICE_NAME.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let Ok(scm) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) else {
            return (false, false);
        };
        let service = OpenServiceW(scm, PCWSTR(wide.as_ptr()), SERVICE_QUERY_STATUS);
        let installed = service.is_ok();
        let mut running = false;
        if let Ok(service) = service {
            let mut state = SERVICE_STATUS::default();
            if QueryServiceStatus(service, &mut state).is_ok() {
                running = state.dwCurrentState == SERVICE_RUNNING;
            }
            let _ = CloseServiceHandle(service);
        }
        let _ = CloseServiceHandle(scm);
        (installed, running)
    }
}

#[cfg(not(windows))]
fn service_state() -> (bool, bool) {
    (false, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_round_trip() {
        for d in [Disposition::Ask, Disposition::Deny, Disposition::Allow] {
            assert_eq!(Disposition::parse(d.as_token()), Some(d));
        }
    }

    #[test]
    fn tokens_are_case_and_space_tolerant() {
        assert_eq!(Disposition::parse(" DENY "), Some(Disposition::Deny));
        assert_eq!(Disposition::parse("Allow"), Some(Disposition::Allow));
    }

    #[test]
    fn an_unknown_token_is_not_a_disposition() {
        // Silently mapping this to a default would turn a typo in the settings
        // file into a change of behaviour nobody asked for.
        assert_eq!(Disposition::parse("block"), None);
        assert_eq!(Disposition::parse(""), None);
    }

    #[test]
    fn the_default_asks() {
        assert_eq!(Disposition::default_disposition(), Disposition::Ask);
    }
}
