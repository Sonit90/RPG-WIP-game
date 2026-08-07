//! Command-line arguments for the showcase.
//!
//! Pure parsing, no I/O, so every rule below is unit-tested.

use std::fmt;

use crate::scene::PRESETS;

/// Usage text printed for `--help` and on a parse error.
pub const USAGE: &str = "\
usage: showcase [options]

options:
  --width <pixels>     client width (default 1920)
  --height <pixels>    client height (default 1080)
  --fullscreen         borderless fullscreen (--width and --height are then ignored)
  --vsync              wait for vertical blank (off by default: vsync measures the
                       monitor, not the engine)
  --validation         enable the D3D12 validation layer (on by default in debug
                       builds; it costs several times the frame time)
  --preset <1-5>       object count to start with: 1=10, 2=100, 3=1000, 4=10000,
                       5=100000 (default 4). F3-F7 switch at runtime
  --frames-in-flight <n>  frames the CPU may run ahead of the GPU, 1 to 3 (default 3).
                       One serializes CPU and GPU, which moves any back-pressure wait
                       from Present to the frame fence where it can be attributed
  --parallel-cull      cull across the job system's workers instead of on the main
                       thread. Produces an identical render list; only the time changes
  --no-culling         start with frustum culling off, so an automated run can measure
                       the same scene with and without it (F8 toggles it at runtime)
  --frames <count>     exit after this many frames, for automated measurement
  --report <seconds>   frame-time report interval (default 2.0, 0 disables)
  -h, --help           print this help and exit";

/// Parsed command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Args {
    /// Requested client width in pixels.
    pub width: u32,
    /// Requested client height in pixels.
    pub height: u32,
    /// Whether to open a borderless fullscreen window.
    pub fullscreen: bool,
    /// Whether to present with vsync.
    pub vsync: bool,
    /// Whether the D3D12 validation layer was explicitly requested.
    pub validation: bool,
    /// Index into [`PRESETS`] of the object count to start with.
    pub preset: usize,
    /// Whether frustum culling starts switched off.
    pub culling: bool,
    /// Whether culling is chunked across the job system.
    pub parallel_cull: bool,
    /// Frames the CPU may run ahead of the GPU.
    pub frames_in_flight: usize,
    /// Optional frame budget after which the showcase exits on its own.
    pub frames: Option<u64>,
    /// Seconds between frame-time log lines. Zero disables them.
    pub report_interval_secs: f64,
    /// Whether `--help` was requested.
    pub help: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fullscreen: false,
            vsync: false,
            validation: cfg!(debug_assertions),
            // Ten thousand objects: enough that culling rejects most of them, which is what
            // the counters in the overlay exist to show.
            preset: 3,
            culling: true,
            parallel_cull: false,
            frames_in_flight: engine_renderer::DEFAULT_FRAMES_IN_FLIGHT,
            frames: None,
            report_interval_secs: 2.0,
            help: false,
        }
    }
}

/// Why a command line was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgsError {
    /// An option that takes a value was given none.
    MissingValue(&'static str),
    /// A value could not be parsed, or was outside the option's range.
    InvalidValue {
        /// The option the value belonged to.
        option: &'static str,
        /// The text that was rejected.
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
/// Returns [`ArgsError`] for an unknown option, a missing value or an unusable one.
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
            "--fullscreen" => args.fullscreen = true,
            "--vsync" => args.vsync = true,
            "--validation" => args.validation = true,
            "--no-culling" => args.culling = false,
            "--parallel-cull" => args.parallel_cull = true,
            "--frames-in-flight" => {
                let value: usize = next_value(&mut iter, "--frames-in-flight")?;
                if !(1..=engine_renderer::MAX_FRAMES_IN_FLIGHT).contains(&value) {
                    return Err(ArgsError::InvalidValue {
                        option: "--frames-in-flight",
                        value: value.to_string(),
                    });
                }
                args.frames_in_flight = value;
            }
            "--frames" => args.frames = Some(next_value(&mut iter, "--frames")?),
            "--report" => args.report_interval_secs = next_value(&mut iter, "--report")?,
            "--preset" => {
                // One-based on the command line because the keys that select the presets
                // are F3 to F7 and nobody counts function keys from zero.
                let number: usize = next_value(&mut iter, "--preset")?;
                if !(1..=PRESETS.len()).contains(&number) {
                    return Err(ArgsError::InvalidValue {
                        option: "--preset",
                        value: number.to_string(),
                    });
                }
                args.preset = number - 1;
            }
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
mod tests {
    use super::*;

    fn parsed(arguments: &[&str]) -> Args {
        match parse(arguments) {
            Ok(args) => args,
            Err(error) => panic!("a well-formed command line is valid: {error}"),
        }
    }

    #[test]
    fn the_default_is_1080p_without_vsync() {
        let args = parsed(&[]);
        assert_eq!((args.width, args.height), (1920, 1080));
        assert!(
            !args.vsync,
            "vsync would cap the measurement at the refresh rate"
        );
        assert_eq!(args.frames, None);
    }

    #[test]
    fn the_default_preset_has_enough_objects_for_culling_to_matter() {
        assert_eq!(PRESETS[parsed(&[]).preset], 10_000);
    }

    #[test]
    fn options_override_the_fields_they_name() {
        let args = parsed(&[
            "--width",
            "1280",
            "--height",
            "720",
            "--vsync",
            "--fullscreen",
            "--validation",
            "--frames",
            "500",
            "--report",
            "0.5",
        ]);
        assert_eq!((args.width, args.height), (1280, 720));
        assert!(args.vsync && args.fullscreen && args.validation);
        assert!(
            args.culling,
            "culling is on unless --no-culling says otherwise"
        );
        assert!(!parsed(&["--no-culling"]).culling);
        assert_eq!(args.frames, Some(500));
        assert!((args.report_interval_secs - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn presets_are_one_based_on_the_command_line() {
        assert_eq!(parsed(&["--preset", "1"]).preset, 0);
        assert_eq!(parsed(&["--preset", "5"]).preset, PRESETS.len() - 1);
    }

    #[test]
    fn an_out_of_range_preset_is_rejected_rather_than_clamped() {
        // Clamping would silently measure a different scene than the one asked for.
        assert_eq!(
            parse(["--preset", "0"]),
            Err(ArgsError::InvalidValue {
                option: "--preset",
                value: "0".to_owned(),
            })
        );
        assert!(parse(["--preset", "6"]).is_err());
    }

    #[test]
    fn help_is_recognised_in_both_spellings() {
        assert!(parsed(&["--help"]).help);
        assert!(parsed(&["-h"]).help);
    }

    #[test]
    fn rejects_a_missing_value_an_unparsable_one_and_an_unknown_option() {
        assert_eq!(parse(["--width"]), Err(ArgsError::MissingValue("--width")));
        assert_eq!(
            parse(["--frames", "many"]),
            Err(ArgsError::InvalidValue {
                option: "--frames",
                value: "many".to_owned(),
            })
        );
        assert_eq!(
            parse(["--ultra"]),
            Err(ArgsError::Unknown("--ultra".to_owned()))
        );
    }
}
