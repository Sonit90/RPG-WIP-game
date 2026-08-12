//! This frame's particles: a flat list of camera-facing quads, ready to upload.
//!
//! ```text
//! VfxSystem  ->  ParticleDrawList  ->  renderer's particle pass  ->  one draw per blend
//! ```
//!
//! A [`ParticleInstance`] is what the GPU reads *per particle*, not per vertex: the quad's
//! four corners are built in the vertex shader from the camera's right and up axes, so a
//! particle costs 36 bytes of upload rather than six vertices of it. That decision belongs
//! here rather than in the renderer because it is what fixes the record's layout, and the
//! layout is what both sides have to agree on.
//!
//! # Why the list is split by blend mode rather than sorted
//!
//! There are two modes and they cannot be interleaved without either a per-particle sort
//! or a pipeline change per particle. Two arrays make the pass two draws, whatever the
//! particle count, and cost the producer one branch at push time.
//!
//! Within a mode, particles are **not** depth sorted. Additive blending is order
//! independent by construction, and the alpha-blended mode is used for soft smoke over a
//! scene it does not occlude. A depth sort of twenty thousand particles a frame would be
//! real CPU time bought for a difference that is not visible in fire.
//!
//! # Capacity
//!
//! Fixed. A list is created with a cap, [`ParticleDrawList::push`] refuses past it and
//! counts the refusal, and the arrays are cleared rather than freed every frame. A stress
//! scene therefore degrades into "the first N particles" instead of into an allocation
//! storm, and [`ParticleDrawList::dropped`] makes the degradation measurable rather than
//! invisible.

/// Which blending a particle is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticleBlend {
    /// `src * a + dst`. Fire, sparks, flashes — anything that adds light.
    Additive,
    /// `src * a + dst * (1 - a)`. Smoke and soot, which take light away.
    Alpha,
}

impl ParticleBlend {
    /// Both modes, in the order the renderer draws them.
    ///
    /// Additive first: smoke drawn over a flash reads as smoke lit by it, and the reverse
    /// order reads as a flash that has punched through the smoke.
    pub const ALL: [Self; 2] = [Self::Additive, Self::Alpha];

    /// Lowercase name, for diagnostics.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Additive => "additive",
            Self::Alpha => "alpha",
        }
    }
}

/// One particle, exactly as the vertex shader's per-instance stream declares it.
///
/// `repr(C)`: the renderer's input layout describes this memory, so Rust must not be free
/// to reorder it. 36 bytes with no padding — the fields are ordered so every one lands on
/// its natural alignment without a hole.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ParticleInstance {
    /// Centre, in world space.
    pub position: [f32; 3],
    /// Half-extent of the quad, in metres. Zero draws nothing.
    pub size: f32,
    /// Linear RGBA. Alpha is coverage for both blend modes, and the shader multiplies the
    /// procedural falloff into it, so a fading particle is an alpha ramp and nothing else.
    pub color: [f32; 4],
    /// Rotation about the view axis, in radians.
    ///
    /// Per particle rather than per effect: a burst of identical unrotated quads reads as
    /// one bright shape, and a random angle per particle is what makes it read as many.
    pub rotation: f32,
}

impl ParticleInstance {
    /// Whether this particle would cover any pixels.
    #[must_use]
    pub fn is_visible(&self) -> bool {
        self.size > 0.0 && self.color[3] > 0.0
    }
}

/// The particles to draw this frame, split by blend mode.
#[derive(Debug, Clone)]
pub struct ParticleDrawList {
    additive: Vec<ParticleInstance>,
    alpha: Vec<ParticleInstance>,
    capacity: usize,
    dropped: u32,
}

impl Default for ParticleDrawList {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_PARTICLE_CAPACITY)
    }
}

/// Particles a default list will draw before it starts dropping them.
///
/// Sized from the stress scenario rather than from taste: sixteen thousand particles is
/// 576 KiB of upload a frame, which sits beside the 4.4 MB of constants the same frame
/// already writes, and it is more simultaneous fire than the combat scene has ever
/// produced (Milestone 21 measures what it actually reaches).
pub const DEFAULT_PARTICLE_CAPACITY: usize = 16_384;

impl ParticleDrawList {
    /// An empty list that will hold at most `capacity` particles per frame.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            additive: Vec::with_capacity(capacity),
            alpha: Vec::with_capacity(capacity / 4),
            capacity,
            dropped: 0,
        }
    }

    /// Appends a particle, or counts it as dropped if the list is full.
    ///
    /// Returns whether it was kept. A particle with no size or no alpha is skipped without
    /// being counted as a drop: it is not a particle the list failed to hold, it is one
    /// that would have covered nothing.
    pub fn push(&mut self, blend: ParticleBlend, instance: ParticleInstance) -> bool {
        if !instance.is_visible() {
            return false;
        }
        if self.len() >= self.capacity {
            self.dropped += 1;
            return false;
        }
        match blend {
            ParticleBlend::Additive => self.additive.push(instance),
            ParticleBlend::Alpha => self.alpha.push(instance),
        }
        true
    }

    /// The additively blended particles.
    #[must_use]
    pub fn additive(&self) -> &[ParticleInstance] {
        &self.additive
    }

    /// The alpha blended particles.
    #[must_use]
    pub fn alpha(&self) -> &[ParticleInstance] {
        &self.alpha
    }

    /// The particles of one blend mode.
    #[must_use]
    pub fn of(&self, blend: ParticleBlend) -> &[ParticleInstance] {
        match blend {
            ParticleBlend::Additive => &self.additive,
            ParticleBlend::Alpha => &self.alpha,
        }
    }

    /// Particles held, across both modes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.additive.len() + self.alpha.len()
    }

    /// Whether nothing will be drawn.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.additive.is_empty() && self.alpha.is_empty()
    }

    /// The most particles this list will hold in one frame.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// How many particles have been refused because the list was full.
    ///
    /// Cumulative across frames, because the question it answers is "did this run ever run
    /// out of room", and a per-frame counter would answer it only for the frame that is
    /// being looked at.
    #[must_use]
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Empties the list for a new frame, keeping both allocations and the drop count.
    pub fn clear(&mut self) {
        self.additive.clear();
        self.alpha.clear();
    }

    /// Forgets the drop count. For a test or a scene change.
    pub fn reset_dropped(&mut self) {
        self.dropped = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn particle(size: f32) -> ParticleInstance {
        ParticleInstance {
            position: [1.0, 2.0, 3.0],
            size,
            color: [1.0, 0.5, 0.2, 1.0],
            rotation: 0.5,
        }
    }

    #[test]
    fn the_record_is_the_size_the_input_layout_will_describe() {
        // The renderer's per-instance element offsets are written against these numbers; a
        // hole or a reorder would make every particle read a neighbour's colour.
        assert_eq!(size_of::<ParticleInstance>(), 36);
        assert_eq!(std::mem::offset_of!(ParticleInstance, position), 0);
        assert_eq!(std::mem::offset_of!(ParticleInstance, size), 12);
        assert_eq!(std::mem::offset_of!(ParticleInstance, color), 16);
        assert_eq!(std::mem::offset_of!(ParticleInstance, rotation), 32);
    }

    #[test]
    fn particles_are_kept_apart_by_blend_mode() {
        let mut list = ParticleDrawList::with_capacity(8);
        assert!(list.push(ParticleBlend::Additive, particle(1.0)));
        assert!(list.push(ParticleBlend::Alpha, particle(2.0)));
        assert_eq!(list.additive().len(), 1);
        assert_eq!(list.alpha().len(), 1);
        assert_eq!(list.of(ParticleBlend::Alpha)[0].size, 2.0);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn an_invisible_particle_is_skipped_rather_than_counted_as_a_drop() {
        let mut list = ParticleDrawList::with_capacity(8);
        assert!(!list.push(ParticleBlend::Additive, particle(0.0)));
        let faded = ParticleInstance {
            color: [1.0, 1.0, 1.0, 0.0],
            ..particle(1.0)
        };
        assert!(!list.push(ParticleBlend::Additive, faded));
        assert!(list.is_empty());
        assert_eq!(list.dropped(), 0, "nothing was refused for want of room");
    }

    #[test]
    fn a_full_list_drops_rather_than_grows() {
        // The property the whole capacity story rests on: a stress scene degrades into the
        // first N particles instead of into an allocation per frame.
        let mut list = ParticleDrawList::with_capacity(4);
        let allocated = list.additive.capacity();
        for _ in 0..10 {
            list.push(ParticleBlend::Additive, particle(1.0));
        }
        assert_eq!(list.len(), 4);
        assert_eq!(list.dropped(), 6);
        assert_eq!(list.additive.capacity(), allocated, "the list reallocated");
    }

    #[test]
    fn the_capacity_is_shared_across_both_modes() {
        // Two arrays, one budget: a frame that spends everything on smoke must not also be
        // able to spend everything on fire.
        let mut list = ParticleDrawList::with_capacity(4);
        for _ in 0..3 {
            list.push(ParticleBlend::Alpha, particle(1.0));
        }
        assert!(list.push(ParticleBlend::Additive, particle(1.0)));
        assert!(!list.push(ParticleBlend::Additive, particle(1.0)));
        assert_eq!(list.len(), 4);
    }

    #[test]
    fn clearing_keeps_the_allocation_and_the_drop_count() {
        let mut list = ParticleDrawList::with_capacity(2);
        for _ in 0..4 {
            list.push(ParticleBlend::Additive, particle(1.0));
        }
        let allocated = list.additive.capacity();
        list.clear();

        assert!(list.is_empty());
        assert_eq!(
            list.dropped(),
            2,
            "the run's drops survive a frame boundary"
        );
        assert_eq!(list.additive.capacity(), allocated);

        list.reset_dropped();
        assert_eq!(list.dropped(), 0);
    }
}
