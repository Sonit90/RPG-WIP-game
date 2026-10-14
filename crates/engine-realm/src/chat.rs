//! Chat: which channels exist, what a message may say, and who may read it.
//!
//! The realm module is the only author of a chat line: the sender's identity and character
//! name come from the connection, never from the client, so there is nothing to spoof. What
//! it decides — is this text sendable, which character does a whisper reach, who sees a
//! stored line, what history is kept — is written here as plain functions, tested natively.

use crate::{ErrorCategory, Micros, RealmError, SECOND};

/// The most characters one message may carry, after cleaning.
pub const MAX_MESSAGE_CHARS: usize = 200;
/// How long a message is kept.
pub const HISTORY_WINDOW: Micros = 600 * SECOND;
/// The most Server messages kept, newest first.
pub const MAX_SERVER_HISTORY: usize = 200;

/// Where a message is heard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Everyone online.
    Server,
    /// The sender's party.
    Party,
    /// One character.
    Whisper,
}

impl Channel {
    /// The number the wire carries.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Server => 0,
            Self::Party => 1,
            Self::Whisper => 2,
        }
    }

    /// The channel a wire number names.
    #[must_use]
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Server),
            1 => Some(Self::Party),
            2 => Some(Self::Whisper),
            _ => None,
        }
    }

    /// Its name as the interface prints it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Server => "Server",
            Self::Party => "Party",
            Self::Whisper => "Whisper",
        }
    }
}

/// Cleans `text` for sending: control characters (newlines, tabs, escapes) are dropped and the
/// ends trimmed.
///
/// # Errors
///
/// A protocol error if nothing is left, or more than [`MAX_MESSAGE_CHARS`] characters are.
pub fn sanitize(text: &str) -> Result<String, RealmError> {
    let cleaned: String = text.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return Err(RealmError::protocol("Type something to say."));
    }
    if cleaned.chars().count() > MAX_MESSAGE_CHARS {
        return Err(RealmError::protocol(format!(
            "That message is too long (at most {MAX_MESSAGE_CHARS} characters)."
        )));
    }
    Ok(cleaned.to_owned())
}

/// Whether two character names are the same one: case does not matter, surrounding space does
/// not either.
#[must_use]
pub fn names_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    !a.is_empty() && a.to_lowercase() == b.to_lowercase()
}

/// Why a whisper could not be delivered. Neither is an error of the protocol: the sender is
/// told in a notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperRefusal {
    /// Nobody online has that character name.
    NotOnline,
    /// The sender named their own character.
    Yourself,
}

impl WhisperRefusal {
    /// What to tell the sender, who typed `target`.
    #[must_use]
    pub fn message(self, target: &str) -> String {
        match self {
            Self::NotOnline => format!("No player named {} is online.", target.trim()),
            Self::Yourself => "You cannot whisper yourself.".to_owned(),
        }
    }
}

/// Finds the connected player a whisper to `target` reaches. `online` is every connected
/// player with a character, as `(who, character name)`; `sender` is the whisperer.
///
/// # Errors
///
/// [`WhisperRefusal::NotOnline`] if no connected character has the name,
/// [`WhisperRefusal::Yourself`] if it is the sender's own.
pub fn resolve_whisper<'a, K: PartialEq>(
    sender: &K,
    target: &str,
    online: &'a [(K, String)],
) -> Result<&'a K, WhisperRefusal> {
    let (who, _) = online
        .iter()
        .find(|(_, name)| names_match(name, target))
        .ok_or(WhisperRefusal::NotOnline)?;
    if who == sender {
        Err(WhisperRefusal::Yourself)
    } else {
        Ok(who)
    }
}

/// The name a player speaks under: their party character, else their queued one, else the
/// account's display name (a player standing in the hub with neither). `None` means the realm
/// knows no name for them at all, and they may not chat.
#[must_use]
pub fn speaker_name<'a>(
    party_character: Option<&'a str>,
    ticket_character: Option<&'a str>,
    display_name: Option<&'a str>,
) -> Option<&'a str> {
    [party_character, ticket_character, display_name]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|name| !name.is_empty())
}

/// Who a stored message was addressed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope<K> {
    /// The channel.
    pub channel: Channel,
    /// Who sent it.
    pub sender: K,
    /// A whisper's target.
    pub recipient: Option<K>,
    /// A party message's party.
    pub party: Option<u64>,
}

/// Someone reading chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader<K> {
    /// Who they are.
    pub who: K,
    /// The party they are in.
    pub party: Option<u64>,
}

/// Whether `reader` may see the message: Server is everyone's; Party is the sender's party's;
/// a whisper is its two ends'.
#[must_use]
pub fn visible_to<K: PartialEq>(message: &Envelope<K>, reader: &Reader<K>) -> bool {
    match message.channel {
        Channel::Server => true,
        Channel::Party => message.party.is_some() && message.party == reader.party,
        Channel::Whisper => {
            reader.who == message.sender || message.recipient.as_ref() == Some(&reader.who)
        }
    }
}

/// One stored message, as far as keeping history is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stored {
    /// Its id; larger is newer.
    pub id: u64,
    /// Its channel.
    pub channel: Channel,
    /// When it was sent.
    pub sent_at: Micros,
}

/// The ids to delete at `now`: everything older than [`HISTORY_WINDOW`], and Server messages
/// beyond the newest [`MAX_SERVER_HISTORY`].
#[must_use]
pub fn prune(messages: &[Stored], now: Micros) -> Vec<u64> {
    let mut doomed: Vec<u64> = messages
        .iter()
        .filter(|m| now - m.sent_at >= HISTORY_WINDOW)
        .map(|m| m.id)
        .collect();
    let mut server: Vec<u64> = messages
        .iter()
        .filter(|m| m.channel == Channel::Server && now - m.sent_at < HISTORY_WINDOW)
        .map(|m| m.id)
        .collect();
    server.sort_unstable_by(|a, b| b.cmp(a));
    doomed.extend(server.into_iter().skip(MAX_SERVER_HISTORY));
    doomed
}

/// The error for chatting without a character.
#[must_use]
pub fn no_character() -> RealmError {
    RealmError::new(
        ErrorCategory::Authorization,
        "Sign in with a character to chat.",
    )
}

/// The error for a party message from outside a party.
#[must_use]
pub fn not_in_party() -> RealmError {
    RealmError::new(ErrorCategory::Party, "You are not in a party.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_round_trip_and_unknown_numbers_are_refused() {
        for channel in [Channel::Server, Channel::Party, Channel::Whisper] {
            assert_eq!(Channel::from_u8(channel.as_u8()), Some(channel));
        }
        assert_eq!(Channel::from_u8(3), None);
        assert_eq!(Channel::from_u8(255), None);
    }

    #[test]
    fn sanitising_trims_and_strips_control_characters() {
        assert_eq!(sanitize("  hello  ").as_deref(), Ok("hello"));
        assert_eq!(
            sanitize("a\u{7}b\tc\nd\u{1b}[31m").as_deref(),
            Ok("abcd[31m")
        );
    }

    #[test]
    fn empty_and_control_only_text_is_refused() {
        assert!(sanitize("").is_err());
        assert!(sanitize("   ").is_err());
        assert!(sanitize("\n\t\u{7}").is_err());
    }

    #[test]
    fn length_is_counted_in_characters_after_cleaning() {
        let exact = "x".repeat(MAX_MESSAGE_CHARS);
        assert_eq!(sanitize(&exact).as_deref(), Ok(exact.as_str()));
        assert!(sanitize(&"x".repeat(MAX_MESSAGE_CHARS + 1)).is_err());
        // Multi-byte characters count once; control characters do not count at all.
        let accents = "é".repeat(MAX_MESSAGE_CHARS);
        assert!(sanitize(&accents).is_ok());
        let padded = format!("{}{}", "x".repeat(MAX_MESSAGE_CHARS), "\n".repeat(50));
        assert!(sanitize(&padded).is_ok());
    }

    fn online() -> Vec<(u32, String)> {
        vec![
            (1, "Aldric".into()),
            (2, "Brienne".into()),
            (3, "Cade".into()),
        ]
    }

    #[test]
    fn a_whisper_finds_its_target_ignoring_case_and_space() {
        let online = online();
        assert_eq!(resolve_whisper(&1, "brienne", &online), Ok(&2));
        assert_eq!(resolve_whisper(&1, "  CADE ", &online), Ok(&3));
    }

    #[test]
    fn a_whisper_to_nobody_or_oneself_is_refused() {
        let online = online();
        assert_eq!(
            resolve_whisper(&1, "Nobody", &online),
            Err(WhisperRefusal::NotOnline)
        );
        assert_eq!(
            resolve_whisper(&1, "", &online),
            Err(WhisperRefusal::NotOnline)
        );
        assert_eq!(
            resolve_whisper(&1, "aldric", &online),
            Err(WhisperRefusal::Yourself)
        );
        assert_eq!(
            WhisperRefusal::NotOnline.message(" Nobody "),
            "No player named Nobody is online."
        );
    }

    #[test]
    fn the_speaker_is_the_party_character_then_the_queued_one_then_the_account() {
        assert_eq!(speaker_name(Some("Aldric"), Some("Other"), Some("acct")), Some("Aldric"));
        assert_eq!(speaker_name(None, Some("Brienne"), Some("acct")), Some("Brienne"));
        assert_eq!(speaker_name(None, None, Some("Hubwalker")), Some("Hubwalker"));
        assert_eq!(speaker_name(None, None, None), None);
        assert_eq!(speaker_name(Some("  "), None, Some(" ")), None);
    }

    fn envelope(channel: Channel) -> Envelope<u32> {
        Envelope {
            channel,
            sender: 1,
            recipient: None,
            party: None,
        }
    }

    fn reader(who: u32, party: Option<u64>) -> Reader<u32> {
        Reader { who, party }
    }

    #[test]
    fn server_messages_reach_everyone() {
        let message = envelope(Channel::Server);
        for who in [1, 2, 9] {
            assert!(visible_to(&message, &reader(who, None)));
        }
    }

    #[test]
    fn party_messages_reach_only_that_party() {
        let message = Envelope {
            party: Some(7),
            ..envelope(Channel::Party)
        };
        assert!(visible_to(&message, &reader(2, Some(7))));
        assert!(!visible_to(&message, &reader(3, Some(8))));
        assert!(!visible_to(&message, &reader(4, None)));
        // A party message without a party is heard by nobody, not by every partyless player.
        let orphan = envelope(Channel::Party);
        assert!(!visible_to(&orphan, &reader(4, None)));
    }

    #[test]
    fn whispers_reach_only_their_two_ends() {
        let message = Envelope {
            recipient: Some(2),
            ..envelope(Channel::Whisper)
        };
        assert!(visible_to(&message, &reader(1, None)));
        assert!(visible_to(&message, &reader(2, None)));
        assert!(!visible_to(&message, &reader(3, None)));
        // Sharing a party with the sender is not enough.
        assert!(!visible_to(&message, &reader(3, Some(7))));
    }

    fn stored(id: u64, channel: Channel, age: Micros, now: Micros) -> Stored {
        Stored {
            id,
            channel,
            sent_at: now - age,
        }
    }

    #[test]
    fn history_drops_what_is_older_than_the_window() {
        let now = 10 * HISTORY_WINDOW;
        let messages = [
            stored(1, Channel::Server, HISTORY_WINDOW + SECOND, now),
            stored(2, Channel::Party, HISTORY_WINDOW, now),
            stored(3, Channel::Whisper, HISTORY_WINDOW - SECOND, now),
            stored(4, Channel::Server, 0, now),
        ];
        let mut doomed = prune(&messages, now);
        doomed.sort_unstable();
        assert_eq!(doomed, vec![1, 2]);
    }

    #[test]
    fn server_history_keeps_only_the_newest() {
        let now = 1_000 * SECOND;
        let total = u64::try_from(MAX_SERVER_HISTORY).unwrap_or(0) + 5;
        let mut messages: Vec<Stored> = (1..=total)
            .map(|id| stored(id, Channel::Server, SECOND, now))
            .collect();
        messages.push(stored(total + 1, Channel::Party, SECOND, now));
        let mut doomed = prune(&messages, now);
        doomed.sort_unstable();
        assert_eq!(doomed, vec![1, 2, 3, 4, 5]);
    }
}
