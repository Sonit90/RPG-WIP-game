//! Display settings: the resolution, the window mode, and the rules for choosing and
//! confirming them.
//!
//! ```text
//! settings.cfg   "display": { "resolution_width": 1920, "resolution_height": 1080,
//!                             "window_mode": "borderless_fullscreen" }
//!      |
//! DisplaySettings      what the player chose: validated on load, never trusted blindly
//!      |
//! resolution_options   what the monitor offers, deduplicated, common sizes first
//! closest_resolution   what to use when the saved size is not offered any more
//!      |
//! DisplayConfirmation  apply, then keep within fifteen seconds or go back
//! ```
//!
//! Pure data and rules. Nothing here knows a monitor exists: the platform layer reports what
//! the monitor offers as plain `(width, height)` pairs and this module decides what to do
//! with them, which is what lets every rule be tested without a display.

use serde::{Deserialize, Serialize};

/// Smallest resolution edge accepted from the file or offered in the menu, in pixels.
///
/// Below 640 the interface's minimum scale no longer fits the HUD; a saved value under it is
/// a hand-edit, not a choice.
pub const RESOLUTION_MIN_EDGE: u32 = 640;

/// Largest resolution edge accepted, in pixels: D3D12's own texture limit.
pub const RESOLUTION_MAX_EDGE: u32 = 16384;

/// The most rows the resolution list offers.
///
/// A monitor reports dozens of modes — every refresh rate, every scaling, every legacy 4:3
/// size. Twelve is a list a player can read at a glance, and it always has room for the
/// common sizes, the desktop's own, and whatever the player is using now.
pub const RESOLUTION_LIST_LIMIT: usize = 12;

/// Seconds a player has to confirm new display settings before they are reverted.
pub const CONFIRM_SECONDS: f64 = 15.0;

/// The sizes a player names, in the order a list wants them: these are always offered when
/// the monitor supports them, before anything less common.
pub const COMMON_RESOLUTIONS: [Resolution; 8] = [
    Resolution::new(3840, 2160),
    Resolution::new(3440, 1440),
    Resolution::new(2560, 1440),
    Resolution::new(1920, 1200),
    Resolution::new(1920, 1080),
    Resolution::new(1600, 900),
    Resolution::new(1366, 768),
    Resolution::new(1280, 720),
];

/// A size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Resolution {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl Resolution {
    /// `width` by `height`.
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Whether both edges are inside the accepted range.
    #[must_use]
    pub fn is_valid(self) -> bool {
        (RESOLUTION_MIN_EDGE..=RESOLUTION_MAX_EDGE).contains(&self.width)
            && (RESOLUTION_MIN_EDGE / 2..=RESOLUTION_MAX_EDGE).contains(&self.height)
    }

    /// How many pixels it has.
    #[must_use]
    pub fn pixels(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// Width over height.
    #[must_use]
    pub fn aspect(self) -> f64 {
        f64::from(self.width) / f64::from(self.height.max(1))
    }

    /// The way the menu prints it: `1920 x 1080`.
    ///
    /// An ASCII `x` rather than `×`: the menus are drawn in the 5x7 bitmap font, which covers
    /// printable ASCII and nothing else, and a missing glyph draws as a hollow box.
    #[must_use]
    pub fn label(self) -> String {
        format!("{} x {}", self.width, self.height)
    }
}

impl From<(u32, u32)> for Resolution {
    fn from((width, height): (u32, u32)) -> Self {
        Self::new(width, height)
    }
}

/// How the game's window covers the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    /// A normal window with a border and a title bar, sized by the resolution.
    #[default]
    Windowed,
    /// A window with no border covering the whole monitor at the desktop's own resolution.
    ///
    /// No display-mode change: alt-tab is instant and other windows can sit on top. The
    /// resolution setting does not apply, because the desktop's is used.
    BorderlessFullscreen,
    /// The monitor switched to the chosen resolution, the game covering all of it.
    ///
    /// A real display-mode change, undone whenever the game is not in front.
    Fullscreen,
}

impl WindowMode {
    /// Every mode, in the order the menu lists them.
    pub const ALL: [Self; 3] = [Self::Windowed, Self::BorderlessFullscreen, Self::Fullscreen];

    /// What the menu calls it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Windowed => "WINDOWED",
            Self::BorderlessFullscreen => "BORDERLESS FULLSCREEN",
            Self::Fullscreen => "FULLSCREEN",
        }
    }

    /// One line saying what it does, for the menu to print under the choice.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Windowed => "A WINDOW YOU CAN MOVE AND RESIZE, AT THE CHOSEN RESOLUTION.",
            Self::BorderlessFullscreen => {
                "COVERS THE MONITOR AT ITS DESKTOP RESOLUTION. INSTANT ALT+TAB."
            }
            Self::Fullscreen => "SWITCHES THE MONITOR TO THE CHOSEN RESOLUTION.",
        }
    }

    /// The word the file uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Windowed => "windowed",
            Self::BorderlessFullscreen => "borderless_fullscreen",
            Self::Fullscreen => "fullscreen",
        }
    }

    /// The mode a word names, or `None`. Case-insensitive and forgiving about separators,
    /// because the file is hand-edited.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let word: String = text
            .trim()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        match word.as_str() {
            "windowed" | "window" => Some(Self::Windowed),
            "borderlessfullscreen" | "borderless" | "fullscreenwindow" | "windowedfullscreen" => {
                Some(Self::BorderlessFullscreen)
            }
            "fullscreen" | "exclusivefullscreen" | "exclusive" => Some(Self::Fullscreen),
            _ => None,
        }
    }

    /// Whether it covers the whole monitor.
    #[must_use]
    pub fn is_fullscreen(self) -> bool {
        !matches!(self, Self::Windowed)
    }

    /// Whether the resolution setting decides the size. Borderless uses the desktop's.
    #[must_use]
    pub fn uses_resolution(self) -> bool {
        !matches!(self, Self::BorderlessFullscreen)
    }
}

/// Reads a window mode, and reads anything that is not one as the default.
fn lenient_mode<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<WindowMode, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_str()
        .and_then(WindowMode::parse)
        .unwrap_or_else(|| {
            log::warn!("settings: {value} is not a window mode; using windowed");
            WindowMode::Windowed
        }))
}

/// Reads a resolution edge, and reads anything that is not a whole number as zero, which
/// validation then replaces.
fn lenient_edge<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value
        .as_u64()
        .and_then(|edge| u32::try_from(edge).ok())
        .unwrap_or(0))
}

/// The display section of the settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    /// Width of the chosen resolution, in pixels.
    #[serde(deserialize_with = "lenient_edge")]
    pub resolution_width: u32,
    /// Height of the chosen resolution, in pixels.
    #[serde(deserialize_with = "lenient_edge")]
    pub resolution_height: u32,
    /// How the window covers the screen.
    #[serde(deserialize_with = "lenient_mode")]
    pub window_mode: WindowMode,
}

impl Default for DisplaySettings {
    /// A 1080p window: what the game did before display settings existed.
    fn default() -> Self {
        Self {
            resolution_width: 1920,
            resolution_height: 1080,
            window_mode: WindowMode::Windowed,
        }
    }
}

impl DisplaySettings {
    /// A resolution and a mode.
    #[must_use]
    pub fn new(resolution: Resolution, window_mode: WindowMode) -> Self {
        Self {
            resolution_width: resolution.width,
            resolution_height: resolution.height,
            window_mode,
        }
    }

    /// The chosen resolution.
    #[must_use]
    pub fn resolution(self) -> Resolution {
        Resolution::new(self.resolution_width, self.resolution_height)
    }

    /// These settings with an impossible resolution replaced by the default one.
    ///
    /// Only what the file can get wrong by itself is checked here. Whether the *monitor*
    /// offers the size is a question for [`DisplaySettings::fitted_to`], asked when there
    /// is a monitor to ask.
    #[must_use]
    pub fn validated(self) -> Self {
        if self.resolution().is_valid() {
            return self;
        }
        let default = Self::default();
        log::warn!(
            "settings: display resolution {}x{} is outside {RESOLUTION_MIN_EDGE}..\
             {RESOLUTION_MAX_EDGE}; using {}x{}",
            self.resolution_width,
            self.resolution_height,
            default.resolution_width,
            default.resolution_height
        );
        Self {
            resolution_width: default.resolution_width,
            resolution_height: default.resolution_height,
            ..self
        }
    }

    /// These settings made possible on a monitor offering `available`: the saved resolution
    /// if it is offered, otherwise the closest one that is.
    #[must_use]
    pub fn fitted_to(self, available: &[Resolution]) -> Self {
        let wanted = self.validated().resolution();
        match closest_resolution(wanted, available) {
            Some(found) => Self::new(found, self.window_mode),
            None => self.validated(),
        }
    }
}

/// The offered resolution nearest to `wanted`: itself if it is offered, otherwise the one
/// with the same shape and the nearest size, otherwise the nearest size of any shape.
///
/// `None` only when nothing is offered.
#[must_use]
pub fn closest_resolution(wanted: Resolution, available: &[Resolution]) -> Option<Resolution> {
    if available.contains(&wanted) {
        return Some(wanted);
    }
    available.iter().copied().min_by(|a, b| {
        distance(wanted, *a)
            .partial_cmp(&distance(wanted, *b))
            .unwrap_or(core::cmp::Ordering::Equal)
    })
}

/// How far `candidate` is from `wanted`: the difference in pixel count, relative, plus a
/// heavy penalty for a different shape, so a 16:9 choice falls back to a 16:9 size.
fn distance(wanted: Resolution, candidate: Resolution) -> f64 {
    let size = (candidate.pixels() as f64 / wanted.pixels().max(1) as f64)
        .ln()
        .abs();
    let shape = (candidate.aspect() - wanted.aspect()).abs();
    size + shape * 4.0
}

/// The resolutions the menu offers, from what the monitor reports.
///
/// Duplicates — the same size at several refresh rates — appear once. Sizes too small for
/// the interface, and sizes larger than the desktop's (which only a misreporting driver
/// offers), are dropped. The common sizes the monitor supports come first in importance,
/// then the desktop's own, then `current`; anything else fills what room is left. The list
/// is largest first, the order a player reads it in.
#[must_use]
pub fn resolution_options(
    reported: &[Resolution],
    desktop: Resolution,
    current: Option<Resolution>,
) -> Vec<Resolution> {
    let mut unique: Vec<Resolution> = reported
        .iter()
        .copied()
        .filter(|resolution| {
            resolution.is_valid()
                && resolution.width <= desktop.width.max(RESOLUTION_MIN_EDGE)
                && resolution.height <= desktop.height.max(RESOLUTION_MIN_EDGE)
        })
        .collect();
    unique.sort_unstable();
    unique.dedup();

    let mut chosen: Vec<Resolution> = Vec::with_capacity(RESOLUTION_LIST_LIMIT);
    let take = |resolution: Resolution, chosen: &mut Vec<Resolution>| {
        if chosen.len() < RESOLUTION_LIST_LIMIT && !chosen.contains(&resolution) {
            chosen.push(resolution);
        }
    };
    if desktop.is_valid() {
        take(desktop, &mut chosen);
    }
    if let Some(current) = current.filter(|current| unique.contains(current)) {
        take(current, &mut chosen);
    }
    for common in COMMON_RESOLUTIONS {
        if unique.contains(&common) {
            take(common, &mut chosen);
        }
    }
    // The rest, largest first, preferring the desktop's shape.
    let mut rest: Vec<Resolution> = unique
        .iter()
        .copied()
        .filter(|resolution| !chosen.contains(resolution))
        .collect();
    rest.sort_by(|a, b| {
        let shape_a = (a.aspect() - desktop.aspect()).abs() > 0.01;
        let shape_b = (b.aspect() - desktop.aspect()).abs() > 0.01;
        shape_a.cmp(&shape_b).then(b.pixels().cmp(&a.pixels()))
    });
    for resolution in rest {
        take(resolution, &mut chosen);
    }
    chosen.sort_by(|a, b| b.pixels().cmp(&a.pixels()).then(b.width.cmp(&a.width)));
    chosen
}

/// What became of display settings waiting to be confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// The player kept them: save them.
    Keep(DisplaySettings),
    /// The player declined, or time ran out: put these back.
    Revert(DisplaySettings),
}

/// Display settings that have been applied and are waiting for the player to keep them.
///
/// ```text
/// apply ──> pending ──keep────────────> Keep(new)      saved
///              │
///              ├──revert──────────────> Revert(old)    put back
///              └──CONFIRM_SECONDS pass─> Revert(old)    put back
/// ```
///
/// Time is passed in rather than read, so the whole machine is tested without a clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayConfirmation {
    previous: DisplaySettings,
    applied: DisplaySettings,
    deadline: f64,
}

impl DisplayConfirmation {
    /// `applied` has just replaced `previous`, at `now` seconds.
    #[must_use]
    pub fn start(previous: DisplaySettings, applied: DisplaySettings, now: f64) -> Self {
        Self {
            previous,
            applied,
            deadline: now + CONFIRM_SECONDS,
        }
    }

    /// What was in force before.
    #[must_use]
    pub fn previous(&self) -> DisplaySettings {
        self.previous
    }

    /// What is being tried.
    #[must_use]
    pub fn applied(&self) -> DisplaySettings {
        self.applied
    }

    /// Whole seconds left before the revert, rounded up: what the dialog counts down.
    #[must_use]
    pub fn seconds_left(&self, now: f64) -> u32 {
        (self.deadline - now).max(0.0).ceil() as u32
    }

    /// The revert, if time has run out at `now`.
    #[must_use]
    pub fn expired(&self, now: f64) -> Option<ConfirmOutcome> {
        (now >= self.deadline).then_some(ConfirmOutcome::Revert(self.previous))
    }

    /// The player kept the new settings.
    #[must_use]
    pub fn keep(self) -> ConfirmOutcome {
        ConfirmOutcome::Keep(self.applied)
    }

    /// The player asked for the old ones back.
    #[must_use]
    pub fn revert(self) -> ConfirmOutcome {
        ConfirmOutcome::Revert(self.previous)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn r(width: u32, height: u32) -> Resolution {
        Resolution::new(width, height)
    }

    #[test]
    fn the_default_is_a_1080p_window() {
        let display = DisplaySettings::default();
        assert_eq!(display.resolution(), r(1920, 1080));
        assert_eq!(display.window_mode, WindowMode::Windowed);
    }

    #[test]
    fn the_file_round_trips_with_the_names_it_promises() {
        let display = DisplaySettings::new(r(2560, 1440), WindowMode::BorderlessFullscreen);
        let text = serde_json::to_string(&display).expect("serializes");
        assert!(text.contains("\"resolution_width\":2560"), "{text}");
        assert!(text.contains("\"resolution_height\":1440"), "{text}");
        assert!(
            text.contains("\"window_mode\":\"borderless_fullscreen\""),
            "{text}"
        );
        let back: DisplaySettings = serde_json::from_str(&text).expect("parses");
        assert_eq!(back, display);
        for mode in WindowMode::ALL {
            let text = serde_json::to_string(&mode).expect("serializes");
            assert_eq!(text, format!("\"{}\"", mode.as_str()));
            assert_eq!(WindowMode::parse(mode.as_str()), Some(mode));
        }
    }

    #[test]
    fn a_wrong_window_mode_costs_only_that_field() {
        let back: DisplaySettings = serde_json::from_str(
            r#"{"resolution_width": 1280, "resolution_height": 720, "window_mode": "sideways"}"#,
        )
        .expect("parses");
        assert_eq!(back.window_mode, WindowMode::Windowed);
        assert_eq!(back.resolution(), r(1280, 720));
        let back: DisplaySettings = serde_json::from_str(r#"{"window_mode": 7}"#).expect("parses");
        assert_eq!(back, DisplaySettings::default());
        assert_eq!(
            WindowMode::parse(" Borderless "),
            Some(WindowMode::BorderlessFullscreen)
        );
        assert_eq!(
            WindowMode::parse("EXCLUSIVE-fullscreen"),
            Some(WindowMode::Fullscreen)
        );
    }

    #[test]
    fn an_impossible_resolution_falls_back_to_the_default() {
        for (width, height) in [(0, 0), (10, 10), (99_999, 1080), (1920, 99_999)] {
            let display = DisplaySettings {
                resolution_width: width,
                resolution_height: height,
                window_mode: WindowMode::Fullscreen,
            }
            .validated();
            assert_eq!(display.resolution(), r(1920, 1080), "{width}x{height}");
            assert_eq!(
                display.window_mode,
                WindowMode::Fullscreen,
                "the mode is kept"
            );
        }
        let back: DisplaySettings =
            serde_json::from_str(r#"{"resolution_width": "big", "resolution_height": -4}"#)
                .expect("parses");
        assert_eq!(back.validated(), DisplaySettings::default());
    }

    #[test]
    fn a_resolution_the_monitor_lost_becomes_the_closest_it_has() {
        let offered = [r(1920, 1080), r(1600, 900), r(1280, 1024), r(2560, 1440)];
        assert_eq!(
            closest_resolution(r(1600, 900), &offered),
            Some(r(1600, 900))
        );
        // Same shape preferred over a nearer pixel count of another shape.
        assert_eq!(
            closest_resolution(r(1366, 768), &offered),
            Some(r(1600, 900))
        );
        assert_eq!(
            closest_resolution(r(3840, 2160), &offered),
            Some(r(2560, 1440))
        );
        assert_eq!(
            closest_resolution(r(1280, 1000), &offered),
            Some(r(1280, 1024))
        );
        assert_eq!(closest_resolution(r(1920, 1080), &[]), None);
        let fitted =
            DisplaySettings::new(r(3840, 2160), WindowMode::Fullscreen).fitted_to(&offered);
        assert_eq!(
            fitted,
            DisplaySettings::new(r(2560, 1440), WindowMode::Fullscreen)
        );
    }

    #[test]
    fn the_list_is_unique_sane_common_first_and_largest_first() {
        let reported = [
            r(3840, 2160),
            r(3840, 2160),
            r(2560, 1440),
            r(1920, 1080),
            r(1920, 1080),
            r(1680, 1050),
            r(1600, 900),
            r(1280, 720),
            r(1024, 768),
            r(800, 600),
            r(640, 480),
            r(320, 200),
            r(1440, 900),
            r(1360, 768),
            r(1366, 768),
            r(1280, 960),
            r(1280, 800),
            r(1152, 864),
            r(7680, 4320),
        ];
        let list = resolution_options(&reported, r(3840, 2160), Some(r(1680, 1050)));
        assert!(list.len() <= RESOLUTION_LIST_LIMIT);
        let mut unique = list.clone();
        unique.dedup();
        assert_eq!(unique.len(), list.len(), "no duplicates");
        for common in [
            r(3840, 2160),
            r(2560, 1440),
            r(1920, 1080),
            r(1600, 900),
            r(1280, 720),
        ] {
            assert!(list.contains(&common), "{common:?} is offered");
        }
        assert!(list.contains(&r(1680, 1050)), "the current size is offered");
        assert!(!list.contains(&r(320, 200)), "too small");
        assert!(!list.contains(&r(7680, 4320)), "larger than the desktop");
        assert!(
            list.windows(2)
                .all(|pair| pair[0].pixels() >= pair[1].pixels())
        );
        assert_eq!(list[0], r(3840, 2160));
        assert_eq!(r(1920, 1080).label(), "1920 x 1080");
        assert!(r(1920, 1080).label().is_ascii(), "the menu font is ASCII");
    }

    #[test]
    fn confirming_keeps_and_declining_or_waiting_reverts() {
        let old = DisplaySettings::default();
        let new = DisplaySettings::new(r(2560, 1440), WindowMode::Fullscreen);
        let pending = DisplayConfirmation::start(old, new, 100.0);
        assert_eq!(pending.seconds_left(100.0), 15);
        assert_eq!(pending.seconds_left(108.2), 7);
        assert_eq!(pending.expired(114.9), None);
        assert_eq!(pending.expired(115.0), Some(ConfirmOutcome::Revert(old)));
        assert_eq!(pending.seconds_left(200.0), 0);
        assert_eq!(pending.keep(), ConfirmOutcome::Keep(new));
        assert_eq!(pending.revert(), ConfirmOutcome::Revert(old));
        assert_eq!(pending.previous(), old);
        assert_eq!(pending.applied(), new);
    }
}
