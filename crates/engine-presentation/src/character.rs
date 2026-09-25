//! The visual controller: gameplay state in, a posed and placed character out.
//!
//! ```text
//!  Simulation (authoritative)
//!        │  position, facing, speed, grounded, movement state, active cast
//!        ▼
//!  CharacterState          ← a snapshot, taken once per frame, that carries no gameplay
//!        │                   authority and is never read back
//!        ▼
//!  CharacterVisual
//!    ├── AnimationSlot     ← which role the character is in right now
//!    ├── AnimationPlayer   ← which clip, where in it, cross-fading from what
//!    ├── Skeleton          ← pose composed to a bone palette
//!    └── ModelBinding      ← which level of detail, which meshes, which materials
//!        │
//!        ▼
//!  RenderWorld's skinned draw list
//! ```
//!
//! # Gameplay is authoritative and never waits
//!
//! Nothing here can change what the simulation does. The controller reads
//! [`CharacterState`], decides what to *show*, and writes into the render world. A cast that
//! takes 1.5 s takes 1.5 s whether or not the cast animation has finished; a landing
//! animation that is still playing does not stop the character running. The one thing the
//! controller owns that gameplay does not is the landing timer, because "just landed" is a
//! cosmetic edge rather than a state the simulation has.
//!
//! # Nothing here knows what character it is driving
//!
//! Every character-specific number — which model, how tall, which clip fills which slot, how
//! fast a walk becomes a run — arrives in a [`CharacterDef`]. Adding a second playable
//! character is adding a second definition and registering its model. This file does not
//! change, and neither does anything downstream of it.

use std::sync::Arc;

use engine_assets::{
    AnimationPlayer, AnimationSlot, AssetError, CharacterDef, ClipBindings, ModelAsset, Pose,
    Skeleton,
};
use engine_core::math::{Mat4, Quat, Sphere, Vec3};
use engine_core::transform::Transform;
use engine_render_data::{InstanceStyle, ModelBinding, PartGeometry, RenderWorld, SkinnedInstance};

/// How long a cross-fade between locomotion clips takes, in seconds.
///
/// Short enough that a character that starts running looks like it started running, long
/// enough that the switch is not a pop. Locomotion clips share a rough shape, so they blend
/// well; a cast does not, which is why it gets its own shorter figure.
const LOCOMOTION_FADE: f32 = 0.16;

/// Cross-fade into and out of a cast.
///
/// Shorter than locomotion: a spell should look like it started the moment the player
/// pressed the key, and gameplay has already committed by then.
const CAST_FADE: f32 = 0.08;

/// Cross-fade into an airborne pose.
const AIR_FADE: f32 = 0.10;

/// How much the bind-pose bounds are enlarged for culling.
///
/// An animated character reaches outside the sphere its bind pose implies — an arm overhead
/// during `cast_heavy` is most of a metre past a T-pose's extent, and the staff is longer
/// than the mage is tall. Culling with the unpadded sphere pops a casting character off the
/// edge of the screen while its staff is still visible.
///
/// A single generous factor rather than a per-clip bound: computing a real animated bound
/// means posing every clip at load and taking the union, which is a build-time tool this
/// phase does not need. Over-estimating costs a draw that could have been skipped;
/// under-estimating is a visible artefact.
const ANIMATED_BOUNDS_PADDING: f32 = 1.6;

/// The furthest the lower body turns into a diagonal, in radians: 45 degrees.
///
/// Exactly the diagonal a forward-and-sideways key pair produces, so a run held on W and D
/// is a run cycle pointing where the character actually goes rather than a forward run
/// sliding sideways. Past this the heading is a strafe, which has its own clip.
const MAX_STRIDE_YAW: f32 = std::f32::consts::FRAC_PI_4;

/// How quickly the stride turn follows the keys, per second.
///
/// Exponential: about 90 % of the way in 0.16 s — the locomotion cross-fade — so the body
/// turns into the diagonal as the cycle settles, and releasing a key does not snap it back.
const STRIDE_YAW_RATE: f32 = 14.0;

/// How much of the stride turn the torso takes back, spread over the spine and chest.
///
/// The legs run along the diagonal; the chest stays mostly square to the facing, the way a
/// runner looking ahead twists at the waist. What keeps the character reading as facing
/// forward — the way it will cast — while its feet go the other way.
const TORSO_COUNTER_TWIST: f32 = 0.55;

/// How much more the neck and head take back, on top of the torso's share.
///
/// Together with [`TORSO_COUNTER_TWIST`] nearly all of it: the head looks where the camera
/// looks.
const HEAD_COUNTER_TWIST: f32 = 0.35;

/// A cast the visual layer should show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastVisual {
    /// An ordinary spell.
    Normal,
    /// Something big. Chosen from the cast's own length rather than from its identity, so a
    /// new ability gets a sensible animation without this file learning its name.
    Heavy,
}

impl CastVisual {
    /// Casts at or above this many seconds read as heavy.
    pub const HEAVY_SECONDS: f32 = 1.4;

    /// Which visual a cast of `duration` seconds deserves.
    #[must_use]
    pub fn for_duration(duration: f32) -> Self {
        if duration >= Self::HEAVY_SECONDS {
            Self::Heavy
        } else {
            Self::Normal
        }
    }

    /// The slot this cast plays in.
    #[must_use]
    pub fn slot(self) -> AnimationSlot {
        match self {
            Self::Normal => AnimationSlot::Cast,
            Self::Heavy => AnimationSlot::CastHeavy,
        }
    }
}

/// Which way the character is moving relative to the way it faces.
///
/// The camera pins a character's facing while it strafes and backpedals, so speed alone
/// cannot say which cycle to play: a character sidestepping at a run and one running
/// forwards have the same speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Heading {
    /// The way it faces, or near enough — a diagonal forwards reads as forwards.
    #[default]
    Forward,
    /// Away from the way it faces.
    Backward,
    /// Sideways, to the character's own left.
    Left,
    /// Sideways, to the character's own right.
    Right,
}

/// What the simulation says the character is doing, this frame.
///
/// A snapshot with no authority: it is built from the simulation, read by the controller,
/// and thrown away. Nothing in the visual layer writes back into it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterState {
    /// Where the **feet** are, interpolated between ticks.
    pub position: Vec3,
    /// Which way the character faces, radians about +Y. Zero faces −Z.
    pub facing_yaw: f32,
    /// Horizontal speed, metres per second.
    pub speed: f32,
    /// Which way the character is moving relative to its facing. Chooses the directional
    /// cycle a model has for it, and without one plays the forward cycle — in reverse when
    /// backpedalling, so the feet step backwards instead of moonwalking.
    pub heading: Heading,
    /// The direction of travel relative to the facing, in radians: zero straight ahead,
    /// positive to the character's right, `±pi` straight back.
    ///
    /// What turns a diagonal run into a run *along* the diagonal: see
    /// [`CharacterVisual::stride_yaw`]. Zero for anything that is not steered with its facing
    /// pinned — a monster turns its whole body to where it walks and has no diagonal.
    pub move_angle: f32,
    /// Whether the feet are on the ground.
    pub grounded: bool,
    /// Whether the character is rising rather than falling. Only read when airborne.
    pub rising: bool,
    /// How tall gameplay says the character is, in metres.
    ///
    /// The model is scaled to this. Gameplay owns the capsule; the model follows it.
    pub height: f32,
    /// A cast in progress, if there is one.
    pub cast: Option<CastVisual>,
    /// How far through the cast in progress, 0 to 1. The cast clip is placed at this
    /// fraction of its wind-up, so its release lands as gameplay completes the cast.
    pub cast_progress: f32,
    /// The tick of the latest cast this character started, from the event stream.
    ///
    /// A tick rather than a flag so a new cast is recognised however it arrives — including
    /// one queued behind the last, which starts on the tick the previous one lands and so
    /// never shows a frame without a cast — and so a frame that sees the same event twice
    /// does not restart the clip twice.
    pub cast_started: Option<u64>,
    /// The tick of the latest ability this character used — a cast completing, or an
    /// instant going off. The cast clip's follow-through plays from here.
    pub released: Option<u64>,
    /// Whether the character is dead. A model with a death clip plays it and holds its last
    /// frame; one without stands where it was, as every character did before deaths.
    pub dead: bool,
}

impl Default for CharacterState {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            facing_yaw: 0.0,
            speed: 0.0,
            heading: Heading::Forward,
            move_angle: 0.0,
            grounded: true,
            rising: false,
            height: 1.8,
            cast: None,
            cast_progress: 0.0,
            cast_started: None,
            released: None,
            dead: false,
        }
    }
}

/// One character on screen: its model, its skeleton, its animation and where it is.
pub struct CharacterVisual {
    /// The asset: skeleton and clips. Shared, because two players of the same class draw
    /// from one model.
    asset: Arc<ModelAsset>,
    /// The uploaded geometry.
    binding: ModelBinding,
    /// Which clip fills each slot, resolved once at construction.
    clips: ClipBindings,
    /// Everything character-specific.
    def: CharacterDef,
    /// Which clip is playing and where in it.
    player: AnimationPlayer,
    /// The palette, rebuilt every frame. Allocated once.
    palette: Vec<Mat4>,
    /// The role the character is currently in.
    slot: AnimationSlot,
    /// The state the last [`CharacterVisual::update`] was given.
    state: CharacterState,
    /// Seconds of landing animation left. Owned here because gameplay has no such state.
    land_remaining: f32,
    /// Whether the character was off the ground last frame, so a landing can be detected.
    was_airborne: bool,
    /// The cast that was showing last frame, so a completing cast knows which clip to
    /// follow through with.
    was_casting: Option<CastVisual>,
    /// The last [`CharacterState::cast_started`] acted on.
    last_cast_started: Option<u64>,
    /// The last [`CharacterState::released`] acted on.
    last_released: Option<u64>,
    /// The cast clip playing out after its spell has fired, until it ends or the character
    /// moves. The part of a cast gameplay has no state for, like a landing.
    follow_through: Option<AnimationSlot>,
    /// Where the level of detail was last selected, for diagnostics.
    lod: usize,
    /// A role the character is held in regardless of what it is doing.
    ///
    /// The character test scene's whole mechanism: a developer presses 3 and the mage runs
    /// on the spot, so a run cycle can be judged without having to run. Nothing in the game
    /// sets it, and it is checked *before* everything else in `choose_slot`, which is what
    /// makes it an override rather than a hint.
    forced: Option<AnimationSlot>,
    /// Whether an instant plays its cast clip from the top rather than from its release.
    ///
    /// A hero's instant is a flick — the spell is already gone, and the follow-through is
    /// all there is to show. A monster's instant is a *blow*, and a blow the player cannot
    /// see coming is exactly what its wind-up is for; see [`CharacterVisual::with_whole_instants`].
    whole_instants: bool,
    /// The lower body's turn into a diagonal, radians about +Y, smoothed. See
    /// [`CharacterVisual::stride_yaw`].
    stride_yaw: f32,
    /// The bones that take the stride turn back: `(bone, share of the turn)`, root first.
    ///
    /// Found by name at construction — `spine`/`chest` for the torso, `neck`/`head` above it,
    /// in whichever naming the model uses (`mixamorig:Spine1` and `chest` both match). A
    /// model with none of them turns whole, which is the same diagonal without the twist.
    twist_bones: Vec<(u16, f32)>,
    /// The pose actually skinned while turned: the sampled pose with the counter-twist
    /// applied. Allocated once.
    twisted: Pose,
}

impl CharacterVisual {
    /// Builds a visual for `def`, drawing `asset` through `binding`.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::Invalid`] if the model has no skeleton, or if the definition
    /// names a clip the model does not contain. Both are content errors, and both would
    /// otherwise appear as a character that stands in its bind pose forever.
    pub fn new(
        def: CharacterDef,
        asset: Arc<ModelAsset>,
        binding: ModelBinding,
    ) -> Result<Self, AssetError> {
        let Some(skeleton) = asset.skeleton.as_ref() else {
            return Err(AssetError::invalid(
                &def.name,
                "model",
                "a character needs a skeleton, and this model has none",
            ));
        };
        let clips = def.resolve(&asset)?;
        let player = AnimationPlayer::new(skeleton);
        let palette = Vec::with_capacity(skeleton.len());
        let twist_bones = twist_bones(skeleton);
        let twisted = skeleton.rest_pose();

        Ok(Self {
            asset,
            binding,
            clips,
            def,
            player,
            palette,
            slot: AnimationSlot::Idle,
            state: CharacterState::default(),
            land_remaining: 0.0,
            was_airborne: false,
            was_casting: None,
            last_cast_started: None,
            last_released: None,
            follow_through: None,
            lod: 0,
            forced: None,
            whole_instants: false,
            stride_yaw: 0.0,
            twist_bones,
            twisted,
        })
    }

    /// The same visual, playing an instant's whole cast clip — wind-up and all — rather than
    /// only what follows its release. For monsters, whose instants are blows.
    #[must_use]
    pub fn with_whole_instants(mut self) -> Self {
        self.whole_instants = true;
        self
    }

    /// The role the character is in right now.
    #[must_use]
    pub fn slot(&self) -> AnimationSlot {
        self.slot
    }

    /// The level of detail the last submission chose.
    #[must_use]
    pub fn lod(&self) -> usize {
        self.lod
    }

    /// Holds the character in one role, or releases it back to gameplay.
    ///
    /// For the character test scene. A forced slot makes the visual layer stop reflecting
    /// gameplay, which is exactly what a developer inspecting a walk cycle wants and exactly
    /// what the game must never do.
    pub fn force_slot(&mut self, slot: Option<AnimationSlot>) {
        self.forced = slot;
    }

    /// Which role is being forced, if any.
    #[must_use]
    pub fn forced_slot(&self) -> Option<AnimationSlot> {
        self.forced
    }

    /// The character definition this was built from.
    #[must_use]
    pub fn def(&self) -> &CharacterDef {
        &self.def
    }

    /// The state the last update was given.
    #[must_use]
    pub fn state(&self) -> &CharacterState {
        &self.state
    }

    /// The bone palette from the last update.
    #[must_use]
    pub fn palette(&self) -> &[Mat4] {
        &self.palette
    }

    /// Where the character is drawn, including its scale to gameplay's height.
    #[must_use]
    pub fn transform(&self) -> Transform {
        let scale = self.def.scale_for_height(self.state.height);
        Transform::from_translation(self.state.position)
            .with_rotation(Quat::from_yaw_pitch_roll(
                self.state.facing_yaw + self.stride_yaw,
                0.0,
                0.0,
            ))
            .with_scale(Vec3::splat(scale))
    }

    /// How far the body is turned off its facing into the diagonal it is running along,
    /// radians about +Y (positive turns it to its left).
    ///
    /// **Visual only.** The simulation's facing — what a cast is aimed along, what the camera
    /// pins — does not move: this turns the drawn hips and legs so the run cycle points where
    /// the character goes, and the spine, neck and head turn most of it back so the character
    /// still looks the way it faces. A forward diagonal turns towards the side it runs to; a
    /// backward diagonal turns the other way, so the backpedal steps along the diagonal
    /// behind it. Straight runs, strafes, standing, casting and being dead are all zero.
    #[must_use]
    pub fn stride_yaw(&self) -> f32 {
        self.stride_yaw
    }

    /// The stride turn `state` asks for, before smoothing.
    fn stride_target(&self, state: &CharacterState) -> f32 {
        let moving = state.speed > IDLE_SPEED
            && state.cast.is_none()
            && !state.dead
            && self.forced.is_none();
        let angle = state.move_angle;
        if !moving || !angle.is_finite() {
            return 0.0;
        }
        // Yaw grows to the character's left, the move angle to its right: a run to the
        // front right turns the body right, which is a smaller yaw.
        let turn = match state.heading {
            Heading::Forward => -angle,
            Heading::Backward => -wrap_angle(angle - std::f32::consts::PI),
            Heading::Left | Heading::Right => 0.0,
        };
        turn.clamp(-MAX_STRIDE_YAW, MAX_STRIDE_YAW)
    }

    /// Advances the character by `dt` seconds of real time under `state`.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError`] if a clip is bound to a different skeleton than the model's —
    /// which validation rejects at load, so reaching it means an asset was mutated after
    /// being validated.
    pub fn update(&mut self, dt: f32, state: CharacterState) -> Result<(), AssetError> {
        let dt = dt.max(0.0);
        // Airborne, the body keeps the turn it left the ground with: a jump along a diagonal
        // lands along it.
        if state.grounded {
            let target = self.stride_target(&state);
            let blend = 1.0 - (-STRIDE_YAW_RATE * dt).exp();
            self.stride_yaw += (target - self.stride_yaw) * blend;
            if (self.stride_yaw - target).abs() < 1e-4 {
                self.stride_yaw = target;
            }
        }
        self.state = state;

        // The landing edge: airborne last frame, grounded this one. Gameplay has no "Land"
        // state because landing is not a thing the simulation does — it is a thing that has
        // just finished happening — so the timer lives here.
        if self.was_airborne && state.grounded {
            self.land_remaining = self.def.land_time;
        }
        self.was_airborne = !state.grounded;
        self.land_remaining = (self.land_remaining - dt).max(0.0);

        // New casts and releases, each acted on once however many frames report it.
        let started = state.cast_started.is_some() && state.cast_started != self.last_cast_started;
        self.last_cast_started = state.cast_started.or(self.last_cast_started);
        let released = state.released.is_some() && state.released != self.last_released;
        self.last_released = state.released.or(self.last_released);

        // A monster's blow is not cut short by the step it is taken on: it swings while it
        // closes, and a swing that vanished on the first frame of movement was never seen.
        let stepping = state.speed > IDLE_SPEED && !self.whole_instants;
        if started || !state.grounded || stepping {
            // A new cast, a jump or a step all cut the follow-through short.
            self.follow_through = None;
        }
        let instant = self.was_casting.is_none();
        if released && state.cast.is_none() && state.grounded && !stepping {
            // The spell left the hands: the rest of the clip plays out. A cast that was
            // showing follows through with its own clip; an instant has none and borrows the
            // ordinary cast's.
            let slot = self
                .was_casting
                .map_or(AnimationSlot::Cast, CastVisual::slot);
            if self.has_follow_through(slot) {
                self.follow_through = Some(slot);
            }
        }
        self.was_casting = state.cast;

        let (slot, fade) = self.choose_slot();
        // The first clip a controller plays is cut to, not faded into: fading from the rest
        // pose is a T-pose flash on every monster that spawns.
        let fade = if self.player.current().is_none() {
            0.0
        } else {
            fade
        };
        if let Some(clip) = self.clip_for(slot) {
            let changed = self.slot != slot;
            self.player.play(clip, fade);
            let release = self
                .asset
                .clips
                .get(clip)
                .map_or(0.0, |clip| clip.release_time());
            if state.cast.is_some() && self.forced.is_none() {
                // The wind-up is driven by the cast, not the clock: however long the cast
                // takes, the release lands as it completes. A new cast of the same spell is
                // the same clip, and this is what takes it back to the top.
                if started && !changed {
                    self.player.restart();
                }
                self.player
                    .seek(state.cast_progress.clamp(0.0, 1.0) * release);
            } else if released && self.follow_through == Some(slot) {
                if instant && self.whole_instants {
                    // The same clip twice in a row is not a new `play`; the blow still has
                    // to start from its wind-up.
                    self.player.restart();
                } else {
                    self.player.seek(release);
                }
            }
        }
        self.slot = slot;
        let speed = if state.cast.is_some() && self.forced.is_none() {
            0.0
        } else {
            self.playback_speed(slot)
        };
        self.player.set_speed(speed);
        self.player.update(dt, &self.asset.clips)?;
        if self.follow_through == Some(slot) && self.player.is_finished() {
            self.follow_through = None;
        }

        if let Some(skeleton) = self.asset.skeleton.as_ref() {
            if self.stride_yaw.abs() > 1e-4 && !self.twist_bones.is_empty() {
                self.twisted.copy_from(self.player.pose());
                counter_twist(
                    skeleton,
                    &mut self.twisted,
                    &self.twist_bones,
                    -self.stride_yaw,
                );
                skeleton.skinning_matrices(&self.twisted, &mut self.palette)?;
            } else {
                skeleton.skinning_matrices(self.player.pose(), &mut self.palette)?;
            }
        }
        Ok(())
    }

    /// Which role the character should be in, and how long to take getting there.
    ///
    /// The order is the priority order, and it is the whole state machine:
    ///
    /// ```text
    /// casting  >  airborne (rising -> Jump, falling -> Fall)  >  follow-through
    ///          >  landing  >  locomotion
    /// ```
    ///
    /// Casting wins over locomotion because the abilities that have a cast time also root
    /// the character, and the ones that do not are instant — so a cast that overlaps a run
    /// is brief and reads better as a cast.
    fn choose_slot(&self) -> (AnimationSlot, f32) {
        if let Some(forced) = self.forced {
            return (forced, LOCOMOTION_FADE);
        }
        if self.state.dead && self.clips.get(AnimationSlot::Death).is_some() {
            return (AnimationSlot::Death, CAST_FADE);
        }
        if let Some(cast) = self.state.cast {
            return (cast.slot(), CAST_FADE);
        }
        if !self.state.grounded {
            let slot = if self.state.rising {
                AnimationSlot::Jump
            } else {
                AnimationSlot::Fall
            };
            return (slot, AIR_FADE);
        }
        if let Some(slot) = self.follow_through {
            return (slot, CAST_FADE);
        }
        if self.land_remaining > 0.0 {
            return (AnimationSlot::Land, AIR_FADE);
        }
        if self.state.speed <= IDLE_SPEED {
            return (AnimationSlot::Idle, LOCOMOTION_FADE);
        }
        (self.locomotion_slot(), LOCOMOTION_FADE)
    }

    /// Which locomotion cycle to play, by pace and heading.
    ///
    /// A directional cycle the model has wins. One it lacks falls back to the forward cycle
    /// for the same pace — reversed by [`CharacterVisual::playback_speed`] when backpedalling —
    /// which is exactly what a character without any directional clips always does.
    fn locomotion_slot(&self) -> AnimationSlot {
        let running = self.state.speed >= self.def.run_speed;
        let forward = if running {
            AnimationSlot::Run
        } else {
            AnimationSlot::Walk
        };
        let directional = match self.state.heading {
            Heading::Forward => return forward,
            Heading::Backward if running => AnimationSlot::RunBackward,
            Heading::Backward => AnimationSlot::WalkBackward,
            Heading::Left => AnimationSlot::StrafeLeft,
            Heading::Right => AnimationSlot::StrafeRight,
        };
        if self.clips.get(directional).is_some() {
            directional
        } else {
            forward
        }
    }

    /// The clip filling `slot`, with the one fallback the character is allowed.
    fn clip_for(&self, slot: AnimationSlot) -> Option<usize> {
        match slot {
            // A character with no walk still moves believably by running slowly, and one
            // with no jump or landing uses its fall pose rather than snapping to idle.
            AnimationSlot::Walk => self.clips.get_or(slot, AnimationSlot::Run),
            AnimationSlot::Jump | AnimationSlot::Land => {
                self.clips.get_or(slot, AnimationSlot::Fall)
            }
            AnimationSlot::CastHeavy => self.clips.get_or(slot, AnimationSlot::Cast),
            other => self.clips.get(other),
        }
    }

    /// Whether `slot`'s clip has anything after its release to play once the spell fires.
    ///
    /// A clip with no release moment releases on its last frame, and following through
    /// from there would flash its final pose for a frame on every instant.
    fn has_follow_through(&self, slot: AnimationSlot) -> bool {
        self.clip_for(slot)
            .and_then(|index| self.asset.clips.get(index))
            .is_some_and(|clip| clip.release_time() < clip.duration - 1e-3)
    }

    /// How fast to play the current clip.
    ///
    /// Locomotion is scaled by how fast the character is actually moving, relative to the
    /// speed its clip was authored at. That is what stops the feet sliding: a walk cycle
    /// played at a fixed rate while the character moves at a different speed is the single
    /// most obvious animation error there is.
    ///
    /// Clamped, because a character crawling at a tenth of walking speed should not play a
    /// tenth-speed walk — it should walk slowly, and there is a floor below which it reads
    /// as broken. Backpedalling plays the same rate in reverse.
    ///
    /// A cast in progress is not played at a rate at all — it is placed at the cast's
    /// progress every frame — so this is only its follow-through, at the authored rate.
    fn playback_speed(&self, slot: AnimationSlot) -> f32 {
        let pace = match slot {
            AnimationSlot::Walk | AnimationSlot::WalkBackward => self.def.walk_speed,
            AnimationSlot::Run
            | AnimationSlot::RunBackward
            | AnimationSlot::StrafeLeft
            | AnimationSlot::StrafeRight => self.def.run_speed,
            _ => return 1.0,
        };
        // The speed the clip itself says it was authored at, when its export says so, is
        // better than the definition's guess: a jogging strafe and a sprint differ.
        let authored = self
            .clip_for(slot)
            .and_then(|index| self.asset.clips.get(index))
            .and_then(|clip| clip.speed)
            .unwrap_or(pace);
        if authored <= 1e-3 {
            return 1.0;
        }
        let rate = (self.state.speed / authored).clamp(MIN_PLAYBACK_RATE, MAX_PLAYBACK_RATE);
        let reversed = self.state.heading == Heading::Backward
            && matches!(slot, AnimationSlot::Walk | AnimationSlot::Run);
        if reversed { -rate } else { rate }
    }

    /// Adds this character's submeshes to `world`'s skinned draw list.
    ///
    /// Chooses a level of detail from the distance to `camera`, pushes the palette once, and
    /// pushes one instance per submesh — all of which name the same palette range, because a
    /// character's parts share its skeleton.
    ///
    /// A character whose palette does not fit the shader's array is **not drawn**, rather
    /// than drawn with someone else's bones.
    pub fn submit(&mut self, world: &mut RenderWorld, camera: Vec3, lod_bias: f32) {
        self.submit_styled(world, camera, lod_bias, InstanceStyle::NEUTRAL);
    }

    /// [`CharacterVisual::submit`], drawn with `style`: a monster's tint, its hit flash and
    /// the glow of being targeted, which a hero does not have.
    pub fn submit_styled(
        &mut self,
        world: &mut RenderWorld,
        camera: Vec3,
        lod_bias: f32,
        style: InstanceStyle,
    ) {
        let transform = self.transform();
        // Distance to the character's middle rather than its feet: a character at the bottom
        // of the screen is closer at the feet than at the head, and picking a level from the
        // feet makes a tall model swap while its head is still in frame.
        let centre = self.state.position + Vec3::new(0.0, self.state.height * 0.5, 0.0);
        let distance = (centre - camera).length();
        self.lod = self.binding.select_lod(distance, lod_bias);

        let Some(lod) = self.binding.lod(self.lod) else {
            return;
        };
        let Some((palette_start, palette_len)) = world.skinned_mut().push_palette(&self.palette)
        else {
            return;
        };

        for part in &lod.parts {
            let PartGeometry::Skinned(mesh) = part.geometry else {
                // A rigid part of a character model — a prop that does not deform. Nothing
                // authors one yet, and drawing it through the skinned path would index a
                // palette its vertices have no joints for.
                continue;
            };
            let bounds = animated_bounds(part.bounds, &transform);
            world.skinned_mut().push(SkinnedInstance {
                transform,
                mesh,
                material: part.material,
                bounds,
                style,
                palette_start,
                palette_len,
                // A character is the shadow the player looks at. Nothing here opts out.
                casts_shadow: true,
                receives_shadow: true,
            });
        }
    }
}

/// Speed below which a character counts as standing still, metres per second.
///
/// Not zero: deceleration leaves a body drifting at a few centimetres a second for a frame
/// or two, and a character that flickers between idle and walk at a stop looks broken.
const IDLE_SPEED: f32 = 0.15;

/// `angle` wrapped into `[-pi, pi)`.
fn wrap_angle(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// The bones that take a stride turn back, and how much of it each takes.
///
/// The torso's share is split evenly over every `spine`/`chest` bone, the head's over every
/// `neck`/`head` bone, so a Mixamo rig's three spine bones and a procedural rig's two turn
/// the chest by the same total. Hierarchy order, so a parent is twisted before its children
/// read it.
fn twist_bones(skeleton: &Skeleton) -> Vec<(u16, f32)> {
    let matches = |index: usize, parts: &[&str]| {
        let lower = skeleton.bones()[index].name.to_ascii_lowercase();
        // The last segment only: `mixamorig:Spine1` is a spine, and `HeadTop_End` is a leaf
        // with nothing bound to it.
        let tail = lower.rsplit([':', '|', '/']).next().unwrap_or(&lower);
        !tail.contains("end") && parts.iter().any(|part| tail.starts_with(part))
    };
    let group = |parts: &[&str]| -> Vec<usize> {
        (0..skeleton.len())
            .filter(|&index| matches(index, parts))
            .collect()
    };
    let mut bones = Vec::new();
    for (members, share) in [
        (group(&["spine", "chest"]), TORSO_COUNTER_TWIST),
        (group(&["neck", "head"]), HEAD_COUNTER_TWIST),
    ] {
        if members.is_empty() {
            continue;
        }
        let each = share / members.len() as f32;
        bones.extend(members.iter().map(|&index| (index as u16, each)));
    }
    bones.sort_by_key(|&(index, _)| index);
    bones
}

/// Turns each of `bones` by its share of `yaw` about the **model's** up axis, in `pose`.
///
/// A bone's local rotation is in its parent's frame, and a rig's spine bones point every
/// which way in theirs — so the turn is built in model space and carried into the parent's
/// frame: `local' = parent^-1 * turn * parent * local`. The parent's model-space rotation is
/// composed from `pose` itself, so a chest whose spine was already turned turns on top of it.
fn counter_twist(skeleton: &Skeleton, pose: &mut Pose, bones: &[(u16, f32)], yaw: f32) {
    let hierarchy = skeleton.bones();
    for &(index, share) in bones {
        let index = index as usize;
        if index >= pose.len() || index >= hierarchy.len() {
            continue;
        }
        let mut parent_rotation = Quat::IDENTITY;
        let mut cursor = hierarchy[index].parent;
        while let Some(parent) = cursor {
            parent_rotation = pose.locals()[parent as usize].rotation * parent_rotation;
            cursor = hierarchy[parent as usize].parent;
        }
        let turn = Quat::from_axis_angle(Vec3::Y, yaw * share);
        let local = &mut pose.locals_mut()[index];
        local.rotation =
            (parent_rotation.conjugate() * turn * parent_rotation * local.rotation).normalize();
    }
}

/// Slowest a locomotion clip is played, relative to its authored rate.
const MIN_PLAYBACK_RATE: f32 = 0.55;

/// Fastest a locomotion clip is played, relative to its authored rate.
const MAX_PLAYBACK_RATE: f32 = 1.75;

/// Places a bind-pose bound in world space and pads it for animation.
///
/// `world_bounds` already applies the transform's scale, so a character scaled to gameplay's
/// height scales its reach with it; the padding is what covers the difference between a
/// T-pose and an arm overhead.
fn animated_bounds(local: Sphere, transform: &Transform) -> Sphere {
    let world = engine_render_data::world_bounds(local, transform);
    Sphere::new(world.center, world.radius * ANIMATED_BOUNDS_PADDING)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_assets::procedural::{
        druid, druid_character, fire_mage, fire_mage_character, warrior, warrior_character,
    };
    use engine_game::hero::Hero;
    use engine_render_data::{MaterialHandle, ModelLodBinding, ModelPart, SkinnedMeshHandle};

    /// A binding that names handles nothing uploaded: the controller never dereferences
    /// them, which is exactly what makes it testable with no device.
    fn binding(model: &ModelAsset) -> ModelBinding {
        ModelBinding {
            lods: model
                .lods
                .iter()
                .enumerate()
                .map(|(level, lod)| ModelLodBinding {
                    parts: lod
                        .submeshes
                        .iter()
                        .enumerate()
                        .map(|(index, submesh)| ModelPart {
                            geometry: PartGeometry::Skinned(SkinnedMeshHandle::new(
                                (level * 8 + index) as u32,
                                1,
                            )),
                            material: MaterialHandle::new(submesh.material as u32, 1),
                            bounds: submesh.bounds,
                        })
                        .collect(),
                    min_distance: lod.min_distance,
                    bounds: lod.bounds(),
                })
                .collect(),
            skinned: true,
            model_height: 1.8,
        }
    }

    fn mage() -> CharacterVisual {
        let Ok(model) = fire_mage() else {
            panic!("the FireMage must build");
        };
        let model = Arc::new(model);
        let binding = binding(&model);
        match CharacterVisual::new(fire_mage_character(), model, binding) {
            Ok(visual) => visual,
            Err(error) => panic!("the FireMage's own definition must resolve: {error}"),
        }
    }

    /// Every playable hero, as the application builds them.
    fn every_hero() -> Vec<(Hero, CharacterVisual)> {
        type Source = (
            Hero,
            fn() -> Result<ModelAsset, AssetError>,
            fn() -> engine_assets::CharacterDef,
        );
        let sources: [Source; 3] = [
            (Hero::FireMage, fire_mage, fire_mage_character),
            (Hero::Warrior, warrior, warrior_character),
            (Hero::Druid, druid, druid_character),
        ];
        sources
            .into_iter()
            .map(|(hero, build, character)| {
                let Ok(model) = build() else {
                    panic!("{hero:?} must build");
                };
                let model = Arc::new(model);
                let binding = binding(&model);
                match CharacterVisual::new(character(), model, binding) {
                    Ok(visual) => (hero, visual),
                    Err(error) => panic!("{hero:?}'s definition must resolve: {error}"),
                }
            })
            .collect()
    }

    /// A run at `angle` off the facing (positive to the right), as the extractor reports it.
    fn running_at(visual: &CharacterVisual, angle: f32) -> CharacterState {
        let right = angle.sin();
        let heading = if angle.cos() < -0.3 {
            Heading::Backward
        } else if right >= 0.8 {
            Heading::Right
        } else if right <= -0.8 {
            Heading::Left
        } else {
            Heading::Forward
        };
        CharacterState {
            speed: visual.def().run_speed + 1.0,
            heading,
            move_angle: angle,
            ..CharacterState::default()
        }
    }

    /// Drives `visual` for a second under `state` and returns where the turn settled.
    fn settle(visual: &mut CharacterVisual, state: CharacterState) -> f32 {
        for _ in 0..60 {
            let Ok(()) = visual.update(0.016, state) else {
                panic!("the update must succeed");
            };
        }
        visual.stride_yaw()
    }

    #[test]
    fn a_diagonal_run_turns_the_body_along_the_diagonal_and_nothing_else_does() {
        use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
        for (hero, mut visual) in every_hero() {
            let cases = [
                // Forward and right: the body turns right, which is a smaller yaw.
                (FRAC_PI_4, -FRAC_PI_4),
                (-FRAC_PI_4, FRAC_PI_4),
                // Back and right: the body turns left, so the backpedal goes back-right.
                (3.0 * FRAC_PI_4, FRAC_PI_4),
                (-3.0 * FRAC_PI_4, -FRAC_PI_4),
                // Straight lines and strafes are not turned at all.
                (0.0, 0.0),
                (PI, 0.0),
                (FRAC_PI_2, 0.0),
                (-FRAC_PI_2, 0.0),
            ];
            for (angle, expected) in cases {
                let state = running_at(&visual, angle);
                let turned = settle(&mut visual, state);
                assert!(
                    (turned - expected).abs() < 1e-3,
                    "{hero:?} at {angle}: turned {turned}, expected {expected}"
                );
            }
            // Standing still after a diagonal brings the body back square.
            let state = running_at(&visual, FRAC_PI_4);
            settle(&mut visual, state);
            assert!(settle(&mut visual, CharacterState::default()).abs() < 1e-3);
        }
    }

    #[test]
    fn the_stride_turn_eases_in_rather_than_snapping() {
        let mut visual = mage();
        let state = running_at(&visual, std::f32::consts::FRAC_PI_4);
        let Ok(()) = visual.update(0.016, state) else {
            panic!("the update must succeed");
        };
        let first = visual.stride_yaw();
        assert!(first < 0.0 && first > -0.3, "first frame turned {first}");
        // The simulation's facing is untouched: the turn is in the drawn transform only.
        assert_eq!(visual.state().facing_yaw, 0.0);
    }

    #[test]
    fn every_hero_twists_its_torso_back_towards_the_facing() {
        // The legs run along the diagonal and the chest is carried most of the way back, so
        // the character still reads as facing where it will cast. Checked on the skinned
        // result: the palette with the twist differs from the one without it, and only
        // above the hips.
        for (hero, mut visual) in every_hero() {
            assert!(
                !visual.twist_bones.is_empty(),
                "{hero:?} has no spine the stride turn can find"
            );
            let total: f32 = visual.twist_bones.iter().map(|&(_, share)| share).sum();
            assert!(
                (total - (TORSO_COUNTER_TWIST + HEAD_COUNTER_TWIST)).abs() < 1e-4
                    || (total - TORSO_COUNTER_TWIST).abs() < 1e-4,
                "{hero:?} shares add to {total}"
            );
            let state = running_at(&visual, std::f32::consts::FRAC_PI_4);
            settle(&mut visual, state);
            let twisted = visual.palette().to_vec();
            let Some(skeleton) = visual.asset.skeleton.as_ref() else {
                panic!("{hero:?} has no skeleton");
            };
            let mut plain = Vec::new();
            let Ok(()) = skeleton.skinning_matrices(visual.player.pose(), &mut plain) else {
                panic!("{hero:?} could not skin");
            };
            let (spine, _) = visual.twist_bones[0];
            let spine = spine as usize;
            assert_ne!(
                twisted[spine], plain[spine],
                "{hero:?}'s spine was not turned"
            );
            let hips_unchanged = (0..spine)
                .filter(|&index| skeleton.bones()[spine].parent == Some(index as u16))
                .all(|index| twisted[index] == plain[index]);
            assert!(hips_unchanged, "{hero:?}'s hips were turned with the chest");
        }
    }

    #[test]
    fn every_hero_falls_when_it_dies_and_gets_back_up_when_revived() {
        for (hero, mut visual) in every_hero() {
            let dead = CharacterState {
                dead: true,
                ..CharacterState::default()
            };
            let Ok(()) = visual.update(0.016, dead) else {
                panic!("{hero:?} could not play its death");
            };
            assert_eq!(visual.slot(), AnimationSlot::Death, "{hero:?}");
            // Long after the clip has finished it is still lying there, not looping.
            for _ in 0..300 {
                let Ok(()) = visual.update(0.016, dead) else {
                    panic!("{hero:?} could not hold its death");
                };
            }
            assert_eq!(visual.slot(), AnimationSlot::Death, "{hero:?}");

            let Ok(()) = visual.update(0.016, CharacterState::default()) else {
                panic!("{hero:?} could not stand back up");
            };
            assert_eq!(visual.slot(), AnimationSlot::Idle, "{hero:?} stayed dead");
        }
    }

    #[test]
    fn every_hero_the_swap_menu_offers_can_actually_be_worn() {
        // What a swap does at runtime, without a device: build the model the hero names,
        // resolve its definition against it and drive one frame of it. A hero whose
        // definition and model disagree is a menu entry that turns the player into a
        // bind-pose statue, and the roster has no way to know.
        for (hero, mut visual) in every_hero() {
            assert_eq!(hero.model_name(), visual.def().model.as_str(), "{hero:?}");
            for slot in AnimationSlot::REQUIRED {
                assert!(
                    visual.clips.get(slot).is_some(),
                    "{hero:?} does not fill {}",
                    slot.as_str()
                );
            }
            assert!(
                visual.clips.get(AnimationSlot::Death).is_some(),
                "{hero:?} cannot die"
            );
            let Ok(()) = visual.update(0.016, CharacterState::default()) else {
                panic!("{hero:?} must animate");
            };
            assert_eq!(visual.slot(), AnimationSlot::Idle, "{hero:?}");
            assert_eq!(visual.palette().len(), 21, "{hero:?} has a different rig");
        }
    }

    #[test]
    fn every_hero_runs_when_it_is_moved_and_lands_when_it_touches_down() {
        // The gameplay-to-animation seam, per hero: the roles are chosen from the state, and
        // a character whose speeds were mis-declared would walk where it should run.
        for (hero, mut visual) in every_hero() {
            let Ok(()) = visual.update(0.016, moving(visual.def().run_speed + 1.0)) else {
                panic!("{hero:?} must animate");
            };
            assert_eq!(visual.slot(), AnimationSlot::Run, "{hero:?} will not run");
        }
    }

    fn moving(speed: f32) -> CharacterState {
        CharacterState {
            speed,
            ..CharacterState::default()
        }
    }

    #[test]
    fn standing_still_is_idle_and_moving_is_walk_then_run() {
        let mut visual = mage();
        let Ok(()) = visual.update(0.016, moving(0.0)) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Idle);

        let Ok(()) = visual.update(0.016, moving(2.5)) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Walk);

        let Ok(()) = visual.update(0.016, moving(6.0)) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Run);
    }

    #[test]
    fn a_body_drifting_to_a_stop_does_not_flicker_between_idle_and_walk() {
        // Deceleration leaves a few centimetres a second for a frame or two.
        let mut visual = mage();
        let Ok(()) = visual.update(0.016, moving(0.05)) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Idle);
    }

    #[test]
    fn leaving_the_ground_jumps_and_descending_falls() {
        let mut visual = mage();
        let airborne = |rising: bool| CharacterState {
            grounded: false,
            rising,
            ..CharacterState::default()
        };

        let Ok(()) = visual.update(0.016, airborne(true)) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Jump);

        let Ok(()) = visual.update(0.016, airborne(false)) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Fall);
    }

    #[test]
    fn touching_down_plays_the_landing_and_then_returns_to_locomotion() {
        // The one piece of state the visual layer owns, because gameplay has no "just
        // landed" — landing is an edge, not a state.
        let mut visual = mage();
        let Ok(()) = visual.update(
            0.016,
            CharacterState {
                grounded: false,
                rising: false,
                ..CharacterState::default()
            },
        ) else {
            panic!("the update must succeed");
        };
        let Ok(()) = visual.update(0.016, CharacterState::default()) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Land);

        // Past the landing window, locomotion resumes.
        let Ok(()) = visual.update(1.0, CharacterState::default()) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Idle);
    }

    #[test]
    fn a_cast_wins_over_locomotion_and_a_long_cast_is_the_heavy_one() {
        let mut visual = mage();
        let casting = |cast: CastVisual| CharacterState {
            speed: 6.0,
            cast: Some(cast),
            ..CharacterState::default()
        };

        let Ok(()) = visual.update(0.016, casting(CastVisual::Normal)) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Cast);

        let Ok(()) = visual.update(0.016, casting(CastVisual::Heavy)) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::CastHeavy);

        assert_eq!(CastVisual::for_duration(0.5), CastVisual::Normal);
        assert_eq!(CastVisual::for_duration(2.5), CastVisual::Heavy);
    }

    #[test]
    fn gameplay_is_not_waited_on_by_the_animation() {
        // The property the phase brief names: a cast animation that has not finished must
        // not hold the character in the cast pose once gameplay says the cast is over.
        let mut visual = mage();
        let Ok(()) = visual.update(
            0.016,
            CharacterState {
                cast: Some(CastVisual::Heavy),
                ..CharacterState::default()
            },
        ) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::CastHeavy);

        // One frame later gameplay has cancelled it. The 1.1 s clip is nowhere near done.
        let Ok(()) = visual.update(0.016, CharacterState::default()) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Idle);
    }

    #[test]
    fn locomotion_playback_scales_with_speed_and_is_clamped() {
        // Feet sliding is the most obvious animation error there is, and a clip played at a
        // fixed rate while the character moves at a different speed is exactly that.
        let mut visual = mage();
        let run_speed = visual.def().run_speed;

        let Ok(()) = visual.update(0.016, moving(run_speed)) else {
            panic!("the update must succeed");
        };
        assert!((visual.playback_speed(AnimationSlot::Run) - 1.0).abs() < 1e-4);

        let Ok(()) = visual.update(0.016, moving(run_speed * 100.0)) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.playback_speed(AnimationSlot::Run), MAX_PLAYBACK_RATE);
        assert_eq!(visual.playback_speed(AnimationSlot::Idle), 1.0);
    }

    /// The mage, delivered with a left strafe (authored at 4 m/s) and a backwards walk.
    fn mage_with_directions() -> CharacterVisual {
        let Ok(mut model) = fire_mage() else {
            panic!("the FireMage must build");
        };
        let Some(walk) = model.clip_index("walk") else {
            panic!("the FireMage has a walk");
        };
        let mut strafe = model.clips[walk].clone().with_speed(4.0);
        strafe.name = "strafe_left".to_owned();
        let mut back = model.clips[walk].clone();
        back.name = "walk_back".to_owned();
        model.clips.push(strafe);
        model.clips.push(back);
        let def = fire_mage_character();
        assert!(matches!(def.mirror_missing(&mut model), Ok(1)));
        let model = Arc::new(model);
        let binding = binding(&model);
        match CharacterVisual::new(def, model, binding) {
            Ok(visual) => visual,
            Err(error) => panic!("the FireMage's own definition must resolve: {error}"),
        }
    }

    fn heading(speed: f32, heading: Heading) -> CharacterState {
        CharacterState {
            speed,
            heading,
            ..CharacterState::default()
        }
    }

    #[test]
    fn a_directional_clip_the_model_has_is_played_for_its_heading() {
        let mut visual = mage_with_directions();
        let walk = visual.def().walk_speed;
        let run = visual.def().run_speed;
        let cases = [
            (heading(walk, Heading::Left), AnimationSlot::StrafeLeft),
            (heading(run + 1.0, Heading::Left), AnimationSlot::StrafeLeft),
            (heading(walk, Heading::Right), AnimationSlot::StrafeRight),
            (
                heading(walk, Heading::Backward),
                AnimationSlot::WalkBackward,
            ),
            // No run_back: running backwards falls back to the run, reversed.
            (heading(run + 1.0, Heading::Backward), AnimationSlot::Run),
            (heading(walk, Heading::Forward), AnimationSlot::Walk),
        ];
        for (state, expected) in cases {
            let Ok(()) = visual.update(0.016, state) else {
                panic!("the update must succeed");
            };
            assert_eq!(
                visual.slot(),
                expected,
                "{:?} at {}",
                state.heading,
                state.speed
            );
        }
        // A backwards walk plays forwards; only a borrowed forward cycle is reversed.
        let Ok(()) = visual.update(0.016, heading(run + 1.0, Heading::Backward)) else {
            panic!("the update must succeed");
        };
        assert!(visual.playback_speed(AnimationSlot::Run) < 0.0);
        let Ok(()) = visual.update(0.016, heading(walk, Heading::Backward)) else {
            panic!("the update must succeed");
        };
        assert!(visual.playback_speed(AnimationSlot::WalkBackward) > 0.0);
    }

    #[test]
    fn a_strafe_plays_against_the_speed_it_was_authored_at() {
        let mut visual = mage_with_directions();
        let Ok(()) = visual.update(0.016, heading(4.0, Heading::Left)) else {
            panic!("the update must succeed");
        };
        assert!((visual.playback_speed(AnimationSlot::StrafeLeft) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_model_without_directional_clips_moves_as_it_always_did() {
        let mut visual = mage();
        let walk = visual.def().walk_speed;
        for direction in [Heading::Left, Heading::Right, Heading::Backward] {
            let Ok(()) = visual.update(0.016, heading(walk, direction)) else {
                panic!("the update must succeed");
            };
            assert_eq!(visual.slot(), AnimationSlot::Walk, "{direction:?}");
        }
        let Ok(()) = visual.update(0.016, heading(walk, Heading::Left)) else {
            panic!("the update must succeed");
        };
        assert!(
            visual.playback_speed(AnimationSlot::Walk) > 0.0,
            "a strafe is not reversed"
        );
    }

    #[test]
    fn backpedalling_plays_the_walk_in_reverse() {
        // Facing pinned by the camera while moving the other way: stepping backwards, not
        // moonwalking on a forward walk.
        let mut visual = mage();
        let walk_speed = visual.def().walk_speed;
        let state = CharacterState {
            speed: walk_speed,
            heading: Heading::Backward,
            ..CharacterState::default()
        };
        let Ok(()) = visual.update(0.016, state) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Walk);
        assert!((visual.playback_speed(AnimationSlot::Walk) + 1.0).abs() < 1e-4);
    }

    /// The mage, with its cast clip releasing halfway through.
    fn mage_with_release() -> CharacterVisual {
        let Ok(mut model) = fire_mage() else {
            panic!("the FireMage must build");
        };
        let Some(index) = model.clip_index("cast") else {
            panic!("the FireMage has a cast clip");
        };
        let half = model.clips[index].duration * 0.5;
        model.clips[index].release = Some(half);
        let model = Arc::new(model);
        let binding = binding(&model);
        match CharacterVisual::new(fire_mage_character(), model, binding) {
            Ok(visual) => visual,
            Err(error) => panic!("the FireMage's own definition must resolve: {error}"),
        }
    }

    fn casting(progress: f32, started: u64) -> CharacterState {
        CharacterState {
            cast: Some(CastVisual::Normal),
            cast_progress: progress,
            cast_started: Some(started),
            ..CharacterState::default()
        }
    }

    fn cast_clip(visual: &CharacterVisual) -> &engine_assets::AnimationClip {
        match visual.clip_for(AnimationSlot::Cast) {
            Some(index) => &visual.asset.clips[index],
            None => panic!("the mage has a cast clip"),
        }
    }

    #[test]
    fn a_cast_is_placed_at_its_progress_so_the_release_lands_as_it_completes() {
        // However long the cast, the wind-up fills it: the clip never reaches its release
        // early and plays its recovery while the cast bar is still filling.
        let mut visual = mage_with_release();
        let release = cast_clip(&visual).release_time();
        for progress in [0.25, 0.5, 0.99] {
            let Ok(()) = visual.update(0.5, casting(progress, 1)) else {
                panic!("the update must succeed");
            };
            assert!(
                (visual.player.time() - progress * release).abs() < 1e-4,
                "at {progress} the clip is at {}",
                visual.player.time()
            );
        }
    }

    #[test]
    fn a_cast_queued_behind_the_last_starts_its_clip_from_the_top() {
        // Spamming a spell: the next cast begins on the tick the last one lands, so there is
        // never a frame without a cast and the cast kind never changes.
        let mut visual = mage_with_release();
        let Ok(()) = visual.update(0.016, casting(0.95, 10)) else {
            panic!("the update must succeed");
        };
        let Ok(()) = visual.update(0.016, casting(0.02, 70)) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Cast);
        let release = cast_clip(&visual).release_time();
        assert!((visual.player.time() - 0.02 * release).abs() < 1e-4);
    }

    #[test]
    fn a_completed_cast_follows_through_and_moving_cuts_it_short() {
        let mut visual = mage_with_release();
        let Ok(()) = visual.update(0.016, casting(0.98, 1)) else {
            panic!("the update must succeed");
        };
        // The cast lands: no cast any more, and the ability went off.
        let landed = CharacterState {
            cast_started: Some(1),
            released: Some(60),
            ..CharacterState::default()
        };
        let Ok(()) = visual.update(0.016, landed) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Cast, "the recovery plays out");
        let release = cast_clip(&visual).release_time();
        assert!(visual.player.time() >= release);

        // The same event seen again next frame does not start it over.
        let Ok(()) = visual.update(0.1, landed) else {
            panic!("the update must succeed");
        };
        assert!(visual.player.time() > release + 0.05);

        // A step cuts it short.
        let Ok(()) = visual.update(
            0.016,
            CharacterState {
                speed: 3.0,
                ..landed
            },
        ) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Walk);
    }

    #[test]
    fn a_follow_through_ends_with_its_clip() {
        let mut visual = mage_with_release();
        let landed = CharacterState {
            released: Some(5),
            ..CharacterState::default()
        };
        let Ok(()) = visual.update(0.016, landed) else {
            panic!("the update must succeed");
        };
        assert_eq!(
            visual.slot(),
            AnimationSlot::Cast,
            "an instant borrows the cast clip"
        );
        let Ok(()) = visual.update(10.0, landed) else {
            panic!("the update must succeed");
        };
        let Ok(()) = visual.update(0.016, landed) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Idle);
    }

    #[test]
    fn a_clip_with_no_release_has_no_follow_through() {
        // Otherwise every instant would flash the cast's last frame.
        let mut visual = mage();
        let landed = CharacterState {
            released: Some(5),
            ..CharacterState::default()
        };
        let Ok(()) = visual.update(0.016, landed) else {
            panic!("the update must succeed");
        };
        assert_eq!(visual.slot(), AnimationSlot::Idle);
    }

    #[test]
    fn the_model_is_scaled_to_the_height_gameplay_says_the_character_is() {
        let mut visual = mage();
        let Ok(()) = visual.update(
            0.016,
            CharacterState {
                height: 0.9,
                ..CharacterState::default()
            },
        ) else {
            panic!("the update must succeed");
        };
        // The FireMage is authored at 1.8 m, so half that is half scale.
        assert!((visual.transform().scale.x - 0.5).abs() < 1e-4);
    }

    #[test]
    fn the_character_faces_where_gameplay_says_it_faces() {
        // Orientation is gameplay's, not the animation's. A quarter turn about +Y sends the
        // model's forward, which is -Z, to -X.
        let mut visual = mage();
        let Ok(()) = visual.update(
            0.016,
            CharacterState {
                facing_yaw: std::f32::consts::FRAC_PI_2,
                ..CharacterState::default()
            },
        ) else {
            panic!("the update must succeed");
        };
        let forward = visual.transform().rotation.rotate(-Vec3::Z);
        assert!((forward - -Vec3::X).length() < 1e-4, "{forward:?}");
    }

    #[test]
    fn submitting_pushes_one_palette_and_one_instance_per_submesh() {
        let mut visual = mage();
        let Ok(()) = visual.update(0.016, CharacterState::default()) else {
            panic!("the update must succeed");
        };

        let mut world = RenderWorld::new();
        visual.submit(&mut world, Vec3::new(0.0, 1.0, 5.0), 1.0);

        let skinned = world.skinned();
        assert_eq!(visual.lod(), 0, "five metres away is the finest level");
        assert_eq!(skinned.len(), 4, "the FireMage's LOD0 is four submeshes");
        assert_eq!(skinned.palette().len(), 21, "one palette, twenty-one bones");
        // Every submesh names the same palette, because they share a skeleton.
        let ranges: Vec<(u32, u32)> = skinned
            .instances()
            .iter()
            .map(|instance| (instance.palette_start, instance.palette_len))
            .collect();
        assert!(
            ranges.windows(2).all(|pair| pair[0] == pair[1]),
            "{ranges:?}"
        );
    }

    #[test]
    fn a_distant_character_is_submitted_at_a_coarser_level() {
        let mut visual = mage();
        let Ok(()) = visual.update(0.016, CharacterState::default()) else {
            panic!("the update must succeed");
        };

        let mut world = RenderWorld::new();
        visual.submit(&mut world, Vec3::new(0.0, 1.0, 60.0), 1.0);
        assert_eq!(visual.lod(), 1, "sixty metres is past the 25 m threshold");
        assert_eq!(
            world.skinned().len(),
            3,
            "LOD1 drops the ember, which is one fewer draw"
        );
    }

    #[test]
    fn the_submitted_bounds_are_larger_than_the_bind_pose_implies() {
        // An arm overhead during a heavy cast reaches well outside a T-pose's sphere, and
        // culling with the unpadded bound pops the character off the edge of the screen.
        let mut visual = mage();
        let Ok(()) = visual.update(0.016, CharacterState::default()) else {
            panic!("the update must succeed");
        };
        let mut world = RenderWorld::new();
        visual.submit(&mut world, Vec3::new(0.0, 1.0, 5.0), 1.0);

        let Some(lod) = visual.binding.lod(0) else {
            panic!("LOD0 exists");
        };
        let Some(instance) = world.skinned().instances().first() else {
            panic!("something was submitted");
        };
        assert!(instance.bounds.radius > lod.parts[0].bounds.radius);
    }

    #[test]
    fn a_model_with_no_skeleton_is_refused_rather_than_standing_in_its_bind_pose() {
        let Ok(mut model) = fire_mage() else {
            panic!("the FireMage must build");
        };
        model.skeleton = None;
        let model = Arc::new(model);
        let binding = binding(&model);
        assert!(CharacterVisual::new(fire_mage_character(), model, binding).is_err());
    }

    #[test]
    fn a_definition_naming_a_clip_the_model_lacks_is_refused_at_construction() {
        let Ok(model) = fire_mage() else {
            panic!("the FireMage must build");
        };
        let model = Arc::new(model);
        let binding = binding(&model);
        let def = fire_mage_character().with_clip(AnimationSlot::Idle, "not_a_clip");
        assert!(CharacterVisual::new(def, model, binding).is_err());
    }
}
