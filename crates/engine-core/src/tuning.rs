//! **The one file to edit when you want to change how the game feels.**
//!
//! Every default the player, the character and the interface are built from, in one place,
//! as plain constants. Nothing here has any logic: the structs that own these numbers
//! (`MovementConfig`, `CharacterConfig`, `SimConfig`, `GameSettings`, `NameplateConfig` …)
//! read them in their `Default` implementations, so changing a value here changes the game
//! everywhere it is used, including in the tests that assert against the defaults.
//!
//! ```text
//! tuning::movement::MAX_SPEED ──▶ MovementConfig::default() ──▶ Simulation
//! tuning::player::HEALTH_REGEN_PER_SECOND ──▶ SimConfig::default() ──▶ regeneration
//! tuning::camera::SENSITIVITY_DEFAULT ──▶ GameSettings::default() ──▶ settings.cfg
//! ```
//!
//! # What is here, and what is deliberately not
//!
//! Here: the player, how they move, what they collide as, the simulation's own knobs, and the
//! default value and bounds of everything in `settings.cfg`.
//!
//! Not here, and not by accident:
//!
//! - **Content.** The enemy archetype table (`engine_game::enemy::ARCHETYPES`), the enemy
//!   attack table (`ENEMY_ABILITIES`), the FireMage's abilities and the talent tree are
//!   *data describing specific things*, not defaults. A Brute's health belongs beside the
//!   Brute, and a table with one row per monster does not become more editable by being moved
//!   into a file with the jump height in it.
//! - **Layout and presentation metrics.** HUD pixel sizes, panel geometry, colours, renderer
//!   limits and audio mix curves live beside the code that draws or plays with them, where
//!   they can be read next to the thing they describe.
//!
//! The dividing line is: *would a designer change this to tune the game?* Those numbers are
//! here. *Does this describe one particular monster, spell or widget?* That stays where it is.
//!
//! # Constants, not a file
//!
//! These are compile-time values, so they cost nothing at runtime and cannot be missing or
//! malformed. The subset a **player** is allowed to change while the game runs — sensitivity,
//! interface size, volumes, nameplates, health regeneration — is loaded from `settings.cfg` by
//! `engine-settings`, and the values below are what that file falls back to. There is exactly
//! one definition of each default; the settings crate re-exports these rather than restating
//! them.

/// The player character: what they are made of.
pub mod player {
    /// Health the player spawns with.
    ///
    /// Every enemy attack in the game is tuned against this number: the caster's blast is a
    /// fifth of it, and the boss's Immolation Nova is a quarter.
    pub const MAX_HEALTH: f32 = 500.0;

    /// Horizontal radius of the player's body, in metres. Used by area targeting.
    pub const RADIUS: f32 = 0.5;

    /// Height of the player's body, in metres.
    pub const HEIGHT: f32 = 1.8;

    /// Health restored per second out of combat, as a fraction of maximum health.
    ///
    /// One percent a second: a full heal takes a hundred seconds, which is a walk between
    /// pulls rather than a reason to sit down. Regeneration stops the moment anything engages
    /// — see `Simulation::tick_regeneration` — so it never competes with an enemy's damage.
    ///
    /// The player can override this in `settings.cfg`; this is what a fresh install runs at.
    pub const HEALTH_REGEN_PER_SECOND: f32 = 0.01;

    /// Least regeneration `settings.cfg` accepts, as a percentage. Zero is "none at all".
    pub const HEALTH_REGEN_PERCENT_MIN: f32 = 0.0;

    /// Most regeneration `settings.cfg` accepts, as a percentage a second.
    ///
    /// Twenty-five percent a second is a full heal in four seconds — past it regeneration
    /// outpaces the roster's damage and the fight stops being one.
    pub const HEALTH_REGEN_PERCENT_MAX: f32 = 25.0;

    /// Granularity of the regeneration setting, as a percentage.
    pub const HEALTH_REGEN_PERCENT_STEP: f32 = 0.1;
}

/// How the player's body moves.
pub mod movement {
    /// Top horizontal speed, metres per second.
    ///
    /// Every enemy in the roster is slower than this in every phase, which is what makes
    /// kiting possible and what makes a telegraph escapable.
    pub const MAX_SPEED: f32 = 7.0;

    /// Fraction of top speed the player keeps moving straight backwards while the camera
    /// holds its facing. The genre's convention: backing away is slower than running away,
    /// so turning to flee is a choice with a cost. A diagonal backwards pays part of it and a
    /// sidestep none.
    pub const BACKPEDAL_SPEED: f32 = 0.7;

    /// How fast the body reaches its intended velocity on the ground, m/s².
    pub const ACCELERATION: f32 = 60.0;

    /// How fast it stops when there is no intent, m/s².
    pub const DECELERATION: f32 = 50.0;

    /// Fraction of ground acceleration available while airborne.
    pub const AIR_CONTROL: f32 = 0.35;

    /// Downward acceleration, m/s². Larger than 9.81: real gravity makes a jump that reaches
    /// a useful height feel like it happens underwater.
    pub const GRAVITY: f32 = 25.0;

    /// Upward speed a jump starts with, m/s.
    ///
    /// 8 m/s against 25 m/s² of gravity is a 1.28 m jump lasting 0.64 s: high enough to read
    /// as a jump, short enough that it does not interrupt combat.
    pub const JUMP_SPEED: f32 = 8.0;

    /// Height of the floor the body stands on, in metres.
    pub const GROUND_HEIGHT: f32 = 0.0;

    /// Half-width of the square arena the body is confined to, in metres.
    pub const ARENA_HALF_EXTENT: f32 = 40.0;

    /// Terminal downward speed, m/s. Bounds the integration step of a long fall.
    pub const TERMINAL_VELOCITY: f32 = 60.0;
}

/// The capsule that walks into things. Collision, not the character's mesh.
pub mod character {
    /// Capsule radius, in metres.
    pub const RADIUS: f32 = 0.4;

    /// Total capsule height, feet to crown, in metres.
    pub const HEIGHT: f32 = 1.8;

    /// The tallest obstacle walked over without jumping, in metres: a curb, a low platform
    /// edge, one stair tread.
    pub const MAX_STEP_HEIGHT: f32 = 0.45;

    /// The steepest surface that can be stood on, in degrees from horizontal.
    ///
    /// Comfortably above the 45-degree ramp a level author will build and comfortably below
    /// a wall.
    pub const MAX_SLOPE_DEGREES: f32 = 50.0;

    /// How far outside a surface the character is kept, in metres.
    pub const SKIN_WIDTH: f32 = 0.005;

    /// How far below the feet the controller looks for ground it has just walked off.
    pub const GROUND_SNAP_DISTANCE: f32 = 0.30;

    /// How many push-out iterations one resolution is allowed.
    pub const MAX_RESOLUTION_ITERATIONS: u32 = 6;
}

/// The simulation's own knobs.
pub mod simulation {
    /// Seed for every chance the class rolls, so a fight can be replayed exactly.
    pub const RNG_SEED: u64 = 0xF12E_5EED;

    /// How long a refusal — "not enough Heat", "no target" — stays on screen, in seconds.
    pub const NOTIFICATION_SECONDS: f64 = 5.0;
}

/// Camera sensitivity, as a multiple of the camera's tuned rate.
pub mod camera {
    /// Slowest the slider offers.
    pub const SENSITIVITY_MIN: f32 = 0.10;

    /// Fastest the slider offers. Already faster than a player can follow; it exists so the
    /// range covers a high-DPI mouse turned down in its own driver.
    pub const SENSITIVITY_MAX: f32 = 5.00;

    /// What a fresh install turns at. One is exactly the rate the camera is tuned for.
    pub const SENSITIVITY_DEFAULT: f32 = 1.00;

    /// Granularity of the sensitivity slider.
    pub const SENSITIVITY_STEP: f32 = 0.05;

    /// Radius of the sphere the camera is swept as when it keeps out of walls, in metres.
    ///
    /// Not a point: the near plane has width, and a point that stops exactly on a wall still
    /// shows the wall's inside at the corners of the screen. A quarter metre covers the near
    /// plane at any field of view the game uses with room to spare.
    pub const COLLISION_RADIUS: f32 = 0.25;

    /// Extra gap kept between the camera's sphere and whatever it stopped at, in metres.
    pub const COLLISION_PADDING: f32 = 0.10;

    /// Closest the camera is pulled in towards the character before it stops, in metres.
    ///
    /// The camera never goes nearer than this when there is room for it here — and when the
    /// character is so boxed in that there is not, it stops where its centre still has the
    /// wall in front of it rather than going through.
    pub const COLLISION_MIN_DISTANCE: f32 = 0.30;

    /// How quickly the camera returns to its zoom once a wall stops being in the way, in
    /// e-folds per second.
    ///
    /// Moving in is instant — a camera that eased into a wall would be inside it for the
    /// easing. Moving back out is not, because a camera that snaps back the frame a pillar
    /// passes reads as a cut.
    pub const COLLISION_RECOVERY_RATE: f32 = 5.0;

    /// Gap between the eye and the character's body, in metres, below which the character
    /// is fully faded out. A camera this close is looking from inside the model.
    pub const CHARACTER_FADE_HIDDEN: f32 = 0.15;

    /// Gap between the eye and the character's body, in metres, from which the character is
    /// drawn whole. The closest zoom leaves about two metres, so only a camera a wall has
    /// pushed in ever fades the character.
    pub const CHARACTER_FADE_WHOLE: f32 = 1.0;

    /// How quickly the character's fade follows the camera, in e-folds per second. Fast, but
    /// not instant: the camera pulls in at once when a wall appears, and a character that
    /// vanished in the same frame would read as a flicker.
    pub const CHARACTER_FADE_RATE: f32 = 12.0;

    /// Distance from the eye, in metres, at and below which a see-through object (decoration
    /// the camera does not collide with, a monster) is not drawn at all.
    pub const NEAR_FADE_START: f32 = 0.2;

    /// Distance from the eye, in metres, from which a see-through object is drawn whole.
    pub const NEAR_FADE_END: f32 = 1.0;

    /// Radius, at the character, of the tube between the eye and the character inside which
    /// see-through objects fade, in metres. Wide enough to clear the character's shoulders.
    pub const OCCLUDER_FADE_RADIUS: f32 = 0.9;

    /// How much of a see-through object standing in that tube is still drawn.
    pub const OCCLUDER_MIN_OPACITY: f32 = 0.3;
}

/// How big the interface is drawn, as a multiple of its design size.
pub mod interface {
    /// Smallest the slider offers. Below it the 5x7 font's glyph scale rounds down to one.
    pub const SCALE_MIN: f32 = 0.75;

    /// Largest the slider offers. Twice the design size, which is what a 4K display wants.
    pub const SCALE_MAX: f32 = 2.00;

    /// What a fresh install draws at.
    pub const SCALE_DEFAULT: f32 = 1.00;

    /// Granularity of the interface-size slider.
    pub const SCALE_STEP: f32 = 0.05;
}

/// The mix.
pub mod audio {
    /// Silence.
    pub const VOLUME_MIN: f32 = 0.00;

    /// As loud as the mix was authored. Above it would be amplification, which belongs to a
    /// limiter rather than to a slider.
    pub const VOLUME_MAX: f32 = 1.00;

    /// Granularity of a volume slider.
    pub const VOLUME_STEP: f32 = 0.05;

    /// How loud a fresh install is overall.
    ///
    /// Not 1.0: a game that opens at full output on a machine whose system volume is already
    /// up is the first impression nobody wants, and 0.8 leaves room to turn *up*.
    pub const MASTER_DEFAULT: f32 = 0.80;

    /// How loud combat is, relative to the master.
    pub const SFX_DEFAULT: f32 = 1.00;

    /// How loud the interface is. Below the world, because a button press is confirmation and
    /// not an event.
    pub const UI_DEFAULT: f32 = 0.70;

    /// How loud the bed under the arena is. Well below everything, because it never stops.
    pub const AMBIENT_DEFAULT: f32 = 0.50;
}

/// Nameplates: what the player sets, and what the layout does with it.
///
/// The first six are in the units the settings screen prints; `engine-settings` converts them.
/// See `nameplates.md`.
pub mod nameplates {
    /// Base plate size. Eleven is the size the interface was designed at.
    pub const SIZE_DEFAULT: f32 = 11.0;

    /// Smallest plate the slider offers.
    pub const SIZE_MIN: f32 = 6.0;

    /// Largest plate the slider offers.
    pub const SIZE_MAX: f32 = 24.0;

    /// Granularity of the size slider. Whole numbers, as the screen prints them.
    pub const SIZE_STEP: f32 = 1.0;

    /// How far above the head a plate floats, in height units.
    pub const HEIGHT_DEFAULT: f32 = 4.0;

    /// Closest to the head a plate may be anchored.
    pub const HEIGHT_MIN: f32 = 0.0;

    /// Furthest above the head a plate may be anchored.
    pub const HEIGHT_MAX: f32 = 20.0;

    /// Granularity of the height slider.
    pub const HEIGHT_STEP: f32 = 1.0;

    /// Metres one unit of height is worth.
    ///
    /// Chosen so the default of 4 is the 0.90 m the plate already floated at.
    pub const HEIGHT_UNIT_METRES: f32 = 0.225;

    /// How visible a non-target plate is during a fight, as a percentage.
    ///
    /// 65 since the redesign: a non-target plate is now a name over a real health bar, and at
    /// 40 per cent its bar sank into the floor — dimmed enough to say "not your target", no
    /// longer bright enough to read how hurt the rest of the pack is.
    pub const OPACITY_DEFAULT: f32 = 65.0;

    /// The non-target opacity every settings file carried before the redesign.
    pub const LEGACY_OPACITY_DEFAULT: f32 = 40.0;

    /// Most transparent a non-target plate goes.
    pub const OPACITY_MIN: f32 = 0.0;

    /// Fully opaque.
    pub const OPACITY_MAX: f32 = 100.0;

    /// Granularity of the opacity slider, in percent.
    pub const OPACITY_STEP: f32 = 5.0;

    /// Minimum vertical gap between two plates, as a multiple of a plate's height.
    ///
    /// 0.16 since the nameplate became a name over a real health bar: the plate is nearly
    /// twice as tall as the one-line text box it replaced, and the old 0.93 turned a pack of
    /// five into a column a third of the screen tall. A settings file still carrying the old
    /// default is moved onto this one when it loads; see `engine_settings::nameplate`.
    pub const VERTICAL_MARGIN_DEFAULT: f32 = 0.16;

    /// The vertical margin every settings file carried before the redesign.
    pub const LEGACY_VERTICAL_MARGIN_DEFAULT: f32 = 0.93;

    /// Minimum horizontal gap between two plates, as a multiple of a plate's width.
    ///
    /// 0.08 for the same reason the vertical margin is 0.16: the plate is wider than it was.
    pub const HORIZONTAL_MARGIN_DEFAULT: f32 = 0.08;

    /// The horizontal margin every settings file carried before the redesign.
    pub const LEGACY_HORIZONTAL_MARGIN_DEFAULT: f32 = 0.63;

    /// Least separation the layout enforces: plates may touch.
    pub const MARGIN_MIN: f32 = 0.0;

    /// Most separation the layout enforces, as a multiple of a plate's own size.
    pub const MARGIN_MAX: f32 = 3.0;

    /// Granularity of both margin sliders.
    pub const MARGIN_STEP: f32 = 0.01;

    /// How much of the top of the screen plates keep out of, as a fraction of its height.
    pub const SAFE_ZONE_DEFAULT: f32 = 0.07;

    /// Smallest top safe zone: none at all.
    pub const SAFE_ZONE_MIN: f32 = 0.0;

    /// Largest top safe zone. Half the screen.
    pub const SAFE_ZONE_MAX: f32 = 0.50;

    /// Granularity of the safe-zone slider.
    pub const SAFE_ZONE_STEP: f32 = 0.01;

    /// How far an enemy may be from the player and still have a plate, in metres.
    pub const MAX_DISTANCE_METRES: f32 = 40.0;

    /// The nearest the display distance can be set, in metres.
    pub const MAX_DISTANCE_MIN: f32 = 10.0;

    /// The furthest the display distance can be set, in metres.
    pub const MAX_DISTANCE_MAX: f32 = 80.0;

    /// How many plates are drawn at once. The nearest survive.
    pub const MAX_PLATES: usize = 20;

    /// How far a plate may be pushed from its monster, in plate heights.
    pub const MAX_DISPLACEMENT: f32 = 10.0;

    /// How many separation passes the layout runs per frame.
    ///
    /// Sixteen since the margins became tight enough for the redesigned plate: a small margin
    /// pushes each pair apart by exactly its overlap, so a chain of eight plates on one point
    /// takes more passes to settle than the old, overshooting margins did. A frame with
    /// nothing overlapping still exits on the first pass.
    pub const LAYOUT_ITERATIONS: usize = 16;

    /// How fast a plate slides towards its resolved position, in fraction per second.
    pub const SMOOTHING_PER_SECOND: f32 = 40.0;
}

/// Floating combat text: how big it is drawn relative to its design size.
pub mod combat_text {
    /// Text size on a fresh install, as a percentage of the design size.
    pub const SIZE_PERCENT_DEFAULT: f32 = 100.0;

    /// Smallest the text can be set, as a percentage.
    pub const SIZE_PERCENT_MIN: f32 = 50.0;

    /// Largest the text can be set, as a percentage.
    pub const SIZE_PERCENT_MAX: f32 = 200.0;

    /// Granularity of the size, in percent.
    pub const SIZE_PERCENT_STEP: f32 = 5.0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_range_contains_its_own_default() {
        // The one mistake this file makes possible: a default edited past the bounds beside
        // it, which every loader would then silently replace with... itself.
        for (name, minimum, default, maximum) in [
            (
                "camera sensitivity",
                camera::SENSITIVITY_MIN,
                camera::SENSITIVITY_DEFAULT,
                camera::SENSITIVITY_MAX,
            ),
            (
                "interface scale",
                interface::SCALE_MIN,
                interface::SCALE_DEFAULT,
                interface::SCALE_MAX,
            ),
            (
                "master volume",
                audio::VOLUME_MIN,
                audio::MASTER_DEFAULT,
                audio::VOLUME_MAX,
            ),
            (
                "nameplate size",
                nameplates::SIZE_MIN,
                nameplates::SIZE_DEFAULT,
                nameplates::SIZE_MAX,
            ),
            (
                "nameplate height",
                nameplates::HEIGHT_MIN,
                nameplates::HEIGHT_DEFAULT,
                nameplates::HEIGHT_MAX,
            ),
            (
                "nameplate opacity",
                nameplates::OPACITY_MIN,
                nameplates::OPACITY_DEFAULT,
                nameplates::OPACITY_MAX,
            ),
            (
                "vertical margin",
                nameplates::MARGIN_MIN,
                nameplates::VERTICAL_MARGIN_DEFAULT,
                nameplates::MARGIN_MAX,
            ),
            (
                "top safe zone",
                nameplates::SAFE_ZONE_MIN,
                nameplates::SAFE_ZONE_DEFAULT,
                nameplates::SAFE_ZONE_MAX,
            ),
            (
                "health regeneration",
                player::HEALTH_REGEN_PERCENT_MIN,
                player::HEALTH_REGEN_PER_SECOND * 100.0,
                player::HEALTH_REGEN_PERCENT_MAX,
            ),
        ] {
            assert!(minimum <= default, "{name}: default is below its minimum");
            assert!(default <= maximum, "{name}: default is above its maximum");
        }
    }

    #[test]
    fn the_numbers_the_game_depends_on_are_sane() {
        // Compile-time, because every value here is: an edit that broke one of these would
        // fail the build rather than a test run.
        const {
            assert!(player::MAX_HEALTH > 0.0);
            assert!(movement::MAX_SPEED > 0.0 && movement::GRAVITY > 0.0);
            // A character that can step over itself is not walking.
            assert!(character::MAX_STEP_HEIGHT < character::HEIGHT);
            // Otherwise the nameplate layout would do nothing at all.
            assert!(nameplates::MAX_PLATES > 0 && nameplates::LAYOUT_ITERATIONS > 0);
        }
        // A jump has to clear the step height, or stepping up makes jumping pointless.
        let jump_height = movement::JUMP_SPEED * movement::JUMP_SPEED / (2.0 * movement::GRAVITY);
        assert!(jump_height > character::MAX_STEP_HEIGHT, "{jump_height} m");
    }
}
