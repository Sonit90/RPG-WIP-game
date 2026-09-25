//! Monsters drawn as animated characters: one [`CharacterVisual`] per living enemy.
//!
//! ```text
//! EnemyArchetype.look.model ──► MonsterModels (name → model, clips, upload)
//!                                      │
//! Body, EnemyBrain::cast(), events ──► CharacterState ──► CharacterVisual ──► skinned draw
//! ```
//!
//! The same seam the player's character goes through, read from an enemy instead: where it
//! stands and which way it faces, how fast it moves, the cast its brain is running, the tick
//! its last cast started and its last blow landed, and whether it is dead. Nothing here
//! knows which monster is which — a table row names a model, and a name with no model behind
//! it falls back to the rigid stand-in the extractor has always drawn.
//!
//! Controllers are kept per gameplay slot and generation, like the rigid path's render
//! entities, so a monster keeps its place in its clip from frame to frame and a recycled
//! slot starts a new one.

use std::sync::Arc;

use engine_assets::{CharacterDef, ModelAsset};
use engine_core::math::Vec3;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::sim::Simulation;
use engine_render_data::{InstanceStyle, ModelBinding, RenderWorld};

use crate::character::{CastVisual, CharacterState, CharacterVisual, Heading};

/// One monster model, uploaded: what a table row's `look.model` resolves to.
#[derive(Debug, Clone)]
pub struct MonsterModel {
    /// The name table rows ask for: `"skeleton_warrior"`.
    pub name: String,
    /// Which clip plays in which slot, and the speeds they were authored at.
    pub definition: CharacterDef,
    /// Skeleton and clips.
    pub model: Arc<ModelAsset>,
    /// The uploaded geometry.
    pub binding: ModelBinding,
}

/// The monster models, and a controller for every enemy drawn with one.
#[derive(Default)]
pub struct MonsterVisuals {
    models: Vec<MonsterModel>,
    slots: Vec<Option<Slot>>,
    /// This frame's latest `(cast started, ability used)` tick per gameplay slot.
    ticks: Vec<(Option<u64>, Option<u64>)>,
}

struct Slot {
    generation: u32,
    model: usize,
    visual: CharacterVisual,
}

/// Everything about one enemy this frame that its animation is driven from.
#[derive(Debug, Clone, Copy)]
pub struct MonsterFrame {
    /// Feet, interpolated.
    pub position: Vec3,
    /// Facing, interpolated.
    pub facing_yaw: f32,
    /// Horizontal speed.
    pub speed: f32,
    /// Movement relative to facing.
    pub heading: Heading,
    /// On the ground.
    pub grounded: bool,
    /// Rising rather than falling.
    pub rising: bool,
    /// Gameplay height, which the model is scaled to.
    pub height: f32,
    /// The cast in progress: `(duration, fraction)`.
    pub cast: Option<(f32, f32)>,
    /// Dead.
    pub dead: bool,
}

impl MonsterVisuals {
    /// An empty set: every enemy is drawn with the rigid stand-in.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the models. Controllers built on the old ones are dropped.
    pub fn set_models(&mut self, models: Vec<MonsterModel>) {
        self.models = models;
        self.slots.clear();
    }

    /// The models, for a count in the log.
    #[must_use]
    pub fn models(&self) -> &[MonsterModel] {
        &self.models
    }

    /// Which model `name` resolves to.
    #[must_use]
    pub fn model_index(&self, name: &str) -> Option<usize> {
        self.models.iter().position(|model| model.name == name)
    }

    /// How many enemies are currently drawn animated.
    #[must_use]
    pub fn animated(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// The controller drawing `id`, if it is drawn with a monster model.
    #[must_use]
    pub fn visual(&self, id: EntityId) -> Option<&CharacterVisual> {
        self.slots
            .get(id.index())?
            .as_ref()
            .filter(|slot| slot.generation == id.generation())
            .map(|slot| &slot.visual)
    }

    /// Reads this frame's events: the tick each enemy last started a cast and last used an
    /// ability. One pass over the slice for every enemy, rather than one per enemy.
    pub fn observe(&mut self, simulation: &Simulation) {
        let count = simulation.world().slot_count();
        self.ticks.clear();
        self.ticks.resize(count, (None, None));
        for event in simulation.events() {
            let (source, tick, started) = match event {
                CombatEvent::CastStarted { source, tick, .. } => (*source, *tick, true),
                CombatEvent::AbilityUsed { source, tick, .. } => (*source, *tick, false),
                _ => continue,
            };
            let Some(entry) = self.ticks.get_mut(source.index()) else {
                continue;
            };
            if started {
                entry.0 = Some(entry.0.map_or(tick, |seen| seen.max(tick)));
            } else {
                entry.1 = Some(entry.1.map_or(tick, |seen| seen.max(tick)));
            }
        }
    }

    /// Advances and draws enemy `id` with model `model`.
    #[allow(
        clippy::too_many_arguments,
        reason = "one enemy's frame and where it is drawn into"
    )]
    pub fn drive(
        &mut self,
        id: EntityId,
        model: usize,
        frame: MonsterFrame,
        delta: f32,
        style: InstanceStyle,
        world: &mut RenderWorld,
        camera: Vec3,
        lod_bias: f32,
    ) {
        let index = id.index();
        if self.slots.len() <= index {
            self.slots.resize_with(index + 1, || None);
        }
        let stale = self.slots[index]
            .as_ref()
            .is_none_or(|slot| slot.generation != id.generation() || slot.model != model);
        if stale {
            self.slots[index] = self.controller(model).map(|visual| Slot {
                generation: id.generation(),
                model,
                visual,
            });
        }
        let Some(slot) = self.slots[index].as_mut() else {
            return;
        };
        let (cast_started, released) = self.ticks.get(index).copied().unwrap_or((None, None));
        let state = CharacterState {
            position: frame.position,
            facing_yaw: frame.facing_yaw,
            speed: frame.speed,
            heading: frame.heading,
            grounded: frame.grounded,
            rising: frame.rising,
            height: frame.height,
            cast: frame
                .cast
                .map(|(duration, _)| CastVisual::for_duration(duration)),
            cast_progress: frame.cast.map_or(0.0, |(_, fraction)| fraction),
            cast_started,
            released,
            dead: frame.dead,
        };
        if let Err(error) = slot.visual.update(delta, state) {
            log::error!("monster animation failed: {error}");
        }
        slot.visual.submit_styled(world, camera, lod_bias, style);
    }

    /// Drops the controller in slot `index`: the enemy there is gone, or is drawn rigid.
    pub fn forget(&mut self, index: usize) {
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = None;
        }
    }

    fn controller(&self, model: usize) -> Option<CharacterVisual> {
        let entry = self.models.get(model)?;
        match CharacterVisual::new(
            entry.definition.clone(),
            Arc::clone(&entry.model),
            entry.binding.clone(),
        ) {
            Ok(visual) => Some(visual.with_whole_instants()),
            Err(error) => {
                log::error!("monster '{}' cannot be animated: {error}", entry.name);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_assets::procedural::{MONSTER_MODELS, monster_character, monster_model};
    use engine_render_data::{
        MaterialHandle, ModelLodBinding, ModelPart, PartGeometry, SkinnedMeshHandle,
    };

    /// A binding over handles nothing uploaded; the controller never dereferences them.
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

    fn visual(name: &str) -> CharacterVisual {
        let Some(Ok(model)) = monster_model(name) else {
            panic!("{name} must build");
        };
        let Some(definition) = monster_character(name) else {
            panic!("{name} has a definition");
        };
        let model = Arc::new(model);
        let binding = binding(&model);
        match CharacterVisual::new(definition, model, binding) {
            Ok(visual) => visual.with_whole_instants(),
            Err(error) => panic!("{name}: {error}"),
        }
    }

    #[test]
    fn a_held_blade_points_the_way_it_was_authored_at_rest() {
        // Lord Ashgrave's greatsword is written in the resting hand's frame pointing forward
        // and a little down. Posed at idle, its tip must be in front of the hand (-Z) and
        // below it, not standing up beside him.
        let mut visual = visual("lord_ashgrave");
        assert!(visual.update(0.001, CharacterState::default()).is_ok());
        let prop = visual.palette()[usize::from(engine_assets::procedural::humanoid::PROP)];
        let hand_bind = Vec3::new(-0.65, 1.39, 0.0);
        let Some(Ok(model)) = monster_model("lord_ashgrave") else {
            panic!("the boss must build");
        };
        // The blade's farthest vertex from the hand, in bind space.
        let blade = model.lods[0]
            .submeshes
            .iter()
            .find(|submesh| submesh.name == "blade")
            .map(|submesh| {
                submesh
                    .geometry
                    .positions()
                    .max_by(|a, b| {
                        (*a - hand_bind)
                            .length()
                            .total_cmp(&(*b - hand_bind).length())
                    })
                    .unwrap_or(hand_bind)
            })
            .unwrap_or(hand_bind);
        let hand = prop.transform_point(hand_bind);
        let tip = prop.transform_point(blade);
        let direction = tip - hand;
        assert!(direction.z < -0.8, "the blade points {direction:?}");
        assert!(direction.y < 0.0, "the blade points {direction:?}");
    }

    #[test]
    fn a_dead_monster_ends_up_lying_on_the_floor() {
        for name in MONSTER_MODELS {
            let mut visual = visual(name);
            assert!(visual.update(0.016, CharacterState::default()).is_ok());
            let dead = CharacterState {
                dead: true,
                ..CharacterState::default()
            };
            for _ in 0..120 {
                assert!(visual.update(0.016, dead).is_ok());
            }
            assert_eq!(visual.slot(), engine_assets::AnimationSlot::Death, "{name}");
            let chest = visual.palette()[usize::from(engine_assets::procedural::humanoid::CHEST)];
            let posed = chest.transform_point(Vec3::new(0.0, 1.26, 0.0));
            assert!(
                posed.y < 0.45,
                "{name}'s chest is {posed:?} after its death played"
            );
            // And it gets back up when the fight is reset.
            for _ in 0..30 {
                assert!(visual.update(0.016, CharacterState::default()).is_ok());
            }
            assert_eq!(visual.slot(), engine_assets::AnimationSlot::Idle, "{name}");
        }
    }

    #[test]
    fn a_standing_monster_is_posed_not_left_in_its_t_pose() {
        for name in MONSTER_MODELS {
            let mut visual = visual(name);
            // One short frame: the pose must be there at once, not faded into from the T-pose.
            assert!(visual.update(0.001, CharacterState::default()).is_ok());
            // Where the left hand's bind position goes: at rest in the T-pose it is level
            // with the shoulder, 1.39 m up; hanging, it is at the hip.
            let palette = visual.palette();
            let hand = palette[usize::from(engine_assets::procedural::humanoid::HAND_L)];
            let posed = hand.transform_point(Vec3::new(0.65, 1.39, 0.0));
            assert!(
                posed.y < 1.15,
                "{name}'s arm is still out in its T-pose: hand at {posed:?}"
            );
        }
    }
}
