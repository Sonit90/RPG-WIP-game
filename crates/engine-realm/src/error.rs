//! What can go wrong, sorted by who should hear about it.
//!
//! Every refusal the realm makes is a [`RealmError`]: a category the client can branch on and
//! a sentence a player can read. The module returns it as its reducer error string
//! ([`RealmError::to_wire`]); the client parses it back ([`RealmError::from_wire`]) so a
//! refusal arrives as "You are not the party leader." rather than "reducer failed".

use std::fmt;

/// The kind of failure, which decides how the client reacts to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    /// Credentials or a session were rejected.
    Authentication,
    /// The connection to a server failed or was lost.
    Connection,
    /// The caller is not allowed to do this.
    Authorization,
    /// The queue refused the request.
    Matchmaking,
    /// A party operation was refused.
    Party,
    /// A dungeon instance refused the request.
    Dungeon,
    /// Persistent data could not be read or written.
    Persistence,
    /// The two sides disagreed about the shape of a message.
    Protocol,
    /// Too many requests in too short a time.
    RateLimited,
}

impl ErrorCategory {
    /// Every category.
    pub const ALL: [Self; 9] = [
        Self::Authentication,
        Self::Connection,
        Self::Authorization,
        Self::Matchmaking,
        Self::Party,
        Self::Dungeon,
        Self::Persistence,
        Self::Protocol,
        Self::RateLimited,
    ];

    /// The stable tag the wire form carries.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Connection => "connection",
            Self::Authorization => "authorization",
            Self::Matchmaking => "matchmaking",
            Self::Party => "party",
            Self::Dungeon => "dungeon",
            Self::Persistence => "persistence",
            Self::Protocol => "protocol",
            Self::RateLimited => "rate_limited",
        }
    }

    /// The category with this tag.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|category| category.tag() == tag)
    }
}

/// A refusal: what kind, and what to tell the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmError {
    /// What kind of failure it was.
    pub category: ErrorCategory,
    /// A complete sentence a player can read. Never contains a credential or an internal id.
    pub message: String,
}

impl RealmError {
    /// A refusal of `category` saying `message`.
    #[must_use]
    pub fn new(category: ErrorCategory, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
        }
    }

    /// The caller may not do this.
    #[must_use]
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Authorization, message)
    }

    /// The queue refused.
    #[must_use]
    pub fn matchmaking(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Matchmaking, message)
    }

    /// A party operation was refused.
    #[must_use]
    pub fn party(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Party, message)
    }

    /// A dungeon refused.
    #[must_use]
    pub fn dungeon(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Dungeon, message)
    }

    /// Persistent data failed.
    #[must_use]
    pub fn persistence(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Persistence, message)
    }

    /// The message was malformed.
    #[must_use]
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(ErrorCategory::Protocol, message)
    }

    /// Too many requests.
    #[must_use]
    pub fn rate_limited() -> Self {
        Self::new(
            ErrorCategory::RateLimited,
            "You are doing that too quickly. Wait a moment and try again.",
        )
    }

    /// The form a reducer returns as its error: `tag: message`.
    #[must_use]
    pub fn to_wire(&self) -> String {
        format!("{}: {}", self.category.tag(), self.message)
    }

    /// Reads a reducer's error back. Anything without a known tag is a protocol error carrying
    /// the whole text, so an unexpected server message is still shown rather than dropped.
    #[must_use]
    pub fn from_wire(text: &str) -> Self {
        if let Some((tag, message)) = text.split_once(": ")
            && let Some(category) = ErrorCategory::from_tag(tag)
        {
            return Self::new(category, message);
        }
        Self::protocol(text)
    }
}

impl fmt::Display for RealmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RealmError {}

impl From<RealmError> for String {
    fn from(error: RealmError) -> Self {
        error.to_wire()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_form_round_trips() {
        for category in ErrorCategory::ALL {
            let error = RealmError::new(category, "Something: with a colon.");
            assert_eq!(RealmError::from_wire(&error.to_wire()), error);
        }
    }

    #[test]
    fn unknown_text_is_a_protocol_error() {
        let error = RealmError::from_wire("the module panicked");
        assert_eq!(error.category, ErrorCategory::Protocol);
        assert_eq!(error.message, "the module panicked");
    }
}
