//! Command-line arguments for the game executable.
//!
//! Pure parsing, no I/O, so every rule below is unit-tested.
//!
//! Arguments are *overrides* on top of [`EngineSettings::default`], not a second source of
//! truth. Parsing produces settings; the engine and its subsystems read only those.

use std::fmt;
use std::str::FromStr;

use engine_core::config::{DisplayMode, EngineSettings};
use engine_renderer::ShadowSettings;
use engine_renderer::shadow::PcfKernel;

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
    /// The encounter test bed: a fight against enemies that fight back.
    ///
    /// Separate from the combat scene because a training dummy and an enemy answer
    /// different questions: the dummy scene is where a rotation is practised against a
    /// target that never moves, and this is where the fight is tested.
    Encounter,
    /// The line-of-sight test bed: three domes, six dummies standing on two of them, and
    /// two pits to drop into.
    ///
    /// Separate from the combat scene because the question it asks is about a *rule* rather
    /// than about a fight: which of a near slope, a crest and a far slope a targeted ability
    /// will refuse to reach. The arena is flat and has nothing in it a spell can be stopped
    /// by, so the rule cannot be exercised there at all — and a course full of test geometry
    /// is not something to put in front of a player practising a rotation.
    LineOfSight,
    /// The first dungeon: five rooms, eight packs and a three-phase boss, played from the
    /// entrance to the boss's death without anything in between being loaded separately.
    ///
    /// Separate from the encounter test bed for the reason every scene here is separate:
    /// the encounter bed answers "does one fight work", and this answers "does a sequence of
    /// them hold together" — which is a question about pulls, resets, flow and pacing that a
    /// single arena cannot ask.
    Dungeon,
    /// The shadow test course: six bays on flat ground, one per shadow failure mode.
    ///
    /// Separate from every other scene because shadows are judged against things the other
    /// scenes deliberately do not have: empty flat ground with one caster on it, a hundred
    /// identical casters in a grid, a line of geometry crossing every cascade split, and
    /// more lights asking for shadow maps than any preset will grant. See
    /// [`engine_environment::shadow_course`] for what stands in each bay and why.
    Shadows,
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
            Self::Encounter => "ENCOUNTER",
            Self::LineOfSight => "LINE OF SIGHT",
            Self::Dungeon => "DUNGEON",
            Self::Shadows => "SHADOWS",
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
        matches!(
            self,
            Self::Combat | Self::Environment | Self::Dungeon | Self::LineOfSight | Self::Shadows
        )
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

    /// Whether this is the encounter test bed.
    #[must_use]
    pub fn is_encounter(self) -> bool {
        matches!(self, Self::Encounter)
    }

    /// Whether this is the dungeon.
    #[must_use]
    pub fn is_dungeon(self) -> bool {
        matches!(self, Self::Dungeon)
    }

    /// Whether this is the line-of-sight test bed.
    #[must_use]
    pub fn is_line_of_sight(self) -> bool {
        matches!(self, Self::LineOfSight)
    }

    /// Whether this is the shadow test course.
    #[must_use]
    pub fn is_shadows(self) -> bool {
        matches!(self, Self::Shadows)
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
            "encounter" | "ai" | "enemies" | "ENCOUNTER" => Ok(Self::Encounter),
            "dungeon" | "reliquary" | "DUNGEON" => Ok(Self::Dungeon),
            "los" | "line-of-sight" | "sight" | "LOS" => Ok(Self::LineOfSight),
            "shadows" | "shadow" | "SHADOWS" => Ok(Self::Shadows),
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
  --lights <count>       extra local lights spread over the scene's floor, as a lighting
                         stress control (default 0). None of them casts shadows: this
                         measures what shading many lights costs, not what shadowing them
                         does.
  --shadow-tweak <list>  override individual shadow settings on top of the preset, as a
                         comma-separated key=value list: cascades, res, distance, pcf
                         (single|3x3|5x5|poisson4|poisson6|poisson8), contact,
                         contact-steps, contact-length, contact-distance,
                         contact-divisor, local, local-views, local-lights, clusters. A
                         measurement control like --shadows: never written back.
  --warmup <count>       discard the first this many frames from the statistics window
                         (default 0). A measurement control: driver warm-up, shader
                         creation and the first upload all land in the opening frames,
                         and a p95 that includes them describes the load, not the frame.
  --report <seconds>     frame-time report interval (default 1.0, 0 disables)
  --overlay              start with the debug overlay visible (F5 toggles it at any
                         time; hidden by default because it costs frame time)
  --dummies <count>      training dummies in the group, on top of the single dummy
                         (default 4, combat scene only)
  --scene <name>         which scenario to load: combat (default), collision,
                         character, environment, vfx, audio, encounter, los,
                         dungeon or shadows. The shadows scene is the shadow test
                         course: six bays on flat ground - one isolated caster, a
                         hundred crates in a grid, a line of rocks crossing every
                         cascade split, alpha-tested foliage, spot lights and point
                         lights - built so each way shadows can go wrong is visible
                         on its own. Pair it with --shadows to compare two quality
                         levels of the same view. The los scene is the line-of-sight course: three
                         domes with a dummy on the near slope, the crest and the far
                         slope of two of them, and two pits to drop into - one place
                         to stand and see which of the three a targeted spell refuses.
                         The dungeon scene is the first complete dungeon: five rooms,
                         eight packs and a three-phase boss, walked from the entrance to
                         the boss with BACKSPACE to put the whole thing back. The encounter
                         scene is the enemy test bed: enemies that notice you, chase you,
                         telegraph their attacks and can kill you, with BACKSPACE to put
                         the fight back. The audio scene is the
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
  --enemies <count>      enemies in the encounter scene (default: its own two, a melee
                         enemy and a caster). Any other number builds a pack of that
                         many, alternating the two kinds, for the stress scenarios
  --no-audio             open no audio device. A measurement control, exactly as --no-ui
                         is: the runtime still runs, still limits and still counts, so
                         the difference between two runs is the mix and the device
  --no-ui                do not draw the game UI. A measurement control: it makes the
                         interface's cost the difference between two runs of the same
                         binary rather than of two builds
  --scene-file <path>    load this scene file instead of the scene's built-in layout.
                         Written by the in-game editor (F6, in a build made with
                         --features editor) and readable by any build
  --shadows <level>      shadow quality for this run: off, low, medium, high or ultra.
                         Overrides settings.cfg without writing to it, which is what
                         makes 'the same scene at two quality levels' one binary and
                         two command lines rather than an edit between runs
  --layout <path.glb>    load a Blender layout export instead of the built-in layout.
                         Only node *names* and transforms are read: name each object
                         after the asset it stands for, as in 'pillar.001', and the
                         engine keeps saying what a pillar looks like
  -h, --help             print this help and exit";


/// Individual shadow settings named on the command line, applied on top of a preset.
///
/// A measurement control, in the same family as `--shadows`: a preset is what a player
/// chooses, and a benchmark that wants to know what one *field* costs has to be able to move
/// that field without inventing a sixth quality level for it. Nothing here is written back to
/// the settings file, and an empty override set leaves the preset exactly as it is.
///
/// ```text
/// --shadow-tweak pcf=poisson6,contact-steps=8,contact-length=0.15
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ShadowOverrides {
    /// Cascade count.
    pub cascades: Option<usize>,
    /// Cascade shadow-map side, in texels.
    pub resolution: Option<u32>,
    /// The last cascade's far distance, in metres. The nearer splits keep their ratios.
    pub distance: Option<f32>,
    /// The filter kernel.
    pub pcf: Option<PcfKernel>,
    /// Whether contact shadows run.
    pub contact: Option<bool>,
    /// Steps in the contact-shadow march.
    pub contact_steps: Option<u32>,
    /// Length of the contact-shadow ray, in metres.
    pub contact_length: Option<f32>,
    /// Distance past which contact shadows are skipped, in metres.
    pub contact_distance: Option<f32>,
    /// Denominator of the contact-shadow resolution: 1 is full, 2 is half on each axis.
    pub contact_divisor: Option<u32>,
    /// Whether local lights may cast shadows.
    pub local: Option<bool>,
    /// Local shadow view budget.
    pub local_views: Option<usize>,
    /// Whether the clustered light grid is used at all.
    ///
    /// Off makes the shader walk the whole light list, which is what it did before the grid
    /// existed. A measurement control: "what does the culling buy?" is two runs of one
    /// binary, and the alternative is comparing against a differently built executable.
    pub clusters: Option<bool>,
    /// Whether local lights are shaded at all.
    ///
    /// Not a shadow setting - it switches off the lights themselves - but it belongs to the
    /// same measurement question ("what does this cost?") and shares its parser.
    pub local_lights: Option<bool>,
}

impl ShadowOverrides {
    /// Parses one `key=value,key=value` list, adding to what is already set.
    ///
    /// # Errors
    ///
    /// Returns [`ArgsError::InvalidValue`] for an unknown key or an unparseable value, so a
    /// typo in a benchmark command is a message rather than a run that silently measured the
    /// preset.
    pub fn parse_into(&mut self, list: &str) -> Result<(), ArgsError> {
        for entry in list.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let invalid = || ArgsError::InvalidValue {
                option: "--shadow-tweak",
                value: entry.to_owned(),
            };
            let (key, value) = entry.split_once('=').ok_or_else(invalid)?;
            let boolean = |v: &str| match v {
                "on" | "true" | "1" => Some(true),
                "off" | "false" | "0" => Some(false),
                _ => None,
            };
            match key.trim() {
                "cascades" => self.cascades = Some(value.parse().map_err(|_| invalid())?),
                "res" | "resolution" => {
                    self.resolution = Some(value.parse().map_err(|_| invalid())?);
                }
                "distance" => self.distance = Some(value.parse().map_err(|_| invalid())?),
                "pcf" => {
                    self.pcf = Some(match value {
                        "1" | "single" => PcfKernel::Single,
                        "3x3" => PcfKernel::Tap3x3,
                        "5x5" => PcfKernel::Tap5x5,
                        "poisson4" => PcfKernel::Poisson4,
                        "poisson6" => PcfKernel::Poisson6,
                        "poisson8" => PcfKernel::Poisson8,
                        _ => return Err(invalid()),
                    });
                }
                "contact" => self.contact = Some(boolean(value).ok_or_else(invalid)?),
                "contact-steps" => {
                    self.contact_steps = Some(value.parse().map_err(|_| invalid())?);
                }
                "contact-length" => {
                    self.contact_length = Some(value.parse().map_err(|_| invalid())?);
                }
                "contact-distance" => {
                    self.contact_distance = Some(value.parse().map_err(|_| invalid())?);
                }
                "contact-divisor" | "contact-scale" => {
                    self.contact_divisor = Some(value.parse().map_err(|_| invalid())?);
                }
                "local" => self.local = Some(boolean(value).ok_or_else(invalid)?),
                "local-views" => self.local_views = Some(value.parse().map_err(|_| invalid())?),
                "local-lights" => self.local_lights = Some(boolean(value).ok_or_else(invalid)?),
                "clusters" => self.clusters = Some(boolean(value).ok_or_else(invalid)?),
                _ => return Err(invalid()),
            }
        }
        Ok(())
    }

    /// Applies whatever was named on top of `settings`, and returns it validated.
    #[must_use]
    pub fn apply(&self, mut settings: ShadowSettings) -> ShadowSettings {
        if let Some(count) = self.cascades {
            settings.cascade_count = count;
        }
        if let Some(resolution) = self.resolution {
            settings.resolution = resolution;
        }
        if let Some(distance) = self.distance {
            settings = settings.with_max_distance(distance);
        }
        if let Some(pcf) = self.pcf {
            settings.pcf = pcf;
        }
        if let Some(enabled) = self.contact {
            settings.contact.enabled = enabled;
        }
        if let Some(steps) = self.contact_steps {
            settings.contact.steps = steps;
        }
        if let Some(length) = self.contact_length {
            settings.contact.length = length;
        }
        if let Some(distance) = self.contact_distance {
            settings.contact.max_distance = distance;
        }
        if let Some(divisor) = self.contact_divisor {
            settings.contact.divisor = divisor;
        }
        if let Some(enabled) = self.local {
            settings.local.enabled = enabled;
        }
        if let Some(views) = self.local_views {
            settings.local.max_views = views;
        }
        settings.validated()
    }
}

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
    /// Extra local lights added to whichever scene loads, as a lighting stress control.
    ///
    /// Zero for an ordinary run. The arena has four brazier lights, which is not enough of
    /// them to answer "does the lighting scale?"; this is the knob that asks.
    pub stress_lights: usize,
    /// Individual shadow fields named on the command line, applied on top of the preset.
    pub shadow_overrides: ShadowOverrides,
    /// Frames whose timings are discarded before the statistics window starts.
    ///
    /// The opening frames of a run include driver warm-up, pipeline creation and the first
    /// uploads. They are part of starting the game and not part of its steady state, so a
    /// benchmark that reports a percentile over them reports the wrong thing.
    pub warmup: u64,
    /// Seconds between frame-time reports. Zero disables reporting.
    pub report_interval_secs: f64,
    /// Whether the debug overlay starts visible. It can always be toggled with F5.
    pub overlay: bool,
    /// Training dummies in the group, on top of the single dummy.
    pub dummies: usize,
    /// Which scenario to load.
    pub scene: Scene,
    /// Shadow quality for this run, overriding the settings file.
    ///
    /// `None` uses whatever the player's `settings.cfg` says, which is the ordinary case.
    /// A value here is *not* written back: it is a measurement control, and a benchmark run
    /// must not change what the player sees next time they start the game.
    pub shadows: Option<engine_settings::ShadowQuality>,
    /// Whether the game UI is drawn.
    ///
    /// A measurement control rather than a game option: with it, the interface's cost is
    /// the difference between two runs of one binary instead of two separately compiled
    /// builds, which is what made the Phase 3 nameplate numbers hard to trust.
    pub ui: bool,
    /// How many enemies the encounter scene spawns.
    ///
    /// Zero means the scene's own two: one melee enemy and one caster. Anything else builds
    /// a pack of that many, alternating the two kinds, which is what the stress scenarios
    /// use.
    pub enemies: usize,
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
    /// A scene file to load instead of the built-in layout, if one was named.
    ///
    /// What makes an authored level playable: the layout functions stay as they are, and a
    /// file written by the editor replaces the objects, the spawn points and the light.
    pub scene_file: Option<String>,
    /// A Blender layout export to load instead of the built-in layout, if one was named.
    ///
    /// The same job as [`Args::scene_file`] from the other authoring direction. Naming both
    /// is refused rather than silently preferring one.
    pub layout: Option<String>,
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
            warmup: 0,
            stress_lights: 0,
            shadow_overrides: ShadowOverrides::default(),
            shadows: None,
            report_interval_secs: 1.0,
            overlay: false,
            dummies: 4,
            scene: Scene::Combat,
            ui: true,
            fullscreen: false,
            validation: None,
            env_density: 1.0,
            vfx_stress: 0.0,
            enemies: 0,
            audio: true,
            scene_file: None,
            layout: None,
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
                Scene::Encounter => "myGame — encounter demo".to_owned(),
                Scene::Dungeon => "myGame — the Ashen Reliquary".to_owned(),
                Scene::LineOfSight => "myGame — line-of-sight course".to_owned(),
                Scene::Shadows => "myGame — shadow course".to_owned(),
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
            "--warmup" => args.warmup = next_value(&mut iter, "--warmup")?,
            "--shadow-tweak" => {
                let raw = iter.next().ok_or(ArgsError::MissingValue("--shadow-tweak"))?;
                args.shadow_overrides.parse_into(raw.as_ref())?;
            }
            "--shadows" => {
                let value: String = next_value(&mut iter, "--shadows")?;
                let Some(quality) = engine_settings::ShadowQuality::from_name(&value) else {
                    return Err(ArgsError::InvalidValue {
                        option: "--shadows",
                        value,
                    });
                };
                args.shadows = Some(quality);
            }
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
            "--enemies" => args.enemies = next_value(&mut iter, "--enemies")?,
            "--lights" => args.stress_lights = next_value(&mut iter, "--lights")?,
            "--scene-file" => {
                let raw = iter.next().ok_or(ArgsError::MissingValue("--scene-file"))?;
                args.scene_file = Some(raw.as_ref().to_owned());
            }
            "--layout" => {
                let raw = iter.next().ok_or(ArgsError::MissingValue("--layout"))?;
                args.layout = Some(raw.as_ref().to_owned());
            }
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
    fn a_scene_file_and_a_layout_are_both_optional_and_both_carry_their_path() {
        let plain = parse::<[&str; 0], &str>([]).expect("empty is valid");
        assert_eq!(plain.scene_file, None);
        assert_eq!(plain.layout, None);

        let named = parse(["--scene-file", "assets/scenes/keep.json"]).expect("valid");
        assert_eq!(named.scene_file.as_deref(), Some("assets/scenes/keep.json"));

        let layout = parse(["--layout", "assets/keep.glb"]).expect("valid");
        assert_eq!(layout.layout.as_deref(), Some("assets/keep.glb"));
    }

    #[test]
    fn a_scene_file_with_no_path_after_it_is_an_error_rather_than_an_empty_name() {
        assert!(matches!(
            parse(["--scene-file"]),
            Err(ArgsError::MissingValue("--scene-file"))
        ));
        assert!(matches!(
            parse(["--layout"]),
            Err(ArgsError::MissingValue("--layout"))
        ));
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
