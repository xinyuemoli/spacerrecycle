//! Per-user secret protection via DPAPI.
//!
//! A OneDrive refresh token is a bearer credential for the user's files. Left
//! in plaintext under `%APPDATA%` it is readable by any process running as that
//! user - including a backup tool, a sync client, or anything they downloaded.
//! `CryptProtectData` ties the ciphertext to the user account instead, so the
//! file alone is useless on another machine or to another user.
//!
//! This is not a vault and is not claimed to be one: a process already running
//! as the same user can call `CryptUnprotectData` too. It removes the trivial
//! copy-the-file attack, which is the realistic threat for stored tokens, and
//! that is all it is asked to do.

use windows::Win32::Foundation::LocalFree;
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
};

/// Extra entropy mixed into the key derivation.
///
/// Not a secret - it ships in the binary - but it means a blob produced by some
/// *other* app's `CryptProtectData` call cannot be dropped into our file and
/// decrypted as if it were ours.
const ENTROPY: &[u8] = b"SpaceRecycle/OneDriveBackup/v1";

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

fn entropy_blob() -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: ENTROPY.len() as u32,
        pbData: ENTROPY.as_ptr() as *mut u8,
    }
}

/// Encrypt for the current user. Returns `None` on any failure.
pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
    let input = blob(plain);
    let entropy = entropy_blob();
    let mut out = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptProtectData(
            &input,
            windows::core::PCWSTR::null(),
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .ok()?;

        // Copy before freeing: `out.pbData` is owned by the system allocator.
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(out.pbData as *mut _)));
        Some(bytes)
    }
}

/// Decrypt a blob produced by [`protect`] for this user.
///
/// Returns `None` when the blob was produced for a different user or machine,
/// which is the normal outcome after a profile move and must degrade to
/// "please sign in again" rather than a crash.
pub fn unprotect(cipher: &[u8]) -> Option<Vec<u8>> {
    if cipher.is_empty() {
        return None;
    }
    let input = blob(cipher);
    let entropy = entropy_blob();
    let mut out = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .ok()?;

        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(out.pbData as *mut _)));
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blob_round_trips_for_the_current_user() {
        let secret = b"refresh-token-value";
        let sealed = protect(secret).expect("protect failed");
        assert_ne!(&sealed[..], &secret[..], "the ciphertext equals the plaintext");
        let opened = unprotect(&sealed).expect("unprotect failed");
        assert_eq!(opened, secret);
    }

    #[test]
    fn the_ciphertext_is_not_the_plaintext_in_disguise() {
        let secret = b"eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9";
        let sealed = protect(secret).unwrap();
        assert!(
            !sealed.windows(secret.len()).any(|w| w == &secret[..]),
            "the plaintext is present verbatim inside the blob"
        );
    }

    #[test]
    fn a_tampered_blob_fails_to_open() {
        let mut sealed = protect(b"another-secret").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF;
        // Either the call fails or the integrity check does; both must not
        // yield the original plaintext.
        match unprotect(&sealed) {
            None => {}
            Some(v) => assert_ne!(v, b"another-secret"),
        }
    }

    #[test]
    fn an_empty_blob_is_rejected_rather_than_guessed_at() {
        assert!(unprotect(&[]).is_none());
    }

    /// Entropy is mixed in, so a blob sealed by a different entropy value -
    /// which is what another application's `CryptProtectData` output amounts
    /// to - must not open here.
    #[test]
    fn foreign_entropy_does_not_open() {
        let secret = b"not-ours";
        // Seal with no entropy at all, the way a different app would.
        let input = blob(secret);
        let mut out = CRYPT_INTEGER_BLOB::default();
        let sealed = unsafe {
            CryptProtectData(
                &input,
                windows::core::PCWSTR::null(),
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
            .expect("protect failed");
            let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            let _ = LocalFree(Some(windows::Win32::Foundation::HLOCAL(out.pbData as *mut _)));
            bytes
        };
        match unprotect(&sealed) {
            None => {}
            Some(v) => assert_ne!(v, secret, "a foreign blob decrypted as ours"),
        }
    }
}