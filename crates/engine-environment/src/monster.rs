//! Where a monster stands, and what a level says about the dungeon it is part of.
//!
//! ```text
//!   MonsterPlacement { monster: "skeleton_warrior", position, yaw, pack, level, … }
//!          │  by name, never by index
//!          ▼
//!   engine_game::enemy::EnemyKind::from_id ──▶ an archetype row ──▶ a spawned enemy
//! ```
//!
//! # Only a name crosses the boundary
//!
//! This crate may not name `engine_game`, so a placement cannot hold an `EnemyKind`. It holds
//! the monster's *id* — the same string the monster table spells it with — and gameplay
//! resolves it when the level is built. The file therefore survives a monster being added,
//! reordered or renamed in the table exactly the way an object survives the asset library
//! growing: see [`crate::scene_file::asset_named`] for the same rule applied to meshes.
//!
//! # A placement is the third kind of thing an author places
//!
//! After water and spawn points. Like them it is a list of records the editor's tagged
//! selection can name — click to select, drag to move, `R` to turn, `CTRL+D` to duplicate,
//! `X` to delete, `CTRL+Z` to take it back — rather than a field of the scene.
//!
//! # What the dungeon settings are for
//!
//! A level with monsters in it is a fight. A level with monsters *and* [`DungeonSettings`] is a
//! dungeon: its kills add up to a completion percentage, its boss is sealed behind a gate until
//! that percentage reaches what the settings require, and the boss's adds come out of a pool
//! the settings name. Every number that is content — the requirement, the gate, where adds
//! appear, how many — lives here, in the file, and not in the code that runs it.

use engine_core::math::Vec3;

use crate::spawn::SpawnPoint;

/// The highest level a placement may be set to.
///
/// Levels scale a monster's health and damage by a few per cent each (see
/// `engine_game::enemy::level_scale`). Ten is far past anything this dungeon uses and low
/// enough that a typo cannot produce a monster with a hundred times its health.
pub const MAX_MONSTER_LEVEL: u8 = 10;

/// One monster standing in a level.
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterPlacement {
    /// Which monster, by the id the monster table spells it with: `"skeleton_warrior"`.
    pub monster: String,
    /// Where its feet go.
    pub position: Vec3,
    /// Which way it faces while it is idle, radians about +Y. Zero faces −Z.
    pub yaw: f32,
    /// Uniform size multiplier. One is the monster as designed.
    ///
    /// Visual and physical together: a larger skeleton is a wider body, because a picture
    /// and a hitbox that disagree are a bug report.
    pub scale: f32,
    /// A name the author gave it, shown instead of the monster's own when set.
    pub name: Option<String>,
    /// Which pack it fights with.
    ///
    /// Placements that share a pack name are one encounter: pulling one pulls them all, and
    /// the pack is cleared when all of them are dead. A placement with no pack is a pack of one.
    pub pack: Option<String>,
    /// How close the player must come before it notices, in metres. `None` is the monster's
    /// own radius.
    pub aggro_radius: Option<f32>,
    /// Its level, `1..=MAX_MONSTER_LEVEL`. One is the monster as designed.
    pub level: u8,
    /// Whether it is spawned at all.
    ///
    /// An author switches a placement off to try the level without it, and switches it back
    /// on without having to remember where it stood.
    pub enabled: bool,
    /// What killing it is worth towards dungeon completion. `None` is the monster's own value.
    pub completion: Option<f32>,
    /// Waypoints `[x, z]` it walks in a loop while idle. Empty means it stands where it is
    /// placed. Aggro works as for any idle monster, from wherever it is on the loop.
    pub patrol: Vec<[f32; 2]>,
}

impl MonsterPlacement {
    /// A `monster` standing at `position`, facing −Z, at level one, enabled.
    #[must_use]
    pub fn new(monster: impl Into<String>, position: Vec3) -> Self {
        Self {
            monster: monster.into(),
            position,
            yaw: 0.0,
            scale: 1.0,
            name: None,
            pack: None,
            aggro_radius: None,
            level: 1,
            enabled: true,
            completion: None,
            patrol: Vec::new(),
        }
    }

    /// The same placement, turned.
    #[must_use]
    pub fn with_yaw(mut self, yaw: f32) -> Self {
        self.yaw = yaw;
        self
    }

    /// The same placement, in `pack`.
    #[must_use]
    pub fn in_pack(mut self, pack: impl Into<String>) -> Self {
        self.pack = Some(pack.into());
        self
    }

    /// Whether the numbers describe a monster that could actually be spawned.
    ///
    /// What the loader checks. A monster at a position that is not a number is a monster
    /// nothing can find again, and a level outside `1..=MAX_MONSTER_LEVEL` is a typo.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let finite = self.position.x.is_finite()
            && self.position.y.is_finite()
            && self.position.z.is_finite()
            && self.yaw.is_finite()
            && self.scale.is_finite();
        finite
            && self.scale > 0.0
            && !self.monster.trim().is_empty()
            && (1..=MAX_MONSTER_LEVEL).contains(&self.level)
            && self
                .aggro_radius
                .is_none_or(|radius| radius.is_finite() && radius >= 0.0)
            && self
                .completion
                .is_none_or(|value| value.is_finite() && value >= 0.0)
            && self
                .patrol
                .iter()
                .all(|point| point[0].is_finite() && point[1].is_finite())
    }

    /// The name the pack is known by: the author's, or one made from the placement's index.
    #[must_use]
    pub fn pack_key(&self, index: usize) -> String {
        match &self.pack {
            Some(pack) if !pack.trim().is_empty() => pack.clone(),
            _ => format!("#{index}"),
        }
    }
}

/// A box the boss is sealed behind until the dungeon is complete enough.
///
/// A doorway's worth of wall, standing across the arena entrance. Axis-aligned in the level's
/// own frame after `yaw` is applied, like every other collision box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BossGate {
    /// The middle of the barrier, on the floor.
    pub center: Vec3,
    /// Half its width, half its height and half its thickness, in metres, before rotation.
    pub half_extents: Vec3,
    /// Rotation about +Y, radians.
    pub yaw: f32,
}

impl BossGate {
    /// Whether every number is one a collision world can use.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        [
            self.center.x,
            self.center.y,
            self.center.z,
            self.half_extents.x,
            self.half_extents.y,
            self.half_extents.z,
            self.yaw,
        ]
        .iter()
        .all(|value| value.is_finite())
            && self.half_extents.x > 0.0
            && self.half_extents.y > 0.0
            && self.half_extents.z > 0.0
    }

    /// The barrier's two ends, on the floor: what presentation draws a wall between.
    #[must_use]
    pub fn ends(&self) -> (Vec3, Vec3) {
        let (sin, cos) = self.yaw.sin_cos();
        // Yaw zero lays the barrier along X; turning follows every placement's convention.
        let along = Vec3::new(cos, 0.0, -sin) * self.half_extents.x;
        (self.center - along, self.center + along)
    }
}

/// What a level says about the dungeon it is.
#[derive(Debug, Clone, PartialEq)]
pub struct DungeonSettings {
    /// What the dungeon is called, for the progress panel: `"GRAVEYARD CRYPT"`.
    pub name: String,
    /// The completion value that counts as 100 %.
    ///
    /// The sum of the completion values of the monsters that have to die. Placing more
    /// monsters than that is what makes some of them optional.
    pub required_completion: f32,
    /// Which pack is the boss, by its pack name.
    pub boss_pack: Option<String>,
    /// The barrier the boss stands behind until the dungeon is complete enough.
    pub boss_gate: Option<BossGate>,
    /// Where the boss's adds appear when the fight starts.
    pub add_spawns: Vec<SpawnPoint>,
    /// How many adds the boss calls, chosen from [`DungeonSettings::add_pool`].
    pub add_count: u8,
    /// Which monsters an add may be, by id. Each one appears in its empowered form.
    pub add_pool: Vec<String>,
    /// Whether the same monster may be chosen twice.
    pub allow_duplicate_adds: bool,
}

impl DungeonSettings {
    /// A dungeon called `name` that is complete at `required_completion`.
    #[must_use]
    pub fn new(name: impl Into<String>, required_completion: f32) -> Self {
        Self {
            name: name.into(),
            required_completion,
            boss_pack: None,
            boss_gate: None,
            add_spawns: Vec::new(),
            add_count: 0,
            add_pool: Vec::new(),
            allow_duplicate_adds: false,
        }
    }

    /// Whether the numbers describe a dungeon that could be run.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.required_completion.is_finite()
            && self.required_completion > 0.0
            && self.boss_gate.is_none_or(|gate| gate.is_valid())
            && self.add_spawns.iter().all(SpawnPoint::is_valid)
            // Asking for more distinct adds than the pool holds is a promise the fight cannot
            // keep; saying so at load beats an encounter that quietly calls fewer.
            && (self.allow_duplicate_adds
                || usize::from(self.add_count) <= self.add_pool.len())
            && (self.add_count == 0 || !self.add_pool.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_placement_is_valid_and_level_one() {
        let placement = MonsterPlacement::new("crypt_ghoul", Vec3::new(1.0, 0.0, -2.0));
        assert!(placement.is_valid());
        assert_eq!(placement.level, 1);
        assert!(placement.enabled);
        assert_eq!(placement.pack_key(4), "#4");
        assert_eq!(placement.clone().in_pack("A").pack_key(4), "A");
    }

    #[test]
    fn nonsense_numbers_are_refused() {
        let mut placement = MonsterPlacement::new("crypt_ghoul", Vec3::ZERO);
        placement.level = 0;
        assert!(!placement.is_valid());
        placement.level = 1;
        placement.scale = 0.0;
        assert!(!placement.is_valid());
        placement.scale = 1.0;
        placement.position.x = f32::NAN;
        assert!(!placement.is_valid());
        placement.position.x = 0.0;
        placement.monster = "  ".to_owned();
        assert!(!placement.is_valid());
    }

    #[test]
    fn a_dungeon_cannot_promise_more_distinct_adds_than_it_has() {
        let mut settings = DungeonSettings::new("CRYPT", 100.0);
        settings.add_count = 2;
        settings.add_pool = vec!["a".to_owned()];
        assert!(!settings.is_valid());
        settings.allow_duplicate_adds = true;
        assert!(settings.is_valid());
        settings.required_completion = 0.0;
        assert!(!settings.is_valid());
    }

    #[test]
    fn a_gate_reaches_either_side_of_its_centre() {
        let gate = BossGate {
            center: Vec3::new(0.0, 0.0, -10.0),
            half_extents: Vec3::new(8.0, 3.0, 0.5),
            yaw: 0.0,
        };
        let (a, b) = gate.ends();
        assert!((a.x + 8.0).abs() < 1e-5 && (b.x - 8.0).abs() < 1e-5);
        assert!((a.z + 10.0).abs() < 1e-5);
        assert!(gate.is_valid());
    }
}
