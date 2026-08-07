//! Command-line arguments for the game executable.
//!
//! Pure parsing, no I/O, so every rule below is unit-tested.
//!
//! Arguments are *overrides* on top of [`EngineSettings::default`], not a second source of
//! truth. Parsing produces settings; the engine and its subsystems read only those.

use std::fmt;

use engine_core::config::{DisplayMode, EngineSettings};

/// Usage text printed for `--help` and on a parse error.
pub const USAGE: &str = "\
usage: mygame [options]

options:
  --width <pixels>       client width (default 1920)
  --height <pixels>      client height (default 1080)
  --fullscreen           borderless fullscreen on the primary monitor (--width and
                         --height are then ignored)
  --vsync                wait for vertical blank (off by default: vsync measures the
                         monitor, not the engine)
  --validation           enable the D3D12 validation layer (on by default in debug
                         builds; it costs several times the frame time)
  --frames <count>       exit after rendering this many frames
  --report <seconds>     frame-time report interval (default 1.0, 0 disables)
  --overlay              start with the debug overlay visible (F5 toggles it at any
                         time; hidden by default because it costs frame time)
  --dummies <count>      training dummies in the group, on top of the single dummy
                         (default 4)
  -h, --help             print this help and exit";

/// Parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    /// Requested client width in pixels.
    pub width: u32,
    /// Requested client height in pixels.
    pub height: u32,
    /// Whether to present with vsync.
    pub vsync: bool,
    /// Optional frame budget after which the application exits on its own.
    pub frames: Option<u64>,
    /// Seconds between frame-time reports. Zero disables reporting.
    pub report_interval_secs: f64,
    /// Whether the debug overlay starts visible. It can always be toggled with F5.
    pub overlay: bool,
    /// Training dummies in the group, on top of the single dummy.
    pub dummies: usize,
    /// Whether to open a borderless fullscreen window.
    pub fullscreen: bool,
    /// Whether the renderer's validation layer was explicitly requested.
    ///
    /// `None` leaves the build's default in place; `Some(true)` forces it on.
    pub validation: Option<bool>,
    /// Whether `--help` was requested.
    pub help: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            vsync: false,
            frames: None,
            report_interval_secs: 1.0,
            overlay: false,
            dummies: 4,
            fullscreen: false,
            validation: None,
            help: false,
        }
    }
}

impl Args {
    /// Applies the command line on top of the default settings.
    ///
    /// Only the fields an option actually names are changed, so a default that moves
    /// moves for every run that did not override it.
    #[must_use]
    pub fn to_settings(&self) -> EngineSettings {
        let defaults = EngineSettings::default();
        EngineSettings {
            width: self.width,
            height: self.height,
            display_mode: if self.fullscreen {
                DisplayMode::BorderlessFullscreen
            } else {
                DisplayMode::Windowed
            },
            vsync: self.vsync,
            debug_overlay: self.overlay,
            renderer_validation: self.validation.unwrap_or(defaults.renderer_validation),
            frame_budget: self.frames,
            report_interval_seconds: self.report_interval_secs,
            title: "myGame — phase 3 training arena".to_owned(),
        }
    }
}

/// Why a command line was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgsError {
    /// An option that takes a value was given none.
    MissingValue(&'static str),
    /// A value could not be parsed as the expected type.
    InvalidValue {
        /// The option the value belonged to.
        option: &'static str,
        /// The text that failed to parse.
        value: String,
    },
    /// An option this build does not know.
    Unknown(String),
}

impl fmt::Display for ArgsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingValue(option) => write!(f, "{option} requires a value"),
            Self::InvalidValue { option, value } => {
                write!(f, "{option} does not accept the value {value:?}")
            }
            Self::Unknown(argument) => write!(f, "unknown option {argument:?}"),
        }
    }
}

impl std::error::Error for ArgsError {}

/// Parses arguments, excluding the executable name.
///
/// # Errors
///
/// Returns [`ArgsError`] for an unknown option, a missing value or an unparsable one.
pub fn parse<I, S>(arguments: I) -> Result<Args, ArgsError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = Args::default();
    let mut iter = arguments.into_iter();

    while let Some(argument) = iter.next() {
        let argument = argument.as_ref();
        match argument {
            "--width" => args.width = next_value(&mut iter, "--width")?,
            "--height" => args.height = next_value(&mut iter, "--height")?,
            "--frames" => args.frames = Some(next_value(&mut iter, "--frames")?),
            "--report" => args.report_interval_secs = next_value(&mut iter, "--report")?,
            "--vsync" => args.vsync = true,
            "--fullscreen" => args.fullscreen = true,
            "--validation" => args.validation = Some(true),
            "--overlay" => args.overlay = true,
            "--dummies" => args.dummies = next_value(&mut iter, "--dummies")?,
            "-h" | "--help" => args.help = true,
            other => return Err(ArgsError::Unknown(other.to_owned())),
        }
    }

    Ok(args)
}

/// Reads and parses the value that follows an option.
fn next_value<I, S, T>(iter: &mut I, option: &'static str) -> Result<T, ArgsError>
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
    T: std::str::FromStr,
{
    let raw = iter.next().ok_or(ArgsError::MissingValue(option))?;
    let raw = raw.as_ref();
    raw.parse::<T>().map_err(|_| ArgsError::InvalidValue {
        option,
        value: raw.to_owned(),
    })
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_1080p_without_vsync_or_a_frame_limit() {
        let args = parse::<[&str; 0], &str>([]).expect("an empty command line is valid");
        assert_eq!((args.width, args.height), (1920, 1080));
        assert!(!args.vsync);
        assert_eq!(args.frames, None);
        assert!((args.report_interval_secs - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn settings_default_where_no_option_was_given() {
        let settings = Args::default().to_settings();
        assert_eq!((settings.width, settings.height), (1920, 1080));
        assert_eq!(settings.display_mode, DisplayMode::Windowed);
        assert!(!settings.vsync);
        assert_eq!(
            settings.renderer_validation,
            EngineSettings::default().renderer_validation,
            "an unnamed option must not change the build's default"
        );
        settings.validate().expect("the defaults must be valid");
    }

    #[test]
    fn options_override_the_settings_they_name() {
        let args = parse(["--fullscreen", "--vsync", "--validation", "--overlay"])
            .expect("a well-formed command line is valid");
        let settings = args.to_settings();

        assert_eq!(settings.display_mode, DisplayMode::BorderlessFullscreen);
        assert!(settings.vsync);
        assert!(settings.renderer_validation);
        assert!(settings.debug_overlay);
        settings.validate().expect("the overrides must be valid");
    }

    #[test]
    fn invalid_settings_are_rejected_by_validation_not_by_parsing() {
        // Parsing accepts any number; the meaning of the number is the configuration's
        // business, so the error names the field and its bounds.
        let args = parse(["--width", "10"]).expect("parsing accepts the value");
        let error = args
            .to_settings()
            .validate()
            .expect_err("10 pixels is below the minimum");
        assert!(error.to_string().contains("width 10"), "{error}");
    }

    #[test]
    fn parses_every_option() {
        let args = parse([
            "--width", "1280", "--height", "720", "--vsync", "--frames", "500", "--report", "0.5",
        ])
        .expect("a well-formed command line is valid");
        assert_eq!((args.width, args.height), (1280, 720));
        assert!(args.vsync);
        assert_eq!(args.frames, Some(500));
        assert!((args.report_interval_secs - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn overlay_starts_hidden_unless_requested() {
        assert!(
            !parse::<[&str; 0], &str>([])
                .expect("empty is valid")
                .overlay,
            "the overlay costs frame time, so it is off until asked for"
        );
        assert!(parse(["--overlay"]).expect("--overlay is valid").overlay);
    }

    #[test]
    fn the_dummy_group_defaults_to_four_and_can_be_set() {
        assert_eq!(
            parse::<[&str; 0], &str>([])
                .expect("empty is valid")
                .dummies,
            4
        );
        assert_eq!(
            parse(["--dummies", "500"])
                .expect("--dummies is valid")
                .dummies,
            500
        );
    }

    #[test]
    fn help_is_recognised_in_both_spellings() {
        assert!(parse(["--help"]).expect("--help is valid").help);
        assert!(parse(["-h"]).expect("-h is valid").help);
    }

    #[test]
    fn rejects_a_missing_value() {
        assert_eq!(parse(["--width"]), Err(ArgsError::MissingValue("--width")));
    }

    #[test]
    fn rejects_an_unparsable_value() {
        assert_eq!(
            parse(["--frames", "many"]),
            Err(ArgsError::InvalidValue {
                option: "--frames",
                value: "many".to_owned(),
            })
        );
    }

    #[test]
    fn rejects_an_unknown_option() {
        assert_eq!(
            parse(["--ultra"]),
            Err(ArgsError::Unknown("--ultra".to_owned()))
        );
    }

    #[test]
    fn error_messages_name_the_option() {
        let message = ArgsError::MissingValue("--height").to_string();
        assert!(message.contains("--height"), "{message}");
    }
}
