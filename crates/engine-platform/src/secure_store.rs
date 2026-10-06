//! Secrets at rest, and the clipboard.
//!
//! [`protect`] and [`unprotect`] wrap DPAPI with the *current user's* key: the bytes written
//! to disk can be read back by this Windows account on this machine and by nobody else —
//! another user, another machine, or a copy of the file sent somewhere. That is where the
//! online client keeps its refresh token when the player ticks "remember me". Nothing here
//! knows what the bytes mean.

use windows::Win32::Foundation::{HGLOBAL, HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::core::PCWSTR;

/// Copies a DPAPI output blob into an owned buffer and frees the OS allocation.
fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if blob.pbData.is_null() {
        return Vec::new();
    }
    // SAFETY: DPAPI returned `cbData` initialised bytes at `pbData`, allocated with LocalAlloc
    // and owned by us until LocalFree; we copy them before freeing and never touch the
    // pointer again.
    let bytes = unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
    // SAFETY: the pointer came from DPAPI's LocalAlloc and is freed exactly once.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(blob.pbData.cast())));
    }
    bytes
}

/// Encrypts `plain` for the current Windows user. `None` if DPAPI refuses.
#[must_use]
pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(plain.len()).ok()?,
        pbData: plain.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: `input` points at `plain`, which outlives the call; DPAPI only reads it.
    // `output` is written by the call and released by `take_blob`.
    let result = unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    match result {
        Ok(()) => Some(take_blob(output)),
        Err(error) => {
            log::warn!("secure store: protect failed: {error}");
            None
        }
    }
}

/// Decrypts what [`protect`] produced, for the same Windows user. `None` if it cannot.
#[must_use]
pub fn unprotect(sealed: &[u8]) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(sealed.len()).ok()?,
        pbData: sealed.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: as in `protect`.
    let result = unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    match result {
        Ok(()) => Some(take_blob(output)),
        Err(error) => {
            log::warn!("secure store: unprotect failed: {error}");
            None
        }
    }
}

/// The clipboard's text, if it holds any.
#[must_use]
pub fn clipboard_text() -> Option<String> {
    const CF_UNICODETEXT: u32 = 13;
    // SAFETY: opened with no owner window and closed on every path below.
    unsafe { OpenClipboard(None) }.ok()?;
    // SAFETY: the clipboard is open; the handle stays valid until it is closed.
    let text = unsafe { GetClipboardData(CF_UNICODETEXT) }
        .ok()
        .and_then(|handle| {
            let global = HGLOBAL(handle.0);
            // SAFETY: a CF_UNICODETEXT handle is a global memory object holding a NUL-terminated
            // UTF-16 string; it is locked for the read and unlocked straight after.
            unsafe {
                let pointer = GlobalLock(global).cast::<u16>();
                if pointer.is_null() {
                    return None;
                }
                let mut len = 0usize;
                while *pointer.add(len) != 0 && len < 1 << 20 {
                    len += 1;
                }
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(pointer, len));
                let _ = GlobalUnlock(global);
                Some(text)
            }
        });
    // SAFETY: closes the clipboard this function opened.
    let _ = unsafe { CloseClipboard() };
    text
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    #[test]
    fn round_trips_for_this_user() {
        let sealed = protect(b"refresh-token-123").expect("DPAPI available");
        assert_ne!(sealed.as_slice(), b"refresh-token-123".as_slice());
        assert_eq!(unprotect(&sealed).expect("same user"), b"refresh-token-123");
    }

    #[test]
    fn tampered_data_is_refused() {
        let mut sealed = protect(b"secret").expect("DPAPI available");
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF;
        assert!(unprotect(&sealed).is_none());
    }
}
