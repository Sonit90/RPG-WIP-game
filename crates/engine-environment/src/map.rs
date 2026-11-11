//! What a level's map is: the rectangle of the world it shows, the encounters on it, and the
//! walkable floor it is drawn around.
//!
//! ```text
//!   EnvironmentScene ──► frame_for ──────► MapFrame    world XZ rectangle ◄──► texture UV
//!          │         ──► walkable_mask ──► WalkableMask  floor / wall / outside, per texel
//!          │         ──► encounters ─────► [MapEncounter]  one per pack, by its pack key
//!          │         ──► geometry_hash ──► u64          "is the baked picture still this level?"
//!          ▼
//!   engine-app bakes a top-down picture through the real renderer, engine-assets styles it
//!   with the mask, and the picture + a [`MapDocument`] land in `assets/generated/maps/`.
//! ```
//!
//! # North is −Z
//!
//! Yaw zero faces −Z everywhere in this project, so that is north, and north is the top of the
//! picture: texel row zero is the frame's `min_z`. `u` grows with +X (east), `v` with +Z
//! (south). Every conversion between the world and the map goes through [`MapFrame`], so the
//! minimap, the full-screen map, the editor's preview and the bake cannot disagree about it.
//!
//! # Only the geometry is baked
//!
//! The picture is the floor and the walls. Encounters are *not* in it: they are
//! [`MapEncounter`]s derived from the monster placements every time they are asked for, so a
//! pack moved in the editor moves on the map without a re-bake, and each marker stays a thing
//! that can be hovered, clicked and described. [`geometry_hash`] leaves the monsters out for
//! the same reason — moving a skeleton does not make the picture stale; moving a wall does.
//!
//! # The walkable mask
//!
//! A top-down render of a crypt is walls on grass: the render alone cannot tell the room from
//! the field outside it. The mask can. Every collision box that would stop a walking character
//! is rasterized, grown by a character's radius so a crack between two wall segments does not
//! count as a doorway, and the floor is flood-filled from every place a player or a monster
//! stands. What the fill reaches is floor; a blocked texel touching it is wall; the rest is
//! outside. That is what lets the styled map draw rooms and corridors rather than a lawn.
//!
//! This crate stays `engine-core` + `serde`: the mask is plain arithmetic over the boxes the
//! scene already describes, and nothing here names a mesh or a device.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use engine_core::math::Vec3;
use serde::{Deserialize, Serialize};

use crate::scene::EnvironmentScene;

/// Schema of a [`MapDocument`]. A document with another version is ignored and re-baked.
///
/// Three: the redesigned map style with a seamless surround. A picture baked in another style
/// is baked again.
pub const MAP_FILE_VERSION: u32 = 3;

/// Texels per metre a map is baked at when the level does not say.
///
/// Five: the Graveyard Crypt's 192 × 304 m frame becomes 960 × 1520 texels, 5.8 MB of RGBA
/// before mips. The full-screen map at its default zoom shows about three texels per screen
/// pixel's worth of world; zoomed four times in it is still under two screen pixels per texel.
pub const DEFAULT_TEXELS_PER_METRE: f32 = 5.0;

/// The longest side a map texture may have, in texels.
///
/// A frame that would be larger at its texel density is baked at a lower density instead,
/// keeping its aspect ratio. 2048 is where a map stops being a picture and starts being a
/// memory budget line of its own.
pub const MAX_MAP_TEXELS: u32 = 2048;

/// The shortest side a map texture may have, in texels.
pub const MIN_MAP_TEXELS: u32 = 64;

/// Metres of margin around the level's contents when the frame is derived.
pub const DEFAULT_PADDING: f32 = 8.0;

/// How far below the floor the bake's vertical range starts, in metres.
pub const DEFAULT_FLOOR_BELOW: f32 = 2.0;

/// How far above the floor the bake's vertical range ends, in metres.
///
/// Twelve: above every wall, pillar and arch the library has, below the canopy of a sky dome.
/// Anything higher is cut by the top-down camera's near plane, which is how a roof or a sky
/// is kept off the map.
pub const DEFAULT_CEILING_ABOVE: f32 = 12.0;

/// The band above the floor a collision box has to reach into to stop a walking character.
///
/// From knee height to chest height. A kerb under it is stepped over; a lintel above it is
/// walked under. Only boxes that overlap this band are walls to the mask.
pub const BLOCKING_BAND: (f32, f32) = (0.35, 1.4);

/// How much every blocking box is grown by, in metres: roughly a character's radius.
///
/// What closes the hairline gaps between adjacent wall segments. A gap a character cannot
/// fit through is not a doorway, and flooding through one would paint the outside as floor.
pub const WALK_CLEARANCE: f32 = 0.35;

/// A footprint larger than this on both axes is a floor slab rather than a wall, in metres.
///
/// A ground plane with a collision box would otherwise block the whole level.
pub const SLAB_FOOTPRINT: f32 = 60.0;

/// The rectangle of the world a map shows, in world XZ.
///
/// `min_z` is north — the top of the picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapFrame {
    /// West edge, world X.
    pub min_x: f32,
    /// North edge, world Z.
    pub min_z: f32,
    /// East edge, world X.
    pub max_x: f32,
    /// South edge, world Z.
    pub max_z: f32,
}

impl MapFrame {
    /// A frame with these edges.
    #[must_use]
    pub const fn new(min_x: f32, min_z: f32, max_x: f32, max_z: f32) -> Self {
        Self {
            min_x,
            min_z,
            max_x,
            max_z,
        }
    }

    /// East–west size, in metres.
    #[must_use]
    pub fn width(&self) -> f32 {
        self.max_x - self.min_x
    }

    /// North–south size, in metres.
    #[must_use]
    pub fn depth(&self) -> f32 {
        self.max_z - self.min_z
    }

    /// Width over depth: what a picture of this frame must keep to not be stretched.
    #[must_use]
    pub fn aspect(&self) -> f32 {
        if self.depth() <= 0.0 {
            1.0
        } else {
            self.width() / self.depth()
        }
    }

    /// The middle, as world `(x, z)`.
    #[must_use]
    pub fn center(&self) -> (f32, f32) {
        (
            (self.min_x + self.max_x) * 0.5,
            (self.min_z + self.max_z) * 0.5,
        )
    }

    /// Whether every edge is a number and the frame has an area.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        [self.min_x, self.min_z, self.max_x, self.max_z]
            .iter()
            .all(|value| value.is_finite())
            && self.width() > 0.0
            && self.depth() > 0.0
    }

    /// Where `position` is on the map, as texture coordinates. Outside `0..1` is off the map.
    ///
    /// `y` is ignored: the map is a plan.
    #[must_use]
    pub fn world_to_uv(&self, position: Vec3) -> (f32, f32) {
        (
            (position.x - self.min_x) / self.width(),
            (position.z - self.min_z) / self.depth(),
        )
    }

    /// The world `(x, z)` at texture coordinate `(u, v)`. The inverse of
    /// [`MapFrame::world_to_uv`].
    #[must_use]
    pub fn uv_to_world(&self, u: f32, v: f32) -> (f32, f32) {
        (self.min_x + u * self.width(), self.min_z + v * self.depth())
    }

    /// Whether `position` is inside the frame, in plan.
    #[must_use]
    pub fn contains(&self, position: Vec3) -> bool {
        (self.min_x..=self.max_x).contains(&position.x)
            && (self.min_z..=self.max_z).contains(&position.z)
    }

    /// The texture size this frame bakes to at `texels_per_metre`, aspect kept.
    ///
    /// Scaled down uniformly when the longer side would pass [`MAX_MAP_TEXELS`], and each side
    /// held to at least [`MIN_MAP_TEXELS`], so a texel is square — the map is never stretched.
    #[must_use]
    pub fn texture_size(&self, texels_per_metre: f32) -> (u32, u32) {
        let density = if texels_per_metre.is_finite() && texels_per_metre > 0.0 {
            texels_per_metre
        } else {
            DEFAULT_TEXELS_PER_METRE
        };
        let longest = self.width().max(self.depth()).max(f32::EPSILON);
        let density = density.min(MAX_MAP_TEXELS as f32 / longest);
        let side =
            |metres: f32| ((metres * density).round() as u32).clamp(MIN_MAP_TEXELS, MAX_MAP_TEXELS);
        (side(self.width()), side(self.depth()))
    }
}

/// What a level says about how its map is baked. Every field is optional in the file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapSettings {
    /// The lowest world `y` the bake sees. `None` is the floor minus [`DEFAULT_FLOOR_BELOW`].
    pub floor: Option<f32>,
    /// The highest world `y` the bake sees. `None` is the floor plus
    /// [`DEFAULT_CEILING_ABOVE`]. Anything above it — a roof, a canopy, the sky — is not on the
    /// map.
    pub ceiling: Option<f32>,
    /// Metres of margin around the derived frame.
    pub padding: f32,
    /// Texture density.
    pub texels_per_metre: f32,
    /// The frame, when the author wants to choose it rather than have it derived.
    pub bounds: Option<MapFrame>,
}

impl Default for MapSettings {
    fn default() -> Self {
        Self {
            floor: None,
            ceiling: None,
            padding: DEFAULT_PADDING,
            texels_per_metre: DEFAULT_TEXELS_PER_METRE,
            bounds: None,
        }
    }
}

impl MapSettings {
    /// Whether these are the defaults, which the scene file then leaves out.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether every number is usable.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.floor.is_none_or(f32::is_finite)
            && self.ceiling.is_none_or(f32::is_finite)
            && self.padding.is_finite()
            && self.padding >= 0.0
            && self.texels_per_metre.is_finite()
            && self.texels_per_metre > 0.0
            && self.bounds.is_none_or(|frame| frame.is_valid())
            && match (self.floor, self.ceiling) {
                (Some(floor), Some(ceiling)) => ceiling > floor,
                _ => true,
            }
    }

    /// The vertical range the bake sees, `(floor, ceiling)`, for a level on `ground`.
    #[must_use]
    pub fn vertical_range(&self, ground: f32) -> (f32, f32) {
        let floor = self.floor.unwrap_or(ground - DEFAULT_FLOOR_BELOW);
        let ceiling = self.ceiling.unwrap_or(ground + DEFAULT_CEILING_ABOVE);
        if ceiling > floor {
            (floor, ceiling)
        } else {
            (floor, floor + DEFAULT_FLOOR_BELOW + DEFAULT_CEILING_ABOVE)
        }
    }
}

/// Whether a marker is an ordinary pack or the dungeon's boss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncounterKind {
    /// Monsters that fight together.
    Pack,
    /// The pack the dungeon settings name as the boss.
    Boss,
}

impl EncounterKind {
    /// What the map's tooltip and the sidecar file call it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pack => "pack",
            Self::Boss => "boss",
        }
    }
}

/// One kind of monster in an encounter, and how many of it.
#[derive(Debug, Clone, PartialEq)]
pub struct EncounterMember {
    /// The monster's id, as the placement spells it.
    pub monster: String,
    /// How many of it stand in the pack.
    pub count: u32,
    /// The highest level any of them is placed at.
    pub level: u8,
}

/// A pack, as the map shows it: where it stands and who is in it.
#[derive(Debug, Clone, PartialEq)]
pub struct MapEncounter {
    /// Its pack key: the author's pack name, or `#index` for a placement with none. Stable
    /// across saves for a named pack, which is what the map selects by.
    pub key: String,
    /// The first custom name a member carries, if any.
    pub name: Option<String>,
    /// Pack or boss.
    pub kind: EncounterKind,
    /// The middle of its members, on the floor.
    pub position: Vec3,
    /// The furthest any member stands from [`MapEncounter::position`], in metres.
    pub radius: f32,
    /// Who is in it, by monster, in the order they first appear in the level.
    pub members: Vec<EncounterMember>,
    /// The indices of its placements in `scene.monsters`.
    pub placements: Vec<usize>,
}

impl MapEncounter {
    /// How many monsters stand in it.
    #[must_use]
    pub fn monster_count(&self) -> u32 {
        self.members.iter().map(|member| member.count).sum()
    }
}

/// Every encounter in `scene`, one per pack, in the order the packs first appear.
///
/// Disabled placements are left out: a monster switched off in the editor is not in the
/// dungeon, so it is not on its map. Deterministic — the same scene always produces the same
/// list in the same order.
#[must_use]
pub fn encounters(scene: &EnvironmentScene) -> Vec<MapEncounter> {
    let boss_pack = scene
        .dungeon
        .as_ref()
        .and_then(|dungeon| dungeon.boss_pack.as_deref())
        .map(str::trim);
    let mut out: Vec<MapEncounter> = Vec::new();
    for (index, placement) in scene.monsters.iter().enumerate() {
        if !placement.enabled || !placement.is_valid() {
            continue;
        }
        let key = placement.pack_key(index);
        let slot = match out.iter().position(|encounter| encounter.key == key) {
            Some(slot) => slot,
            None => {
                let kind = if boss_pack.is_some_and(|boss| boss == key.trim()) {
                    EncounterKind::Boss
                } else {
                    EncounterKind::Pack
                };
                out.push(MapEncounter {
                    key,
                    name: None,
                    kind,
                    position: Vec3::ZERO,
                    radius: 0.0,
                    members: Vec::new(),
                    placements: Vec::new(),
                });
                out.len() - 1
            }
        };
        let encounter = &mut out[slot];
        encounter.placements.push(index);
        if encounter.name.is_none() {
            encounter.name = placement
                .name
                .as_ref()
                .filter(|name| !name.trim().is_empty())
                .cloned();
        }
        let id = placement.monster.trim();
        match encounter
            .members
            .iter_mut()
            .find(|member| member.monster.eq_ignore_ascii_case(id))
        {
            Some(member) => {
                member.count += 1;
                member.level = member.level.max(placement.level);
            }
            None => encounter.members.push(EncounterMember {
                monster: id.to_owned(),
                count: 1,
                level: placement.level,
            }),
        }
    }
    for encounter in &mut out {
        let count = encounter.placements.len().max(1) as f32;
        let sum = encounter
            .placements
            .iter()
            .map(|&index| scene.monsters[index].position)
            .fold(Vec3::ZERO, |sum, position| sum + position);
        encounter.position = sum * (1.0 / count);
        encounter.radius = encounter
            .placements
            .iter()
            .map(|&index| {
                let position = scene.monsters[index].position;
                let (dx, dz) = (
                    position.x - encounter.position.x,
                    position.z - encounter.position.z,
                );
                (dx * dx + dz * dz).sqrt()
            })
            .fold(0.0, f32::max);
    }
    out
}

/// The rectangle of the world `scene`'s map shows.
///
/// The author's [`MapSettings::bounds`] when they chose one. Otherwise everything that marks
/// out the level — where each object stands, the reach of every wall-sized collision box,
/// every monster, spawn point, light and body of water — grown by the padding and held inside
/// the square a character is confined to. Positions rather than meshes: a sky dome or a ground
/// plane is one point at its origin, not a kilometre of extent.
#[must_use]
pub fn frame_for(scene: &EnvironmentScene) -> MapFrame {
    if let Some(bounds) = scene.map.bounds.filter(MapFrame::is_valid) {
        return bounds;
    }
    let mut low = (f32::MAX, f32::MAX);
    let mut high = (f32::MIN, f32::MIN);
    let mut include = |x: f32, z: f32| {
        if x.is_finite() && z.is_finite() {
            low = (low.0.min(x), low.1.min(z));
            high = (high.0.max(x), high.1.max(z));
        }
    };
    for object in &scene.objects {
        let position = object.position();
        include(position.x, position.z);
        for world in scene.world_boxes(object) {
            if let Some((min, max)) = footprint(&world, 0.0)
                && (max.0 - min.0 < SLAB_FOOTPRINT || max.1 - min.1 < SLAB_FOOTPRINT)
            {
                include(min.0, min.1);
                include(max.0, max.1);
            }
        }
    }
    for monster in &scene.monsters {
        include(monster.position.x, monster.position.z);
    }
    for spawn in &scene.spawn_points {
        include(spawn.position.x, spawn.position.z);
    }
    let player = scene.player_spawn().position;
    include(player.x, player.z);
    for light in &scene.lights {
        include(light.position.x, light.position.z);
    }
    for body in &scene.water {
        include(
            body.center.x - body.half_extents.x,
            body.center.z - body.half_extents.y,
        );
        include(
            body.center.x + body.half_extents.x,
            body.center.z + body.half_extents.y,
        );
    }
    if let Some(dungeon) = &scene.dungeon {
        for spawn in &dungeon.add_spawns {
            include(spawn.position.x, spawn.position.z);
        }
    }

    let pad = if scene.map.padding.is_finite() {
        scene.map.padding.max(0.0)
    } else {
        DEFAULT_PADDING
    };
    let limit = if scene.half_extent.is_finite() && scene.half_extent > 0.0 {
        scene.half_extent
    } else {
        f32::MAX
    };
    let mut frame = MapFrame::new(
        (low.0 - pad).max(-limit),
        (low.1 - pad).max(-limit),
        (high.0 + pad).min(limit),
        (high.1 + pad).min(limit),
    );
    if !frame.is_valid() {
        // An empty level: a square around where the player starts, so there is still a map.
        let half = 20.0;
        frame = MapFrame::new(
            player.x - half,
            player.z - half,
            player.x + half,
            player.z + half,
        );
    }
    frame
}

/// The plan-view rectangle a box covers, grown by `grow` on every side, as
/// `((min_x, min_z), (max_x, max_z))`.
fn footprint(world: &crate::object::WorldBox, grow: f32) -> Option<((f32, f32), (f32, f32))> {
    let (axis_x, axis_z) = plan_axes(world);
    let (hx, hz) = (world.half_extents.x + grow, world.half_extents.z + grow);
    let reach_x = (axis_x.0 * hx).abs() + (axis_z.0 * hz).abs();
    let reach_z = (axis_x.1 * hx).abs() + (axis_z.1 * hz).abs();
    let center = world.center;
    if !(reach_x.is_finite() && reach_z.is_finite()) {
        return None;
    }
    Some((
        (center.x - reach_x, center.z - reach_z),
        (center.x + reach_x, center.z + reach_z),
    ))
}

/// A box's own X and Z axes, laid flat on the plan.
fn plan_axes(world: &crate::object::WorldBox) -> ((f32, f32), (f32, f32)) {
    let x = world.rotation.rotate(Vec3::new(1.0, 0.0, 0.0));
    let z = world.rotation.rotate(Vec3::new(0.0, 0.0, 1.0));
    ((x.x, x.z), (z.x, z.z))
}

/// What a texel of the map is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum MapCell {
    /// Nobody can reach it: beyond the walls.
    #[default]
    Outside = 0,
    /// A wall at the edge of the floor.
    Wall = 1,
    /// Floor a character can walk to.
    Floor = 2,
}

/// The walkable floor of a level, one cell per map texel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkableMask {
    /// Cells across, west to east.
    pub width: u32,
    /// Cells down, north to south.
    pub height: u32,
    /// Row-major, row zero north.
    pub cells: Vec<MapCell>,
}

impl WalkableMask {
    /// The cell at `(x, y)`, or [`MapCell::Outside`] off the edge.
    #[must_use]
    pub fn cell(&self, x: u32, y: u32) -> MapCell {
        if x >= self.width || y >= self.height {
            return MapCell::Outside;
        }
        self.cells[(y * self.width + x) as usize]
    }

    /// How many cells are `kind`.
    #[must_use]
    pub fn count(&self, kind: MapCell) -> usize {
        self.cells.iter().filter(|cell| **cell == kind).count()
    }
}

/// Floods `scene`'s walkable floor over a `width × height` grid laid on `frame`.
///
/// See the module documentation for the method. A level with no enclosing walls floods the
/// whole frame, which is the right answer for an open field.
#[must_use]
pub fn walkable_mask(
    scene: &EnvironmentScene,
    frame: &MapFrame,
    width: u32,
    height: u32,
) -> WalkableMask {
    let (w, h) = (width.max(1) as usize, height.max(1) as usize);
    let mut blocked = vec![false; w * h];
    let cell_x = frame.width() / w as f32;
    let cell_z = frame.depth() / h as f32;
    let to_cell = |x: f32, z: f32| -> (isize, isize) {
        (
            ((x - frame.min_x) / cell_x).floor() as isize,
            ((z - frame.min_z) / cell_z).floor() as isize,
        )
    };

    // The confinement square is a wall too: nothing walks past it.
    if scene.half_extent.is_finite() && scene.half_extent > 0.0 {
        let limit = scene.half_extent;
        for y in 0..h {
            for x in 0..w {
                let (wx, wz) =
                    frame.uv_to_world((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                if wx.abs() > limit || wz.abs() > limit {
                    blocked[y * w + x] = true;
                }
            }
        }
    }

    for object in &scene.objects {
        for world in scene.world_boxes(object) {
            let floor = floor_height(scene, world.center);
            let bottom =
                world.center.y - world.half_extents.y.abs().max(0.0) * vertical_reach(&world);
            let top = world.center.y + world.half_extents.y.abs() * vertical_reach(&world);
            if top < floor + BLOCKING_BAND.0 || bottom > floor + BLOCKING_BAND.1 {
                continue;
            }
            let Some((min, max)) = footprint(&world, WALK_CLEARANCE) else {
                continue;
            };
            if max.0 - min.0 >= SLAB_FOOTPRINT && max.1 - min.1 >= SLAB_FOOTPRINT {
                continue;
            }
            let (axis_x, axis_z) = plan_axes(&world);
            let (hx, hz) = (
                world.half_extents.x + WALK_CLEARANCE,
                world.half_extents.z + WALK_CLEARANCE,
            );
            let (x0, z0) = to_cell(min.0, min.1);
            let (x1, z1) = to_cell(max.0, max.1);
            for cz in z0.max(0)..=z1.min(h as isize - 1) {
                for cx in x0.max(0)..=x1.min(w as isize - 1) {
                    let (px, pz) = frame
                        .uv_to_world((cx as f32 + 0.5) / w as f32, (cz as f32 + 0.5) / h as f32);
                    let (dx, dz) = (px - world.center.x, pz - world.center.z);
                    let along_x = dx * axis_x.0 + dz * axis_x.1;
                    let along_z = dx * axis_z.0 + dz * axis_z.1;
                    if along_x.abs() <= hx && along_z.abs() <= hz {
                        blocked[cz as usize * w + cx as usize] = true;
                    }
                }
            }
        }
    }

    // Seeds: everywhere a player or a monster is known to stand.
    let mut seeds: Vec<Vec3> = Vec::new();
    seeds.push(scene.player_spawn().position);
    seeds.extend(scene.spawn_points.iter().map(|spawn| spawn.position));
    seeds.extend(
        scene
            .monsters
            .iter()
            .filter(|monster| monster.enabled)
            .map(|monster| monster.position),
    );
    if let Some(dungeon) = &scene.dungeon {
        seeds.extend(dungeon.add_spawns.iter().map(|spawn| spawn.position));
    }

    let mut floor = vec![false; w * h];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for seed in seeds {
        if !frame.contains(seed) {
            continue;
        }
        let (sx, sz) = to_cell(seed.x, seed.z);
        // A monster standing against a wall may land in the clearance margin; the nearest
        // open cell within a metre or so is where it really stands.
        if let Some(start) = nearest_open(&blocked, w, h, sx, sz, 6)
            && !floor[start]
        {
            floor[start] = true;
            queue.push_back(start);
        }
    }
    while let Some(index) = queue.pop_front() {
        let (x, y) = (index % w, index / w);
        let neighbours = [
            (x > 0).then(|| index - 1),
            (x + 1 < w).then(|| index + 1),
            (y > 0).then(|| index - w),
            (y + 1 < h).then(|| index + w),
        ];
        for next in neighbours.into_iter().flatten() {
            if !blocked[next] && !floor[next] {
                floor[next] = true;
                queue.push_back(next);
            }
        }
    }

    // Walls: blocked cells within a wall's thickness of the floor, found by a breadth-first
    // walk out of the floor into the blocked cells, so the cost is one visit per cell rather
    // than a neighbourhood search around each. The grown margin is a character's radius, so
    // the reach is that plus a wall's own thickness.
    let cell = cell_x.min(cell_z).max(f32::EPSILON);
    let reach = (((WALK_CLEARANCE + 0.6) / cell).ceil() as u32).max(2);
    let mut cells = vec![MapCell::Outside; w * h];
    let mut depth = vec![u32::MAX; w * h];
    let mut walk: VecDeque<usize> = VecDeque::new();
    for index in 0..w * h {
        if floor[index] {
            cells[index] = MapCell::Floor;
            depth[index] = 0;
            walk.push_back(index);
        }
    }
    while let Some(index) = walk.pop_front() {
        let next_depth = depth[index] + 1;
        if next_depth > reach {
            continue;
        }
        let (x, y) = ((index % w) as isize, (index / w) as isize);
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (nx, ny) = (x + dx, y + dy);
                if (dx == 0 && dy == 0) || nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize
                {
                    continue;
                }
                let next = ny as usize * w + nx as usize;
                if blocked[next] && depth[next] == u32::MAX {
                    depth[next] = next_depth;
                    cells[next] = MapCell::Wall;
                    walk.push_back(next);
                }
            }
        }
    }
    WalkableMask {
        width: w as u32,
        height: h as u32,
        cells,
    }
}

/// How much of a box's local height reaches straight up after its rotation. One for an
/// upright box.
fn vertical_reach(world: &crate::object::WorldBox) -> f32 {
    let up = world.rotation.rotate(Vec3::new(0.0, 1.0, 0.0));
    up.y.abs().max(0.2)
}

/// The floor under `position`: the terrain there if the level has one, else its ground.
fn floor_height(scene: &EnvironmentScene, position: Vec3) -> f32 {
    scene
        .terrain
        .as_ref()
        .map_or(scene.ground_height, |terrain| terrain.height_at(position))
}

/// The open cell nearest `(x, z)` within `reach` cells, if any.
fn nearest_open(
    blocked: &[bool],
    w: usize,
    h: usize,
    x: isize,
    z: isize,
    reach: isize,
) -> Option<usize> {
    let mut best: Option<(isize, usize)> = None;
    for dz in -reach..=reach {
        for dx in -reach..=reach {
            let (cx, cz) = (x + dx, z + dz);
            if cx < 0 || cz < 0 || cx >= w as isize || cz >= h as isize {
                continue;
            }
            let index = cz as usize * w + cx as usize;
            if blocked[index] {
                continue;
            }
            let distance = dx * dx + dz * dz;
            if best.is_none_or(|(held, _)| distance < held) {
                best = Some((distance, index));
            }
        }
    }
    best.map(|(_, index)| index)
}

/// A fingerprint of everything in `scene` that the baked picture depends on.
///
/// Objects, collision shapes, terrain, water, lighting, the floor and the map settings — the
/// scene file's own encoding of them, hashed with FNV-1a. Monsters, the dungeon settings and
/// spawn points are left out: they are drawn as markers over the picture, never into it, so
/// moving one does not make the picture stale.
#[must_use]
pub fn geometry_hash(scene: &EnvironmentScene) -> u64 {
    let mut document = crate::scene_file::SceneDocument::from_scene(scene);
    document.monsters.clear();
    document.dungeon = None;
    document.spawn_points.clear();
    document.spawns = None;
    document.name.clear();
    let text = serde_json::to_string(&document).unwrap_or_default();
    fnv1a(text.as_bytes())
}

/// FNV-1a, 64-bit. Stable across runs, builds and machines, unlike `DefaultHasher`.
#[must_use]
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// One encounter, as the sidecar file writes it: what the map drew, for tools and inspection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncounterRecord {
    /// Its pack key.
    pub key: String,
    /// `"pack"` or `"boss"`.
    pub kind: String,
    /// The author's name for it, if any member has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The middle of its members, in the world.
    pub position: [f32; 3],
    /// The same point on the map.
    pub uv: [f32; 2],
    /// Who is in it.
    pub members: Vec<EncounterMemberRecord>,
}

/// One monster kind in an [`EncounterRecord`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncounterMemberRecord {
    /// The monster id.
    pub monster: String,
    /// How many.
    pub count: u32,
    /// The highest level among them.
    pub level: u8,
}

/// What a baked map is, beside the picture: `assets/generated/maps/<level>.map.cfg`.
///
/// The picture's frame and size are authoritative for drawing it; the encounters are a
/// snapshot for tools and for a reader of the file — the game derives its markers from the
/// level itself, so they are never stale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapDocument {
    /// [`MAP_FILE_VERSION`].
    pub version: u32,
    /// The level's name.
    pub scene: String,
    /// [`geometry_hash`] of the level that was baked, as sixteen hex digits.
    pub geometry_hash: String,
    /// Picture width, texels.
    pub width: u32,
    /// Picture height, texels.
    pub height: u32,
    /// The frame: `[min_x, min_z, max_x, max_z]`.
    pub frame: [f32; 4],
    /// The vertical range the bake saw: `[floor, ceiling]`.
    pub vertical_range: [f32; 2],
    /// Texels per metre, after any clamp to [`MAX_MAP_TEXELS`].
    pub texels_per_metre: f32,
    /// Milliseconds the bake took, capture and styling together.
    #[serde(default)]
    pub bake_ms: f32,
    /// The encounters at bake time.
    #[serde(default)]
    pub encounters: Vec<EncounterRecord>,
}

impl MapDocument {
    /// The document for `scene` baked into a `width × height` picture of `frame`.
    #[must_use]
    pub fn describe(
        scene: &EnvironmentScene,
        frame: &MapFrame,
        width: u32,
        height: u32,
        bake_ms: f32,
    ) -> Self {
        let (floor, ceiling) = scene.map.vertical_range(scene.ground_height);
        Self {
            version: MAP_FILE_VERSION,
            scene: scene.name.to_string(),
            geometry_hash: format!("{:016x}", geometry_hash(scene)),
            width,
            height,
            frame: [frame.min_x, frame.min_z, frame.max_x, frame.max_z],
            vertical_range: [floor, ceiling],
            texels_per_metre: width as f32 / frame.width().max(f32::EPSILON),
            bake_ms,
            encounters: encounters(scene)
                .iter()
                .map(|encounter| {
                    let (u, v) = frame.world_to_uv(encounter.position);
                    EncounterRecord {
                        key: encounter.key.clone(),
                        kind: encounter.kind.as_str().to_owned(),
                        name: encounter.name.clone(),
                        position: [
                            encounter.position.x,
                            encounter.position.y,
                            encounter.position.z,
                        ],
                        uv: [u, v],
                        members: encounter
                            .members
                            .iter()
                            .map(|member| EncounterMemberRecord {
                                monster: member.monster.clone(),
                                count: member.count,
                                level: member.level,
                            })
                            .collect(),
                    }
                })
                .collect(),
        }
    }

    /// The frame it records.
    #[must_use]
    pub fn frame(&self) -> MapFrame {
        MapFrame::new(self.frame[0], self.frame[1], self.frame[2], self.frame[3])
    }

    /// Whether this document describes a picture of `scene` as it is now.
    #[must_use]
    pub fn matches(&self, scene: &EnvironmentScene) -> bool {
        self.version == MAP_FILE_VERSION
            && self.geometry_hash == format!("{:016x}", geometry_hash(scene))
            && self.frame().is_valid()
            && self.width >= MIN_MAP_TEXELS
            && self.height >= MIN_MAP_TEXELS
    }

    /// Writes it as pretty JSON.
    ///
    /// # Errors
    ///
    /// The I/O error, if the directory cannot be made or the file cannot be written.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, text)
    }

    /// Reads one, or `None` if it is missing, unreadable or another version.
    #[must_use]
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let document: Self = serde_json::from_str(&text).ok()?;
        (document.version == MAP_FILE_VERSION).then_some(document)
    }
}

/// Where a level's baked map lives: `(<root>/generated/maps/<name>.map.png, ….map.cfg)`.
///
/// The document is JSON with a `.cfg` extension, like `settings.cfg`: the editor's asset
/// browser reads a `.json` under `assets/` as a material, and this is not one.
///
/// `assets_root` is the `assets/` directory. Keyed by the level's name, which is its file stem
/// for a level on disk and its layout name for a built-in one.
#[must_use]
pub fn map_paths(assets_root: &Path, scene_name: &str) -> (PathBuf, PathBuf) {
    let stem: String = scene_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stem = if stem.is_empty() {
        "level".to_owned()
    } else {
        stem
    };
    let directory = assets_root.join("generated").join("maps");
    (
        directory.join(format!("{stem}.map.png")),
        directory.join(format!("{stem}.map.cfg")),
    )
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use crate::asset::WALL_SEGMENT;
    use crate::monster::{DungeonSettings, MonsterPlacement};
    use crate::object::{EnvironmentCollision, EnvironmentObject};

    fn wall(x: f32, z: f32, half_x: f32, half_z: f32) -> EnvironmentObject {
        EnvironmentObject::decoration(WALL_SEGMENT, Vec3::new(x, 0.0, z)).with_collision(
            EnvironmentCollision::Box {
                center: Vec3::new(0.0, 1.5, 0.0),
                half_extents: Vec3::new(half_x, 1.5, half_z),
            },
        )
    }

    /// A closed 20 × 20 m room around the origin, with the player inside it.
    fn room() -> EnvironmentScene {
        let mut scene = EnvironmentScene::empty("room");
        scene.half_extent = 60.0;
        scene.push(wall(0.0, -10.0, 10.5, 0.5));
        scene.push(wall(0.0, 10.0, 10.5, 0.5));
        scene.push(wall(-10.0, 0.0, 0.5, 10.5));
        scene.push(wall(10.0, 0.0, 0.5, 10.5));
        scene.spawns.player = Vec3::new(0.0, 0.0, 0.0);
        scene.map.padding = 5.0;
        scene
    }

    #[test]
    fn world_and_map_coordinates_are_inverses_and_north_is_the_top() {
        let frame = MapFrame::new(-96.0, -296.0, 96.0, 8.0);
        for (x, z) in [(-96.0, -296.0), (0.0, -100.0), (96.0, 8.0), (13.5, -7.25)] {
            let (u, v) = frame.world_to_uv(Vec3::new(x, 3.0, z));
            let (bx, bz) = frame.uv_to_world(u, v);
            assert!((bx - x).abs() < 1e-3 && (bz - z).abs() < 1e-3, "{x},{z}");
        }
        assert_eq!(frame.world_to_uv(Vec3::new(-96.0, 0.0, -296.0)), (0.0, 0.0));
        let (_, v_north) = frame.world_to_uv(Vec3::new(0.0, 0.0, -200.0));
        let (_, v_south) = frame.world_to_uv(Vec3::new(0.0, 0.0, 0.0));
        assert!(v_north < v_south, "−Z is north, the top of the picture");
    }

    #[test]
    fn a_non_square_frame_keeps_its_aspect_in_texels() {
        let frame = MapFrame::new(-96.0, -296.0, 96.0, 8.0);
        let (w, h) = frame.texture_size(5.0);
        assert_eq!((w, h), (960, 1520));
        let texel_x = frame.width() / w as f32;
        let texel_z = frame.depth() / h as f32;
        assert!((texel_x - texel_z).abs() < 1e-3, "square texels");

        // Too big: scaled down as a whole, not squashed.
        let huge = MapFrame::new(0.0, 0.0, 1000.0, 250.0);
        let (w, h) = huge.texture_size(5.0);
        assert_eq!(w, MAX_MAP_TEXELS);
        assert!((w as f32 / h as f32 - 4.0).abs() < 0.01, "{w}x{h}");
    }

    #[test]
    fn the_frame_covers_the_level_and_its_padding_and_nothing_past_the_boundary() {
        let scene = room();
        let frame = frame_for(&scene);
        // Walls reach ±11 (half 10.5 + 0.5 thick), plus 5 m of padding.
        assert!((frame.min_x + 16.0).abs() < 0.6, "{frame:?}");
        assert!((frame.max_z - 16.0).abs() < 0.6, "{frame:?}");

        let mut tight = room();
        tight.half_extent = 12.0;
        let frame = frame_for(&tight);
        assert!(frame.min_x >= -12.0 && frame.max_x <= 12.0, "{frame:?}");

        let mut chosen = room();
        chosen.map.bounds = Some(MapFrame::new(-1.0, -2.0, 3.0, 4.0));
        assert_eq!(frame_for(&chosen), MapFrame::new(-1.0, -2.0, 3.0, 4.0));
    }

    #[test]
    fn the_mask_floods_the_room_and_leaves_the_outside_out() {
        let scene = room();
        let frame = frame_for(&scene);
        let (w, h) = frame.texture_size(4.0);
        let mask = walkable_mask(&scene, &frame, w, h);
        let at = |x: f32, z: f32| {
            let (u, v) = frame.world_to_uv(Vec3::new(x, 0.0, z));
            mask.cell((u * w as f32) as u32, (v * h as f32) as u32)
        };
        assert_eq!(at(0.0, 0.0), MapCell::Floor);
        assert_eq!(at(8.0, -8.0), MapCell::Floor);
        assert_eq!(at(0.0, -10.0), MapCell::Wall);
        assert_eq!(at(0.0, 14.0), MapCell::Outside, "beyond the south wall");
        assert_eq!(at(-14.0, -14.0), MapCell::Outside);
        // The room is roughly 19 × 19 m of floor at 16 cells per square metre.
        let floor_m2 = mask.count(MapCell::Floor) as f32 / 16.0;
        assert!((300.0..400.0).contains(&floor_m2), "{floor_m2}");
    }

    #[test]
    fn a_doorway_lets_the_floor_through_and_a_hairline_gap_does_not() {
        let mut scene = room();
        // Replace the east wall with two halves and a 3 m doorway.
        scene.objects.pop();
        scene.push(wall(10.0, -6.5, 0.5, 4.0));
        scene.push(wall(10.0, 6.5, 0.5, 4.0));
        let frame = MapFrame::new(-16.0, -16.0, 16.0, 16.0);
        let mask = walkable_mask(&scene, &frame, 128, 128);
        let at = |x: f32, z: f32| {
            let (u, v) = frame.world_to_uv(Vec3::new(x, 0.0, z));
            mask.cell((u * 128.0) as u32, (v * 128.0) as u32)
        };
        assert_eq!(at(14.0, 0.0), MapCell::Floor, "through the doorway");

        let mut sealed = room();
        sealed.objects.pop();
        // A 0.2 m crack: narrower than a character, so not a doorway.
        sealed.push(wall(10.0, -5.3, 0.5, 5.2));
        sealed.push(wall(10.0, 5.3, 0.5, 5.2));
        let mask = walkable_mask(&sealed, &frame, 128, 128);
        let (u, v) = frame.world_to_uv(Vec3::new(14.0, 0.0, 0.0));
        assert_eq!(
            mask.cell((u * 128.0) as u32, (v * 128.0) as u32),
            MapCell::Outside
        );
    }

    #[test]
    fn encounters_are_one_per_pack_centred_on_their_members_and_the_boss_is_marked() {
        let mut scene = room();
        scene
            .monsters
            .push(MonsterPlacement::new("crypt_ghoul", Vec3::new(2.0, 0.0, 2.0)).in_pack("A"));
        scene
            .monsters
            .push(MonsterPlacement::new("crypt_ghoul", Vec3::new(4.0, 0.0, 2.0)).in_pack("A"));
        scene
            .monsters
            .push(MonsterPlacement::new("crypt_archer", Vec3::new(3.0, 0.0, 5.0)).in_pack("A"));
        scene.monsters.push(
            MonsterPlacement::new("lord_ashgrave", Vec3::new(-5.0, 0.0, -5.0)).in_pack("BOSS"),
        );
        scene.monsters.push(MonsterPlacement::new(
            "grave_brute",
            Vec3::new(6.0, 0.0, -6.0),
        ));
        let mut off = MonsterPlacement::new("grave_brute", Vec3::new(1.0, 0.0, 1.0));
        off.enabled = false;
        scene.monsters.push(off);
        let mut dungeon = DungeonSettings::new("ROOM", 10.0);
        dungeon.boss_pack = Some("BOSS".to_owned());
        scene.dungeon = Some(dungeon);

        let list = encounters(&scene);
        assert_eq!(
            list.len(),
            3,
            "A, BOSS and the lone brute; the disabled one is left out"
        );
        assert_eq!(list[0].key, "A");
        assert_eq!(list[0].monster_count(), 3);
        assert_eq!(list[0].members.len(), 2);
        assert_eq!(list[0].members[0].count, 2);
        assert!((list[0].position.x - 3.0).abs() < 1e-5 && (list[0].position.z - 3.0).abs() < 1e-5);
        assert_eq!(list[0].kind, EncounterKind::Pack);
        assert_eq!(list[1].kind, EncounterKind::Boss);
        assert_eq!(list[2].key, "#4");
        assert_eq!(encounters(&scene), list, "deterministic");
    }

    #[test]
    fn moving_a_monster_changes_the_encounters_but_not_the_geometry_hash() {
        let mut scene = room();
        scene
            .monsters
            .push(MonsterPlacement::new("crypt_ghoul", Vec3::new(2.0, 0.0, 2.0)).in_pack("A"));
        let hash = geometry_hash(&scene);
        let before = encounters(&scene);
        scene.monsters[0].position.x += 3.0;
        assert_eq!(geometry_hash(&scene), hash);
        assert_ne!(encounters(&scene), before);

        scene.objects[0].transform.translation.x += 1.0;
        assert_ne!(
            geometry_hash(&scene),
            hash,
            "moving a wall makes the picture stale"
        );
    }

    #[test]
    fn the_sidecar_round_trips_and_knows_when_it_is_stale() {
        let mut scene = room();
        scene.monsters.push(MonsterPlacement::new(
            "crypt_ghoul",
            Vec3::new(2.0, 0.0, 2.0),
        ));
        let frame = frame_for(&scene);
        let (w, h) = frame.texture_size(scene.map.texels_per_metre);
        let document = MapDocument::describe(&scene, &frame, w, h, 12.0);
        assert!(document.matches(&scene));
        assert_eq!(document.encounters.len(), 1);
        let (u, v) = frame.world_to_uv(Vec3::new(2.0, 0.0, 2.0));
        assert!((document.encounters[0].uv[0] - u).abs() < 1e-6);
        assert!((document.encounters[0].uv[1] - v).abs() < 1e-6);

        let directory = std::env::temp_dir().join(format!("map-doc-{}", std::process::id()));
        let (_, json) = map_paths(&directory, "room");
        document.save(&json).expect("writes");
        let loaded = MapDocument::load(&json).expect("reads");
        assert_eq!(loaded, document);
        let _ = std::fs::remove_dir_all(&directory);

        scene.objects[1].transform.translation.z += 2.0;
        assert!(!document.matches(&scene));
    }

    #[test]
    fn map_settings_survive_the_scene_file_and_the_defaults_are_left_out() {
        let mut scene = room();
        scene.map.ceiling = Some(9.0);
        scene.map.bounds = Some(MapFrame::new(-20.0, -20.0, 20.0, 20.0));
        let text = serde_json::to_string(&crate::scene_file::SceneDocument::from_scene(&scene))
            .expect("encodes");
        assert!(text.contains("\"map\""));
        let back = serde_json::from_str::<crate::scene_file::SceneDocument>(&text)
            .expect("decodes")
            .to_scene("test")
            .expect("valid");
        assert_eq!(back.map, scene.map);

        let plain = EnvironmentScene::empty("plain");
        let text = serde_json::to_string(&crate::scene_file::SceneDocument::from_scene(&plain))
            .expect("encodes");
        assert!(
            !text.contains("\"map\""),
            "a level with default settings carries no key"
        );
    }

    #[test]
    fn paths_are_keyed_by_a_file_safe_level_name() {
        let (png, json) = map_paths(Path::new("assets"), "graveyard crypt/1");
        assert!(png.ends_with("generated/maps/graveyard_crypt_1.map.png"));
        assert!(json.ends_with("generated/maps/graveyard_crypt_1.map.cfg"));
    }

    #[test]
    fn the_vertical_range_defaults_around_the_floor_and_rejects_an_inverted_one() {
        let settings = MapSettings::default();
        assert_eq!(settings.vertical_range(0.0), (-2.0, 12.0));
        let mut custom = MapSettings {
            floor: Some(1.0),
            ceiling: Some(5.0),
            ..MapSettings::default()
        };
        assert!(custom.is_valid());
        assert_eq!(custom.vertical_range(0.0), (1.0, 5.0));
        custom.ceiling = Some(0.0);
        assert!(!custom.is_valid());
    }
}
