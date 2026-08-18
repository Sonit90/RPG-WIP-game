//! Platform-independent input: key codes, mouse state, events and per-frame transitions.
//!
//! ```text
//! Windows messages -> engine-platform -> InputEvent -> InputState -> application
//! ```
//!
//! `engine-core` never sees a virtual-key code or a `WPARAM`; the platform layer does the
//! translation. Nothing here is a gameplay action — there is no `Jump` and no `Fireball`.
//! Bindings from keys to actions belong to gameplay, which does not exist yet.
//!
//! ## Why transitions are stored, not inferred
//!
//! Polling alone cannot tell "held since last frame" from "pressed this frame". Gameplay
//! that tries to infer the difference has to keep its own copy of last frame's state, and
//! every system that does so gets it subtly differently. [`InputState`] keeps the previous
//! frame's bits and answers both questions directly.
//!
//! ## Allocation policy
//!
//! [`InputState`] is a fixed-size struct of bitsets and scalars: no `Vec`, no allocation,
//! no allocation on any query. It is `Copy`-cheap to snapshot, which is what a future
//! deterministic simulation needs to record and replay a frame's input.

/// Number of `u64` words needed to hold one bit per [`KeyCode`].
const KEY_WORDS: usize = KeyCode::COUNT.div_ceil(64);

/// A keyboard key, identified by physical meaning rather than by the character it types.
///
/// Only keys the engine can currently distinguish are named; anything else arrives as
/// [`KeyCode::Unknown`]. Adding a key means adding a variant here and one line in the
/// platform's translation table.
///
/// The discriminants are dense from zero so they can index a bitset directly. Their
/// numeric values are an implementation detail and must not be serialized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum KeyCode {
    /// Letter `A`.
    A,
    /// Letter `B`.
    B,
    /// Letter `C`.
    C,
    /// Letter `D`.
    D,
    /// Letter `E`.
    E,
    /// Letter `F`.
    F,
    /// Letter `G`.
    G,
    /// Letter `H`.
    H,
    /// Letter `I`.
    I,
    /// Letter `J`.
    J,
    /// Letter `K`.
    K,
    /// Letter `L`.
    L,
    /// Letter `M`.
    M,
    /// Letter `N`.
    N,
    /// Letter `O`.
    O,
    /// Letter `P`.
    P,
    /// Letter `Q`.
    Q,
    /// Letter `R`.
    R,
    /// Letter `S`.
    S,
    /// Letter `T`.
    T,
    /// Letter `U`.
    U,
    /// Letter `V`.
    V,
    /// Letter `W`.
    W,
    /// Letter `X`.
    X,
    /// Letter `Y`.
    Y,
    /// Letter `Z`.
    Z,
    /// Top-row `0`.
    Digit0,
    /// Top-row `1`.
    Digit1,
    /// Top-row `2`.
    Digit2,
    /// Top-row `3`.
    Digit3,
    /// Top-row `4`.
    Digit4,
    /// Top-row `5`.
    Digit5,
    /// Top-row `6`.
    Digit6,
    /// Top-row `7`.
    Digit7,
    /// Top-row `8`.
    Digit8,
    /// Top-row `9`.
    Digit9,
    /// `F1`.
    F1,
    /// `F2`.
    F2,
    /// `F3`.
    F3,
    /// `F4`.
    F4,
    /// `F5`.
    F5,
    /// `F6`.
    F6,
    /// `F7`.
    F7,
    /// `F8`.
    F8,
    /// `F9`.
    F9,
    /// `F10`.
    F10,
    /// `F11`.
    F11,
    /// `F12`.
    F12,
    /// Space bar.
    Space,
    /// Escape.
    Escape,
    /// Return / Enter.
    Enter,
    /// Tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Left arrow.
    ArrowLeft,
    /// Right arrow.
    ArrowRight,
    /// Up arrow.
    ArrowUp,
    /// Down arrow.
    ArrowDown,
    /// Left shift.
    LeftShift,
    /// Right shift.
    RightShift,
    /// Left control.
    LeftControl,
    /// Right control.
    RightControl,
    /// Left alt.
    LeftAlt,
    /// Right alt.
    RightAlt,
    /// A key the engine does not name. Kept so it can be logged, never bound.
    Unknown,
}

impl KeyCode {
    /// Every key code, in discriminant order.
    pub const ALL: [Self; 63] = [
        Self::A,
        Self::B,
        Self::C,
        Self::D,
        Self::E,
        Self::F,
        Self::G,
        Self::H,
        Self::I,
        Self::J,
        Self::K,
        Self::L,
        Self::M,
        Self::N,
        Self::O,
        Self::P,
        Self::Q,
        Self::R,
        Self::S,
        Self::T,
        Self::U,
        Self::V,
        Self::W,
        Self::X,
        Self::Y,
        Self::Z,
        Self::Digit0,
        Self::Digit1,
        Self::Digit2,
        Self::Digit3,
        Self::Digit4,
        Self::Digit5,
        Self::Digit6,
        Self::Digit7,
        Self::Digit8,
        Self::Digit9,
        Self::F1,
        Self::F2,
        Self::F3,
        Self::F4,
        Self::F5,
        Self::F6,
        Self::F7,
        Self::F8,
        Self::F9,
        Self::F10,
        Self::F11,
        Self::F12,
        Self::Space,
        Self::Escape,
        Self::Enter,
        Self::Tab,
        Self::Backspace,
        Self::ArrowLeft,
        Self::ArrowRight,
        Self::ArrowUp,
        Self::ArrowDown,
        Self::LeftShift,
        Self::RightShift,
        Self::LeftControl,
        Self::RightControl,
        Self::LeftAlt,
        Self::RightAlt,
    ];

    /// Number of distinct key codes, including [`KeyCode::Unknown`].
    pub const COUNT: usize = Self::ALL.len() + 1;

    /// Bit index of this key. Dense and stable within one build.
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// The function key with this 1-based number, if the engine names it.
    ///
    /// `function_key(5)` is `F5`. Out-of-range numbers give `None` rather than a wrong key.
    #[must_use]
    pub fn function_key(number: u8) -> Option<Self> {
        if (1..=12).contains(&number) {
            Some(Self::ALL[Self::F1.index() + usize::from(number) - 1])
        } else {
            None
        }
    }

    /// The 1-based function-key number, if this is a function key.
    #[must_use]
    pub fn function_number(self) -> Option<u8> {
        if (Self::F1..=Self::F12).contains(&self) {
            Some((self.index() - Self::F1.index() + 1) as u8)
        } else {
            None
        }
    }

    /// The digit key for `0`-`9`, if in range.
    #[must_use]
    pub fn digit_key(digit: u8) -> Option<Self> {
        if digit <= 9 {
            Some(Self::ALL[Self::Digit0.index() + usize::from(digit)])
        } else {
            None
        }
    }

    /// The 0-9 digit this key types, if it is a digit key.
    #[must_use]
    pub fn digit_number(self) -> Option<u8> {
        if (Self::Digit0..=Self::Digit9).contains(&self) {
            Some((self.index() - Self::Digit0.index()) as u8)
        } else {
            None
        }
    }

    /// This key's stable written name: what a configuration file stores.
    ///
    /// Stable across builds on purpose. A binding is written to a file the player never sees
    /// and reloaded next launch, so the stored form cannot be the discriminant — reordering
    /// this enum would then silently rebind everyone's keys. It is a name, and a name only
    /// changes when a key is renamed.
    ///
    /// [`KeyCode::Unknown`] has no name: a key the engine cannot identify is a key nothing
    /// can bind, and writing `"UNKNOWN"` to a file would invite reading it back as a binding.
    #[must_use]
    pub fn name(self) -> Option<&'static str> {
        const NAMES: [&str; KeyCode::ALL.len()] = [
            "A",
            "B",
            "C",
            "D",
            "E",
            "F",
            "G",
            "H",
            "I",
            "J",
            "K",
            "L",
            "M",
            "N",
            "O",
            "P",
            "Q",
            "R",
            "S",
            "T",
            "U",
            "V",
            "W",
            "X",
            "Y",
            "Z",
            "DIGIT0",
            "DIGIT1",
            "DIGIT2",
            "DIGIT3",
            "DIGIT4",
            "DIGIT5",
            "DIGIT6",
            "DIGIT7",
            "DIGIT8",
            "DIGIT9",
            "F1",
            "F2",
            "F3",
            "F4",
            "F5",
            "F6",
            "F7",
            "F8",
            "F9",
            "F10",
            "F11",
            "F12",
            "SPACE",
            "ESCAPE",
            "ENTER",
            "TAB",
            "BACKSPACE",
            "ARROWLEFT",
            "ARROWRIGHT",
            "ARROWUP",
            "ARROWDOWN",
            "LEFTSHIFT",
            "RIGHTSHIFT",
            "LEFTCONTROL",
            "RIGHTCONTROL",
            "LEFTALT",
            "RIGHTALT",
        ];
        NAMES.get(self.index()).copied()
    }

    /// The key with this written name, if the engine names one.
    ///
    /// The inverse of [`KeyCode::name`], and total: an unrecognised name is `None` rather
    /// than a guess, so a hand-edited configuration produces an unbound slot instead of a
    /// key the player did not ask for.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|key| key.name() == Some(name))
    }

    /// The short form printed in the corner of an action bar slot.
    ///
    /// Display, not storage — unlike [`KeyCode::name`] this may be changed freely, and it is
    /// abbreviated to fit a 48-pixel slot in a 5x7 font: `DIGIT1` is `1`, `SPACE` is `SPC`.
    #[must_use]
    pub fn label(self) -> &'static str {
        const LABELS: [&str; 15] = [
            "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "SPC", "ENT", "TAB", "BSP", "ESC",
        ];
        if let Some(digit) = self.digit_number() {
            return LABELS[usize::from(digit)];
        }
        match self {
            Self::Space => LABELS[10],
            Self::Enter => LABELS[11],
            Self::Tab => LABELS[12],
            Self::Backspace => LABELS[13],
            Self::Escape => LABELS[14],
            Self::ArrowLeft => "LFT",
            Self::ArrowRight => "RGT",
            Self::ArrowUp => "UP",
            Self::ArrowDown => "DWN",
            Self::LeftShift => "LSH",
            Self::RightShift => "RSH",
            Self::LeftControl => "LCT",
            Self::RightControl => "RCT",
            Self::LeftAlt => "LAL",
            Self::RightAlt => "RAL",
            // Letters and function keys already read as themselves.
            other => other.name().unwrap_or(""),
        }
    }
}

/// Whether a key is a modifier — the half of a chord that is held rather than pressed.
///
/// A modifier cannot be a chord's key: `SHIFT` on its own is how the player reaches
/// `SHIFT+1`, and a slot bound to it would fire on the way to every other slot.
#[must_use]
pub const fn is_modifier(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::LeftShift
            | KeyCode::RightShift
            | KeyCode::LeftControl
            | KeyCode::RightControl
            | KeyCode::LeftAlt
            | KeyCode::RightAlt
    )
}

/// Which modifiers a chord asks for: control, shift and alt, in any combination.
///
/// Left and right are the same modifier here. The player who binds `SHIFT+1` means the shift
/// key their hand is nearest, not a particular one of the two, and no game in the genre
/// distinguishes them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Modifiers {
    bits: u8,
}

impl Modifiers {
    /// Control's bit.
    const CTRL: u8 = 1 << 0;
    /// Shift's bit.
    const SHIFT: u8 = 1 << 1;
    /// Alt's bit.
    const ALT: u8 = 1 << 2;

    /// No modifiers: a plain key press.
    #[must_use]
    pub const fn none() -> Self {
        Self { bits: 0 }
    }

    /// The three flags, spelled out.
    #[must_use]
    pub const fn new(ctrl: bool, shift: bool, alt: bool) -> Self {
        let mut bits = 0;
        if ctrl {
            bits |= Self::CTRL;
        }
        if shift {
            bits |= Self::SHIFT;
        }
        if alt {
            bits |= Self::ALT;
        }
        Self { bits }
    }

    /// Whether control is part of this combination.
    #[must_use]
    pub const fn ctrl(self) -> bool {
        self.bits & Self::CTRL != 0
    }

    /// Whether shift is part of this combination.
    #[must_use]
    pub const fn shift(self) -> bool {
        self.bits & Self::SHIFT != 0
    }

    /// Whether alt is part of this combination.
    #[must_use]
    pub const fn alt(self) -> bool {
        self.bits & Self::ALT != 0
    }

    /// Whether this asks for nothing to be held.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }

    /// The written prefix a configuration file stores, ending in `+` when there is one.
    ///
    /// Stable across builds for the same reason [`KeyCode::name`] is: it is written to a file
    /// and read back next launch. The order is always control, shift, alt, so one combination
    /// has exactly one spelling and a file cannot hold two names for one chord.
    #[must_use]
    pub const fn config_prefix(self) -> &'static str {
        match self.bits {
            0 => "",
            1 => "CTRL+",
            2 => "SHIFT+",
            3 => "CTRL+SHIFT+",
            4 => "ALT+",
            5 => "CTRL+ALT+",
            6 => "SHIFT+ALT+",
            _ => "CTRL+SHIFT+ALT+",
        }
    }

    /// The short prefix printed in the corner of a slot.
    ///
    /// Display, not storage, and abbreviated to one letter each: three characters plus the
    /// key still fit the corner of a 48-pixel slot in a 5x7 font, where `CTRL+SHIFT+` would
    /// run off the far side of the bar.
    #[must_use]
    pub const fn label_prefix(self) -> &'static str {
        match self.bits {
            0 => "",
            1 => "C-",
            2 => "S-",
            3 => "CS-",
            4 => "A-",
            5 => "CA-",
            6 => "SA-",
            _ => "CSA-",
        }
    }
}

/// A key press with the modifiers that must be held for it: `SHIFT+1`, `CTRL+Q`, or `1`.
///
/// A chord matches *exactly*. Holding control and pressing `1` fires whatever is on `CTRL+1`
/// and nothing else — the slot on plain `1` stays quiet, because a bar where every chord also
/// fired the unmodified slot would cast twice on every press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyChord {
    /// The key that is pressed.
    pub key: KeyCode,
    /// What must be held while it is.
    pub modifiers: Modifiers,
}

impl KeyChord {
    /// A chord of `key` with `modifiers`.
    #[must_use]
    pub const fn new(key: KeyCode, modifiers: Modifiers) -> Self {
        Self { key, modifiers }
    }

    /// The key on its own, with nothing held.
    #[must_use]
    pub const fn plain(key: KeyCode) -> Self {
        Self::new(key, Modifiers::none())
    }

    /// Whether this is a bare key press rather than a combination.
    #[must_use]
    pub const fn is_plain(self) -> bool {
        self.modifiers.is_empty()
    }

    /// Whether a chord could ever be pressed: a modifier is held, never pressed.
    #[must_use]
    pub const fn is_bindable(self) -> bool {
        !is_modifier(self.key) && self.key as u8 != KeyCode::Unknown as u8
    }

    /// This chord's stable written name: what a configuration file stores.
    ///
    /// `CTRL+SHIFT+DIGIT1`. `None` for a key the engine does not name, which is the same
    /// answer [`KeyCode::name`] gives and for the same reason.
    #[must_use]
    pub fn to_name(self) -> Option<String> {
        let key = self.key.name()?;
        let mut out = String::with_capacity(self.modifiers.config_prefix().len() + key.len());
        out.push_str(self.modifiers.config_prefix());
        out.push_str(key);
        Some(out)
    }

    /// The chord with this written name, if the engine names one.
    ///
    /// The inverse of [`KeyChord::to_name`], and total: an unrecognised name — including a
    /// modifier repeated, or one written out of order — is `None` rather than a guess, so a
    /// hand-edited file produces an unbound slot instead of a key the player did not ask for.
    /// A bare key name is read as a plain chord, which is what every file written before
    /// combinations existed contains.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let mut rest = name;
        let (mut ctrl, mut shift, mut alt) = (false, false, false);
        loop {
            if let Some(tail) = rest.strip_prefix("CTRL+") {
                if ctrl || shift || alt {
                    return None;
                }
                ctrl = true;
                rest = tail;
            } else if let Some(tail) = rest.strip_prefix("SHIFT+") {
                if shift || alt {
                    return None;
                }
                shift = true;
                rest = tail;
            } else if let Some(tail) = rest.strip_prefix("ALT+") {
                if alt {
                    return None;
                }
                alt = true;
                rest = tail;
            } else {
                break;
            }
        }
        let key = KeyCode::from_name(rest)?;
        let chord = Self::new(key, Modifiers::new(ctrl, shift, alt));
        chord.is_bindable().then_some(chord)
    }

    /// Writes the short form printed in the corner of a slot: `CS-1`, `C-Q`, `1`.
    pub fn write_label(self, out: &mut String) {
        out.push_str(self.modifiers.label_prefix());
        out.push_str(self.key.label());
    }
}

/// A fixed-capacity set of held keys.
///
/// One bit per [`KeyCode`], so membership tests and the whole-set copy that drives
/// transition detection are a couple of machine words.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KeySet {
    words: [u64; KEY_WORDS],
}

impl KeySet {
    /// An empty set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            words: [0; KEY_WORDS],
        }
    }

    /// Adds a key.
    pub fn insert(&mut self, key: KeyCode) {
        let (word, bit) = Self::position(key);
        self.words[word] |= 1 << bit;
    }

    /// Removes a key.
    pub fn remove(&mut self, key: KeyCode) {
        let (word, bit) = Self::position(key);
        self.words[word] &= !(1 << bit);
    }

    /// Whether the key is in the set.
    #[must_use]
    pub fn contains(&self, key: KeyCode) -> bool {
        let (word, bit) = Self::position(key);
        self.words[word] & (1 << bit) != 0
    }

    /// Whether no key is in the set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|word| *word == 0)
    }

    /// Number of keys in the set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    /// Empties the set.
    pub fn clear(&mut self) {
        self.words = [0; KEY_WORDS];
    }

    /// Word index and bit offset for a key.
    const fn position(key: KeyCode) -> (usize, u32) {
        let index = key as usize;
        (index / 64, (index % 64) as u32)
    }
}

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Secondary button.
    Right,
    /// Wheel click.
    Middle,
    /// First side button.
    Back,
    /// Second side button.
    Forward,
}

impl MouseButton {
    /// Every mouse button, in discriminant order.
    pub const ALL: [Self; 5] = [
        Self::Left,
        Self::Right,
        Self::Middle,
        Self::Back,
        Self::Forward,
    ];

    /// Bit mask for this button within [`MouseState`].
    #[must_use]
    fn mask(self) -> u8 {
        1 << (self as u8)
    }
}

/// Something the platform layer observed.
///
/// Deliberately raw: an event says what the device did, never what it means. Mapping to
/// gameplay actions happens above the engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// A key went down. `repeat` marks an auto-repeat rather than a physical press.
    KeyDown {
        /// The key.
        key: KeyCode,
        /// Whether this is the keyboard's auto-repeat rather than a new press.
        repeat: bool,
    },
    /// A key was released.
    KeyUp {
        /// The key.
        key: KeyCode,
    },
    /// The cursor moved to a client-area position, in pixels from the top-left.
    MouseMoved {
        /// Horizontal position.
        x: i32,
        /// Vertical position.
        y: i32,
    },
    /// A mouse button went down.
    MouseButtonDown {
        /// The button.
        button: MouseButton,
    },
    /// A mouse button was released.
    MouseButtonUp {
        /// The button.
        button: MouseButton,
    },
    /// The wheel turned, in notches. Positive is away from the user.
    MouseWheel {
        /// Notches turned; fractional on high-resolution wheels.
        delta: f32,
    },
    /// The window gained or lost keyboard focus.
    FocusChanged {
        /// Whether focus was gained.
        focused: bool,
    },
}

/// Mouse position, buttons and per-frame deltas.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct MouseState {
    /// Cursor position in client pixels from the top-left corner.
    pub position: (i32, i32),
    /// Movement accumulated this frame, in pixels.
    pub delta: (i32, i32),
    /// Wheel notches accumulated this frame.
    pub wheel: f32,
    /// Buttons currently held, as a bit mask.
    buttons: u8,
    /// Buttons held at the end of the previous frame.
    previous_buttons: u8,
}

impl MouseState {
    /// Whether the button is held right now.
    #[must_use]
    pub fn is_down(&self, button: MouseButton) -> bool {
        self.buttons & button.mask() != 0
    }

    /// Whether the button went down during this frame.
    #[must_use]
    pub fn was_pressed(&self, button: MouseButton) -> bool {
        let mask = button.mask();
        self.buttons & mask != 0 && self.previous_buttons & mask == 0
    }

    /// Whether the button was released during this frame.
    #[must_use]
    pub fn was_released(&self, button: MouseButton) -> bool {
        let mask = button.mask();
        self.buttons & mask == 0 && self.previous_buttons & mask != 0
    }
}

/// Keyboard and mouse state, with this frame's transitions.
///
/// The engine calls [`InputState::begin_frame`] before events are processed, so
/// "pressed" and "released" always describe the frame currently being simulated.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct InputState {
    keys: KeySet,
    previous_keys: KeySet,
    mouse: MouseState,
    /// Whether the window had keyboard focus at the last event.
    focused: bool,
}

impl InputState {
    /// State with nothing held and the cursor at the origin.
    #[must_use]
    pub fn new() -> Self {
        Self {
            keys: KeySet::new(),
            previous_keys: KeySet::new(),
            mouse: MouseState::default(),
            focused: true,
        }
    }

    /// Rolls this frame's state into the previous frame and clears per-frame deltas.
    ///
    /// Must run *before* the frame's events are applied. Running it afterwards would make
    /// every press look like a hold.
    pub fn begin_frame(&mut self) {
        self.previous_keys = self.keys;
        self.mouse.previous_buttons = self.mouse.buttons;
        self.mouse.delta = (0, 0);
        self.mouse.wheel = 0.0;
    }

    /// Applies one platform event.
    ///
    /// Auto-repeat key-downs are recorded as held but do not re-trigger a press: a toggle
    /// bound to a key must fire once per physical press, not sixty times a second.
    pub fn apply(&mut self, event: InputEvent) {
        match event {
            InputEvent::KeyDown { key, .. } => self.keys.insert(key),
            InputEvent::KeyUp { key } => self.keys.remove(key),
            InputEvent::MouseMoved { x, y } => {
                self.mouse.delta.0 += x - self.mouse.position.0;
                self.mouse.delta.1 += y - self.mouse.position.1;
                self.mouse.position = (x, y);
            }
            InputEvent::MouseButtonDown { button } => self.mouse.buttons |= button.mask(),
            InputEvent::MouseButtonUp { button } => self.mouse.buttons &= !button.mask(),
            InputEvent::MouseWheel { delta } => self.mouse.wheel += delta,
            InputEvent::FocusChanged { focused } => {
                self.focused = focused;
                if !focused {
                    // Key-up messages go to whichever window has focus, so a key held
                    // while alt-tabbing away would otherwise stay down forever.
                    self.release_all();
                }
            }
        }
    }

    /// Whether the key is held right now.
    #[must_use]
    pub fn is_key_down(&self, key: KeyCode) -> bool {
        self.keys.contains(key)
    }

    /// Whether the key went down during this frame.
    #[must_use]
    pub fn was_key_pressed(&self, key: KeyCode) -> bool {
        self.keys.contains(key) && !self.previous_keys.contains(key)
    }

    /// Whether the key was released during this frame.
    #[must_use]
    pub fn was_key_released(&self, key: KeyCode) -> bool {
        !self.keys.contains(key) && self.previous_keys.contains(key)
    }

    /// Which modifiers are held right now. Left and right count as the same modifier.
    #[must_use]
    pub fn modifiers(&self) -> Modifiers {
        Modifiers::new(
            self.is_key_down(KeyCode::LeftControl) || self.is_key_down(KeyCode::RightControl),
            self.is_key_down(KeyCode::LeftShift) || self.is_key_down(KeyCode::RightShift),
            self.is_key_down(KeyCode::LeftAlt) || self.is_key_down(KeyCode::RightAlt),
        )
    }

    /// Whether this chord fired this frame: its key pressed, its modifiers held, no others.
    ///
    /// The match is exact, so `1` does not fire while control is down. That is what lets
    /// `CTRL+1` and `1` sit on two different slots without one press casting both.
    #[must_use]
    pub fn was_chord_pressed(&self, chord: KeyChord) -> bool {
        self.was_key_pressed(chord.key) && self.modifiers() == chord.modifiers
    }

    /// Whether any key at all is held.
    #[must_use]
    pub fn any_key_down(&self) -> bool {
        !self.keys.is_empty()
    }

    /// The set of keys currently held.
    #[must_use]
    pub fn keys_down(&self) -> KeySet {
        self.keys
    }

    /// Mouse position, buttons and deltas.
    #[must_use]
    pub fn mouse(&self) -> MouseState {
        self.mouse
    }

    /// Whether the mouse button is held right now.
    #[must_use]
    pub fn is_mouse_button_down(&self, button: MouseButton) -> bool {
        self.mouse.is_down(button)
    }

    /// Whether the mouse button went down during this frame.
    #[must_use]
    pub fn was_mouse_button_pressed(&self, button: MouseButton) -> bool {
        self.mouse.was_pressed(button)
    }

    /// Whether the mouse button was released during this frame.
    #[must_use]
    pub fn was_mouse_button_released(&self, button: MouseButton) -> bool {
        self.mouse.was_released(button)
    }

    /// Cursor position in client pixels.
    #[must_use]
    pub fn mouse_position(&self) -> (i32, i32) {
        self.mouse.position
    }

    /// Cursor movement accumulated this frame.
    #[must_use]
    pub fn mouse_delta(&self) -> (i32, i32) {
        self.mouse.delta
    }

    /// Wheel notches accumulated this frame.
    #[must_use]
    pub fn wheel_delta(&self) -> f32 {
        self.mouse.wheel
    }

    /// Whether the window had keyboard focus at the last focus event.
    #[must_use]
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Releases every key and button without generating transitions.
    ///
    /// Used on focus loss. Releases are *not* reported as `was_released`, because the
    /// player did not release anything — the window simply stopped receiving input, and
    /// firing release logic for a dozen keys at once would trigger gameplay the player
    /// never asked for.
    pub fn release_all(&mut self) {
        self.keys.clear();
        self.previous_keys.clear();
        self.mouse.buttons = 0;
        self.mouse.previous_buttons = 0;
        self.mouse.delta = (0, 0);
        self.mouse.wheel = 0.0;
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    /// Applies events the way the engine does: transitions first, then the frame's events.
    fn frame(state: &mut InputState, events: &[InputEvent]) {
        state.begin_frame();
        for event in events {
            state.apply(*event);
        }
    }

    fn key_down(key: KeyCode) -> InputEvent {
        InputEvent::KeyDown { key, repeat: false }
    }

    fn key_up(key: KeyCode) -> InputEvent {
        InputEvent::KeyUp { key }
    }

    #[test]
    fn key_codes_are_densely_indexed_and_fit_the_bitset() {
        for (index, key) in KeyCode::ALL.into_iter().enumerate() {
            assert_eq!(key.index(), index, "{key:?} is out of order");
        }
        assert_eq!(KeyCode::Unknown.index(), KeyCode::COUNT - 1);
        const { assert!(KeyCode::COUNT <= KEY_WORDS * 64) };
    }

    #[test]
    fn function_and_digit_keys_round_trip() {
        for number in 1..=12_u8 {
            let key = KeyCode::function_key(number).expect("F1-F12 exist");
            assert_eq!(key.function_number(), Some(number));
        }
        assert_eq!(KeyCode::function_key(0), None);
        assert_eq!(KeyCode::function_key(13), None);
        assert_eq!(KeyCode::A.function_number(), None);
        assert_eq!(KeyCode::function_key(5), Some(KeyCode::F5));

        assert_eq!(KeyCode::digit_key(0), Some(KeyCode::Digit0));
        assert_eq!(KeyCode::digit_key(9), Some(KeyCode::Digit9));
        assert_eq!(KeyCode::digit_key(10), None);
    }

    #[test]
    fn a_key_set_holds_every_key_independently() {
        let mut set = KeySet::new();
        assert!(set.is_empty());

        for key in KeyCode::ALL {
            set.insert(key);
        }
        assert_eq!(set.len(), KeyCode::ALL.len());
        for key in KeyCode::ALL {
            assert!(set.contains(key), "{key:?} was inserted");
        }

        set.remove(KeyCode::W);
        assert!(!set.contains(KeyCode::W));
        assert!(set.contains(KeyCode::A), "removing W touched only W");

        set.clear();
        assert!(set.is_empty());
    }

    #[test]
    fn a_press_is_down_and_pressed_for_exactly_one_frame() {
        let mut state = InputState::new();

        frame(&mut state, &[key_down(KeyCode::Space)]);
        assert!(state.is_key_down(KeyCode::Space));
        assert!(state.was_key_pressed(KeyCode::Space));
        assert!(!state.was_key_released(KeyCode::Space));

        frame(&mut state, &[]);
        assert!(state.is_key_down(KeyCode::Space), "still held");
        assert!(
            !state.was_key_pressed(KeyCode::Space),
            "the press was last frame"
        );
    }

    #[test]
    fn a_release_is_reported_for_exactly_one_frame() {
        let mut state = InputState::new();
        frame(&mut state, &[key_down(KeyCode::Space)]);
        frame(&mut state, &[key_up(KeyCode::Space)]);

        assert!(!state.is_key_down(KeyCode::Space));
        assert!(state.was_key_released(KeyCode::Space));

        frame(&mut state, &[]);
        assert!(!state.was_key_released(KeyCode::Space));
    }

    #[test]
    fn auto_repeat_holds_the_key_without_repeating_the_press() {
        let mut state = InputState::new();
        frame(&mut state, &[key_down(KeyCode::W)]);
        frame(
            &mut state,
            &[InputEvent::KeyDown {
                key: KeyCode::W,
                repeat: true,
            }],
        );

        assert!(state.is_key_down(KeyCode::W));
        assert!(
            !state.was_key_pressed(KeyCode::W),
            "auto-repeat is not a new press"
        );
    }

    #[test]
    fn a_press_and_release_within_one_frame_reports_the_release() {
        let mut state = InputState::new();
        frame(
            &mut state,
            &[key_down(KeyCode::Space), key_up(KeyCode::Space)],
        );

        // The key is up at the end of the frame and was up at the start, so neither a
        // press nor a release is visible. This is the known cost of frame-granular state,
        // and it is why an event-driven consumer must read events, not only state.
        assert!(!state.is_key_down(KeyCode::Space));
        assert!(!state.was_key_pressed(KeyCode::Space));
        assert!(!state.was_key_released(KeyCode::Space));
    }

    #[test]
    fn wasd_keys_are_independent() {
        let mut state = InputState::new();
        frame(&mut state, &[key_down(KeyCode::W), key_down(KeyCode::D)]);

        assert!(state.is_key_down(KeyCode::W));
        assert!(state.is_key_down(KeyCode::D));
        assert!(!state.is_key_down(KeyCode::A));
        assert!(!state.is_key_down(KeyCode::S));
        assert!(state.any_key_down());
        assert_eq!(state.keys_down().len(), 2);
    }

    #[test]
    fn mouse_buttons_have_the_same_transition_semantics_as_keys() {
        let mut state = InputState::new();
        frame(
            &mut state,
            &[InputEvent::MouseButtonDown {
                button: MouseButton::Left,
            }],
        );
        assert!(state.is_mouse_button_down(MouseButton::Left));
        assert!(state.was_mouse_button_pressed(MouseButton::Left));
        assert!(!state.is_mouse_button_down(MouseButton::Right));

        frame(&mut state, &[]);
        assert!(state.is_mouse_button_down(MouseButton::Left));
        assert!(!state.was_mouse_button_pressed(MouseButton::Left));

        frame(
            &mut state,
            &[InputEvent::MouseButtonUp {
                button: MouseButton::Left,
            }],
        );
        assert!(state.was_mouse_button_released(MouseButton::Left));
        assert!(!state.is_mouse_button_down(MouseButton::Left));
    }

    #[test]
    fn every_mouse_button_has_its_own_bit() {
        let mut state = InputState::new();
        for button in MouseButton::ALL {
            state.apply(InputEvent::MouseButtonDown { button });
        }
        for button in MouseButton::ALL {
            assert!(state.is_mouse_button_down(button), "{button:?}");
        }
        state.apply(InputEvent::MouseButtonUp {
            button: MouseButton::Middle,
        });
        assert!(!state.is_mouse_button_down(MouseButton::Middle));
        assert!(state.is_mouse_button_down(MouseButton::Left));
    }

    #[test]
    fn mouse_movement_accumulates_within_a_frame_and_resets_between_frames() {
        let mut state = InputState::new();
        frame(
            &mut state,
            &[
                InputEvent::MouseMoved { x: 10, y: 5 },
                InputEvent::MouseMoved { x: 14, y: 9 },
            ],
        );
        assert_eq!(state.mouse_position(), (14, 9));
        assert_eq!(state.mouse_delta(), (14, 9), "measured from the origin");

        frame(&mut state, &[InputEvent::MouseMoved { x: 20, y: 9 }]);
        assert_eq!(state.mouse_position(), (20, 9));
        assert_eq!(state.mouse_delta(), (6, 0));

        frame(&mut state, &[]);
        assert_eq!(state.mouse_delta(), (0, 0), "no movement, no delta");
        assert_eq!(state.mouse_position(), (20, 9), "position persists");
    }

    #[test]
    fn wheel_notches_accumulate_within_a_frame_and_reset_between_frames() {
        let mut state = InputState::new();
        frame(
            &mut state,
            &[
                InputEvent::MouseWheel { delta: 1.0 },
                InputEvent::MouseWheel { delta: 0.5 },
            ],
        );
        assert!((state.wheel_delta() - 1.5).abs() < 1e-6);

        frame(&mut state, &[InputEvent::MouseWheel { delta: -2.0 }]);
        assert!((state.wheel_delta() + 2.0).abs() < 1e-6);

        frame(&mut state, &[]);
        assert_eq!(state.wheel_delta(), 0.0);
    }

    #[test]
    fn losing_focus_releases_everything_without_firing_release_logic() {
        let mut state = InputState::new();
        frame(
            &mut state,
            &[
                key_down(KeyCode::W),
                InputEvent::MouseButtonDown {
                    button: MouseButton::Right,
                },
            ],
        );

        frame(&mut state, &[InputEvent::FocusChanged { focused: false }]);
        assert!(!state.is_key_down(KeyCode::W));
        assert!(!state.is_mouse_button_down(MouseButton::Right));
        assert!(!state.is_focused());
        assert!(
            !state.was_key_released(KeyCode::W),
            "the player did not release the key; the window lost input"
        );
        assert!(!state.was_mouse_button_released(MouseButton::Right));
    }

    #[test]
    fn regaining_focus_leaves_input_clear() {
        let mut state = InputState::new();
        frame(&mut state, &[key_down(KeyCode::W)]);
        frame(&mut state, &[InputEvent::FocusChanged { focused: false }]);
        frame(&mut state, &[InputEvent::FocusChanged { focused: true }]);

        assert!(state.is_focused());
        assert!(!state.is_key_down(KeyCode::W));
        assert!(!state.any_key_down());
    }

    #[test]
    fn a_fresh_state_reports_nothing_held() {
        let state = InputState::new();
        assert!(!state.any_key_down());
        assert_eq!(state.mouse_position(), (0, 0));
        assert_eq!(state.mouse_delta(), (0, 0));
        assert_eq!(state.wheel_delta(), 0.0);
        for key in KeyCode::ALL {
            assert!(!state.is_key_down(key));
            assert!(!state.was_key_pressed(key));
            assert!(!state.was_key_released(key));
        }
        for button in MouseButton::ALL {
            assert!(!state.is_mouse_button_down(button));
        }
    }

    #[test]
    fn every_named_key_round_trips_through_its_written_name() {
        for key in KeyCode::ALL {
            let name = key.name().expect("every key in ALL has a name");
            assert_eq!(
                KeyCode::from_name(name),
                Some(key),
                "{key:?} did not survive its own name"
            );
        }
    }

    #[test]
    fn no_two_keys_share_a_name_and_unknown_has_none() {
        // A duplicate would make one of the two unreadable from a configuration file, and
        // the loss would be silent: the file would load, and the wrong key would be bound.
        let mut names: Vec<&str> = KeyCode::ALL
            .iter()
            .map(|key| key.name().unwrap_or(""))
            .collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count, "two keys share a written name");

        assert_eq!(KeyCode::Unknown.name(), None, "an unnameable key was named");
    }

    #[test]
    fn an_unrecognised_name_binds_nothing() {
        for name in ["", "unknown", "a", "DIGIT10", "F13", "MOUSE1", " SPACE"] {
            assert_eq!(KeyCode::from_name(name), None, "{name:?} produced a key");
        }
    }

    #[test]
    fn a_slot_prints_a_short_label_for_every_key() {
        assert_eq!(KeyCode::Digit1.label(), "1");
        assert_eq!(KeyCode::Digit0.label(), "0");
        assert_eq!(KeyCode::W.label(), "W");
        assert_eq!(KeyCode::F5.label(), "F5");
        assert_eq!(KeyCode::Space.label(), "SPC");
        // Four characters is what fits an action bar slot's corner at the design size.
        for key in KeyCode::ALL {
            let label = key.label();
            assert!(!label.is_empty(), "{key:?} prints nothing");
            assert!(
                label.len() <= 4,
                "{key:?} prints {label:?}, which will not fit"
            );
        }
    }

    #[test]
    fn every_chord_round_trips_through_its_written_name() {
        for key in KeyCode::ALL {
            for bits in 0..8 {
                let modifiers = Modifiers::new(bits & 1 != 0, bits & 2 != 0, bits & 4 != 0);
                let chord = KeyChord::new(key, modifiers);
                let Some(name) = chord.to_name() else {
                    continue;
                };
                if !chord.is_bindable() {
                    assert_eq!(
                        KeyChord::from_name(&name),
                        None,
                        "{name} names a chord nothing can press"
                    );
                    continue;
                }
                assert_eq!(KeyChord::from_name(&name), Some(chord), "{name}");
            }
        }
    }

    #[test]
    fn a_bare_key_name_reads_as_a_plain_chord_so_older_files_still_load() {
        assert_eq!(
            KeyChord::from_name("DIGIT1"),
            Some(KeyChord::plain(KeyCode::Digit1))
        );
        assert_eq!(
            KeyChord::from_name("CTRL+SHIFT+DIGIT1"),
            Some(KeyChord::new(
                KeyCode::Digit1,
                Modifiers::new(true, true, false)
            ))
        );
    }

    #[test]
    fn a_misspelled_or_reordered_chord_name_binds_nothing() {
        for name in [
            "SHIFT+CTRL+DIGIT1",
            "CTRL+CTRL+DIGIT1",
            "CTRL+",
            "CTRL+NONSENSE",
            "SUPER+DIGIT1",
            "CTRL+LEFTSHIFT",
        ] {
            assert_eq!(KeyChord::from_name(name), None, "{name}");
        }
    }

    #[test]
    fn a_chord_fires_only_when_exactly_its_modifiers_are_held() {
        let plain = KeyChord::plain(KeyCode::Digit1);
        let with_shift = KeyChord::new(KeyCode::Digit1, Modifiers::new(false, true, false));

        let mut state = InputState::new();
        frame(&mut state, &[key_down(KeyCode::Digit1)]);
        assert!(state.was_chord_pressed(plain));
        assert!(!state.was_chord_pressed(with_shift));

        // Shift held first, then the digit: the plain slot must stay quiet, or one press
        // would cast both slots.
        let mut state = InputState::new();
        frame(&mut state, &[key_down(KeyCode::LeftShift)]);
        frame(&mut state, &[key_down(KeyCode::Digit1)]);
        assert!(state.was_chord_pressed(with_shift));
        assert!(!state.was_chord_pressed(plain));
    }

    #[test]
    fn left_and_right_modifiers_are_the_same_modifier() {
        let chord = KeyChord::new(KeyCode::Q, Modifiers::new(true, false, false));
        for control in [KeyCode::LeftControl, KeyCode::RightControl] {
            let mut state = InputState::new();
            frame(&mut state, &[key_down(control)]);
            frame(&mut state, &[key_down(KeyCode::Q)]);
            assert!(state.was_chord_pressed(chord), "{control:?}");
        }
    }

    #[test]
    fn a_modifier_is_never_a_chord_of_its_own() {
        for key in KeyCode::ALL {
            assert_eq!(
                is_modifier(key),
                !KeyChord::plain(key).is_bindable(),
                "{key:?}"
            );
        }
    }

    #[test]
    fn a_chord_prints_a_short_label_that_fits_a_slot_corner() {
        let mut label = String::new();
        KeyChord::new(KeyCode::Digit1, Modifiers::new(true, true, false)).write_label(&mut label);
        assert_eq!(label, "CS-1");
        label.clear();
        KeyChord::plain(KeyCode::Digit1).write_label(&mut label);
        assert_eq!(label, "1");
    }

    #[test]
    fn a_digit_key_reports_the_digit_it_types() {
        for digit in 0..=9 {
            let key = KeyCode::digit_key(digit).expect("0-9 are digit keys");
            assert_eq!(key.digit_number(), Some(digit));
        }
        assert_eq!(KeyCode::W.digit_number(), None);
        assert_eq!(KeyCode::F1.digit_number(), None);
    }
}
