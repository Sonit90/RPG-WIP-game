//! The monsters an author can place, as the editor knows them: names, sizes and colours.
//!
//! ```text
//!   engine_game::enemy::ARCHETYPES ──(engine-app)──▶ MonsterCatalogue ──▶ browser rows
//!                                                                   ├──▶ pick boxes
//!                                                                   └──▶ inspector
//! ```
//!
//! # Why the editor is handed a list rather than reading the table
//!
//! This crate edits *data* and depends on neither the renderer nor gameplay. The monster
//! table is gameplay's, so the application — which links both — turns it into this plain list
//! when it builds the editor. The editor then offers exactly what the game can spawn, and a
//! seventh monster added to the table appears in the browser with no line changed here.
//!
//! A monster the catalogue does not know — a level written by a newer build, a typo in a
//! hand-edited file — is still selectable, movable and deletable: it is drawn with a default
//! size, and the inspector says its id is unknown.

use engine_core::math::{Quat, Vec3};
use engine_environment::MonsterPlacement;

/// One monster the author can place.
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterEntry {
    /// Its id in a level file: `"skeleton_warrior"`.
    pub id: String,
    /// What it is called: `"SKELETON WARRIOR"`.
    pub name: String,
    /// What it does in a fight: `"MELEE"`, `"RANGED"`, `"BOSS"`.
    pub role: String,
    /// One line on the problem it poses.
    pub summary: String,
    /// The colour its marker is drawn in.
    pub tint: [f32; 3],
    /// How tall it stands, in metres.
    pub height: f32,
    /// How wide it is, in metres from its centre.
    pub radius: f32,
    /// How close the player must come before it notices, in metres.
    pub aggro_radius: f32,
    /// What killing it is worth towards dungeon completion.
    pub completion: f32,
    /// Whether it is ordinarily placed by hand. Empowered adds are called by a boss, not
    /// placed, and are listed after the rest.
    pub placeable: bool,
}

/// Every monster the editor offers, in the order the browser lists them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MonsterCatalogue {
    entries: Vec<MonsterEntry>,
}

/// The size a monster the catalogue does not know is drawn at: a person.
const UNKNOWN_HEIGHT: f32 = 1.9;
/// The width a monster the catalogue does not know is drawn at.
const UNKNOWN_RADIUS: f32 = 0.5;

impl MonsterCatalogue {
    /// A catalogue of `entries`.
    #[must_use]
    pub fn new(entries: Vec<MonsterEntry>) -> Self {
        Self { entries }
    }

    /// Every entry.
    #[must_use]
    pub fn entries(&self) -> &[MonsterEntry] {
        &self.entries
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry at `index`.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&MonsterEntry> {
        self.entries.get(index)
    }

    /// The entry whose id is `id`, if the catalogue has it.
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&MonsterEntry> {
        self.entries
            .iter()
            .find(|entry| entry.id.eq_ignore_ascii_case(id.trim()))
    }

    /// The index of the entry whose id is `id`.
    #[must_use]
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.id.eq_ignore_ascii_case(id.trim()))
    }

    /// The box a placement is clicked on and framed by: centre, half-extents, rotation.
    ///
    /// The monster's own body, scaled by the placement — what the game will spawn is what the
    /// editor lets the author grab.
    #[must_use]
    pub fn body_box(&self, placement: &MonsterPlacement) -> (Vec3, Vec3, Quat) {
        let (height, radius) = self
            .find(&placement.monster)
            .map_or((UNKNOWN_HEIGHT, UNKNOWN_RADIUS), |entry| {
                (entry.height, entry.radius)
            });
        let scale = placement.scale.max(0.1);
        let half = Vec3::new(radius * scale, height * scale * 0.5, radius * scale);
        (
            placement.position + Vec3::new(0.0, half.y, 0.0),
            half,
            Quat::from_yaw_pitch_roll(placement.yaw, 0.0, 0.0),
        )
    }

    /// What a placement's monster is called, or its raw id if the catalogue does not know it.
    #[must_use]
    pub fn display_name(&self, placement: &MonsterPlacement) -> String {
        if let Some(name) = placement.name.as_deref()
            && !name.trim().is_empty()
        {
            return name.to_owned();
        }
        self.find(&placement.monster).map_or_else(
            || format!("Unknown '{}'", placement.monster),
            |entry| crate::theme::display_name(&entry.name.to_ascii_lowercase()),
        )
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small catalogue for the editor's own tests: one melee, one ranged, one boss.
    #[must_use]
    pub(crate) fn catalogue() -> MonsterCatalogue {
        let entry = |id: &str, name: &str, role: &str, height: f32| MonsterEntry {
            id: id.to_owned(),
            name: name.to_owned(),
            role: role.to_owned(),
            summary: String::new(),
            tint: [0.9, 0.9, 0.8],
            height,
            radius: 0.6,
            aggro_radius: 14.0,
            completion: 4.0,
            placeable: true,
        };
        MonsterCatalogue::new(vec![
            entry("skeleton_warrior", "SKELETON WARRIOR", "MELEE", 2.1),
            entry("crypt_archer", "CRYPT ARCHER", "RANGED", 1.9),
            entry("lord_ashgrave", "LORD ASHGRAVE", "BOSS", 3.2),
        ])
    }

    #[test]
    fn a_monster_is_found_by_its_id_whatever_the_case() {
        let catalogue = catalogue();
        assert_eq!(catalogue.index_of("CRYPT_ARCHER"), Some(1));
        assert!(catalogue.find("nobody").is_none());
    }

    #[test]
    fn a_body_box_stands_on_the_placement_and_grows_with_its_scale() {
        let catalogue = catalogue();
        let mut placement = MonsterPlacement::new("skeleton_warrior", Vec3::new(3.0, 1.0, -2.0));
        let (centre, half, _) = catalogue.body_box(&placement);
        assert!((centre.y - (1.0 + 1.05)).abs() < 1e-4);
        assert!((half.x - 0.6).abs() < 1e-4);
        placement.scale = 2.0;
        let (_, bigger, _) = catalogue.body_box(&placement);
        assert!((bigger.y - 2.1).abs() < 1e-4);
        // Unknown: a person-sized box, so it can still be found and deleted.
        placement.monster = "from_a_newer_build".to_owned();
        let (_, unknown, _) = catalogue.body_box(&placement);
        assert!(unknown.y > 0.5);
        assert!(
            catalogue
                .display_name(&placement)
                .contains("from_a_newer_build")
        );
    }
}
