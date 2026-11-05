//! Secrets at rest on macOS, and the clipboard.
//!
//! [`protect`] and [`unprotect`] use the login Keychain: the secret itself never touches the
//! disk file the caller writes. The caller still stores *something* next to its other
//! settings (on Windows that something is the DPAPI ciphertext); here it is a short fixed
//! marker, and the real bytes live in the Keychain under the current user's login, readable
//! by this user and, subject to the Keychain's per-application access list, by nobody else.
//!
//! Each `protect` gets its own Keychain account, named in the blob it returns, so two callers
//! sealing two secrets (the online client keeps one remembered session per profile) do not
//! overwrite each other. The cost is that a replaced secret's old Keychain item is not
//! deleted — the caller never hands the old blob back — which is one small item per sign-in.

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use security_framework::passwords::{get_generic_password, set_generic_password};

/// The Keychain service the slot lives under.
const SERVICE: &str = "mygame";

/// Prefix of every blob [`protect`] returns; the Keychain account name follows it.
///
/// Lets [`unprotect`] tell a blob this module made from random bytes in a file, and a format
/// change be told apart later.
const MARKER: &[u8] = b"mygame-keychain-v2:";

/// A Keychain account name no earlier `protect` in this or another process has used.
fn fresh_account() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("session-{}-{nanos:x}-{count}", std::process::id())
}

/// Stores `plain` in the login Keychain. `None` if the Keychain refuses.
///
/// Returns a blob for the caller to keep on disk: a marker and the account name, not the
/// secret.
#[must_use]
pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
    let account = fresh_account();
    match set_generic_password(SERVICE, &account, plain) {
        Ok(()) => {
            let mut sealed = MARKER.to_vec();
            sealed.extend_from_slice(account.as_bytes());
            Some(sealed)
        }
        Err(error) => {
            log::warn!("secure store: protect failed: {error}");
            None
        }
    }
}

/// Reads back what [`protect`] stored. `None` if `sealed` is not a blob it made or the
/// Keychain has nothing under that account (or refuses).
#[must_use]
pub fn unprotect(sealed: &[u8]) -> Option<Vec<u8>> {
    let account = std::str::from_utf8(sealed.strip_prefix(MARKER)?).ok()?;
    if account.is_empty() {
        return None;
    }
    match get_generic_password(SERVICE, account) {
        Ok(plain) => Some(plain),
        Err(error) => {
            log::warn!("secure store: unprotect failed: {error}");
            None
        }
    }
}

/// The clipboard's text, if it holds any.
///
/// `NSPasteboard` is thread-safe for reads, so unlike most AppKit this needs no main-thread
/// marker.
#[must_use]
pub fn clipboard_text() -> Option<String> {
    // SAFETY: `NSPasteboardTypeString` is an AppKit constant that is valid for the life of
    // the process; the pasteboard calls are plain message sends with no borrowed pointers.
    let text = unsafe {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.stringForType(NSPasteboardTypeString)
    }?;
    Some(text.to_string())
}
