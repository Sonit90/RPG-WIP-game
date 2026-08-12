//! Command-line arguments for the game executable.
//!
//! Pure parsing, no I/O, so every rule below is unit-tested.
//!
//! Arguments are *overrides* on top of [`EngineSettings::default`], not a second source of
//! truth. Parsing produces settings; the engine and its subsystems read only those.

use std::fmt;
use std::str::FromStr;

use engine_core::config::{DisplayMode, EngineSettings};

/// Which scenario the executable loads.
///
/// Independently launchable scenes, not one scene with debug modes: a collision bug has to
/// be reproducible without abilities, dummies and a damage meter in the way, test geometry
/// in the combat arena would be geometry a player trips over during a fight, and judging a
/// walk cycle should not require winning a fight to get a clear view of the character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scene {
    /// The training arena: player, dummies, abilities, combat, the game UI.
    #[default]
    Combat,
    /// The collision test course: geometry, a character, and nothing else.
    Collision,
    /// The character and model test bed: the FireMage, one dummy, a floor, and keys that
    /// hold the character in one animation so it can be judged without playing the game.
    Character,
    /// The environment test bed: the combat arena with one of every library asset laid out
    /// in a row beside it, and no dummies to fight.
    ///
    /// Separate from the combat scene for the same reason the collision course is separate
    /// from the arena: inspecting terrain, props, materials, lighting, levels of detail and
    /// culling should not require a fight going on in front of them, and a display row in
    /// the combat arena would be scenery a player trips over.
    Environment,
    /// The VFX test bed: a flat floor, a few dummies, and one key per effect in the
    /// catalogue.
    ///
    /// Separate from the combat scene for the reason every other test bed is: judging
    /// whether an explosion reads means playing it on demand, from a chosen angle, as often
    /// as it takes — not winning a fight to get one. It brings no environment, so nothing
    /// but the effect is in the picture.
    Vfx,
    /// The audio test bed: a flat floor, a few dummies, and one key per sound in the
    /// catalogue.
    ///
    /// Separate from the VFX scene for the same reason that one is separate from combat:
    /// judging whether an explosion *reads* means playing it on demand, from a chosen
    /// distance, as often as it takes — and judging whether it reads with a picture on top
    /// of it is a different question, asked later, in the combat scene.
    Audio,
}

impl Scene {
    /// Short uppercase name, for a log line and the developer overlay.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Combat => "COMBAT",
            Self::Collision => "COLLISION",
            Self::Character => "CHARACTER",
            Self::Environment => "ENVIRONMENT",
            Self::Vfx => "VFX",
            Self::Audio => "AUDIO",
        }
    }

    /// Whether this is the environment test bed.
    #[must_use]
    pub fn is_environment(self) -> bool {
        matches!(self, Self::Environment)
    }

    /// Whether this scene is built from an environment scene rather than a bare floor.
    ///
    /// The combat arena and the environment demo both are; the collision course brings its
    /// own geometry and the character test bed deliberately brings none.
    #[must_use]
    pub fn has_environment(self) -> bool {
        matches!(self, Self::Combat | Self::Environment)
    }

    /// Whether this is the character and model test bed.
    #[must_use]
    pub fn is_character(self) -> bool {
        matches!(self, Self::Character)
    }

    /// Whether this is the collision demo.
    #[must_use]
    pub fn is_collision(self) -> bool {
        matches!(self, Self::Collision)
    }

    /// Whether this is the VFX test bed.
    #[must_use]
    pub fn is_vfx(self) -> bool {
        matches!(self, Self::Vfx)
    }

    /// Whether this is the audio test bed.
    #[must_use]
    pub fn is_audio(self) -> bool {
        matches!(self, Self::Audio)
    }
}

impl FromStr for Scene {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "combat" | "arena" | "COMBAT" => Ok(Self::Combat),
            "collision" | "demo" | "COLLISION" => Ok(Self::Collision),
            "character" | "model" | "CHARACTER" => Ok(Self::Character),
            "environment" | "env" | "level" | "ENVIRONMENT" => Ok(Self::Environment),
            "vfx" | "effects" | "VFX" => Ok(Self::Vfx),
            "audio" | "sound" | "AUDIO" => Ok(Self::Audio),
            _ => Err(()),
        }
    }
}

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
                         (default 4, combat scene only)
  --scene <name>         which scenario to load: combat (default), collision,
                         character, environment, vfx or audio. The audio scene is the
                         sound test bed: a floor, a few dummies, and one key per sound
                         in the catalogue, so a sound can be judged on its own and from
                         a chosen distance. The vfx scene is the effect
                         test bed: a floor, a few dummies, and one key per effect in
                         the catalogue, so an explosion can be played on demand from a
                         chosen angle instead of being waited for in a fight. The environment scene is the arena
                         with one of every environment asset laid out beside it and no
                         fight going on: terrain, props, materials, lighting, LOD and
                         culling, with nothing in the way of looking at them. The character scene is the model test bed - the
                         FireMage, one dummy, a floor and nothing else. Keys 1 to 6 hold
                         the character in one animation, 0 releases it back to gameplay.
                         The
                         collision scene is a dedicated test course - floor, walls, a
                         corner, steps, ramps, thin barriers, a ledge - with no combat
                         in it at all. Keys 1 to 7 stand at a test station, R resets
  --env-density <mult>   scatter multiplier for the environment scene (default 1.0).
                         Above 1 loads the stress layout: the same arena boundary with
                         several times the rocks, trees and ground cover, which is the
                         configuration that answers whether the environment scales
  --vfx-stress <rate>    play <rate> catalogue effects a second around the player, for
                         measurement. Presentation only: it starts no ability and deals
                         no damage, so what it measures is what effects cost to draw
  --no-audio             open no audio device. A measurement control, exactly as --no-ui
                         is: the runtime still runs, still limits and still counts, so
                         the difference between two runs is the mix and the device
  --no-ui                do not draw the game UI. A measurement control: it makes the
                         interface's cost the difference between two runs of the same
                         binary rather than of two builds
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
    /// Which scenario to load.
    pub scene: Scene,
    /// Whether the game UI is drawn.
    ///
    /// A measurement control rather than a game option: with it, the interface's cost is
    /// the difference between two runs of one binary instead of two separately compiled
    /// builds, which is what made the Phase 3 nameplate numbers hard to trust.
    pub ui: bool,
    /// Whether an audio device is opened.
    ///
    /// A measurement control, exactly as [`Args::ui`] is: the audio runtime runs either
    /// way, so what the flag removes is the device and the mix, and the difference between
    /// two runs is what audio actually costs.
    pub audio: bool,
    /// Whether to open a borderless fullscreen window.
    pub fullscreen: bool,
    /// Whether the renderer's validation layer was explicitly requested.
    ///
    /// `None` leaves the build's default in place; `Some(true)` forces it on.
    pub validation: Option<bool>,
    /// Scatter multiplier for the environment scene.
    ///
    /// 1.0 is the shipping arena. Higher loads the stress layout, which has the same
    /// boundary and several times the scatter — the configuration that answers "does the
    /// environment scale?" rather than "is the game fast?".
    pub env_density: f32,
    /// Effects per second the stress driver plays, or zero to leave it off.
    ///
    /// A *presentation* stress: it plays catalogue effects around the player without
    /// touching gameplay, so what it measures is the cost of drawing effects rather than the
    /// cost of a fight that happens to produce them. Available in every scene, because "what
    /// do fifty simultaneous explosions cost over the arena" and "over an empty floor" are
    /// both worth knowing.
    pub vfx_stress: f32,
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
            scene: Scene::Combat,
            ui: true,
            fullscreen: false,
            validation: None,
            env_density: 1.0,
            vfx_stress: 0.0,
            audio: true,
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
            title: match self.scene {
                Scene::Combat => "myGame — training arena".to_owned(),
                Scene::Collision => "myGame — collision demo".to_owned(),
                Scene::Character => "myGame — character test".to_owned(),
                Scene::Environment => "myGame — environment demo".to_owned(),
                Scene::Vfx => "myGame — VFX demo".to_owned(),
                Scene::Audio => "myGame — audio demo".to_owned(),
            },
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
            "--scene" => {
                let raw = iter.next().ok_or(ArgsError::MissingValue("--scene"))?;
                let raw = raw.as_ref();
                args.scene = raw.parse().map_err(|()| ArgsError::InvalidValue {
                    option: "--scene",
                    value: raw.to_owned(),
                })?;
            }
            "--env-density" => args.env_density = next_value(&mut iter, "--env-density")?,
            "--vfx-stress" => args.vfx_stress = next_value(&mut iter, "--vfx-stress")?,
            "--no-audio" => args.audio = false,
            "--no-ui" => args.ui = false,
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
    fn the_scene_defaults_to_combat_and_can_be_switched() {
        assert_eq!(
            parse::<[&str; 0], &str>([]).expect("empty is valid").scene,
            Scene::Combat
        );
        assert_eq!(
            parse(["--scene", "collision"])
                .expect("--scene collision is valid")
                .scene,
            Scene::Collision
        );
        assert_eq!(
            parse(["--scene", "character"])
                .expect("--scene character is valid")
                .scene,
            Scene::Character
        );
        assert!(Scene::Collision.is_collision());
        assert!(!Scene::Combat.is_collision());
        assert_eq!(
            parse(["--scene", "environment"])
                .expect("--scene environment is valid")
                .scene,
            Scene::Environment
        );
        assert!(Scene::Character.is_character());
        assert!(!Scene::Character.is_collision());
        assert!(Scene::Environment.is_environment());
        assert_eq!(Scene::Collision.as_str(), "COLLISION");
        assert_eq!(Scene::Character.as_str(), "CHARACTER");
        assert_eq!(Scene::Environment.as_str(), "ENVIRONMENT");

        // Which scenes are built from an environment scene, and which bring their own
        // ground. A collision course with the arena's kerb in it would be a course with
        // scenery in the way of the tests.
        assert!(Scene::Combat.has_environment());
        assert!(Scene::Environment.has_environment());
        assert!(!Scene::Collision.has_environment());
        assert!(!Scene::Character.has_environment());
    }

    #[test]
    fn the_scene_name_is_in_the_window_title_so_a_screenshot_says_which_one_it_is() {
        let combat = parse::<[&str; 0], &str>([])
            .expect("empty is valid")
            .to_settings();
        let collision = parse(["--scene", "collision"])
            .expect("valid")
            .to_settings();
        assert_ne!(combat.title, collision.title);
        assert!(collision.title.contains("collision"), "{}", collision.title);
    }

    #[test]
    fn an_unknown_scene_is_rejected_by_name() {
        assert_eq!(
            parse(["--scene", "racing"]),
            Err(ArgsError::InvalidValue {
                option: "--scene",
                value: "racing".to_owned(),
            })
        );
        assert_eq!(parse(["--scene"]), Err(ArgsError::MissingValue("--scene")));
    }

    #[test]
    fn the_environment_density_defaults_to_the_shipping_arena_and_can_be_raised() {
        assert_eq!(
            parse::<[&str; 0], &str>([])
                .expect("empty is valid")
                .env_density,
            1.0
        );
        assert_eq!(
            parse(["--env-density", "4"])
                .expect("--env-density is valid")
                .env_density,
            4.0
        );
        assert_eq!(
            parse(["--env-density"]),
            Err(ArgsError::MissingValue("--env-density"))
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
