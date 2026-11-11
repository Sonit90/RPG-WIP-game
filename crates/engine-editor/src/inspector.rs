//! What the inspector's numbers are, and how typing one changes the level.
//!
//! ```text
//!  OBJECT  POS [x][y][z]   YAW [deg]         SCALE [x][y][z]
//!  WATER   POS [x][y][z]   SIZE [x][z][depth] YAW [deg]
//!  SPAWN   POS [x][y][z]   YAW [deg]
//!  LIGHT   POS [x][y][z]   LIGHT [range][intensity]  COLOR [r][g][b]
//!  MONSTER POS [x][y][z]   YAW [deg]   SIZE [scale]  LEVEL [n]  AGGRO [m]  WORTH [%]
//! ```
//!
//! A monster's aggro radius and its completion worth are the monster's own until an author
//! types one: the inspector shows the value that will be used either way, read from the
//! catalogue when the placement does not override it.
//!
//! One table per kind, read by both the painter and the pointer code, so the cell a click
//! lands in and the cell that is drawn are the same cell.
//!
//! # A group
//!
//! A position typed into a group moves every member by the amount the primary moved, so the
//! group keeps its shape. Everything else — a yaw, a scale, a colour — is set on each member,
//! because "make all of these face north" is what an author typing one yaw into a group means.

use engine_core::math::{Quat, Vec3};
use engine_environment::EnvironmentScene;

use crate::Selected;
use crate::monster::MonsterCatalogue;

/// One number the inspector shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    /// Position along X, metres.
    PositionX,
    /// Position along Y, metres.
    PositionY,
    /// Position along Z, metres.
    PositionZ,
    /// Rotation about +Y, degrees.
    Yaw,
    /// Scale along the object's own X.
    ScaleX,
    /// Scale along the object's own Y.
    ScaleY,
    /// Scale along the object's own Z.
    ScaleZ,
    /// A body of water's width, metres.
    SizeX,
    /// A body of water's length, metres.
    SizeZ,
    /// A body of water's depth, metres.
    Depth,
    /// How far a light reaches, metres.
    Range,
    /// How strong a light is.
    Intensity,
    /// A light's red.
    Red,
    /// A light's green.
    Green,
    /// A light's blue.
    Blue,
    /// A monster's uniform size.
    MonsterScale,
    /// A monster's level.
    Level,
    /// How close the player must come before a monster notices, in metres.
    Aggro,
    /// What killing a monster is worth towards dungeon completion.
    Worth,
}

impl Property {
    /// How much one notch of the wheel over the field changes it.
    #[must_use]
    pub fn wheel_step(self, grid: f32, yaw_step: f32) -> f32 {
        match self {
            Self::PositionX | Self::PositionY | Self::PositionZ => {
                if grid > 0.0 {
                    grid
                } else {
                    0.1
                }
            }
            Self::Yaw => yaw_step,
            Self::ScaleX | Self::ScaleY | Self::ScaleZ => 0.1,
            Self::SizeX | Self::SizeZ | Self::Depth => 0.5,
            Self::Range => 1.0,
            Self::Intensity => 0.25,
            Self::Red | Self::Green | Self::Blue => 0.05,
            Self::MonsterScale => 0.1,
            Self::Level | Self::Aggro | Self::Worth => 1.0,
        }
    }

    /// The smallest value it may take, so nothing typed produces a level the loader refuses.
    fn minimum(self) -> f32 {
        match self {
            Self::PositionX | Self::PositionY | Self::PositionZ | Self::Yaw => f32::NEG_INFINITY,
            Self::ScaleX | Self::ScaleY | Self::ScaleZ => 0.01,
            Self::SizeX | Self::SizeZ => crate::WATER_MIN_HALF_EXTENT * 2.0,
            Self::Depth | Self::Range => 0.1,
            Self::Intensity | Self::Red | Self::Green | Self::Blue => 0.0,
            Self::MonsterScale => 0.25,
            Self::Level => 1.0,
            Self::Aggro | Self::Worth => 0.0,
        }
    }

    /// The largest value it may take, for the few that have one.
    fn maximum(self) -> f32 {
        match self {
            Self::MonsterScale => 4.0,
            Self::Level => f32::from(engine_environment::monster::MAX_MONSTER_LEVEL),
            Self::Aggro => 80.0,
            _ => f32::INFINITY,
        }
    }

    /// Whether a group moves by the difference rather than every member taking the value.
    fn is_position(self) -> bool {
        matches!(self, Self::PositionX | Self::PositionY | Self::PositionZ)
    }
}

/// One row of the inspector: a short label and up to three numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// What is written at the left, five characters at most so the cells line up.
    pub label: &'static str,
    /// The numbers, left to right.
    pub properties: &'static [Property],
}

const POSITION: Row = Row {
    label: "POS",
    properties: &[
        Property::PositionX,
        Property::PositionY,
        Property::PositionZ,
    ],
};
const YAW: Row = Row {
    label: "YAW",
    properties: &[Property::Yaw],
};

/// The rows the inspector shows for one kind of thing, in order.
#[must_use]
pub fn rows(what: Selected) -> &'static [Row] {
    match what {
        Selected::Object(_) => &[
            POSITION,
            YAW,
            Row {
                label: "SCALE",
                properties: &[Property::ScaleX, Property::ScaleY, Property::ScaleZ],
            },
        ],
        Selected::Water(_) => &[
            POSITION,
            Row {
                label: "SIZE",
                properties: &[Property::SizeX, Property::SizeZ, Property::Depth],
            },
            YAW,
        ],
        Selected::Spawn(_) => &[POSITION, YAW],
        Selected::Monster(_) => &[
            POSITION,
            YAW,
            Row {
                label: "SIZE",
                properties: &[Property::MonsterScale, Property::Level],
            },
            Row {
                label: "PULL",
                properties: &[Property::Aggro, Property::Worth],
            },
        ],
        Selected::Light(_) => &[
            POSITION,
            Row {
                label: "LIGHT",
                properties: &[Property::Range, Property::Intensity],
            },
            Row {
                label: "COLOR",
                properties: &[Property::Red, Property::Green, Property::Blue],
            },
        ],
    }
}

/// Every property of a kind, in reading order: what `TAB` walks.
pub fn properties(what: Selected) -> impl Iterator<Item = Property> {
    rows(what)
        .iter()
        .flat_map(|row| row.properties.iter().copied())
}

/// A property's value on one thing, as the inspector shows it.
#[must_use]
pub fn get(
    scene: &EnvironmentScene,
    monsters: &MonsterCatalogue,
    what: Selected,
    property: Property,
) -> Option<f32> {
    use Property as P;
    let position = crate::position_of(scene, what)?;
    match property {
        P::PositionX => return Some(position.x),
        P::PositionY => return Some(position.y),
        P::PositionZ => return Some(position.z),
        _ => {}
    }
    match what {
        Selected::Object(index) => {
            let object = scene.objects.get(index)?;
            match property {
                P::Yaw => Some(crate::yaw_of(object.transform).to_degrees()),
                P::ScaleX => Some(object.transform.scale.x),
                P::ScaleY => Some(object.transform.scale.y),
                P::ScaleZ => Some(object.transform.scale.z),
                _ => None,
            }
        }
        Selected::Water(index) => {
            let body = scene.water.get(index)?;
            match property {
                P::Yaw => Some(body.yaw.to_degrees()),
                P::SizeX => Some(body.half_extents.x * 2.0),
                P::SizeZ => Some(body.half_extents.y * 2.0),
                P::Depth => Some(body.depth),
                _ => None,
            }
        }
        Selected::Spawn(index) => {
            let spawn = scene.spawn_points.get(index)?;
            (property == P::Yaw).then(|| spawn.yaw.to_degrees())
        }
        Selected::Light(index) => {
            let light = scene.lights.get(index)?;
            match property {
                P::Range => Some(light.range),
                P::Intensity => Some(light.intensity),
                P::Red => Some(light.color.x),
                P::Green => Some(light.color.y),
                P::Blue => Some(light.color.z),
                _ => None,
            }
        }
        Selected::Monster(index) => {
            let monster = scene.monsters.get(index)?;
            let own = monsters.find(&monster.monster);
            match property {
                P::Yaw => Some(monster.yaw.to_degrees()),
                P::MonsterScale => Some(monster.scale),
                P::Level => Some(f32::from(monster.level)),
                // What will be used: the placement's own number, or the monster's.
                P::Aggro => monster
                    .aggro_radius
                    .or_else(|| own.map(|entry| entry.aggro_radius)),
                P::Worth => monster
                    .completion
                    .or_else(|| own.map(|entry| entry.completion)),
                _ => None,
            }
        }
    }
}

/// Sets `property` on one thing. Returns whether it had that property.
fn set_one(scene: &mut EnvironmentScene, what: Selected, property: Property, value: f32) -> bool {
    use Property as P;
    let value = value.max(property.minimum()).min(property.maximum());
    if property.is_position() {
        let Some(position) = crate::position_of(scene, what) else {
            return false;
        };
        let target = match property {
            P::PositionX => Vec3::new(value, position.y, position.z),
            P::PositionY => Vec3::new(position.x, value, position.z),
            _ => Vec3::new(position.x, position.y, value),
        };
        crate::move_by(scene, what, target - position);
        return true;
    }
    match what {
        Selected::Object(index) => {
            let Some(object) = scene.objects.get_mut(index) else {
                return false;
            };
            match property {
                P::Yaw => {
                    object.transform.rotation =
                        Quat::from_yaw_pitch_roll(value.to_radians(), 0.0, 0.0);
                }
                P::ScaleX => object.transform.scale.x = value,
                P::ScaleY => object.transform.scale.y = value,
                P::ScaleZ => object.transform.scale.z = value,
                _ => return false,
            }
        }
        Selected::Water(index) => {
            let Some(body) = scene.water.get_mut(index) else {
                return false;
            };
            match property {
                P::Yaw => body.yaw = value.to_radians(),
                P::SizeX => body.half_extents.x = value * 0.5,
                P::SizeZ => body.half_extents.y = value * 0.5,
                P::Depth => body.depth = value,
                _ => return false,
            }
        }
        Selected::Spawn(index) => {
            let Some(spawn) = scene.spawn_points.get_mut(index) else {
                return false;
            };
            if property != P::Yaw {
                return false;
            }
            spawn.yaw = value.to_radians();
        }
        Selected::Light(index) => {
            let Some(light) = scene.lights.get_mut(index) else {
                return false;
            };
            match property {
                P::Range => light.range = value,
                P::Intensity => light.intensity = value,
                P::Red => light.color.x = value,
                P::Green => light.color.y = value,
                P::Blue => light.color.z = value,
                _ => return false,
            }
        }
        Selected::Monster(index) => {
            let Some(monster) = scene.monsters.get_mut(index) else {
                return false;
            };
            match property {
                P::Yaw => monster.yaw = value.to_radians(),
                P::MonsterScale => monster.scale = value,
                P::Level => monster.level = value.round().clamp(1.0, 255.0) as u8,
                P::Aggro => monster.aggro_radius = Some(value),
                P::Worth => monster.completion = Some(value),
                _ => return false,
            }
        }
    }
    true
}

/// Sets `property` to `value` on the primary and applies it to the rest of `selection`.
///
/// Positions move every member by what the primary moved; everything else is set on each.
/// Returns whether anything changed.
pub fn set(
    scene: &mut EnvironmentScene,
    monsters: &MonsterCatalogue,
    selection: &[Selected],
    primary: Selected,
    property: Property,
    value: f32,
) -> bool {
    let Some(before) = get(scene, monsters, primary, property) else {
        return false;
    };
    if property.is_position() {
        let delta = value - before;
        if delta == 0.0 {
            return false;
        }
        for what in selection {
            let Some(current) = get(scene, monsters, *what, property) else {
                continue;
            };
            set_one(scene, *what, property, current + delta);
        }
        return true;
    }
    let mut changed = false;
    for what in selection {
        if get(scene, monsters, *what, property).is_some_and(|current| current != value) {
            changed |= set_one(scene, *what, property, value);
        }
    }
    changed
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_environment::EnvironmentObject;
    use engine_environment::asset::PILLAR;

    fn two_pillars() -> EnvironmentScene {
        let mut scene = EnvironmentScene::empty("inspect");
        scene.push(EnvironmentObject::decoration(
            PILLAR,
            Vec3::new(0.0, 0.0, 0.0),
        ));
        scene.push(EnvironmentObject::decoration(
            PILLAR,
            Vec3::new(4.0, 0.0, 0.0),
        ));
        scene
    }

    #[test]
    fn a_position_typed_into_a_group_moves_the_group_and_keeps_its_shape() {
        let mut scene = two_pillars();
        let group = [Selected::Object(0), Selected::Object(1)];
        assert!(set(
            &mut scene,
            &MonsterCatalogue::default(),
            &group,
            Selected::Object(1),
            Property::PositionX,
            10.0
        ));
        assert_eq!(scene.objects[1].position().x, 10.0);
        assert_eq!(
            scene.objects[0].position().x,
            6.0,
            "moved by the same six metres"
        );
    }

    #[test]
    fn a_yaw_typed_into_a_group_turns_every_member_to_it() {
        let mut scene = two_pillars();
        let group = [Selected::Object(0), Selected::Object(1)];
        set(
            &mut scene,
            &MonsterCatalogue::default(),
            &group,
            Selected::Object(0),
            Property::Yaw,
            90.0,
        );
        for index in 0..2 {
            let yaw = get(
                &scene,
                &MonsterCatalogue::default(),
                Selected::Object(index),
                Property::Yaw,
            )
            .expect("a yaw");
            assert!((yaw - 90.0).abs() < 1e-3, "{index} faces {yaw}");
        }
    }

    #[test]
    fn nothing_typed_can_make_a_level_the_loader_refuses() {
        let mut scene = two_pillars();
        let one = [Selected::Object(0)];
        set(
            &mut scene,
            &MonsterCatalogue::default(),
            &one,
            Selected::Object(0),
            Property::ScaleY,
            0.0,
        );
        assert!(scene.objects[0].transform.scale.y > 0.0);

        scene.push_water(engine_environment::water::WaterBody::new(
            Vec3::ZERO,
            engine_core::math::Vec2::new(4.0, 4.0),
            2.0,
        ));
        let water = [Selected::Water(0)];
        let none = MonsterCatalogue::default();
        set(
            &mut scene,
            &none,
            &water,
            Selected::Water(0),
            Property::SizeX,
            -3.0,
        );
        set(
            &mut scene,
            &none,
            &water,
            Selected::Water(0),
            Property::Depth,
            0.0,
        );
        assert!(scene.water[0].is_valid());
    }

    #[test]
    fn every_kind_has_a_position_first_and_every_row_fits_three_cells() {
        for what in [
            Selected::Object(0),
            Selected::Water(0),
            Selected::Spawn(0),
            Selected::Light(0),
            Selected::Monster(0),
        ] {
            assert_eq!(rows(what)[0], POSITION);
            for row in rows(what) {
                assert!(row.properties.len() <= 3 && row.label.len() <= 5);
            }
        }
    }
}
