//! Ground area indicators: translucent fills drawn from dynamic meshes.
//!
//! Two indicators use this: the player's targeting decal (a flame cone, a meteor circle) and
//! the monsters' telegraphs. Both want an *interior* as well as an outline, and the interior
//! is a terrain-conforming triangle mesh from `engine_presentation::ui::ground_fill`. The
//! mesh changes with the indicator, so it is written into a dynamic mesh each time it changes
//! rather than uploaded as a new mesh.
//!
//! # Pooled, never per frame
//!
//! A dynamic mesh holds several committed upload buffers for its whole life and a render
//! entity is a world slot, so neither is created or destroyed as indicators come and go.
//! Telegraphs draw from a fixed set of slots created lazily up to [`MAX_DRAWN_TELEGRAPHS`];
//! a slot that is not in use is hidden and handed to the next telegraph.
//!
//! # Static means static
//!
//! A monster's telegraph never moves, so its border and its full fill are built once, when
//! it appears, and only the progress fill is rewritten as the strike approaches. Only a mark
//! that rides a player is rebuilt every frame. That is what keeps a footprint from twitching:
//! nothing about a fixed area is recomputed from anything that moves.

use engine_core::math::{Sphere, Vec2, Vec3};
use engine_core::transform::Transform;
use engine_presentation::telegraph_indicator::{
    IndicatorKind, TelegraphIndicator, TelegraphIndicators,
};
use engine_presentation::ui::ground_decal::{DecalBand, DecalScratch, GroundSurface, build_band};
use engine_presentation::ui::ground_fill::{
    FillMesh, MAX_FILL_INDICES, MAX_FILL_VERTICES, build_fill,
};
use engine_presentation::ui::targeting::TargetShape;
use engine_render_data::{
    InstanceStyle, RenderEntity, RenderEntityDesc, RenderWorld, ShadowCasting,
};
use engine_renderer::mesh::MeshVertex;
use engine_renderer::{MaterialDesc, MaterialHandle, MeshHandle, Renderer, RendererError};

/// How far above the ground the player's targeting fill sits, in metres.
///
/// Below the border (which is lifted further) so the two never tie for depth where they meet.
pub(crate) const TARGET_FILL_LIFT: f32 = 0.03;

/// How far above the ground a telegraph's full, faint fill sits, in metres.
const TELEGRAPH_FULL_LIFT: f32 = 0.025;

/// How far above the ground a telegraph's progress fill sits, in metres.
///
/// Above the full fill so the brighter layer wins the depth test where they overlap, and
/// below the border.
const TELEGRAPH_PROGRESS_LIFT: f32 = 0.035;

/// The most telegraphs drawn at once. Past it the extras are skipped.
///
/// Each slot owns two dynamic meshes, so the cap is also a memory budget. Two dozen
/// simultaneous ground areas is already more than a player can read.
pub(crate) const MAX_DRAWN_TELEGRAPHS: usize = 24;

/// Slack around a fill's bounds for the terrain's relief, in metres.
///
/// The fill is anchor-relative and its vertices follow the ground, so on a hillside they sit
/// above and below the anchor by more than the shape's reach alone accounts for.
const BOUNDS_MARGIN: f32 = 6.0;

/// A reach this close to the one already written is not worth an upload.
const REACH_EPSILON: f32 = 1e-4;

/// Local-space bounds for a fill of `shape`, centred on its anchor.
fn fill_bounds(shape: TargetShape) -> Sphere {
    Sphere::new(Vec3::ZERO, shape.extent() + BOUNDS_MARGIN)
}

/// Builds fills and writes them into dynamic meshes, with buffers reused between calls.
#[derive(Debug, Default)]
pub(crate) struct FillWriter {
    /// The fill's geometry as `ground_fill` produces it.
    fill: FillMesh,
    /// The same vertices in the renderer's format.
    vertices: Vec<MeshVertex>,
    /// Whether a failed write has been reported, so a broken mesh logs once and not per frame.
    reported: bool,
}

impl FillWriter {
    /// Builds a fill covering `reach` of `shape` and writes it into `mesh`.
    #[allow(
        clippy::too_many_arguments,
        reason = "a fill is its shape, its placement, its reach and where it lands; bundling them would only move the list"
    )]
    pub(crate) fn write(
        &mut self,
        renderer: &mut Renderer,
        mesh: MeshHandle,
        shape: TargetShape,
        anchor: Vec3,
        facing: f32,
        reach: f32,
        lift: f32,
        ground: &GroundSurface<'_>,
    ) {
        build_fill(shape, anchor, facing, reach, lift, ground, &mut self.fill);
        self.vertices.clear();
        self.vertices.extend(
            self.fill
                .positions
                .iter()
                .map(|position| MeshVertex::new(*position, Vec3::Y, Vec2::ZERO)),
        );
        if let Err(error) = renderer.write_dynamic_mesh(
            mesh,
            &self.vertices,
            &self.fill.indices,
            fill_bounds(shape),
        ) && !self.reported
        {
            self.reported = true;
            log::warn!("area indicator: could not write a fill: {error}");
        }
    }
}

/// One dynamic mesh and the render entity that draws it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FillEntity {
    /// The geometry, rewritten as the fill changes.
    mesh: MeshHandle,
    /// The entity drawing it, hidden while the fill is unused.
    entity: RenderEntity,
}

impl FillEntity {
    /// Creates the mesh and a hidden entity for it.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] if the mesh's buffers cannot be allocated.
    pub(crate) fn create(
        renderer: &mut Renderer,
        world: &mut RenderWorld,
        material: MaterialHandle,
    ) -> Result<Self, RendererError> {
        let mesh = renderer.create_dynamic_mesh(MAX_FILL_VERTICES, MAX_FILL_INDICES)?;
        let entity = world.spawn(RenderEntityDesc {
            transform: Transform::IDENTITY,
            mesh,
            material,
            bounds: Sphere::new(Vec3::ZERO, 1.0),
            visible: false,
            style: InstanceStyle::NEUTRAL,
            // Interface painted on the ground: it casts nothing, and a shadow falling across
            // it must not make the area it marks harder to read.
            shadow: ShadowCasting::NONE,
        });
        Ok(Self { mesh, entity })
    }

    /// The mesh to write geometry into.
    pub(crate) fn mesh(&self) -> MeshHandle {
        self.mesh
    }

    /// Puts the entity at `anchor` and shows it.
    ///
    /// The fill's vertices are relative to its anchor, so this translation is what places it.
    pub(crate) fn place(
        &self,
        world: &mut RenderWorld,
        anchor: Vec3,
        shape: TargetShape,
        material: MaterialHandle,
    ) {
        world.set_transform(
            self.entity,
            Transform::from_translation(anchor),
            fill_bounds(shape),
        );
        world.set_material(self.entity, material);
        world.set_visible(self.entity, true);
    }

    /// Hides the entity.
    pub(crate) fn hide(&self, world: &mut RenderWorld) {
        world.set_visible(self.entity, false);
    }
}

/// Points `pool` at `transforms`, spawning, reusing and hiding entities as the count changes.
///
/// The piece count of an outline changes with the shape and the radius, and it changes while
/// the level is being edited under it. Despawning and respawning on every change would churn
/// render-world slots; a pool grows to the largest outline the session has drawn and then
/// never allocates again. Surplus entities are hidden rather than removed, because the next
/// shape is very likely to want them back.
pub(crate) fn sync_decal_band(
    world: &mut RenderWorld,
    pool: &mut Vec<RenderEntity>,
    transforms: &[Transform],
    mesh: MeshHandle,
    material: MaterialHandle,
    bounds: Sphere,
) {
    for (entity, transform) in pool.iter().zip(transforms) {
        world.set_transform(*entity, *transform, bounds);
        world.set_material(*entity, material);
        world.set_visible(*entity, true);
    }
    for transform in transforms.iter().skip(pool.len()) {
        pool.push(world.spawn(RenderEntityDesc {
            transform: *transform,
            mesh,
            material,
            bounds,
            visible: true,
            style: InstanceStyle::NEUTRAL,
            // The targeting indicator: a shape painted on the ground to say where a spell
            // will land. It is interface, not matter - it casts nothing, and a shadow
            // falling across it must not make the area it marks harder to read.
            shadow: ShadowCasting::NONE,
        }));
    }
    for entity in pool.iter().skip(transforms.len()) {
        world.set_visible(*entity, false);
    }
}

/// The materials monster telegraphs are drawn with, one set per kind.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TelegraphVisuals {
    /// The unit quad every border piece is an instance of.
    quad_mesh: MeshHandle,
    /// The quad's local bounds, for the cull.
    quad_bounds: Sphere,
    /// Border, faint full fill and brighter progress fill of a monster's area: red.
    enemy: KindMaterials,
    /// The same three for a mark riding a player: violet, so it is never mistaken for
    /// either a monster's area or the player's own orange targeting.
    mark: KindMaterials,
    /// The same three for the player's own Moving Fire Beam: the orange of the targeting
    /// decal's family, so the floor under a beam reads as the player's and never as a threat.
    beam: KindMaterials,
}

/// The three materials one kind of telegraph is drawn with.
#[derive(Debug, Clone, Copy)]
struct KindMaterials {
    /// The opaque outline.
    border: MaterialHandle,
    /// The faint fill over the whole footprint: where the strike will land.
    full: MaterialHandle,
    /// The brighter fill that grows to the strike: how long until it does.
    progress: MaterialHandle,
}

impl TelegraphVisuals {
    /// Creates the telegraph materials, sharing the decal's quad.
    ///
    /// Colour goes in the emissive term over a near-black base, for the reason the targeting
    /// decal's does: the indicator must be one flat colour whatever the ground under it is
    /// doing.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] if a material cannot be created.
    pub(crate) fn upload(
        renderer: &mut Renderer,
        quad_mesh: MeshHandle,
        quad_bounds: Sphere,
    ) -> Result<Self, RendererError> {
        let mut kind = |base: [f32; 3], border: Vec3, fill: Vec3| {
            let translucent = |renderer: &mut Renderer, opacity: f32| {
                renderer.create_material(
                    MaterialDesc::colored(base[0] * 0.5, base[1] * 0.5, base[2] * 0.5)
                        .with_roughness(1.0)
                        .with_emissive(fill)
                        .with_opacity(opacity),
                )
            };
            Ok::<_, RendererError>(KindMaterials {
                border: renderer.create_material(
                    MaterialDesc::colored(base[0], base[1], base[2])
                        .with_roughness(1.0)
                        .with_emissive(border),
                )?,
                full: translucent(renderer, 0.12)?,
                progress: translucent(renderer, 0.35)?,
            })
        };
        let enemy = kind(
            [0.02, 0.004, 0.003],
            Vec3::new(1.1, 0.12, 0.08),
            Vec3::new(0.95, 0.10, 0.07),
        )?;
        let mark = kind(
            [0.012, 0.006, 0.02],
            Vec3::new(0.72, 0.35, 1.0),
            Vec3::new(0.62, 0.30, 0.88),
        )?;
        let beam = kind(
            [0.02, 0.01, 0.003],
            Vec3::new(1.2, 0.55, 0.12),
            Vec3::new(1.0, 0.42, 0.08),
        )?;
        Ok(Self {
            quad_mesh,
            quad_bounds,
            enemy,
            mark,
            beam,
        })
    }

    /// The materials for `kind`.
    fn of(&self, kind: IndicatorKind) -> KindMaterials {
        match kind {
            IndicatorKind::Enemy => self.enemy,
            IndicatorKind::Mark => self.mark,
            IndicatorKind::Beam => self.beam,
        }
    }
}

/// The render objects one telegraph is drawn with.
#[derive(Debug)]
struct Slot {
    /// The telegraph this slot is showing, or `None` while it is free.
    id: Option<u32>,
    /// The faint fill over the whole footprint.
    full: FillEntity,
    /// The fill that grows with the cast.
    progress: FillEntity,
    /// The border's pieces. Kept as they are for a static telegraph.
    border: Vec<RenderEntity>,
    /// Whether the border and full fill must be rebuilt: the telegraph is new or the ground
    /// under it changed.
    dirty: bool,
    /// The reach the progress fill was last written at, or `None` if never.
    written_reach: Option<f32>,
}

/// Draws [`TelegraphIndicators`]: a border, a faint fill and a progress fill each.
#[derive(Debug)]
pub(crate) struct TelegraphView {
    /// Materials and the border quad.
    visuals: TelegraphVisuals,
    /// Pooled slots, created lazily up to [`MAX_DRAWN_TELEGRAPHS`].
    slots: Vec<Slot>,
    /// Geometry scratch, shared by every fill.
    writer: FillWriter,
    /// Outline scratch for [`build_band`].
    scratch: DecalScratch,
    /// The border's piece transforms, rebuilt only when a border changes.
    pieces: Vec<Transform>,
    /// Whether slot creation failed, so it is attempted and reported once.
    exhausted: bool,
}

impl TelegraphView {
    /// A view with no slots; they are created when telegraphs first appear.
    pub(crate) fn new(visuals: TelegraphVisuals) -> Self {
        Self {
            visuals,
            slots: Vec::with_capacity(MAX_DRAWN_TELEGRAPHS),
            writer: FillWriter::default(),
            scratch: DecalScratch::new(),
            pieces: Vec::new(),
            exhausted: false,
        }
    }

    /// Marks every drawn telegraph for a rebuild, because the ground under it changed.
    pub(crate) fn invalidate_ground(&mut self) {
        for slot in &mut self.slots {
            slot.dirty = true;
        }
    }

    /// Brings the drawn telegraphs in line with `indicators`.
    pub(crate) fn update(
        &mut self,
        renderer: &mut Renderer,
        world: &mut RenderWorld,
        indicators: &TelegraphIndicators,
        border: DecalBand,
        ground: &GroundSurface<'_>,
    ) {
        // Free the slots whose telegraph has ended.
        for slot in &mut self.slots {
            if let Some(id) = slot.id
                && !indicators.iter().any(|indicator| indicator.id == id)
            {
                release(slot, world);
            }
        }

        for indicator in indicators.iter() {
            let slot = match self
                .slots
                .iter()
                .position(|slot| slot.id == Some(indicator.id))
            {
                Some(index) => index,
                None => match self.acquire(renderer, world) {
                    Some(index) => {
                        self.slots[index].id = Some(indicator.id);
                        self.slots[index].dirty = true;
                        self.slots[index].written_reach = None;
                        index
                    }
                    // Over the cap: skipped, never allocated for.
                    None => continue,
                },
            };
            self.draw(slot, indicator, renderer, world, border, ground);
        }
    }

    /// A free slot, creating one if the cap allows, or `None`.
    fn acquire(&mut self, renderer: &mut Renderer, world: &mut RenderWorld) -> Option<usize> {
        if let Some(index) = self.slots.iter().position(|slot| slot.id.is_none()) {
            return Some(index);
        }
        if self.slots.len() >= MAX_DRAWN_TELEGRAPHS || self.exhausted {
            return None;
        }
        let materials = self.visuals.enemy;
        let created = FillEntity::create(renderer, world, materials.full).and_then(|full| {
            let progress = FillEntity::create(renderer, world, materials.progress)?;
            Ok((full, progress))
        });
        match created {
            Ok((full, progress)) => {
                self.slots.push(Slot {
                    id: None,
                    full,
                    progress,
                    border: Vec::new(),
                    dirty: true,
                    written_reach: None,
                });
                Some(self.slots.len() - 1)
            }
            Err(error) => {
                self.exhausted = true;
                log::warn!("telegraph indicators: could not create a slot: {error}");
                None
            }
        }
    }

    /// Draws one telegraph into its slot, rebuilding only what moved.
    fn draw(
        &mut self,
        index: usize,
        indicator: &TelegraphIndicator,
        renderer: &mut Renderer,
        world: &mut RenderWorld,
        border: DecalBand,
        ground: &GroundSurface<'_>,
    ) {
        let materials = self.visuals.of(indicator.kind);
        // A mark rides its victim, so everything is rebuilt around its new anchor each frame.
        // Anything else is fixed where gameplay put it.
        let rebuild = self.slots[index].dirty || indicator.follow.is_some();
        let progress = indicator.progress();

        if rebuild {
            build_band(
                indicator.shape,
                indicator.anchor,
                indicator.facing,
                border,
                ground,
                &mut self.scratch,
                &mut self.pieces,
            );
            let slot = &mut self.slots[index];
            sync_decal_band(
                world,
                &mut slot.border,
                &self.pieces,
                self.visuals.quad_mesh,
                materials.border,
                self.visuals.quad_bounds,
            );
            self.writer.write(
                renderer,
                slot.full.mesh(),
                indicator.shape,
                indicator.anchor,
                indicator.facing,
                1.0,
                TELEGRAPH_FULL_LIFT,
                ground,
            );
            slot.full
                .place(world, indicator.anchor, indicator.shape, materials.full);
            slot.progress
                .place(world, indicator.anchor, indicator.shape, materials.progress);
            slot.dirty = false;
            slot.written_reach = None;
        }

        let slot = &mut self.slots[index];
        let stale = slot
            .written_reach
            .is_none_or(|written| (written - progress).abs() > REACH_EPSILON);
        if stale {
            self.writer.write(
                renderer,
                slot.progress.mesh(),
                indicator.shape,
                indicator.anchor,
                indicator.facing,
                progress,
                TELEGRAPH_PROGRESS_LIFT,
                ground,
            );
            slot.written_reach = Some(progress);
        }
    }
}

/// Frees a slot and hides everything it draws.
fn release(slot: &mut Slot, world: &mut RenderWorld) {
    if slot.id.take().is_none() {
        return;
    }
    slot.full.hide(world);
    slot.progress.hide(world);
    for entity in &slot.border {
        world.set_visible(*entity, false);
    }
}
