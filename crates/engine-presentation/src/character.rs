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
    AnimationPlayer, AnimationSlot, AssetError, CharacterDef, ClipBindings, ModelAsset,
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
}

impl Default for CharacterState {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            facing_yaw: 0.0,
            speed: 0.0,
            grounded: true,
            rising: false,
            height: 1.8,
            cast: None,
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
    /// A cast that was showing last frame, so a repeat of the same spell restarts the clip.
    was_casting: Option<CastVisual>,
    /// Where the level of detail was last selected, for diagnostics.
    lod: usize,
    /// A role the character is held in regardless of what it is doing.
    ///
    /// The character test scene's whole mechanism: a developer presses 3 and the mage runs
    /// on the spot, so a run cycle can be judged without having to run. Nothing in the game
    /// sets it, and it is checked *before* everything else in `choose_slot`, which is what
    /// makes it an override rather than a hint.
    forced: Option<AnimationSlot>,
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
            lod: 0,
            forced: None,
        })
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
            .with_rotation(Quat::from_yaw_pitch_roll(self.state.facing_yaw, 0.0, 0.0))
            .with_scale(Vec3::splat(scale))
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
        self.state = state;

        // The landing edge: airborne last frame, grounded this one. Gameplay has no "Land"
        // state because landing is not a thing the simulation does — it is a thing that has
        // just finished happening — so the timer lives here.
        if self.was_airborne && state.grounded {
            self.land_remaining = self.def.land_time;
        }
        self.was_airborne = !state.grounded;
        self.land_remaining = (self.land_remaining - dt).max(0.0);

        let (slot, fade) = self.choose_slot();
        // A second cast of the same spell is the same slot, so `play` would correctly decide
        // nothing changed. It really does want the animation from the top.
        let recast = matches!(state.cast, Some(cast) if self.was_casting != Some(cast));
        self.was_casting = state.cast;

        if let Some(clip) = self.clip_for(slot) {
            let changed = self.slot != slot;
            self.player.play(clip, fade);
            if recast && !changed {
                self.player.restart();
            }
        }
        self.slot = slot;
        self.player.set_speed(self.playback_speed(slot));
        self.player.update(dt, &self.asset.clips)?;

        if let Some(skeleton) = self.asset.skeleton.as_ref() {
            skeleton.skinning_matrices(self.player.pose(), &mut self.palette)?;
        }
        Ok(())
    }

    /// Which role the character should be in, and how long to take getting there.
    ///
    /// The order is the priority order, and it is the whole state machine:
    ///
    /// ```text
    /// casting  >  airborne (rising -> Jump, falling -> Fall)  >  landing  >  locomotion
    /// ```
    ///
    /// Casting wins over locomotion because the abilities that have a cast time also root
    /// the character, and the ones that do not are instant — so a cast that overlaps a run
    /// is brief and reads better as a cast.
    fn choose_slot(&self) -> (AnimationSlot, f32) {
        if let Some(forced) = self.forced {
            return (forced, LOCOMOTION_FADE);
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
        if self.land_remaining > 0.0 {
            return (AnimationSlot::Land, AIR_FADE);
        }
        if self.state.speed >= self.def.run_speed {
            return (AnimationSlot::Run, LOCOMOTION_FADE);
        }
        if self.state.speed > IDLE_SPEED {
            return (AnimationSlot::Walk, LOCOMOTION_FADE);
        }
        (AnimationSlot::Idle, LOCOMOTION_FADE)
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

    /// How fast to play the current clip.
    ///
    /// Locomotion is scaled by how fast the character is actually moving, relative to the
    /// speed its clip was authored at. That is what stops the feet sliding: a walk cycle
    /// played at a fixed rate while the character moves at a different speed is the single
    /// most obvious animation error there is.
    ///
    /// Clamped, because a character crawling at a tenth of walking speed should not play a
    /// tenth-speed walk — it should walk slowly, and there is a floor below which it reads
    /// as broken.
    fn playback_speed(&self, slot: AnimationSlot) -> f32 {
        let authored = match slot {
            AnimationSlot::Walk => self.def.walk_speed,
            AnimationSlot::Run => self.def.run_speed,
            _ => return 1.0,
        };
        if authored <= 1e-3 {
            return 1.0;
        }
        (self.state.speed / authored).clamp(MIN_PLAYBACK_RATE, MAX_PLAYBACK_RATE)
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
                style: InstanceStyle::NEUTRAL,
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

    #[test]
    fn every_hero_the_swap_menu_offers_can_actually_be_worn() {
        // What a swap does at runtime, without a device: build the model the hero names,
        // resolve its definition against it and drive one frame of it. A hero whose
        // definition and model disagree is a menu entry that turns the player into a
        // bind-pose statue, and the roster has no way to know.
        for (hero, mut visual) in every_hero() {
            assert_eq!(hero.model_name(), visual.def().model.as_str(), "{hero:?}");
            assert_eq!(
                visual.def().clips.len(),
                AnimationSlot::ALL.len(),
                "{hero:?} does not fill every animation slot"
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
