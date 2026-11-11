//! A line of text an author types into: a number in the inspector, a file name for Save As.
//!
//! ```text
//!  click a value ──► TextField ──► ENTER commits   ESC cancels   TAB commits and moves on
//! ```
//!
//! # Why not the browser's filter
//!
//! The filter types letters into a search and never has to be *read back* as anything. A
//! number has to parse, and a file name must not contain a character a path cannot hold, so a
//! field knows what it accepts and refuses the rest at the key rather than at the commit.

use engine_core::input::{InputState, KeyCode};

/// What a field accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    /// Digits, one leading minus and one decimal point.
    Number,
    /// Lower-case letters, digits, `-` and `_` (`SHIFT` with `-`).
    FileName,
    /// Prose: letters (`SHIFT` for capitals), digits, space, `-` and `.`, up to `max` characters.
    ByKey {
        /// The most characters the field holds.
        max: usize,
    },
}

/// What a frame of typing came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldAnswer {
    /// Still typing (or nothing pressed).
    Editing,
    /// `ENTER`: the text is what the author meant.
    Commit,
    /// `TAB`: commit, and go to the next field.
    CommitNext,
    /// `ESC`: forget it.
    Cancel,
}

/// A line of text being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextField {
    text: String,
    charset: Charset,
    /// Whether the text is still the value the field was opened with, untouched: the first
    /// key typed replaces it rather than appending to it, which is what every numeric field
    /// in every editor does when it is clicked.
    pristine: bool,
}

impl TextField {
    /// A field holding `initial`, selected so the first key replaces it.
    #[must_use]
    pub fn new(initial: &str, charset: Charset) -> Self {
        Self {
            text: initial.to_owned(),
            charset,
            pristine: true,
        }
    }

    /// A field holding `initial` that the first key adds to rather than replaces: a filter
    /// being refined.
    #[must_use]
    pub fn appending(initial: &str, charset: Charset) -> Self {
        Self {
            pristine: false,
            ..Self::new(initial, charset)
        }
    }

    /// What has been typed.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the first key will replace the text rather than add to it.
    #[must_use]
    pub fn is_pristine(&self) -> bool {
        self.pristine
    }

    /// The text as a number, if it is one.
    #[must_use]
    pub fn number(&self) -> Option<f32> {
        self.text
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
    }

    /// One frame of keys.
    pub fn update(&mut self, input: &InputState) -> FieldAnswer {
        if input.was_key_pressed(KeyCode::Escape) {
            return FieldAnswer::Cancel;
        }
        if input.was_key_pressed(KeyCode::Enter) {
            return FieldAnswer::Commit;
        }
        if input.was_key_pressed(KeyCode::Tab) {
            return FieldAnswer::CommitNext;
        }
        if input.was_key_pressed(KeyCode::Backspace) {
            if self.pristine {
                self.text.clear();
            } else {
                self.text.pop();
            }
            self.pristine = false;
            return FieldAnswer::Editing;
        }
        let shift = input.is_key_down(KeyCode::LeftShift) || input.is_key_down(KeyCode::RightShift);
        for key in KeyCode::ALL {
            if !input.was_key_pressed(key) {
                continue;
            }
            let Some(character) = self.character(key, shift) else {
                continue;
            };
            if self.pristine {
                self.text.clear();
                self.pristine = false;
            }
            if self.accepts(character) {
                self.text.push(character);
            }
        }
        FieldAnswer::Editing
    }

    /// The character a key types in this field, if it types one.
    fn character(&self, key: KeyCode, shift: bool) -> Option<char> {
        if let Some(digit) = key.digit_number() {
            return Some(char::from(b'0' + digit));
        }
        match (self.charset, key) {
            (_, KeyCode::Minus) if shift && self.charset == Charset::FileName => Some('_'),
            (_, KeyCode::Minus) => Some('-'),
            (Charset::Number, KeyCode::Period) => Some('.'),
            (Charset::FileName, key) if (KeyCode::A..=KeyCode::Z).contains(&key) => {
                Some(char::from(b'a' + (key.index() - KeyCode::A.index()) as u8))
            }
            (Charset::ByKey { .. }, key) if (KeyCode::A..=KeyCode::Z).contains(&key) => {
                let letter = char::from(b'a' + (key.index() - KeyCode::A.index()) as u8);
                Some(if shift {
                    letter.to_ascii_uppercase()
                } else {
                    letter
                })
            }
            (Charset::ByKey { .. }, KeyCode::Space) => Some(' '),
            (Charset::ByKey { .. }, KeyCode::Period) => Some('.'),
            _ => None,
        }
    }

    /// Whether `character` may go at the end of the text as it stands.
    fn accepts(&self, character: char) -> bool {
        match self.charset {
            Charset::FileName => self.text.len() < 48,
            Charset::ByKey { max } => self.text.chars().count() < max,
            Charset::Number => match character {
                '-' => self.text.is_empty(),
                '.' => !self.text.contains('.'),
                _ => self.text.len() < 12,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_core::input::InputEvent;

    fn press(field: &mut TextField, keys: &[KeyCode]) -> FieldAnswer {
        let mut answer = FieldAnswer::Editing;
        for key in keys {
            let mut input = InputState::new();
            input.begin_frame();
            input.apply(InputEvent::KeyDown {
                key: *key,
                repeat: false,
            });
            answer = field.update(&input);
        }
        answer
    }

    #[test]
    fn the_first_key_replaces_the_value_the_field_opened_with() {
        let mut field = TextField::new("12.5", Charset::Number);
        press(&mut field, &[KeyCode::Digit3]);
        assert_eq!(field.text(), "3");
        press(&mut field, &[KeyCode::Period, KeyCode::Digit2]);
        assert_eq!(field.number(), Some(3.2));
    }

    #[test]
    fn a_number_takes_one_minus_at_the_front_and_one_point() {
        let mut field = TextField::new("", Charset::Number);
        press(
            &mut field,
            &[
                KeyCode::Minus,
                KeyCode::Digit4,
                KeyCode::Minus,
                KeyCode::Period,
                KeyCode::Digit5,
                KeyCode::Period,
                KeyCode::A,
            ],
        );
        assert_eq!(field.text(), "-4.5");
        assert_eq!(field.number(), Some(-4.5));
    }

    #[test]
    fn enter_commits_tab_moves_on_and_escape_cancels() {
        let mut field = TextField::new("1", Charset::Number);
        assert_eq!(press(&mut field, &[KeyCode::Enter]), FieldAnswer::Commit);
        assert_eq!(press(&mut field, &[KeyCode::Tab]), FieldAnswer::CommitNext);
        assert_eq!(press(&mut field, &[KeyCode::Escape]), FieldAnswer::Cancel);
    }

    #[test]
    fn a_file_name_types_letters_digits_and_dashes_only() {
        let mut field = TextField::new("", Charset::FileName);
        press(
            &mut field,
            &[
                KeyCode::C,
                KeyCode::A,
                KeyCode::V,
                KeyCode::E,
                KeyCode::Minus,
                KeyCode::Digit2,
            ],
        );
        assert_eq!(field.text(), "cave-2");
        press(&mut field, &[KeyCode::Backspace, KeyCode::Period]);
        assert_eq!(
            field.text(),
            "cave-",
            "a point is not part of a level's name"
        );
    }

    #[test]
    fn prose_takes_capitals_with_shift_spaces_and_stops_at_its_limit() {
        let mut field = TextField::new("", Charset::ByKey { max: 5 });
        let mut shifted = InputState::new();
        shifted.begin_frame();
        for key in [KeyCode::LeftShift, KeyCode::C] {
            shifted.apply(InputEvent::KeyDown { key, repeat: false });
        }
        field.update(&shifted);
        press(
            &mut field,
            &[KeyCode::R, KeyCode::Space, KeyCode::Period, KeyCode::Digit9],
        );
        assert_eq!(field.text(), "Cr .9");
        press(&mut field, &[KeyCode::A]);
        assert_eq!(field.text(), "Cr .9", "the limit holds");
    }
}
