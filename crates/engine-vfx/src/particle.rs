//! The particle pool: a fixed-capacity, structure-of-arrays CPU simulation.
//!
//! ```text
//! emit  ->  [ position | velocity | age | lifetime | size | colour | ... ]  ->  write
//!                          integrate, then compact the dead away
//! ```
//!
//! # Why the CPU and not the GPU
//!
//! A GPU particle system buys its advantage when the particle count is large enough that
//! the *per-particle* update dominates, and it costs a compute pass, a persistent buffer per
//! effect, an indirect draw and a readback story for anything the CPU needs to know. This
//! engine's combat produces thousands of particles, not millions, and the update below is
//! four multiply-adds over contiguous `f32` arrays — the kind of loop that vectorizes and
//! that a 5800X runs in microseconds. Milestone 21 measures it; if the measurement ever says
//! otherwise, the emitters and the draw list stay exactly as they are and only this file
//! changes.
//!
//! # Structure of arrays
//!
//! The update touches position, velocity, age and the two constants that drive them, and
//! never looks at colour. Interleaving them would drag colour, blend mode and owner through
//! every cache line the integrator reads.
//!
//! # Allocation
//!
//! Every array is allocated once at construction and never grows. Emission past the
//! capacity is refused and counted ([`ParticlePool::dropped`]), so a stress scene degrades
//! into fewer particles rather than into an allocation storm. Death is a `swap_remove`,
//! which is why nothing here holds a handle to a particle: they move.

use engine_core::math::Vec3;
use engine_render_data::{ParticleBlend, ParticleDrawList, ParticleInstance};

/// Owner slot meaning "nothing owns this any more".
///
/// A particle whose effect has retired keeps living out its own life where it is. That is
/// the difference between a trail and a leash: killing the trail with the bolt would delete
/// the smoke the bolt left behind.
pub const NO_OWNER: u32 = u32::MAX;

/// What one particle is born with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParticleSpawn {
    /// Where it starts, in world space.
    pub position: Vec3,
    /// How fast it is going, in metres per second.
    pub velocity: Vec3,
    /// Seconds it lives.
    pub lifetime: f32,
    /// Half-extent in metres at birth and at death.
    pub size: (f32, f32),
    /// Linear RGBA at birth.
    pub color_start: [f32; 4],
    /// Linear RGBA at death.
    pub color_end: [f32; 4],
    /// Radians about the view axis at birth.
    pub rotation: f32,
    /// Radians per second it turns.
    pub spin: f32,
    /// Metres per second squared, downwards.
    pub gravity: f32,
    /// Fraction of speed shed per second.
    pub drag: f32,
    /// How it is blended.
    pub blend: ParticleBlend,
    /// Which instance it rides, or [`NO_OWNER`] if it stays where it is put.
    pub owner: u32,
    /// The generation of that instance, so a reused slot does not adopt it.
    pub owner_generation: u32,
}

/// Every live particle, in parallel arrays.
#[derive(Debug, Clone)]
pub struct ParticlePool {
    position: Vec<Vec3>,
    velocity: Vec<Vec3>,
    age: Vec<f32>,
    lifetime: Vec<f32>,
    size_start: Vec<f32>,
    size_end: Vec<f32>,
    color_start: Vec<[f32; 4]>,
    color_end: Vec<[f32; 4]>,
    rotation: Vec<f32>,
    spin: Vec<f32>,
    gravity: Vec<f32>,
    drag: Vec<f32>,
    blend: Vec<ParticleBlend>,
    owner: Vec<u32>,
    owner_generation: Vec<u32>,
    capacity: usize,
    spawned: u64,
    dropped: u64,
    peak: usize,
}

impl ParticlePool {
    /// An empty pool that will hold at most `capacity` particles.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            position: Vec::with_capacity(capacity),
            velocity: Vec::with_capacity(capacity),
            age: Vec::with_capacity(capacity),
            lifetime: Vec::with_capacity(capacity),
            size_start: Vec::with_capacity(capacity),
            size_end: Vec::with_capacity(capacity),
            color_start: Vec::with_capacity(capacity),
            color_end: Vec::with_capacity(capacity),
            rotation: Vec::with_capacity(capacity),
            spin: Vec::with_capacity(capacity),
            gravity: Vec::with_capacity(capacity),
            drag: Vec::with_capacity(capacity),
            blend: Vec::with_capacity(capacity),
            owner: Vec::with_capacity(capacity),
            owner_generation: Vec::with_capacity(capacity),
            capacity,
            spawned: 0,
            dropped: 0,
            peak: 0,
        }
    }

    /// Particles alive.
    #[must_use]
    pub fn len(&self) -> usize {
        self.position.len()
    }

    /// Whether nothing is alive.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.position.is_empty()
    }

    /// The most particles that can be alive at once.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The most that ever were alive at once, this run.
    #[must_use]
    pub fn peak(&self) -> usize {
        self.peak
    }

    /// How many have ever been emitted.
    #[must_use]
    pub fn spawned(&self) -> u64 {
        self.spawned
    }

    /// How many emissions were refused because the pool was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Adds a particle, or counts it as dropped if the pool is full.
    ///
    /// Returns whether it was added. A particle with no lifetime is refused without being
    /// counted: it would be dead before it was drawn.
    pub fn emit(&mut self, spawn: ParticleSpawn) -> bool {
        if spawn.lifetime <= 0.0 {
            return false;
        }
        if self.len() >= self.capacity {
            self.dropped += 1;
            return false;
        }
        self.position.push(spawn.position);
        self.velocity.push(spawn.velocity);
        self.age.push(0.0);
        self.lifetime.push(spawn.lifetime);
        self.size_start.push(spawn.size.0);
        self.size_end.push(spawn.size.1);
        self.color_start.push(spawn.color_start);
        self.color_end.push(spawn.color_end);
        self.rotation.push(spawn.rotation);
        self.spin.push(spawn.spin);
        self.gravity.push(spawn.gravity);
        self.drag.push(spawn.drag);
        self.blend.push(spawn.blend);
        self.owner.push(spawn.owner);
        self.owner_generation.push(spawn.owner_generation);
        self.spawned += 1;
        self.peak = self.peak.max(self.len());
        true
    }

    /// Advances every particle by `seconds` and removes the ones that finished.
    ///
    /// `owner_motion` is how far each instance slot moved this frame, and
    /// `owner_generations` is the generation living in that slot. A particle riding an
    /// instance is carried by the motion; one whose owner has retired — or whose slot has
    /// been reused by a different effect — is left where it is and continues under its own
    /// velocity.
    pub fn update(&mut self, seconds: f32, owner_motion: &[Vec3], owner_generations: &[u32]) {
        if seconds <= 0.0 {
            return;
        }
        let mut index = 0;
        while index < self.position.len() {
            self.age[index] += seconds;
            if self.age[index] >= self.lifetime[index] {
                self.remove(index);
                continue;
            }

            // Gravity, then drag, then the step. Drag as a fraction shed per second and
            // clamped, so a large `drag` on a long frame stops the particle rather than
            // reversing it.
            let mut velocity = self.velocity[index];
            velocity.y -= self.gravity[index] * seconds;
            let shed = (1.0 - self.drag[index] * seconds).clamp(0.0, 1.0);
            velocity *= shed;

            let mut position = self.position[index] + velocity * seconds;
            let owner = self.owner[index];
            if owner != NO_OWNER
                && let Some(motion) = owner_motion.get(owner as usize)
                && owner_generations.get(owner as usize) == Some(&self.owner_generation[index])
            {
                position += *motion;
            }

            self.velocity[index] = velocity;
            self.position[index] = position;
            self.rotation[index] += self.spin[index] * seconds;
            index += 1;
        }
    }

    /// Removes the particle in `index` by swapping the last one into its place.
    ///
    /// Order is not meaningful — nothing addresses a particle by index across a frame, and
    /// the draw list is not depth sorted — so a swap is the whole of the removal.
    fn remove(&mut self, index: usize) {
        self.position.swap_remove(index);
        self.velocity.swap_remove(index);
        self.age.swap_remove(index);
        self.lifetime.swap_remove(index);
        self.size_start.swap_remove(index);
        self.size_end.swap_remove(index);
        self.color_start.swap_remove(index);
        self.color_end.swap_remove(index);
        self.rotation.swap_remove(index);
        self.spin.swap_remove(index);
        self.gravity.swap_remove(index);
        self.drag.swap_remove(index);
        self.blend.swap_remove(index);
        self.owner.swap_remove(index);
        self.owner_generation.swap_remove(index);
    }

    /// Appends every live particle to `list`, interpolated to where it is now.
    pub fn write(&self, list: &mut ParticleDrawList) {
        for index in 0..self.position.len() {
            let life = self.lifetime[index];
            let fraction = if life > 0.0 {
                (self.age[index] / life).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let size = lerp(self.size_start[index], self.size_end[index], fraction);
            let color = lerp_color(self.color_start[index], self.color_end[index], fraction);
            let position = self.position[index];
            list.push(
                self.blend[index],
                ParticleInstance {
                    position: [position.x, position.y, position.z],
                    size,
                    color,
                    rotation: self.rotation[index],
                },
            );
        }
    }

    /// Kills every particle riding `owner`, whatever its age.
    ///
    /// For a stop that has to be immediate — a shield that ends the instant the buff does.
    /// Ordinary retirement does *not* do this: a trail outlives the thing that left it.
    pub fn kill_owned_by(&mut self, owner: u32, generation: u32) {
        let mut index = 0;
        while index < self.position.len() {
            if self.owner[index] == owner && self.owner_generation[index] == generation {
                self.remove(index);
            } else {
                index += 1;
            }
        }
    }

    /// Detaches every particle riding `owner`, leaving them where they are.
    pub fn detach(&mut self, owner: u32, generation: u32) {
        for index in 0..self.owner.len() {
            if self.owner[index] == owner && self.owner_generation[index] == generation {
                self.owner[index] = NO_OWNER;
            }
        }
    }

    /// Removes every particle, keeping the allocations and the counters.
    pub fn clear(&mut self) {
        self.position.clear();
        self.velocity.clear();
        self.age.clear();
        self.lifetime.clear();
        self.size_start.clear();
        self.size_end.clear();
        self.color_start.clear();
        self.color_end.clear();
        self.rotation.clear();
        self.spin.clear();
        self.gravity.clear();
        self.drag.clear();
        self.blend.clear();
        self.owner.clear();
        self.owner_generation.clear();
    }

    /// Forgets the run counters. For a test or a scene change.
    pub fn reset_counters(&mut self) {
        self.spawned = 0;
        self.dropped = 0;
        self.peak = self.len();
    }

    /// Where a live particle is. For tests and diagnostics.
    #[must_use]
    pub fn position_at(&self, index: usize) -> Option<Vec3> {
        self.position.get(index).copied()
    }
}

/// Linear interpolation.
fn lerp(from: f32, to: f32, fraction: f32) -> f32 {
    from + (to - from) * fraction
}

/// Linear interpolation of a colour, alpha included.
fn lerp_color(from: [f32; 4], to: [f32; 4], fraction: f32) -> [f32; 4] {
    [
        lerp(from[0], to[0], fraction),
        lerp(from[1], to[1], fraction),
        lerp(from[2], to[2], fraction),
        lerp(from[3], to[3], fraction),
    ]
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn spawn() -> ParticleSpawn {
        ParticleSpawn {
            position: Vec3::ZERO,
            velocity: Vec3::new(0.0, 0.0, 0.0),
            lifetime: 1.0,
            size: (1.0, 0.0),
            color_start: [1.0, 1.0, 1.0, 1.0],
            color_end: [1.0, 0.0, 0.0, 0.0],
            rotation: 0.0,
            spin: 0.0,
            gravity: 0.0,
            drag: 0.0,
            blend: ParticleBlend::Additive,
            owner: NO_OWNER,
            owner_generation: 0,
        }
    }

    #[test]
    fn a_particle_lives_exactly_as_long_as_it_was_given() {
        let mut pool = ParticlePool::with_capacity(8);
        assert!(pool.emit(spawn()));
        assert_eq!(pool.len(), 1);

        pool.update(0.9, &[], &[]);
        assert_eq!(pool.len(), 1, "it died early");
        pool.update(0.2, &[], &[]);
        assert!(pool.is_empty(), "it outlived its lifetime");
    }

    #[test]
    fn a_particle_moves_under_its_velocity_and_gravity() {
        let mut pool = ParticlePool::with_capacity(8);
        pool.emit(ParticleSpawn {
            velocity: Vec3::new(2.0, 0.0, 0.0),
            gravity: 10.0,
            lifetime: 10.0,
            ..spawn()
        });
        pool.update(0.5, &[], &[]);

        let position = pool.position_at(0).expect("it is alive");
        assert!(position.x > 0.9 && position.x < 1.1, "{position:?}");
        assert!(
            position.y < 0.0,
            "gravity pulled it the wrong way: {position:?}"
        );
    }

    #[test]
    fn drag_slows_a_particle_without_reversing_it() {
        // A long frame with a strong drag used to flip the velocity, which sent a spray of
        // sparks backwards through the caster.
        let mut pool = ParticlePool::with_capacity(8);
        pool.emit(ParticleSpawn {
            velocity: Vec3::new(10.0, 0.0, 0.0),
            drag: 20.0,
            lifetime: 10.0,
            ..spawn()
        });
        pool.update(0.5, &[], &[]);
        let position = pool.position_at(0).expect("it is alive");
        assert!(position.x >= 0.0, "drag reversed it: {position:?}");
    }

    #[test]
    fn an_attached_particle_is_carried_by_its_owner() {
        let mut pool = ParticlePool::with_capacity(8);
        pool.emit(ParticleSpawn {
            lifetime: 10.0,
            owner: 0,
            owner_generation: 1,
            ..spawn()
        });
        pool.update(0.1, &[Vec3::new(0.0, 0.0, -1.0)], &[1]);
        let position = pool.position_at(0).expect("it is alive");
        assert!((position.z + 1.0).abs() < 1e-5, "{position:?}");
    }

    #[test]
    fn a_particle_whose_slot_was_reused_is_not_adopted() {
        // The generation is what stops a new effect in a recycled slot from dragging the
        // previous effect's smoke around with it.
        let mut pool = ParticlePool::with_capacity(8);
        pool.emit(ParticleSpawn {
            lifetime: 10.0,
            owner: 0,
            owner_generation: 1,
            ..spawn()
        });
        pool.update(0.1, &[Vec3::new(5.0, 0.0, 0.0)], &[2]);
        let position = pool.position_at(0).expect("it is alive");
        assert!(position.x.abs() < 1e-5, "{position:?}");
    }

    #[test]
    fn detaching_leaves_the_particles_where_they_are() {
        let mut pool = ParticlePool::with_capacity(8);
        pool.emit(ParticleSpawn {
            lifetime: 10.0,
            owner: 3,
            owner_generation: 1,
            ..spawn()
        });
        pool.detach(3, 1);
        pool.update(
            0.1,
            &[Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::new(9.0, 0.0, 0.0)],
            &[1, 1, 1, 1],
        );
        let position = pool.position_at(0).expect("it is still alive");
        assert!(position.x.abs() < 1e-5, "{position:?}");
        assert_eq!(pool.len(), 1, "detaching is not killing");
    }

    #[test]
    fn killing_an_owners_particles_removes_only_theirs() {
        let mut pool = ParticlePool::with_capacity(8);
        for owner in [1_u32, 2, 1] {
            pool.emit(ParticleSpawn {
                lifetime: 10.0,
                owner,
                owner_generation: 1,
                ..spawn()
            });
        }
        pool.kill_owned_by(1, 1);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn a_full_pool_drops_rather_than_grows() {
        let mut pool = ParticlePool::with_capacity(4);
        for _ in 0..10 {
            pool.emit(spawn());
        }
        assert_eq!(pool.len(), 4);
        assert_eq!(pool.dropped(), 6);
        assert_eq!(pool.spawned(), 4);
        assert_eq!(pool.peak(), 4);
    }

    #[test]
    fn a_particle_with_no_life_is_refused_without_being_a_drop() {
        let mut pool = ParticlePool::with_capacity(4);
        assert!(!pool.emit(ParticleSpawn {
            lifetime: 0.0,
            ..spawn()
        }));
        assert_eq!(pool.dropped(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn writing_interpolates_size_and_colour_over_the_life() {
        let mut pool = ParticlePool::with_capacity(4);
        pool.emit(spawn());
        let mut list = ParticleDrawList::with_capacity(16);

        pool.write(&mut list);
        assert_eq!(list.additive()[0].size, 1.0);
        assert_eq!(list.additive()[0].color[3], 1.0);

        pool.update(0.5, &[], &[]);
        list.clear();
        pool.write(&mut list);
        let half = list.additive()[0];
        assert!((half.size - 0.5).abs() < 1e-5, "{half:?}");
        assert!((half.color[3] - 0.5).abs() < 1e-5, "{half:?}");
    }

    #[test]
    fn blend_modes_reach_the_right_side_of_the_draw_list() {
        let mut pool = ParticlePool::with_capacity(4);
        pool.emit(spawn());
        pool.emit(ParticleSpawn {
            blend: ParticleBlend::Alpha,
            ..spawn()
        });
        let mut list = ParticleDrawList::with_capacity(16);
        pool.write(&mut list);
        assert_eq!(list.additive().len(), 1);
        assert_eq!(list.alpha().len(), 1);
    }

    #[test]
    fn a_steady_state_pool_stops_allocating() {
        // The whole reason the pool is fixed capacity and death is a swap.
        let mut pool = ParticlePool::with_capacity(256);
        for _ in 0..256 {
            pool.emit(spawn());
        }
        let allocated = pool.position.capacity();
        for _ in 0..100 {
            pool.update(0.02, &[], &[]);
            for _ in 0..8 {
                pool.emit(spawn());
            }
        }
        assert_eq!(pool.position.capacity(), allocated, "the pool reallocated");
    }
}
