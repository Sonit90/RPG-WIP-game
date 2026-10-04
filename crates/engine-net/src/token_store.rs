//! Where a "remember me" session survives a restart.
//!
//! Only the refresh token is ever stored; the access token lives in memory and is re-derived
//! from it. The store is a trait so this crate needs no OS dependency: the game supplies the
//! Windows implementation (DPAPI, user scope — `engine-platform::secure_store`), and tests and
//! the headless test client use [`MemoryTokenStore`].

use std::sync::Mutex;

/// A stored session.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredSession {
    /// The refresh token.
    pub refresh_token: String,
    /// The email it belongs to, to pre-fill the login form.
    pub email: String,
}

impl std::fmt::Debug for StoredSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredSession")
            .field("refresh_token", &"<redacted>")
            .field("email", &self.email)
            .finish()
    }
}

/// Persistent storage for one session.
pub trait TokenStore: Send {
    /// The stored session, if there is one and it can be read.
    fn load(&self) -> Option<StoredSession>;
    /// Replaces the stored session. Failure is logged by the implementation, never fatal.
    fn save(&self, session: &StoredSession);
    /// Forgets the stored session.
    fn clear(&self);
}

/// A store that forgets everything when the process exits.
#[derive(Default)]
pub struct MemoryTokenStore {
    session: Mutex<Option<StoredSession>>,
}

impl TokenStore for MemoryTokenStore {
    fn load(&self) -> Option<StoredSession> {
        self.session.lock().ok().and_then(|guard| guard.clone())
    }

    fn save(&self, session: &StoredSession) {
        if let Ok(mut guard) = self.session.lock() {
            *guard = Some(session.clone());
        }
    }

    fn clear(&self) {
        if let Ok(mut guard) = self.session.lock() {
            *guard = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trips_and_clears() {
        let store = MemoryTokenStore::default();
        assert!(store.load().is_none());
        let session = StoredSession {
            refresh_token: "secret".into(),
            email: "a@b.c".into(),
        };
        store.save(&session);
        assert_eq!(store.load(), Some(session.clone()));
        assert!(!format!("{session:?}").contains("secret"));
        store.clear();
        assert!(store.load().is_none());
    }
}
