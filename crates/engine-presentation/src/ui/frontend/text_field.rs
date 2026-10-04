//! A single-line text field: what the login screen types into.
//!
//! Fed the frame's typed characters (`InputState::typed`, which carries the OS's own layout,
//! shift state, dead keys and auto-repeat) plus the few navigation keys that produce no
//! character. A password field stores the text and draws bullets; nothing here ever logs it.

/// One editable line.
#[derive(Clone, Default)]
pub struct TextField {
    text: String,
    /// Caret position, in characters.
    caret: usize,
    /// Draw bullets instead of the text.
    pub masked: bool,
    /// The most characters it accepts.
    pub max_chars: usize,
}

impl std::fmt::Debug for TextField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextField")
            .field("text", &if self.masked { "<masked>" } else { self.text.as_str() })
            .field("caret", &self.caret)
            .finish()
    }
}

/// What a frame of typing did besides editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldOutcome {
    /// Enter was pressed.
    pub submitted: bool,
    /// Tab was pressed (move to the next field).
    pub tabbed: bool,
    /// Ctrl+V was pressed: the caller should read the clipboard and [`TextField::paste`].
    pub paste_requested: bool,
}

/// Navigation keys that type nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldKeys {
    /// Caret one left.
    pub left: bool,
    /// Caret one right.
    pub right: bool,
    /// Caret to the start.
    pub home: bool,
    /// Caret to the end.
    pub end: bool,
    /// Delete the character after the caret.
    pub delete: bool,
}

impl TextField {
    /// An empty field accepting `max_chars`.
    #[must_use]
    pub fn new(max_chars: usize) -> Self {
        Self {
            max_chars,
            ..Self::default()
        }
    }

    /// A password field.
    #[must_use]
    pub fn password(max_chars: usize) -> Self {
        Self {
            masked: true,
            ..Self::new(max_chars)
        }
    }

    /// The text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces the text and puts the caret at its end.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.chars().filter(|c| !c.is_control()).take(self.max_chars).collect();
        self.caret = self.text.chars().count();
    }

    /// Empties the field. Used on a password after every attempt.
    pub fn clear(&mut self) {
        self.text.clear();
        self.caret = 0;
    }

    /// The caret, in characters.
    #[must_use]
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// What to draw: the text, or one bullet per character.
    #[must_use]
    pub fn display(&self) -> String {
        if self.masked {
            "*".repeat(self.text.chars().count())
        } else {
            self.text.clone()
        }
    }

    /// The displayed text before the caret, for placing the caret.
    #[must_use]
    pub fn display_before_caret(&self) -> String {
        if self.masked {
            "*".repeat(self.caret)
        } else {
            self.text.chars().take(self.caret).collect()
        }
    }

    fn byte_at(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(index, _)| index)
    }

    fn insert(&mut self, character: char) {
        if self.text.chars().count() >= self.max_chars {
            return;
        }
        let at = self.byte_at(self.caret);
        self.text.insert(at, character);
        self.caret += 1;
    }

    /// Inserts pasted text at the caret: control characters and line breaks are dropped.
    pub fn paste(&mut self, text: &str) {
        for character in text.chars().filter(|c| !c.is_control()) {
            self.insert(character);
        }
    }

    /// Applies one frame of typing.
    pub fn apply(&mut self, typed: &[char], keys: FieldKeys) -> FieldOutcome {
        let mut outcome = FieldOutcome::default();
        for &character in typed {
            match character {
                '\u{8}' => {
                    if self.caret > 0 {
                        let at = self.byte_at(self.caret - 1);
                        self.text.remove(at);
                        self.caret -= 1;
                    }
                }
                '\r' | '\n' => outcome.submitted = true,
                '\t' => outcome.tabbed = true,
                '\u{16}' => outcome.paste_requested = true,
                c if c.is_control() => {}
                c => self.insert(c),
            }
        }
        let len = self.text.chars().count();
        if keys.left {
            self.caret = self.caret.saturating_sub(1);
        }
        if keys.right {
            self.caret = (self.caret + 1).min(len);
        }
        if keys.home {
            self.caret = 0;
        }
        if keys.end {
            self.caret = len;
        }
        if keys.delete && self.caret < len {
            let at = self.byte_at(self.caret);
            self.text.remove(at);
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn types_edits_and_moves() {
        let mut field = TextField::new(64);
        field.apply(&typed("hello"), FieldKeys::default());
        assert_eq!(field.text(), "hello");
        field.apply(&['\u{8}'], FieldKeys::default());
        assert_eq!(field.text(), "hell");
        field.apply(&[], FieldKeys { home: true, ..FieldKeys::default() });
        field.apply(&typed("s"), FieldKeys::default());
        assert_eq!(field.text(), "shell");
        field.apply(&[], FieldKeys { delete: true, ..FieldKeys::default() });
        assert_eq!(field.text(), "sell");
        field.apply(&['\u{8}', '\u{8}'], FieldKeys::default());
        assert_eq!(field.text(), "ell");
    }

    #[test]
    fn handles_non_ascii_and_limits() {
        let mut field = TextField::new(4);
        field.apply(&typed("äöüßx"), FieldKeys::default());
        assert_eq!(field.text(), "äöüß");
        field.apply(&[], FieldKeys { left: true, ..FieldKeys::default() });
        field.apply(&['\u{8}'], FieldKeys::default());
        assert_eq!(field.text(), "äöß");
    }

    #[test]
    fn controls_become_outcomes() {
        let mut field = TextField::new(8);
        let outcome = field.apply(&['a', '\t', '\r', '\u{16}', '\u{1b}'], FieldKeys::default());
        assert_eq!(field.text(), "a");
        assert!(outcome.submitted && outcome.tabbed && outcome.paste_requested);
        field.paste("b\nc\td");
        assert_eq!(field.text(), "abcd");
    }

    #[test]
    fn passwords_never_show_or_print() {
        let mut field = TextField::password(32);
        field.set_text("hunter2");
        assert_eq!(field.display(), "*******");
        assert!(!format!("{field:?}").contains("hunter2"));
    }
}
