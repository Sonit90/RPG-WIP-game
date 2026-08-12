//! The VFX runtime: instances, their lifecycle, and what they emit and draw.
//!
//! ```text
//! VfxAsset  --spawn-->  VfxInstance  --update-->  particles + mesh draws  --retire-->  slot
//!                            ^                                                          |
//!                            +------------------ reused ---------------------------------+
//! ```
//!
//! # Lifecycle in one paragraph
//!
//! [`VfxSystem::spawn`] takes a slot — a freed one if there is one — and returns a
//! [`VfxHandle`] carrying that slot's generation. [`VfxSystem::update`] ages every instance,
//! runs its emitters into the particle pool, rebuilds this frame's mesh draws, and retires
//! anything whose life is over. Retiring bumps the slot's generation, which makes every
//! outstanding handle to it stale — the same mechanism `RenderWorld` uses, for the same
//! reason: a caller that kept a handle across a retirement must not be able to address the
//! effect that replaced it.
//!
//! # What retirement does to particles
//!
//! It **detaches** them. A bolt's trail is not part of the bolt: killing it with the bolt
//! would delete the smoke as the impact starts, which is the one frame the player is
//! looking. An effect that has to take its particles with it — a shield ending the instant
//! the buff does — is stopped with [`VfxSystem::stop_and_clear`].
//!
//! # Allocation
//!
//! Instances are a `Vec` with a free list, the particle pool is fixed capacity, and the
//! mesh-draw list is cleared rather than freed each frame. After the first busy fight the
//! whole system allocates nothing; `MAX_INSTANCES` is a real bound and the refusals are
//! counted rather than silently ignored.

use engine_core::math::Vec3;
use engine_render_data::ParticleDrawList;

use crate::asset::{EmitterDesc, LayerAlign, MAX_EMITTERS, SpawnShape, VfxId, VfxShape};
use crate::particle::{NO_OWNER, ParticlePool, ParticleSpawn};
use crate::rng::Rng;

/// Instances that can exist at once by default.
///
/// A hundred is well past what combat produces — the stress scenario in Milestone 22 peaks
/// far below it — and the bound exists so the system's memory is a number rather than a
/// function of how long the session has run.
pub const DEFAULT_MAX_INSTANCES: usize = 128;

/// Particles that can exist at once by default.
pub const DEFAULT_MAX_PARTICLES: usize = 16_384;

/// Names one live effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VfxHandle {
    index: u32,
    generation: u32,
}

impl VfxHandle {
    /// The slot this handle names.
    #[must_use]
    pub fn index(self) -> usize {
        self.index as usize
    }

    /// The generation it was issued with.
    #[must_use]
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// What a caller supplies to start an effect.
///
/// Every geometric field is in **metres taken from gameplay**: the radius an ability hits
/// with, the length a line burns. The asset decides proportions, the spawn decides size, and
/// that split is what keeps a visual honest about what it covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VfxSpawn {
    /// Which effect.
    pub effect: VfxId,
    /// Where it is anchored, in world space.
    pub position: Vec3,
    /// Which way it points, radians about +Y. Zero faces −Z, the engine's convention.
    pub yaw: f32,
    /// Its radius in metres. Layers scale their own multipliers by this.
    pub radius: f32,
    /// Its length in metres, for the shapes that run along their facing.
    pub length: f32,
    /// Seconds it lives, or `None` to use the asset's own lifetime.
    pub lifetime: Option<f32>,
    /// A colour multiplied into every layer and particle, or `None` to leave the asset's.
    pub tint: Option<[f32; 4]>,
    /// An external key — an entity — this effect rides, or `None` to stand still.
    pub attach: Option<u32>,
    /// Metres above the anchor the effect is centred.
    pub height: f32,
}

impl VfxSpawn {
    /// An effect at a place, with the asset's own lifetime and no attachment.
    #[must_use]
    pub fn at(effect: VfxId, position: Vec3) -> Self {
        Self {
            effect,
            position,
            yaw: 0.0,
            radius: 1.0,
            length: 1.0,
            lifetime: None,
            tint: None,
            attach: None,
            height: 0.0,
        }
    }

    /// The same spawn with a radius in metres.
    #[must_use]
    pub fn with_radius(mut self, radius: f32) -> Self {
        self.radius = radius.max(0.0);
        self
    }

    /// The same spawn with a length in metres.
    #[must_use]
    pub fn with_length(mut self, length: f32) -> Self {
        self.length = length.max(0.0);
        self
    }

    /// The same spawn facing `yaw`.
    #[must_use]
    pub fn facing(mut self, yaw: f32) -> Self {
        self.yaw = yaw;
        self
    }

    /// The same spawn living for `seconds`.
    #[must_use]
    pub fn lasting(mut self, seconds: f32) -> Self {
        self.lifetime = Some(seconds.max(0.0));
        self
    }

    /// The same spawn riding `key`.
    #[must_use]
    pub fn attached_to(mut self, key: u32) -> Self {
        self.attach = Some(key);
        self
    }

    /// The same spawn tinted.
    #[must_use]
    pub fn tinted(mut self, tint: [f32; 4]) -> Self {
        self.tint = Some(tint);
        self
    }

    /// The same spawn raised `metres` above its anchor.
    #[must_use]
    pub fn raised(mut self, metres: f32) -> Self {
        self.height = metres;
        self
    }
}

/// One piece of geometry an effect wants drawn this frame.
///
/// Deliberately not a render entity: this crate cannot name a mesh handle. Presentation
/// binds [`VfxShape`] to uploaded geometry once and turns these into instances of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshDraw {
    /// Which shape.
    pub shape: VfxShape,
    /// Where its origin is, in world space.
    pub position: Vec3,
    /// Which way it points, radians about +Y.
    pub yaw: f32,
    /// Its radius in metres — half-width for the shapes that have one.
    pub radius: f32,
    /// Its length in metres, for a strip.
    pub length: f32,
    /// Its height in metres, for the shapes that stand up.
    pub height: f32,
    /// Linear RGBA multiplied into the material presentation binds.
    pub tint: [f32; 4],
    /// Emissive term added to the surface.
    pub emissive: f32,
    /// Whether it lies on the ground and should be lifted clear of it.
    pub ground: bool,
}

/// What the system is currently holding, for a readout and for the budget.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct VfxStats {
    /// Effects alive.
    pub instances: usize,
    /// Particles alive.
    pub particles: usize,
    /// Mesh draws this frame.
    pub mesh_draws: usize,
    /// Effects ever started.
    pub spawned: u64,
    /// Effects refused because the system was full.
    pub dropped_instances: u64,
    /// Particles refused because the pool was full.
    pub dropped_particles: u64,
    /// The most particles ever alive at once, this run.
    pub peak_particles: usize,
}

/// One live effect.
#[derive(Debug, Clone, Copy)]
struct Instance {
    effect: VfxId,
    position: Vec3,
    yaw: f32,
    radius: f32,
    length: f32,
    height: f32,
    tint: Option<[f32; 4]>,
    attach: Option<u32>,
    age: f32,
    lifetime: f32,
    /// Fraction of the life the previous update ended at, so a window is entered exactly
    /// once however long a frame is.
    previous_fraction: f32,
    /// Fractional particles owed by each rate emitter, carried between frames.
    ///
    /// Without it, an emitter of 22 particles a second at 300 FPS would owe 0.07 particles
    /// each frame, round to zero, and emit nothing at all.
    accumulator: [f32; MAX_EMITTERS],
    alive: bool,
}

/// Every live effect, the particles they have thrown, and what they draw.
#[derive(Debug)]
pub struct VfxSystem {
    instances: Vec<Instance>,
    generations: Vec<u32>,
    free: Vec<u32>,
    /// How far each slot moved on the last update, for the particles riding it.
    motion: Vec<Vec3>,
    particles: ParticlePool,
    meshes: Vec<MeshDraw>,
    rng: Rng,
    live: usize,
    max_instances: usize,
    spawned: u64,
    dropped_instances: u64,
}

impl Default for VfxSystem {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_INSTANCES, DEFAULT_MAX_PARTICLES, 0xF12E_0F0F)
    }
}

impl VfxSystem {
    /// A system bounded at `max_instances` effects and `max_particles` particles.
    #[must_use]
    pub fn new(max_instances: usize, max_particles: usize, seed: u64) -> Self {
        Self {
            instances: Vec::with_capacity(max_instances),
            generations: Vec::with_capacity(max_instances),
            free: Vec::with_capacity(max_instances),
            motion: Vec::with_capacity(max_instances),
            particles: ParticlePool::with_capacity(max_particles),
            meshes: Vec::with_capacity(max_instances * 2),
            rng: Rng::new(seed),
            live: 0,
            max_instances,
            spawned: 0,
            dropped_instances: 0,
        }
    }

    /// Starts an effect, or returns `None` if the system is full.
    ///
    /// Full is a real answer rather than an error: a frame that wanted a hundred and first
    /// explosion gets a hundred, and the refusal is counted so the cap can be judged against
    /// what combat actually asks for.
    pub fn spawn(&mut self, spawn: VfxSpawn) -> Option<VfxHandle> {
        let asset = spawn.effect.asset();
        let lifetime = spawn.lifetime.unwrap_or(asset.lifetime);
        if lifetime <= 0.0 {
            return None;
        }

        let instance = Instance {
            effect: spawn.effect,
            position: spawn.position + Vec3::new(0.0, spawn.height, 0.0),
            yaw: spawn.yaw,
            radius: spawn.radius,
            length: spawn.length,
            height: spawn.height,
            tint: spawn.tint,
            attach: spawn.attach,
            age: 0.0,
            lifetime,
            // Below zero so a window starting at exactly zero is still *entered* on the
            // first update rather than being treated as already open.
            previous_fraction: -1.0,
            accumulator: [0.0; MAX_EMITTERS],
            alive: true,
        };

        let index = match self.free.pop() {
            Some(index) => {
                let slot = index as usize;
                self.instances[slot] = instance;
                self.motion[slot] = Vec3::ZERO;
                index
            }
            None => {
                if self.instances.len() >= self.max_instances {
                    self.dropped_instances += 1;
                    return None;
                }
                self.instances.push(instance);
                self.motion.push(Vec3::ZERO);
                // Generation 1, not 0, so a default-constructed handle names nothing.
                self.generations.push(1);
                (self.instances.len() - 1) as u32
            }
        };

        self.live += 1;
        self.spawned += 1;
        Some(VfxHandle {
            index,
            generation: self.generations[index as usize],
        })
    }

    /// Whether the handle still names a live effect.
    #[must_use]
    pub fn contains(&self, handle: VfxHandle) -> bool {
        let slot = handle.index();
        slot < self.instances.len()
            && self.generations[slot] == handle.generation
            && self.instances[slot].alive
    }

    /// Moves an effect, if the handle is still live.
    ///
    /// What a bolt in flight uses: presentation decides where it is this frame, and the
    /// effect's particles are carried with it.
    pub fn set_position(&mut self, handle: VfxHandle, position: Vec3) -> bool {
        if !self.contains(handle) {
            return false;
        }
        let slot = handle.index();
        let raised = position + Vec3::new(0.0, self.instances[slot].height, 0.0);
        self.motion[slot] += raised - self.instances[slot].position;
        self.instances[slot].position = raised;
        true
    }

    /// Turns an effect, if the handle is still live.
    pub fn set_yaw(&mut self, handle: VfxHandle, yaw: f32) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.instances[handle.index()].yaw = yaw;
        true
    }

    /// Ends an effect now, leaving its particles to live out their own lives.
    pub fn stop(&mut self, handle: VfxHandle) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.retire(handle.index(), false);
        true
    }

    /// Ends an effect now and removes its particles with it.
    ///
    /// For a state that stops rather than finishes: a shield whose buff expired should not
    /// leave a second of sparks where the shield used to be.
    pub fn stop_and_clear(&mut self, handle: VfxHandle) -> bool {
        if !self.contains(handle) {
            return false;
        }
        self.retire(handle.index(), true);
        true
    }

    /// Where an effect is, if it is still live.
    #[must_use]
    pub fn position_of(&self, handle: VfxHandle) -> Option<Vec3> {
        self.contains(handle)
            .then(|| self.instances[handle.index()].position)
    }

    /// How far through its life an effect is, in `0..=1`.
    #[must_use]
    pub fn progress_of(&self, handle: VfxHandle) -> Option<f32> {
        if !self.contains(handle) {
            return None;
        }
        let instance = &self.instances[handle.index()];
        Some((instance.age / instance.lifetime).clamp(0.0, 1.0))
    }

    /// Effects alive.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live
    }

    /// Whether nothing is playing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// This frame's geometry.
    #[must_use]
    pub fn mesh_draws(&self) -> &[MeshDraw] {
        &self.meshes
    }

    /// The particle pool, for diagnostics.
    #[must_use]
    pub fn particles(&self) -> &ParticlePool {
        &self.particles
    }

    /// What the system is holding.
    #[must_use]
    pub fn stats(&self) -> VfxStats {
        VfxStats {
            instances: self.live,
            particles: self.particles.len(),
            mesh_draws: self.meshes.len(),
            spawned: self.spawned,
            dropped_instances: self.dropped_instances,
            dropped_particles: self.particles.dropped(),
            peak_particles: self.particles.peak(),
        }
    }

    /// Ends everything at once, particles included. For a scene change or a reset.
    pub fn clear(&mut self) {
        for slot in 0..self.instances.len() {
            if self.instances[slot].alive {
                self.retire(slot, true);
            }
        }
        self.particles.clear();
        self.meshes.clear();
    }

    /// Ages every effect, emits what they emit, and rebuilds this frame's geometry.
    ///
    /// `anchor_of` answers where an attached effect's owner is now; `None` means the owner
    /// has gone, and the effect ends rather than hanging in the air where the target used to
    /// stand.
    pub fn update(&mut self, seconds: f32, mut anchor_of: impl FnMut(u32) -> Option<Vec3>) {
        self.meshes.clear();
        if seconds < 0.0 {
            return;
        }

        for slot in 0..self.instances.len() {
            if !self.instances[slot].alive {
                self.motion[slot] = Vec3::ZERO;
                continue;
            }

            // Follow the owner first, so this frame's emission comes out of where the target
            // is now rather than out of where it was last frame.
            if let Some(key) = self.instances[slot].attach {
                match anchor_of(key) {
                    Some(anchor) => {
                        let raised = anchor + Vec3::new(0.0, self.instances[slot].height, 0.0);
                        self.motion[slot] += raised - self.instances[slot].position;
                        self.instances[slot].position = raised;
                    }
                    None => {
                        // The thing it was riding is gone. The effect goes with it; the
                        // particles it already threw do not.
                        self.retire(slot, false);
                        continue;
                    }
                }
            }

            self.instances[slot].age += seconds;
            let instance = self.instances[slot];
            let fraction = (instance.age / instance.lifetime).clamp(0.0, 1.0);

            self.run_emitters(slot, fraction, seconds);
            self.push_layers(&instance, fraction);

            self.instances[slot].previous_fraction = fraction;
            if instance.age >= instance.lifetime {
                self.retire(slot, false);
            }
        }

        self.particles
            .update(seconds, &self.motion, &self.generations);
        for motion in &mut self.motion {
            *motion = Vec3::ZERO;
        }
    }

    /// Appends every live particle to `list`.
    pub fn write_particles(&self, list: &mut ParticleDrawList) {
        self.particles.write(list);
    }

    /// Frees a slot, making every handle to it stale.
    fn retire(&mut self, slot: usize, kill_particles: bool) {
        if !self.instances[slot].alive {
            return;
        }
        let generation = self.generations[slot];
        if kill_particles {
            self.particles.kill_owned_by(slot as u32, generation);
        } else {
            self.particles.detach(slot as u32, generation);
        }
        self.instances[slot].alive = false;
        self.generations[slot] = generation.wrapping_add(1).max(1);
        self.motion[slot] = Vec3::ZERO;
        self.free.push(slot as u32);
        self.live -= 1;
    }

    /// Runs one instance's emitters for this frame.
    fn run_emitters(&mut self, slot: usize, fraction: f32, seconds: f32) {
        let instance = self.instances[slot];
        let asset = instance.effect.asset();
        for (index, emitter) in asset.emitters.iter().enumerate() {
            if index >= MAX_EMITTERS {
                break;
            }
            let open = emitter.window.contains(fraction);
            // A window is *entered* when this frame crossed its start. Compared against the
            // previous fraction rather than tested for equality, because at 300 FPS a
            // fraction almost never lands exactly on a boundary and a burst tested that way
            // would fire on some frame rates and not others.
            let entered = instance.previous_fraction < emitter.window.start
                && fraction >= emitter.window.start;

            if entered && emitter.burst > 0 {
                for _ in 0..emitter.burst {
                    self.emit_one(slot, emitter);
                }
            }
            if !open || emitter.rate <= 0.0 {
                continue;
            }

            let owed = self.instances[slot].accumulator[index] + emitter.rate * seconds;
            let count = owed as u32;
            self.instances[slot].accumulator[index] = owed - count as f32;
            for _ in 0..count {
                self.emit_one(slot, emitter);
            }
        }
    }

    /// Throws one particle from `emitter`, scattered by the instance's own geometry.
    fn emit_one(&mut self, slot: usize, emitter: &EmitterDesc) {
        let instance = self.instances[slot];
        let forward = Vec3::new(-instance.yaw.sin(), 0.0, -instance.yaw.cos());
        let right = Vec3::new(-forward.z, 0.0, forward.x);

        let (offset, direction) = match emitter.shape {
            SpawnShape::Point => (Vec3::ZERO, self.random_direction()),
            SpawnShape::Sphere { radius } => {
                let direction = self.random_direction();
                (direction * (radius * self.rng.unit()), direction)
            }
            SpawnShape::Disc { radius } => {
                let angle = self.rng.unit() * std::f32::consts::TAU;
                // The square root is what makes the scatter uniform over the *area*: without
                // it every ring of fire is dense in the middle and thin at the edge, which
                // reads as a effect that does not reach where it says it does.
                let distance = radius * instance.radius * self.rng.unit().sqrt();
                let offset = Vec3::new(angle.cos() * distance, 0.0, angle.sin() * distance);
                let outward = Vec3::new(angle.cos(), 0.6, angle.sin());
                (offset, outward.normalize())
            }
            SpawnShape::Cone { half_angle } => {
                let spread = half_angle * self.rng.signed();
                let lift = half_angle * 0.5 * self.rng.signed();
                let yaw = instance.yaw + spread;
                let direction = Vec3::new(-yaw.sin(), lift, -yaw.cos()).normalize();
                (Vec3::ZERO, direction)
            }
            SpawnShape::Line { length, width } => {
                let along = length * instance.length * self.rng.unit();
                let across = width * instance.radius * 0.5 * self.rng.signed();
                let offset = forward * along + right * across;
                let direction =
                    Vec3::new(self.rng.signed() * 0.4, 1.0, self.rng.signed() * 0.4).normalize();
                (offset, direction)
            }
        };

        let speed = self.rng.range(emitter.speed.0, emitter.speed.1);
        let lifetime = self.rng.range(emitter.lifetime.0, emitter.lifetime.1);
        let (owner, generation) = if emitter.attached {
            (slot as u32, self.generations[slot])
        } else {
            (NO_OWNER, 0)
        };
        let tint = instance.tint.unwrap_or([1.0, 1.0, 1.0, 1.0]);

        self.particles.emit(ParticleSpawn {
            position: instance.position + offset,
            velocity: direction * speed,
            lifetime,
            size: emitter.size,
            color_start: modulate(emitter.color_start, tint),
            color_end: modulate(emitter.color_end, tint),
            rotation: self.rng.unit() * std::f32::consts::TAU,
            spin: self.rng.range(emitter.spin.0, emitter.spin.1),
            gravity: emitter.gravity,
            drag: emitter.drag,
            blend: emitter.blend,
            owner,
            owner_generation: generation,
        });
    }

    /// A direction drawn from the unit sphere.
    fn random_direction(&mut self) -> Vec3 {
        // Rejection-free: a height and an angle, which is the standard uniform sampling of a
        // sphere and costs one sine, one cosine and a square root.
        let height = self.rng.signed();
        let radius = (1.0 - height * height).max(0.0).sqrt();
        let angle = self.rng.unit() * std::f32::consts::TAU;
        Vec3::new(angle.cos() * radius, height, angle.sin() * radius)
    }

    /// Appends this frame's geometry for one instance.
    fn push_layers(&mut self, instance: &Instance, fraction: f32) {
        let asset = instance.effect.asset();
        let tint = instance.tint.unwrap_or([1.0, 1.0, 1.0, 1.0]);
        for layer in asset.layers {
            if !layer.window.contains(fraction) {
                continue;
            }
            let progress = layer.window.progress(fraction);
            let radius_scale = lerp(layer.radius.0, layer.radius.1, progress);
            let height = lerp(layer.height.0, layer.height.1, progress);
            let emissive = lerp(layer.emissive.0, layer.emissive.1, progress).max(0.0);
            let base = if layer.uses_length {
                instance.length
            } else {
                instance.radius
            };
            let position = match layer.align {
                LayerAlign::Ground => Vec3::new(
                    instance.position.x,
                    instance.position.y - instance.height,
                    instance.position.z,
                ),
                LayerAlign::Upright => instance.position,
            };
            self.meshes.push(MeshDraw {
                shape: layer.shape,
                position,
                yaw: instance.yaw + layer.spin * instance.age,
                radius: instance.radius * radius_scale,
                length: base * radius_scale,
                height,
                tint: modulate(layer.color, tint),
                emissive,
                ground: layer.shape.is_ground(),
            });
        }
    }
}

/// Multiplies two linear colours, component by component.
fn modulate(color: [f32; 4], tint: [f32; 4]) -> [f32; 4] {
    [
        color[0] * tint[0],
        color[1] * tint[1],
        color[2] * tint[2],
        color[3] * tint[3],
    ]
}

/// Linear interpolation.
fn lerp(from: f32, to: f32, fraction: f32) -> f32 {
    from + (to - from) * fraction
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn system() -> VfxSystem {
        VfxSystem::new(8, 4_096, 42)
    }

    #[test]
    fn an_effect_lives_for_its_lifetime_and_then_frees_its_slot() {
        let mut system = system();
        let handle = system
            .spawn(VfxSpawn::at(VfxId::Impact, Vec3::ZERO))
            .expect("the system has room");
        assert!(system.contains(handle));
        assert_eq!(system.len(), 1);

        let lifetime = VfxId::Impact.asset().lifetime;
        system.update(lifetime * 0.5, |_| None);
        assert!(system.contains(handle), "it ended early");

        system.update(lifetime, |_| None);
        assert!(!system.contains(handle));
        assert!(system.is_empty());
    }

    #[test]
    fn a_retired_slot_is_reused_and_the_old_handle_cannot_address_it() {
        // The generation rule, which is what stops a caller holding a stale handle from
        // moving somebody else's effect.
        let mut system = system();
        let first = system
            .spawn(VfxSpawn::at(VfxId::Impact, Vec3::ZERO))
            .expect("room");
        system.update(10.0, |_| None);
        let second = system
            .spawn(VfxSpawn::at(VfxId::Explosion, Vec3::ZERO))
            .expect("room");

        assert_eq!(first.index(), second.index(), "the slot must be reused");
        assert!(!system.contains(first));
        assert!(system.contains(second));
        assert!(!system.set_position(first, Vec3::new(9.0, 9.0, 9.0)));
        assert_eq!(
            system.position_of(second),
            Some(Vec3::ZERO),
            "a stale handle moved the effect that replaced it"
        );
    }

    #[test]
    fn a_burst_emitter_fires_once_however_the_frame_is_chopped_up() {
        // Once per window entry, not once per frame and not once per boundary hit exactly.
        let mut system = system();
        system
            .spawn(VfxSpawn::at(VfxId::Impact, Vec3::ZERO))
            .expect("room");
        system.update(0.01, |_| None);
        let after_first = system.particles().len();
        assert!(after_first > 0, "the burst never fired");

        for _ in 0..5 {
            system.update(0.01, |_| None);
        }
        // Impact has no rate emitters, so nothing more may be thrown.
        assert_eq!(
            system.particles().len(),
            after_first,
            "the burst fired more than once"
        );
    }

    #[test]
    fn a_rate_emitter_throws_the_same_number_at_any_frame_rate() {
        // The accumulator's whole purpose: at 300 FPS an emitter of 22 a second owes 0.07
        // particles a frame, and truncating that to zero emits nothing at all.
        let mut counts = Vec::new();
        for (steps, step) in [(1_u32, 1.0_f32), (60, 1.0 / 60.0), (300, 1.0 / 300.0)] {
            let mut system = VfxSystem::new(8, 4_096, 42);
            system
                .spawn(VfxSpawn::at(VfxId::Ignited, Vec3::ZERO).lasting(4.0))
                .expect("room");
            for _ in 0..steps {
                system.update(step, |_| None);
            }
            counts.push(system.particles().spawned());
        }
        let expected = VfxId::Ignited.asset().emitters[0].rate as u64;
        for count in &counts {
            assert!(
                count.abs_diff(expected) <= 2,
                "one second of emission produced {counts:?}, expected about {expected}"
            );
        }
    }

    #[test]
    fn an_attached_effect_follows_its_owner_and_ends_when_the_owner_goes() {
        let mut system = system();
        let handle = system
            .spawn(VfxSpawn::at(VfxId::Ignited, Vec3::ZERO).attached_to(7))
            .expect("room");

        system.update(0.1, |key| {
            assert_eq!(key, 7);
            Some(Vec3::new(3.0, 0.0, 0.0))
        });
        assert_eq!(system.position_of(handle), Some(Vec3::new(3.0, 0.0, 0.0)));

        system.update(0.1, |_| None);
        assert!(!system.contains(handle), "it outlived the target it burned");
    }

    #[test]
    fn a_retired_effects_particles_live_out_their_own_lives() {
        // A trail is not part of the bolt: deleting it with the bolt would take the smoke
        // away on exactly the frame the impact starts.
        let mut system = system();
        let handle = system
            .spawn(VfxSpawn::at(VfxId::Projectile, Vec3::ZERO).lasting(0.2))
            .expect("room");
        system.update(0.1, |_| None);
        let thrown = system.particles().len();
        assert!(thrown > 0);

        system.stop(handle);
        assert!(system.is_empty());
        assert_eq!(system.particles().len(), thrown, "the trail was deleted");
    }

    #[test]
    fn stopping_and_clearing_takes_the_particles_with_it() {
        let mut system = system();
        let handle = system
            .spawn(VfxSpawn::at(VfxId::Shield, Vec3::ZERO).attached_to(1))
            .expect("room");
        system.update(0.2, |_| Some(Vec3::ZERO));
        assert!(!system.particles().is_empty());

        system.stop_and_clear(handle);
        assert_eq!(
            system.particles().len(),
            0,
            "the shield left sparks behind after it ended"
        );
    }

    #[test]
    fn geometry_is_sized_by_the_gameplay_radius_it_was_given() {
        // The accuracy requirement at the level this crate can state it: a layer multiplies
        // the radius it was handed, and the handler is the ability's own number.
        let mut system = system();
        system
            .spawn(VfxSpawn::at(VfxId::GroundArea, Vec3::ZERO).with_radius(6.0))
            .expect("room");
        system.update(0.016, |_| None);

        let fill = system
            .mesh_draws()
            .iter()
            .find(|draw| draw.shape == VfxShape::Disc)
            .copied()
            .expect("the area draws a fill");
        assert!((fill.radius - 6.0).abs() < 1e-4, "{fill:?}");

        let ring = system
            .mesh_draws()
            .iter()
            .find(|draw| draw.shape == VfxShape::Ring)
            .copied()
            .expect("the area draws an edge");
        assert!(
            ring.radius > fill.radius,
            "the edge must show past the fill"
        );
        assert!(ring.radius < fill.radius * 1.1, "and only just");
    }

    #[test]
    fn a_layer_outside_its_window_is_not_drawn() {
        let mut system = system();
        system
            .spawn(VfxSpawn::at(VfxId::Explosion, Vec3::ZERO).with_radius(3.0))
            .expect("room");
        system.update(0.05, |_| None);
        assert!(
            system
                .mesh_draws()
                .iter()
                .any(|draw| draw.shape == VfxShape::Shell),
            "the flash is missing at the start"
        );

        // Past the flash's window, the shell is gone and the ring is not.
        let lifetime = VfxId::Explosion.asset().lifetime;
        system.update(lifetime * 0.7, |_| None);
        assert!(
            !system
                .mesh_draws()
                .iter()
                .any(|draw| draw.shape == VfxShape::Shell),
            "the flash outstayed its window"
        );
    }

    #[test]
    fn the_system_refuses_rather_than_grows_when_it_is_full() {
        let mut system = VfxSystem::new(2, 64, 1);
        assert!(
            system
                .spawn(VfxSpawn::at(VfxId::Impact, Vec3::ZERO))
                .is_some()
        );
        assert!(
            system
                .spawn(VfxSpawn::at(VfxId::Impact, Vec3::ZERO))
                .is_some()
        );
        assert!(
            system
                .spawn(VfxSpawn::at(VfxId::Impact, Vec3::ZERO))
                .is_none()
        );
        assert_eq!(system.stats().dropped_instances, 1);
        assert_eq!(system.len(), 2);
    }

    #[test]
    fn a_tint_reaches_both_the_geometry_and_the_particles() {
        let mut system = system();
        system
            .spawn(
                VfxSpawn::at(VfxId::GroundArea, Vec3::ZERO)
                    .with_radius(2.0)
                    .tinted([0.0, 1.0, 0.0, 1.0]),
            )
            .expect("room");
        system.update(0.05, |_| None);

        let draw = system.mesh_draws()[0];
        assert_eq!(draw.tint[0], 0.0, "the red channel survived the tint");
        assert!(draw.tint[1] > 0.0);

        let mut list = ParticleDrawList::with_capacity(4_096);
        system.write_particles(&mut list);
        assert!(!list.is_empty(), "the area threw no particles");
        assert!(
            list.additive()
                .iter()
                .all(|particle| particle.color[0] == 0.0)
        );
    }

    #[test]
    fn clearing_ends_everything_including_the_particles() {
        let mut system = system();
        for _ in 0..4 {
            system.spawn(VfxSpawn::at(VfxId::Explosion, Vec3::ZERO));
        }
        system.update(0.05, |_| None);
        assert!(!system.is_empty() && !system.particles().is_empty());

        system.clear();
        assert!(system.is_empty());
        assert_eq!(system.particles().len(), 0);
        assert!(system.mesh_draws().is_empty());
    }

    #[test]
    fn a_long_run_of_combat_stops_allocating() {
        // The whole point of the free list and the fixed pool. Fifty explosions and a
        // thousand frames must not grow anything.
        let mut system = VfxSystem::new(64, 8_192, 5);
        for _ in 0..64 {
            system.spawn(VfxSpawn::at(VfxId::Explosion, Vec3::ZERO).with_radius(4.0));
        }
        for _ in 0..200 {
            system.update(0.016, |_| None);
        }
        let instances = system.instances.capacity();
        let meshes = system.meshes.capacity();

        for _ in 0..1_000 {
            system.spawn(VfxSpawn::at(VfxId::Impact, Vec3::new(1.0, 0.0, 0.0)));
            system.update(0.016, |_| None);
        }
        assert_eq!(system.instances.capacity(), instances);
        assert_eq!(system.meshes.capacity(), meshes);
    }
}
