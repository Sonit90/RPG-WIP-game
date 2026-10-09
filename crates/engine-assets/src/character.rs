//! What makes a character a *character*: which model, how big, and which clip plays when.
//!
//! ```text
//! CharacterDef ─── model: ModelId ──────────► ModelLibrary ──► ModelAsset
//!              ├── height / scale
//!              ├── locomotion speeds
//!              └── clips: AnimationSlot → clip name
//!                              │
//!                    resolved once at load
//!                              ▼
//!                        ClipBindings: AnimationSlot → clip index
//! ```
//!
//! # This is the file that replaces `if entity == fire_mage`
//!
//! Everything specific to one character lives in a value of this type. Adding a second
//! playable character is adding a second [`CharacterDef`] and registering its model — no
//! gameplay code, no presentation code, no renderer code changes, because nothing downstream
//! reads a character's *name* to decide anything.
//!
//! # Why slots and not clip names at the call site
//!
//! The visual controller knows "this character is running". It must not know that this
//! particular character's run animation is called `"run_cycle_02"`. [`AnimationSlot`] is the
//! vocabulary the controller speaks; the mapping to a name is per character, and the mapping
//! from a name to an index is resolved once into [`ClipBindings`] so the per-frame path is an
//! array index.

use crate::error::AssetError;
use crate::library::ModelId;
use crate::model::ModelAsset;

/// A role an animation plays, independent of what any particular character calls it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimationSlot {
    /// Standing still on the ground.
    Idle,
    /// Moving slowly on the ground.
    Walk,
    /// Moving quickly on the ground.
    Run,
    /// Leaving the ground.
    Jump,
    /// Airborne and descending.
    Fall,
    /// The moment after touching down.
    Land,
    /// Casting an ordinary ability.
    Cast,
    /// Casting something big — the variation the phase asks to *prepare*, not to fill in for
    /// all ten abilities.
    CastHeavy,
    /// Stepping backwards at walking pace. Optional: without it the walk plays in reverse.
    WalkBackward,
    /// Moving backwards at running pace. Optional: without it the run plays in reverse.
    RunBackward,
    /// Moving sideways to the character's left. Optional: mirrored from [`Self::StrafeRight`]
    /// if only that exists, and the forward walk or run if neither does.
    StrafeLeft,
    /// Moving sideways to the character's right. Optional, as [`Self::StrafeLeft`].
    StrafeRight,
    /// Dying, ending on the ground, and held there. Optional: every hero and monster has one,
    /// and a model delivered without it simply stops where it stood.
    Death,
    /// A one-handed slash. Optional: an ability whose motion is a swing borrows the ordinary
    /// cast's follow-through on a model without it.
    Swing,
    /// A sweeping blow around the body. Optional, as [Self::Swing].
    Cleave,
    /// A two-handed overhead smash. Optional, as [Self::Swing].
    Smash,
    /// A shove with the shield. Optional, as [Self::Swing].
    Bash,
    /// A forward charge or lunge. Optional, as [Self::Swing].
    Lunge,
    /// Raising the guard. Optional, as [Self::Swing].
    Guard,
    /// Gathering strength: a flex or a clenched fist. Optional, as [Self::Swing].
    PowerUp,
    /// A shout. Optional, as [Self::Swing].
    Battlecry,
}

impl AnimationSlot {
    /// Every slot, in a stable order. The array [`ClipBindings`] is indexed by.
    pub const ALL: [Self; 21] = [
        Self::Idle,
        Self::Walk,
        Self::Run,
        Self::Jump,
        Self::Fall,
        Self::Land,
        Self::Cast,
        Self::CastHeavy,
        Self::WalkBackward,
        Self::RunBackward,
        Self::StrafeLeft,
        Self::StrafeRight,
        Self::Death,
        Self::Swing,
        Self::Cleave,
        Self::Smash,
        Self::Bash,
        Self::Lunge,
        Self::Guard,
        Self::PowerUp,
        Self::Battlecry,
    ];

    /// The slots every character is expected to fill. The rest are refinements a character
    /// may add and the visual layer falls back from: a model without them still moves.
    pub const REQUIRED: [Self; 8] = [
        Self::Idle,
        Self::Walk,
        Self::Run,
        Self::Jump,
        Self::Fall,
        Self::Land,
        Self::Cast,
        Self::CastHeavy,
    ];

    /// The pairs of slots that are each other's mirror image, left first.
    pub const MIRRORED: [(Self, Self); 1] = [(Self::StrafeLeft, Self::StrafeRight)];

    /// This slot's position in [`AnimationSlot::ALL`].
    #[must_use]
    pub fn index(self) -> usize {
        match self {
            Self::Idle => 0,
            Self::Walk => 1,
            Self::Run => 2,
            Self::Jump => 3,
            Self::Fall => 4,
            Self::Land => 5,
            Self::Cast => 6,
            Self::CastHeavy => 7,
            Self::WalkBackward => 8,
            Self::RunBackward => 9,
            Self::StrafeLeft => 10,
            Self::StrafeRight => 11,
            Self::Death => 12,
            Self::Swing => 13,
            Self::Cleave => 14,
            Self::Smash => 15,
            Self::Bash => 16,
            Self::Lunge => 17,
            Self::Guard => 18,
            Self::PowerUp => 19,
            Self::Battlecry => 20,
        }
    }

    /// Short uppercase name, for the developer overlay.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Walk => "WALK",
            Self::Run => "RUN",
            Self::Jump => "JUMP",
            Self::Fall => "FALL",
            Self::Land => "LAND",
            Self::Cast => "CAST",
            Self::CastHeavy => "CAST+",
            Self::WalkBackward => "WALK BACK",
            Self::RunBackward => "RUN BACK",
            Self::StrafeLeft => "STRAFE L",
            Self::StrafeRight => "STRAFE R",
            Self::Death => "DEATH",
            Self::Swing => "SWING",
            Self::Cleave => "CLEAVE",
            Self::Smash => "SMASH",
            Self::Bash => "BASH",
            Self::Lunge => "LUNGE",
            Self::Guard => "GUARD",
            Self::PowerUp => "POWER UP",
            Self::Battlecry => "BATTLECRY",
        }
    }

    /// Whether a character may leave this slot empty.
    #[must_use]
    pub fn is_optional(self) -> bool {
        !Self::REQUIRED.contains(&self)
    }

    /// Whether a clip in this slot should repeat.
    ///
    /// A property of the *role*, not of the clip: every character's idle loops and every
    /// character's landing does not, so this is one rule rather than a flag each character
    /// could get wrong.
    #[must_use]
    pub fn loops(self) -> bool {
        matches!(
            self,
            Self::Idle
                | Self::Walk
                | Self::Run
                | Self::Fall
                | Self::WalkBackward
                | Self::RunBackward
                | Self::StrafeLeft
                | Self::StrafeRight
        )
    }
}

/// Which clip index fills each slot, for one character.
///
/// An array rather than a map: a dozen slots, resolved once, read every frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClipBindings {
    clips: [Option<usize>; AnimationSlot::ALL.len()],
}

impl ClipBindings {
    /// The clip filling `slot`, if the character has one.
    #[must_use]
    pub fn get(&self, slot: AnimationSlot) -> Option<usize> {
        self.clips[slot.index()]
    }

    /// The clip filling `slot`, falling back to `fallback`'s.
    ///
    /// How a character that has no walk animation still moves believably: the controller
    /// asks for `Walk` and gets `Run`, rather than the character sliding in its idle pose.
    /// Deliberately one level deep, so a missing clip is visible rather than silently
    /// resolving through a chain to something unrelated.
    #[must_use]
    pub fn get_or(&self, slot: AnimationSlot, fallback: AnimationSlot) -> Option<usize> {
        self.get(slot).or_else(|| self.get(fallback))
    }

    /// Binds `slot` to a clip index.
    pub fn set(&mut self, slot: AnimationSlot, clip: usize) {
        self.clips[slot.index()] = Some(clip);
    }

    /// How many slots are filled.
    #[must_use]
    pub fn len(&self) -> usize {
        self.clips.iter().filter(|clip| clip.is_some()).count()
    }

    /// Whether no slot is filled.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Everything the visual layer needs to know about one kind of character.
///
/// Plain data. It names a model rather than holding one, so a definition can be written
/// before the model exists and can be pointed at a different model without being rebuilt.
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterDef {
    /// What this character is called, for diagnostics.
    pub name: String,
    /// Which model to draw.
    pub model: ModelId,
    /// How tall the model is authored, in metres.
    ///
    /// The visual layer scales by `gameplay_height / model_height`, so a model authored at
    /// any size lines up with the capsule the simulation actually collides with. Gameplay
    /// stays authoritative about how tall the character is; the model just has to say what
    /// it was drawn at.
    pub model_height: f32,
    /// Horizontal speed, m/s, at or below which the walk animation plays.
    pub walk_speed: f32,
    /// Horizontal speed, m/s, at or above which the run animation plays.
    pub run_speed: f32,
    /// How long the landing animation holds before locomotion resumes, seconds.
    pub land_time: f32,
    /// Which clip fills each slot, by name.
    pub clips: Vec<(AnimationSlot, String)>,
}

impl CharacterDef {
    /// A definition with no clips bound.
    #[must_use]
    pub fn new(name: impl Into<String>, model: ModelId, model_height: f32) -> Self {
        Self {
            name: name.into(),
            model,
            model_height,
            walk_speed: 1.6,
            run_speed: 5.0,
            land_time: 0.18,
            clips: Vec::new(),
        }
    }

    /// The same definition with `slot` bound to the clip called `clip`.
    #[must_use]
    pub fn with_clip(mut self, slot: AnimationSlot, clip: impl Into<String>) -> Self {
        self.clips.push((slot, clip.into()));
        self
    }

    /// The same definition asking for the optional directional clips by their conventional
    /// names: `walk_back`, `run_back`, `strafe_left`, `strafe_right`.
    ///
    /// Asking costs nothing: a model without them resolves anyway and the visual layer falls
    /// back. A model delivered with them is picked up without a code change.
    #[must_use]
    pub fn with_directional_clips(self) -> Self {
        self.with_clip(AnimationSlot::WalkBackward, "walk_back")
            .with_clip(AnimationSlot::RunBackward, "run_back")
            .with_clip(AnimationSlot::StrafeLeft, "strafe_left")
            .with_clip(AnimationSlot::StrafeRight, "strafe_right")
    }

    /// The same definition with its locomotion speeds replaced.
    #[must_use]
    pub fn with_speeds(mut self, walk: f32, run: f32) -> Self {
        self.walk_speed = walk.max(0.0);
        self.run_speed = run.max(self.walk_speed);
        self
    }

    /// Resolves every clip name against `model`.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::Invalid`] naming the first clip the model does not contain. A
    /// mistyped clip name would otherwise be a character that silently never runs, which is
    /// exactly the class of bug that survives to a build.
    pub fn resolve(&self, model: &ModelAsset) -> Result<ClipBindings, AssetError> {
        let mut bindings = ClipBindings::default();
        for (slot, name) in &self.clips {
            let Some(index) = model.clip_index(name) else {
                if slot.is_optional() {
                    // A refinement this model does not carry: the visual layer falls back.
                    continue;
                }
                return Err(AssetError::invalid(
                    &self.name,
                    format!("{} clip", slot.as_str()),
                    format!("the model has no animation called '{name}'"),
                ));
            };
            bindings.set(*slot, index);
        }
        Ok(bindings)
    }

    /// Adds the missing half of every mirrored pair this definition binds.
    ///
    /// A model delivered with a left strafe and no right one gets the left strafe mirrored
    /// under the name the definition gives the right, and the other way round. A model with
    /// both, or neither, or no skeleton, is left as it is. Returns how many clips were made.
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::Invalid`] if a clip was not made for the model's skeleton.
    pub fn mirror_missing(&self, model: &mut ModelAsset) -> Result<usize, AssetError> {
        let name_of = |slot: AnimationSlot| {
            self.clips
                .iter()
                .find(|(bound, _)| *bound == slot)
                .map(|(_, name)| name.clone())
        };
        let mut made = 0;
        for (left, right) in AnimationSlot::MIRRORED {
            let (Some(left), Some(right)) = (name_of(left), name_of(right)) else {
                continue;
            };
            let (source, missing) = match (model.clip_index(&left), model.clip_index(&right)) {
                (Some(source), None) => (source, right),
                (None, Some(source)) => (source, left),
                _ => continue,
            };
            let Some(skeleton) = model.skeleton.as_ref() else {
                continue;
            };
            let clip = crate::mirror::mirrored(&model.clips[source], skeleton, missing)?;
            model.clips.push(clip);
            made += 1;
        }
        Ok(made)
    }

    /// Makes every clip this definition binds loop or play once, as its slot says.
    ///
    /// glTF has no notion of looping, so an importer marks every clip as looping. Left that
    /// way, a cast replays for as long as the cast lasts and a landing restarts before it
    /// is over. A procedural model already builds its clips this way, so applying it there
    /// changes nothing.
    pub fn apply_playback(&self, model: &mut ModelAsset) {
        for (slot, name) in &self.clips {
            if let Some(index) = model.clip_index(name) {
                model.clips[index].looping = slot.loops();
            }
        }
    }

    /// The uniform scale that makes this character `height` metres tall.
    ///
    /// A model with no authored height is drawn at 1:1 rather than at infinity.
    #[must_use]
    pub fn scale_for_height(&self, height: f32) -> f32 {
        if self.model_height > 1e-3 {
            height / self.model_height
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::AnimationClip;
    use crate::material::MaterialAsset;
    use crate::mesh::{Geometry, MeshVertex, SubMesh};
    use engine_core::math::{Vec2, Vec3};

    fn model_with_clips(names: &[&str]) -> ModelAsset {
        let vertices = vec![
            MeshVertex::new(Vec3::ZERO, Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::X, Vec3::Y, Vec2::ZERO),
            MeshVertex::new(Vec3::Y, Vec3::Y, Vec2::ZERO),
        ];
        let mut model = ModelAsset::rigid(
            "test",
            vec![SubMesh::new(
                "tri",
                Geometry::Static(vertices),
                vec![0, 1, 2],
                0,
            )],
            vec![MaterialAsset::default()],
            vec![],
        );
        model.clips = names
            .iter()
            .map(|name| AnimationClip::new(*name, true, vec![]))
            .collect();
        model
    }

    #[test]
    fn every_slot_has_a_distinct_index_inside_the_array() {
        let mut seen = vec![];
        for slot in AnimationSlot::ALL {
            assert!(slot.index() < AnimationSlot::ALL.len());
            assert!(!seen.contains(&slot.index()), "{slot:?} collides");
            seen.push(slot.index());
        }
        assert_eq!(seen.len(), AnimationSlot::ALL.len());
    }

    #[test]
    fn an_imported_one_shot_clip_stops_looping_once_its_slot_is_applied() {
        // An importer marks every clip looping; the slot is what knows better.
        let mut model = model_with_clips(&["idle", "cast"]);
        let def = CharacterDef::new("test", ModelId::new("test"), 1.8)
            .with_clip(AnimationSlot::Idle, "idle")
            .with_clip(AnimationSlot::Cast, "cast");
        def.apply_playback(&mut model);
        assert!(model.clips[0].looping);
        assert!(!model.clips[1].looping);
    }

    #[test]
    fn an_optional_clip_the_model_lacks_is_left_unbound_rather_than_refused() {
        let model = model_with_clips(&["idle"]);
        let def = CharacterDef::new("test", ModelId::new("test"), 1.8)
            .with_clip(AnimationSlot::Idle, "idle")
            .with_directional_clips();
        let Ok(bindings) = def.resolve(&model) else {
            panic!("optional clips must not block a model that lacks them");
        };
        assert_eq!(bindings.get(AnimationSlot::Idle), Some(0));
        assert_eq!(bindings.get(AnimationSlot::StrafeLeft), None);
    }

    #[test]
    fn a_required_clip_the_model_lacks_is_still_refused() {
        let model = model_with_clips(&["idle"]);
        let def = CharacterDef::new("test", ModelId::new("test"), 1.8)
            .with_clip(AnimationSlot::Run, "run");
        assert!(def.resolve(&model).is_err());
    }

    #[test]
    fn a_strafe_delivered_for_one_side_is_mirrored_to_the_other() {
        use crate::animation::{BoneTrack, Key};
        use crate::skeleton::{Bone, Skeleton};
        use engine_core::math::{Mat4, Quat};
        use engine_core::transform::Transform;

        let bone = |name: &str, parent: Option<u16>, x: f32| Bone {
            name: name.to_owned(),
            parent,
            rest: Transform::from_translation(Vec3::new(x, 0.0, 0.0)),
            inverse_bind: Mat4::IDENTITY,
        };
        let Ok(skeleton) = Skeleton::new(vec![
            bone("root", None, 0.0),
            bone("LeftLeg", Some(0), 0.2),
            bone("RightLeg", Some(0), -0.2),
        ]) else {
            panic!("the test skeleton must build");
        };
        let step = AnimationClip::new(
            "strafe_left",
            true,
            vec![
                BoneTrack::new(1)
                    .with_rotation(vec![Key::new(0.5, Quat::from_axis_angle(Vec3::Z, 0.4))]),
            ],
        )
        .with_speed(4.0);
        let mut model = model_with_clips(&[]).with_animation(skeleton, vec![step]);
        let def = CharacterDef::new("test", ModelId::new("test"), 1.8).with_directional_clips();

        assert!(matches!(def.mirror_missing(&mut model), Ok(1)));
        let Some(right) = model.clip_index("strafe_right") else {
            panic!("the right strafe must have been made");
        };
        assert_eq!(
            model.clips[right].speed,
            Some(4.0),
            "the authored speed carries over"
        );
        // Both present now: a second pass makes nothing.
        assert!(matches!(def.mirror_missing(&mut model), Ok(0)));
    }

    #[test]
    fn looping_is_a_property_of_the_role_not_of_the_character() {
        assert!(AnimationSlot::Idle.loops());
        assert!(AnimationSlot::Run.loops());
        assert!(!AnimationSlot::Land.loops());
        assert!(!AnimationSlot::Jump.loops());
        assert!(!AnimationSlot::Cast.loops());
    }

    #[test]
    fn a_mistyped_clip_name_fails_at_load_rather_than_never_playing() {
        let model = model_with_clips(&["idle", "run"]);
        let def = CharacterDef::new("mage", ModelId::new("m"), 1.8)
            .with_clip(AnimationSlot::Idle, "idle")
            .with_clip(AnimationSlot::Run, "runn");
        let Err(error) = def.resolve(&model) else {
            panic!("a clip the model does not have must be reported");
        };
        assert!(error.to_string().contains("runn"), "{error}");
    }

    #[test]
    fn resolved_bindings_are_indices_into_the_model() {
        let model = model_with_clips(&["idle", "walk", "run"]);
        let def = CharacterDef::new("mage", ModelId::new("m"), 1.8)
            .with_clip(AnimationSlot::Idle, "idle")
            .with_clip(AnimationSlot::Run, "run");
        let Ok(bindings) = def.resolve(&model) else {
            panic!("every named clip exists");
        };
        assert_eq!(bindings.get(AnimationSlot::Idle), Some(0));
        assert_eq!(bindings.get(AnimationSlot::Run), Some(2));
        assert_eq!(bindings.get(AnimationSlot::Walk), None);
        assert_eq!(bindings.len(), 2);
    }

    #[test]
    fn a_missing_slot_falls_back_one_level_and_no_further() {
        let model = model_with_clips(&["idle", "run"]);
        let def =
            CharacterDef::new("mage", ModelId::new("m"), 1.8).with_clip(AnimationSlot::Run, "run");
        let Ok(bindings) = def.resolve(&model) else {
            panic!("every named clip exists");
        };
        assert_eq!(
            bindings.get_or(AnimationSlot::Walk, AnimationSlot::Run),
            Some(1)
        );
        // Jump falls back to Fall, which is also unbound: the answer is None, not a search.
        assert_eq!(
            bindings.get_or(AnimationSlot::Jump, AnimationSlot::Fall),
            None
        );
    }

    #[test]
    fn a_model_is_scaled_to_the_height_gameplay_says_the_character_is() {
        let def = CharacterDef::new("mage", ModelId::new("m"), 1.8);
        assert!((def.scale_for_height(0.9) - 0.5).abs() < 1e-6);

        // A model that forgot to say how tall it is must not scale to infinity.
        let unmeasured = CharacterDef::new("mage", ModelId::new("m"), 0.0);
        assert_eq!(unmeasured.scale_for_height(2.0), 1.0);
    }

    #[test]
    fn run_speed_cannot_be_below_walk_speed() {
        let def = CharacterDef::new("mage", ModelId::new("m"), 1.8).with_speeds(3.0, 1.0);
        assert_eq!(def.walk_speed, 3.0);
        assert_eq!(def.run_speed, 3.0);
    }
}
