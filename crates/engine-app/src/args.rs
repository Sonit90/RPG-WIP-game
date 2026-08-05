//! Command-line arguments for the game executable.
//!
//! Pure parsing, no I/O, so every rule below is unit-tested.

use std::fmt;

/// Usage text printed for `--help` and on a parse error.
pub const USAGE: &str = "\
usage: mygame [options]

options:
  --width <pixels>       client width (default 1920)
  --height <pixels>      client height (default 1080)
  --vsync                wait for vertical blank (off by default: vsync measures the
                         monitor, not the engine)
  --frames <count>       exit after rendering this many frames
  --report <seconds>     frame-time report interval (default 1.0, 0 disables)
  --overlay              start with the debug overlay visible (F5 toggles it at any
                         time; hidden by default because it costs frame time)
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
            help: false,
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
            "--overlay" => args.overlay = true,
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
