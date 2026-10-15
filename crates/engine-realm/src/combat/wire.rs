//! Combat events over the wire.
//!
//! ```text
//! server Simulation ─ CombatEvent{EntityId…} ─ encode ─► WireEvent{EntityRef…} ─ JSON ─►
//! client ─ decode(resolve EntityRef → local EntityId) ─► CombatEvent ─► inject into local stream
//! ```
//!
//! The server's entity ids mean nothing to a client, so every entity in an event travels as
//! an [`EntityRef`]: a player by account, a monster by its server key. Enumerations travel by
//! their position in the gameplay crate's own tables — server and client are built from the
//! same `engine-game`, which the realm's protocol version already requires.
//!
//! Only what presentation and the interface consume is carried. Resource bookkeeping events
//! (`ManaSpent`, `ValorGained` …) are not: resources are corrected from state, not
//! replayed.

use engine_core::math::Vec3;
use engine_game::ability::AbilityId;
use engine_game::character::{DefenseOutcome, ItemProps, MailReason, QUALITY_DEFAULT};
use engine_game::combat::DamageType;
use engine_game::enemy::{EnemyAbility, TelegraphShape};
use engine_game::entity::EntityId;
use engine_game::event::{AggroChange, CombatEvent, ControlKind, DamageEvent};
use engine_game::firemage::{FireAbility, ZoneShape};
use engine_game::sim::CastEnd;
use engine_game::status::{StatusEnd, StatusKind};
use serde::{Deserialize, Serialize};

use crate::loot::ensure_items_registered;

fn default_quality() -> u8 {
    QUALITY_DEFAULT
}

/// An item's quality, enhancement and broken state as they travel: flat fields, each
/// defaulted, so an event from a build that predates quality still reads (as the legacy item).
fn props_of(quality: u8, enhancement: u8, broken: bool) -> ItemProps {
    ItemProps::new(quality, enhancement, broken)
}

/// The currency or gold resource id this build knows by `id`, as the `'static` string events
/// carry. `None` for a resource no loot rule drops.
fn known_resource(id: &str) -> Option<&'static str> {
    let rules = crate::loot::builtin_rules()?;
    [rules.currency.resource, rules.gold.resource]
        .into_iter()
        .find(|resource| *resource == id)
}

/// An entity, as both ends of the connection can name it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
pub enum EntityRef {
    /// Nobody: the environment, a fall.
    None,
    /// A player, by account id.
    Player(String),
    /// A monster, by its key in the instance.
    Monster(u64),
}

/// An ability, as both ends can name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
pub enum AbilityRef {
    /// No ability.
    None,
    /// A player ability, by its index in `FireAbility::ALL`.
    Player(u16),
    /// A monster ability, by its index in `EnemyAbility::ALL`.
    Monster(u16),
}

impl AbilityRef {
    /// The wire form of `ability`.
    #[must_use]
    pub fn of(ability: AbilityId) -> Self {
        if let Some(fire) = ability.fire() {
            return Self::Player(u16::try_from(fire.index()).unwrap_or(u16::MAX));
        }
        if let Some(enemy) = ability.enemy() {
            return Self::Monster(u16::try_from(enemy.index()).unwrap_or(u16::MAX));
        }
        Self::None
    }

    /// The gameplay id. Unknown indices become [`AbilityId::NONE`].
    #[must_use]
    pub fn id(self) -> AbilityId {
        match self {
            Self::None => AbilityId::NONE,
            Self::Player(index) => FireAbility::ALL
                .get(usize::from(index))
                .map_or(AbilityId::NONE, |a| AbilityId::from_fire(*a)),
            Self::Monster(index) => EnemyAbility::ALL
                .get(usize::from(index))
                .map_or(AbilityId::NONE, |a| AbilityId::from_enemy(*a)),
        }
    }
}

fn v3(v: Vec3) -> [f32; 3] {
    [v.x, v.y, v.z]
}

fn vec(v: [f32; 3]) -> Vec3 {
    Vec3::new(v[0], v[1], v[2])
}

fn index_of<T: PartialEq + Copy>(all: &[T], value: T) -> u16 {
    all.iter()
        .position(|candidate| *candidate == value)
        .and_then(|i| u16::try_from(i).ok())
        .unwrap_or(u16::MAX)
}

fn status_index(kind: StatusKind) -> u16 {
    index_of(&StatusKind::ALL, kind)
}

fn status_of(index: u16) -> Option<StatusKind> {
    StatusKind::ALL.get(usize::from(index)).copied()
}

const DAMAGE_TYPES: [DamageType; 3] = [DamageType::Fire, DamageType::Physical, DamageType::Nature];

fn damage_type_index(kind: DamageType) -> u8 {
    u8::try_from(DAMAGE_TYPES.iter().position(|d| *d == kind).unwrap_or(0)).unwrap_or(0)
}

/// A zone's shape on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WireZone {
    /// A circle.
    Circle(f32),
    /// A line: yaw, length, width.
    Line(f32, f32, f32),
}

/// A telegraph's shape on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WireTelegraph {
    /// One target.
    Single,
    /// A circle of radius.
    Circle(f32),
    /// A cone: yaw, range, half angle.
    Cone(f32, f32, f32),
    /// A line: yaw, length, width.
    Line(f32, f32, f32),
}

/// One combat event on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "e")]
#[allow(
    missing_docs,
    reason = "each field mirrors the CombatEvent field of the same name"
)]
pub enum WireEvent {
    AbilityUsed {
        source: EntityRef,
        ability: AbilityRef,
        targets: u32,
        facing: f32,
    },
    Damage {
        source: EntityRef,
        target: EntityRef,
        amount: f32,
        overkill: f32,
        ability: AbilityRef,
        damage_type: u8,
        killed: bool,
        critical: bool,
        periodic: bool,
        position: [f32; 3],
    },
    Healing {
        source: EntityRef,
        target: EntityRef,
        amount: f32,
        ability: AbilityRef,
    },
    StatusApplied {
        source: EntityRef,
        target: EntityRef,
        status: u16,
        duration: f32,
    },
    StatusRemoved {
        target: EntityRef,
        status: u16,
        reason: u8,
    },
    CastStarted {
        source: EntityRef,
        ability: AbilityRef,
        duration: f32,
    },
    CastEnded {
        source: EntityRef,
        ability: AbilityRef,
        end: u8,
    },
    Death {
        entity: EntityRef,
        killer: EntityRef,
    },
    ProjectileLaunched {
        source: EntityRef,
        target: EntityRef,
        from: [f32; 3],
        to: [f32; 3],
        ability: AbilityRef,
    },
    ImpactScheduled {
        source: EntityRef,
        ability: AbilityRef,
        position: [f32; 3],
        radius: f32,
        delay: f32,
    },
    ImpactLanded {
        source: EntityRef,
        ability: AbilityRef,
        position: [f32; 3],
        radius: f32,
        targets: u32,
    },
    AreaEffectStarted {
        id: u32,
        source: EntityRef,
        ability: AbilityRef,
        position: [f32; 3],
        shape: WireZone,
        duration: f32,
    },
    AreaEffectEnded {
        id: u32,
        position: [f32; 3],
    },
    TelegraphStarted {
        id: u32,
        source: EntityRef,
        ability: AbilityRef,
        anchor: [f32; 3],
        shape: WireTelegraph,
        delay: f32,
        follow: EntityRef,
    },
    TelegraphEnded {
        id: u32,
        anchor: [f32; 3],
        resolved: bool,
    },
    AttackDefended {
        attacker: EntityRef,
        defender: EntityRef,
        outcome: u8,
        ability: AbilityRef,
        position: [f32; 3],
    },
    SpellReflected {
        source: EntityRef,
        target: EntityRef,
        ability: AbilityRef,
        amount: f32,
    },
    ControlApplied {
        source: EntityRef,
        target: EntityRef,
        control: u8,
        seconds: f32,
        ability: AbilityRef,
    },
    Aggro {
        enemy: EntityRef,
        entity: EntityRef,
        change: u8,
    },
    Absorbed {
        target: EntityRef,
        amount: f32,
        remaining: f32,
    },
    DungeonProgress {
        credited: f32,
        fraction: f32,
    },
    BossUnlocked {
        position: [f32; 3],
    },
    DungeonCompleted {
        seconds: f32,
    },
    ChestSpawned {
        chest: u32,
        position: [f32; 3],
        drop_height: f32,
        fall_seconds: f32,
    },
    ChestOpened {
        chest: u32,
        by: EntityRef,
    },
    LootAwarded {
        player: EntityRef,
        /// The item's stable key (`engine_game::character::item_by_key`).
        item: String,
        /// Rolled quality, per cent of the base stats.
        #[serde(default = "default_quality")]
        quality: u8,
        #[serde(default)]
        enhancement: u8,
        #[serde(default)]
        broken: bool,
        /// Which of the player's chest items this is, in roll order.
        #[serde(default)]
        index: u8,
    },
    LootMailed {
        player: EntityRef,
        /// The item's stable key.
        item: String,
        #[serde(default = "default_quality")]
        quality: u8,
        #[serde(default)]
        enhancement: u8,
        #[serde(default)]
        broken: bool,
        #[serde(default)]
        index: u8,
        /// `0` the bag was full, `1` never opened.
        reason: u8,
    },
    MobLoot {
        player: EntityRef,
        /// The item's stable key.
        item: String,
        #[serde(default = "default_quality")]
        quality: u8,
        #[serde(default)]
        enhancement: u8,
        #[serde(default)]
        broken: bool,
    },
    MobCurrency {
        player: EntityRef,
        /// The resource's id in the economy catalog.
        resource: String,
        amount: u32,
    },
    ModifierBeamsStarted {
        id: u32,
        center: [f32; 3],
        start_angle: f32,
        angular_speed: f32,
        radius: f32,
        width: f32,
        beams: u8,
        /// Simulation seconds, the server's: only the gap between the two means anything to
        /// a client, which rebases both onto its own clock.
        warmup_end_time: f64,
        start_time: f64,
    },
    ModifierBeamsEnded {
        id: u32,
    },
    PlagueArmed {
        id: u32,
        position: [f32; 3],
        radius: f32,
        armed_time: f64,
        explode_time: f64,
    },
    PlagueExploded {
        id: u32,
        position: [f32; 3],
        radius: f32,
    },
    PlagueCloudEnded {
        id: u32,
    },
}

/// The wire form of `event`, naming entities with `name`. `None` for events not carried.
#[must_use]
#[allow(clippy::too_many_lines, reason = "one arm per carried event")]
pub fn encode(event: &CombatEvent, name: &dyn Fn(EntityId) -> EntityRef) -> Option<WireEvent> {
    ensure_items_registered();
    let a = AbilityRef::of;
    Some(match *event {
        CombatEvent::AbilityUsed {
            source,
            ability,
            targets,
            facing,
            ..
        } => WireEvent::AbilityUsed {
            source: name(source),
            ability: a(ability),
            targets,
            facing,
        },
        CombatEvent::Damage(d) => WireEvent::Damage {
            source: name(d.source),
            target: name(d.target),
            amount: d.amount,
            overkill: d.overkill,
            ability: a(d.ability),
            damage_type: damage_type_index(d.damage_type),
            killed: d.killed,
            critical: d.critical,
            periodic: d.periodic,
            position: v3(d.position),
        },
        CombatEvent::Healing {
            source,
            target,
            amount,
            ability,
            ..
        } => WireEvent::Healing {
            source: name(source),
            target: name(target),
            amount,
            ability: a(ability),
        },
        CombatEvent::StatusApplied {
            source,
            target,
            status,
            duration,
            ..
        } => WireEvent::StatusApplied {
            source: name(source),
            target: name(target),
            status: status_index(status),
            duration,
        },
        CombatEvent::StatusRemoved {
            target,
            status,
            reason,
            ..
        } => WireEvent::StatusRemoved {
            target: name(target),
            status: status_index(status),
            reason: match reason {
                StatusEnd::Expired => 0,
                StatusEnd::Consumed => 1,
                StatusEnd::Cleared => 2,
            },
        },
        CombatEvent::CastStarted {
            source,
            ability,
            duration,
            ..
        } => WireEvent::CastStarted {
            source: name(source),
            ability: a(ability),
            duration,
        },
        CombatEvent::CastEnded {
            source,
            ability,
            end,
            ..
        } => WireEvent::CastEnded {
            source: name(source),
            ability: a(ability),
            end: cast_end_index(end),
        },
        CombatEvent::Death { entity, killer, .. } => WireEvent::Death {
            entity: name(entity),
            killer: name(killer),
        },
        CombatEvent::ProjectileLaunched {
            source,
            target,
            from,
            to,
            ability,
            ..
        } => WireEvent::ProjectileLaunched {
            source: name(source),
            target: name(target),
            from: v3(from),
            to: v3(to),
            ability: a(ability),
        },
        CombatEvent::ImpactScheduled {
            source,
            ability,
            position,
            radius,
            delay,
            ..
        } => WireEvent::ImpactScheduled {
            source: name(source),
            ability: a(ability),
            position: v3(position),
            radius,
            delay,
        },
        CombatEvent::ImpactLanded {
            source,
            ability,
            position,
            radius,
            targets,
            ..
        } => WireEvent::ImpactLanded {
            source: name(source),
            ability: a(ability),
            position: v3(position),
            radius,
            targets,
        },
        CombatEvent::AreaEffectStarted {
            id,
            source,
            ability,
            position,
            shape,
            duration,
            ..
        } => WireEvent::AreaEffectStarted {
            id,
            source: name(source),
            ability: a(ability),
            position: v3(position),
            shape: match shape {
                ZoneShape::Circle { radius } => WireZone::Circle(radius),
                ZoneShape::Line { yaw, length, width } => WireZone::Line(yaw, length, width),
            },
            duration,
        },
        CombatEvent::AreaEffectEnded { id, position, .. } => WireEvent::AreaEffectEnded {
            id,
            position: v3(position),
        },
        CombatEvent::TelegraphStarted {
            id,
            source,
            ability,
            anchor,
            shape,
            delay,
            follow,
            ..
        } => WireEvent::TelegraphStarted {
            id,
            source: name(source),
            ability: a(ability),
            anchor: v3(anchor),
            shape: match shape {
                TelegraphShape::Single => WireTelegraph::Single,
                TelegraphShape::Circle { radius } => WireTelegraph::Circle(radius),
                TelegraphShape::Cone {
                    yaw,
                    range,
                    half_angle,
                } => WireTelegraph::Cone(yaw, range, half_angle),
                TelegraphShape::Line { yaw, length, width } => {
                    WireTelegraph::Line(yaw, length, width)
                }
            },
            delay,
            follow: follow.map_or(EntityRef::None, name),
        },
        CombatEvent::TelegraphEnded {
            id,
            anchor,
            resolved,
            ..
        } => WireEvent::TelegraphEnded {
            id,
            anchor: v3(anchor),
            resolved,
        },
        CombatEvent::AttackDefended {
            attacker,
            defender,
            outcome,
            ability,
            position,
            ..
        } => WireEvent::AttackDefended {
            attacker: name(attacker),
            defender: name(defender),
            outcome: match outcome {
                DefenseOutcome::Hit => 0,
                DefenseOutcome::Blocked => 1,
                DefenseOutcome::Parried => 2,
                DefenseOutcome::Dodged => 3,
            },
            ability: a(ability),
            position: v3(position),
        },
        CombatEvent::SpellReflected {
            source,
            target,
            ability,
            amount,
            ..
        } => WireEvent::SpellReflected {
            source: name(source),
            target: name(target),
            ability: a(ability),
            amount,
        },
        CombatEvent::ControlApplied {
            source,
            target,
            control,
            seconds,
            ability,
            ..
        } => WireEvent::ControlApplied {
            source: name(source),
            target: name(target),
            control: match control {
                ControlKind::Stun => 0,
                ControlKind::Silence => 1,
                ControlKind::Taunt => 2,
            },
            seconds,
            ability: a(ability),
        },
        CombatEvent::Aggro {
            enemy,
            entity,
            change,
            ..
        } => WireEvent::Aggro {
            enemy: name(enemy),
            entity: name(entity),
            change: match change {
                AggroChange::Gained => 0,
                AggroChange::Lost => 1,
                AggroChange::Gaining => 2,
                AggroChange::Losing => 3,
            },
        },
        CombatEvent::Absorbed {
            target,
            amount,
            remaining,
            ..
        } => WireEvent::Absorbed {
            target: name(target),
            amount,
            remaining,
        },
        CombatEvent::DungeonProgress {
            credited, fraction, ..
        } => WireEvent::DungeonProgress { credited, fraction },
        CombatEvent::BossUnlocked { position, .. } => WireEvent::BossUnlocked {
            position: v3(position),
        },
        CombatEvent::DungeonCompleted { seconds, .. } => WireEvent::DungeonCompleted { seconds },
        CombatEvent::ChestSpawned {
            chest,
            position,
            drop_height,
            fall_seconds,
            ..
        } => WireEvent::ChestSpawned {
            chest,
            position: v3(position),
            drop_height,
            fall_seconds,
        },
        CombatEvent::ChestOpened { chest, by, .. } => WireEvent::ChestOpened {
            chest,
            by: name(by),
        },
        CombatEvent::LootAwarded {
            player,
            item,
            props,
            index,
            ..
        } => WireEvent::LootAwarded {
            player: name(player),
            item: engine_game::character::item(item)?.key.to_owned(),
            quality: props.quality,
            enhancement: props.enhancement,
            broken: props.broken,
            index,
        },
        CombatEvent::MobLoot {
            player,
            item,
            props,
            ..
        } => WireEvent::MobLoot {
            player: name(player),
            item: engine_game::character::item(item)?.key.to_owned(),
            quality: props.quality,
            enhancement: props.enhancement,
            broken: props.broken,
        },
        CombatEvent::MobCurrency {
            player,
            resource,
            amount,
            ..
        } => WireEvent::MobCurrency {
            player: name(player),
            resource: resource.to_owned(),
            amount,
        },
        CombatEvent::LootMailed {
            player,
            item,
            props,
            index,
            reason,
            ..
        } => WireEvent::LootMailed {
            player: name(player),
            item: engine_game::character::item(item)?.key.to_owned(),
            quality: props.quality,
            enhancement: props.enhancement,
            broken: props.broken,
            index,
            reason: match reason {
                MailReason::BagFull => 0,
                MailReason::Unclaimed => 1,
                MailReason::SlotRemoved => 2,
            },
        },
        CombatEvent::ModifierBeamsStarted {
            id,
            center,
            start_angle,
            angular_speed,
            radius,
            width,
            beams,
            warmup_end_time,
            start_time,
            ..
        } => WireEvent::ModifierBeamsStarted {
            id,
            center: v3(center),
            start_angle,
            angular_speed,
            radius,
            width,
            beams,
            warmup_end_time,
            start_time,
        },
        CombatEvent::ModifierBeamsEnded { id, .. } => WireEvent::ModifierBeamsEnded { id },
        CombatEvent::PlagueArmed {
            id,
            position,
            radius,
            armed_time,
            explode_time,
            ..
        } => WireEvent::PlagueArmed {
            id,
            position: v3(position),
            radius,
            armed_time,
            explode_time,
        },
        CombatEvent::PlagueExploded {
            id,
            position,
            radius,
            ..
        } => WireEvent::PlagueExploded {
            id,
            position: v3(position),
            radius,
        },
        CombatEvent::PlagueCloudEnded { id, .. } => WireEvent::PlagueCloudEnded { id },
        _ => return None,
    })
}

fn cast_end_index(end: CastEnd) -> u8 {
    match end {
        CastEnd::Completed => 0,
        CastEnd::Moved => 1,
        CastEnd::Cancelled => 2,
        CastEnd::CasterDied => 3,
        #[allow(
            unreachable_patterns,
            reason = "future cast endings travel as cancelled"
        )]
        _ => 2,
    }
}

fn cast_end_of(index: u8) -> CastEnd {
    match index {
        0 => CastEnd::Completed,
        1 => CastEnd::Moved,
        3 => CastEnd::CasterDied,
        _ => CastEnd::Cancelled,
    }
}

/// The gameplay event `wire` describes, naming entities with `resolve` and stamping `tick`.
/// `None` if an entity it needs cannot be resolved (the client has not seen it yet).
#[must_use]
#[allow(clippy::too_many_lines, reason = "one arm per carried event")]
pub fn decode(
    wire: &WireEvent,
    resolve: &dyn Fn(&EntityRef) -> Option<EntityId>,
    tick: u64,
) -> Option<CombatEvent> {
    ensure_items_registered();
    let any = |r: &EntityRef| resolve(r).unwrap_or(EntityId::INVALID);
    let need = |r: &EntityRef| resolve(r);
    Some(match wire {
        WireEvent::AbilityUsed {
            source,
            ability,
            targets,
            facing,
        } => CombatEvent::AbilityUsed {
            source: need(source)?,
            ability: ability.id(),
            targets: *targets,
            facing: *facing,
            tick,
        },
        WireEvent::Damage {
            source,
            target,
            amount,
            overkill,
            ability,
            damage_type,
            killed,
            critical,
            periodic,
            position,
        } => CombatEvent::Damage(DamageEvent {
            source: any(source),
            target: need(target)?,
            amount: *amount,
            overkill: *overkill,
            ability: ability.id(),
            damage_type: DAMAGE_TYPES
                .get(usize::from(*damage_type))
                .copied()
                .unwrap_or(DamageType::Physical),
            killed: *killed,
            critical: *critical,
            periodic: *periodic,
            position: vec(*position),
            tick,
        }),
        WireEvent::Healing {
            source,
            target,
            amount,
            ability,
        } => CombatEvent::Healing {
            source: any(source),
            target: need(target)?,
            amount: *amount,
            ability: ability.id(),
            tick,
        },
        WireEvent::StatusApplied {
            source,
            target,
            status,
            duration,
        } => CombatEvent::StatusApplied {
            source: any(source),
            target: need(target)?,
            status: status_of(*status)?,
            duration: *duration,
            tick,
        },
        WireEvent::StatusRemoved {
            target,
            status,
            reason,
        } => CombatEvent::StatusRemoved {
            target: need(target)?,
            status: status_of(*status)?,
            reason: match reason {
                0 => StatusEnd::Expired,
                1 => StatusEnd::Consumed,
                _ => StatusEnd::Cleared,
            },
            tick,
        },
        WireEvent::CastStarted {
            source,
            ability,
            duration,
        } => CombatEvent::CastStarted {
            source: need(source)?,
            ability: ability.id(),
            duration: *duration,
            tick,
        },
        WireEvent::CastEnded {
            source,
            ability,
            end,
        } => CombatEvent::CastEnded {
            source: need(source)?,
            ability: ability.id(),
            end: cast_end_of(*end),
            tick,
        },
        WireEvent::Death { entity, killer } => CombatEvent::Death {
            entity: need(entity)?,
            killer: any(killer),
            tick,
        },
        WireEvent::ProjectileLaunched {
            source,
            target,
            from,
            to,
            ability,
        } => CombatEvent::ProjectileLaunched {
            source: any(source),
            target: any(target),
            from: vec(*from),
            to: vec(*to),
            ability: ability.id(),
            tick,
        },
        WireEvent::ImpactScheduled {
            source,
            ability,
            position,
            radius,
            delay,
        } => CombatEvent::ImpactScheduled {
            source: any(source),
            ability: ability.id(),
            position: vec(*position),
            radius: *radius,
            delay: *delay,
            tick,
        },
        WireEvent::ImpactLanded {
            source,
            ability,
            position,
            radius,
            targets,
        } => CombatEvent::ImpactLanded {
            source: any(source),
            ability: ability.id(),
            position: vec(*position),
            radius: *radius,
            targets: *targets,
            tick,
        },
        WireEvent::AreaEffectStarted {
            id,
            source,
            ability,
            position,
            shape,
            duration,
        } => CombatEvent::AreaEffectStarted {
            id: *id,
            source: any(source),
            ability: ability.id(),
            position: vec(*position),
            shape: match *shape {
                WireZone::Circle(radius) => ZoneShape::Circle { radius },
                WireZone::Line(yaw, length, width) => ZoneShape::Line { yaw, length, width },
            },
            duration: *duration,
            tick,
        },
        WireEvent::AreaEffectEnded { id, position } => CombatEvent::AreaEffectEnded {
            id: *id,
            position: vec(*position),
            tick,
        },
        WireEvent::TelegraphStarted {
            id,
            source,
            ability,
            anchor,
            shape,
            delay,
            follow,
        } => CombatEvent::TelegraphStarted {
            id: *id,
            source: any(source),
            ability: ability.id(),
            anchor: vec(*anchor),
            shape: match *shape {
                WireTelegraph::Single => TelegraphShape::Single,
                WireTelegraph::Circle(radius) => TelegraphShape::Circle { radius },
                WireTelegraph::Cone(yaw, range, half_angle) => TelegraphShape::Cone {
                    yaw,
                    range,
                    half_angle,
                },
                WireTelegraph::Line(yaw, length, width) => {
                    TelegraphShape::Line { yaw, length, width }
                }
            },
            delay: *delay,
            follow: resolve(follow),
            tick,
        },
        WireEvent::TelegraphEnded {
            id,
            anchor,
            resolved,
        } => CombatEvent::TelegraphEnded {
            id: *id,
            anchor: vec(*anchor),
            resolved: *resolved,
            tick,
        },
        WireEvent::AttackDefended {
            attacker,
            defender,
            outcome,
            ability,
            position,
        } => CombatEvent::AttackDefended {
            attacker: any(attacker),
            defender: need(defender)?,
            outcome: match outcome {
                1 => DefenseOutcome::Blocked,
                2 => DefenseOutcome::Parried,
                3 => DefenseOutcome::Dodged,
                _ => DefenseOutcome::Hit,
            },
            ability: ability.id(),
            position: vec(*position),
            tick,
        },
        WireEvent::SpellReflected {
            source,
            target,
            ability,
            amount,
        } => CombatEvent::SpellReflected {
            source: any(source),
            target: any(target),
            ability: ability.id(),
            amount: *amount,
            tick,
        },
        WireEvent::ControlApplied {
            source,
            target,
            control,
            seconds,
            ability,
        } => CombatEvent::ControlApplied {
            source: any(source),
            target: need(target)?,
            control: match control {
                1 => ControlKind::Silence,
                2 => ControlKind::Taunt,
                _ => ControlKind::Stun,
            },
            seconds: *seconds,
            ability: ability.id(),
            tick,
        },
        WireEvent::Aggro {
            enemy,
            entity,
            change,
        } => CombatEvent::Aggro {
            enemy: need(enemy)?,
            entity: need(entity)?,
            change: match change {
                1 => AggroChange::Lost,
                2 => AggroChange::Gaining,
                3 => AggroChange::Losing,
                _ => AggroChange::Gained,
            },
            tick,
        },
        WireEvent::Absorbed {
            target,
            amount,
            remaining,
        } => CombatEvent::Absorbed {
            target: need(target)?,
            amount: *amount,
            remaining: *remaining,
            tick,
        },
        WireEvent::DungeonProgress { credited, fraction } => CombatEvent::DungeonProgress {
            credited: *credited,
            fraction: *fraction,
            tick,
        },
        WireEvent::BossUnlocked { position } => CombatEvent::BossUnlocked {
            position: vec(*position),
            tick,
        },
        WireEvent::DungeonCompleted { seconds } => CombatEvent::DungeonCompleted {
            seconds: *seconds,
            tick,
        },
        WireEvent::ChestSpawned {
            chest,
            position,
            drop_height,
            fall_seconds,
        } => CombatEvent::ChestSpawned {
            chest: *chest,
            position: vec(*position),
            drop_height: *drop_height,
            fall_seconds: *fall_seconds,
            tick,
        },
        WireEvent::ChestOpened { chest, by } => CombatEvent::ChestOpened {
            chest: *chest,
            by: need(by)?,
            tick,
        },
        WireEvent::LootAwarded {
            player,
            item,
            quality,
            enhancement,
            broken,
            index,
        } => CombatEvent::LootAwarded {
            player: need(player)?,
            item: engine_game::character::item_by_key(item)?.id,
            props: props_of(*quality, *enhancement, *broken),
            index: *index,
            tick,
        },
        WireEvent::MobLoot {
            player,
            item,
            quality,
            enhancement,
            broken,
        } => CombatEvent::MobLoot {
            player: need(player)?,
            item: engine_game::character::item_by_key(item)?.id,
            props: props_of(*quality, *enhancement, *broken),
            tick,
        },
        WireEvent::MobCurrency {
            player,
            resource,
            amount,
        } => CombatEvent::MobCurrency {
            player: need(player)?,
            resource: known_resource(resource)?,
            amount: *amount,
            tick,
        },
        WireEvent::LootMailed {
            player,
            item,
            quality,
            enhancement,
            broken,
            index,
            reason,
        } => CombatEvent::LootMailed {
            player: need(player)?,
            item: engine_game::character::item_by_key(item)?.id,
            props: props_of(*quality, *enhancement, *broken),
            index: *index,
            reason: match *reason {
                0 => MailReason::BagFull,
                2 => MailReason::SlotRemoved,
                _ => MailReason::Unclaimed,
            },
            tick,
        },
        WireEvent::ModifierBeamsStarted {
            id,
            center,
            start_angle,
            angular_speed,
            radius,
            width,
            beams,
            warmup_end_time,
            start_time,
        } => CombatEvent::ModifierBeamsStarted {
            id: *id,
            center: vec(*center),
            start_angle: *start_angle,
            angular_speed: *angular_speed,
            radius: *radius,
            width: *width,
            beams: *beams,
            warmup_end_time: *warmup_end_time,
            start_time: *start_time,
            tick,
        },
        WireEvent::ModifierBeamsEnded { id } => CombatEvent::ModifierBeamsEnded { id: *id, tick },
        WireEvent::PlagueArmed {
            id,
            position,
            radius,
            armed_time,
            explode_time,
        } => CombatEvent::PlagueArmed {
            id: *id,
            position: vec(*position),
            radius: *radius,
            armed_time: *armed_time,
            explode_time: *explode_time,
            tick,
        },
        WireEvent::PlagueExploded {
            id,
            position,
            radius,
        } => CombatEvent::PlagueExploded {
            id: *id,
            position: vec(*position),
            radius: *radius,
            tick,
        },
        WireEvent::PlagueCloudEnded { id } => CombatEvent::PlagueCloudEnded { id: *id, tick },
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    #[test]
    fn abilities_round_trip() {
        for ability in FireAbility::ALL {
            let id = AbilityId::from_fire(ability);
            assert_eq!(AbilityRef::of(id).id(), id);
        }
        for ability in EnemyAbility::ALL {
            let id = AbilityId::from_enemy(ability);
            assert_eq!(AbilityRef::of(id).id(), id);
        }
        assert_eq!(AbilityRef::of(AbilityId::NONE), AbilityRef::None);
    }

    #[test]
    fn a_damage_event_round_trips_through_json_with_remapped_entities() {
        let server_a = EntityId::new(7, 1);
        let server_b = EntityId::new(9, 3);
        let client_a = EntityId::new(2, 1);
        let client_b = EntityId::new(5, 1);
        let event = CombatEvent::Damage(DamageEvent {
            source: server_a,
            target: server_b,
            amount: 120.0,
            overkill: 0.0,
            ability: AbilityId::from_fire(FireAbility::Firebolt),
            damage_type: DamageType::Fire,
            killed: false,
            critical: true,
            periodic: false,
            position: Vec3::new(1.0, 2.0, 3.0),
            tick: 99,
        });
        let name = |id: EntityId| {
            if id == server_a {
                EntityRef::Player("acct".into())
            } else {
                EntityRef::Monster(42)
            }
        };
        let wire = encode(&event, &name).expect("carried");
        let json = serde_json::to_string(&wire).expect("serializes");
        let back: WireEvent = serde_json::from_str(&json).expect("deserializes");
        let resolve = |r: &EntityRef| match r {
            EntityRef::Player(_) => Some(client_a),
            EntityRef::Monster(42) => Some(client_b),
            _ => None,
        };
        let decoded = decode(&back, &resolve, 5).expect("resolves");
        match decoded {
            CombatEvent::Damage(d) => {
                assert_eq!(d.source, client_a);
                assert_eq!(d.target, client_b);
                assert!(d.critical);
                assert_eq!(d.damage_type, DamageType::Fire);
                assert_eq!(d.ability, AbilityId::from_fire(FireAbility::Firebolt));
                assert_eq!(d.tick, 5);
            }
            other => panic!("decoded into {other:?}"),
        }
    }

    #[test]
    fn chest_events_round_trip_through_json() {
        let server = EntityId::new(3, 1);
        let client = EntityId::new(8, 2);
        let def = &engine_game::character::loot_pool()[0];
        let props = ItemProps::new(107, 4, false);
        let rules = crate::loot::builtin_rules().expect("the shipped rules build");
        let name = |_: EntityId| EntityRef::Player("acct".into());
        let resolve = |r: &EntityRef| matches!(r, EntityRef::Player(_)).then_some(client);
        let events = [
            CombatEvent::ChestSpawned {
                chest: 4,
                position: Vec3::new(1.0, 0.0, -2.0),
                drop_height: 20.0,
                fall_seconds: 2.5,
                tick: 1,
            },
            CombatEvent::ChestOpened {
                chest: 4,
                by: server,
                tick: 2,
            },
            CombatEvent::LootAwarded {
                player: server,
                item: def.id,
                props,
                index: 1,
                tick: 3,
            },
            CombatEvent::LootMailed {
                player: server,
                item: def.id,
                props,
                index: 0,
                reason: MailReason::Unclaimed,
                tick: 4,
            },
            CombatEvent::MobLoot {
                player: server,
                item: def.id,
                props,
                tick: 5,
            },
            CombatEvent::MobCurrency {
                player: server,
                resource: rules.currency.resource,
                amount: 2,
                tick: 6,
            },
        ];
        for event in events {
            let wire = encode(&event, &name).expect("carried");
            let json = serde_json::to_string(&wire).expect("serializes");
            let back: WireEvent = serde_json::from_str(&json).expect("deserializes");
            assert_eq!(back, wire);
            let decoded = decode(&back, &resolve, 9).expect("resolves");
            match (event, decoded) {
                (
                    CombatEvent::ChestSpawned { chest: a, .. },
                    CombatEvent::ChestSpawned {
                        chest: b,
                        fall_seconds,
                        tick,
                        ..
                    },
                ) => {
                    assert_eq!(a, b);
                    assert!((fall_seconds - 2.5).abs() < f32::EPSILON);
                    assert_eq!(tick, 9);
                }
                (CombatEvent::ChestOpened { .. }, CombatEvent::ChestOpened { by, .. }) => {
                    assert_eq!(by, client);
                }
                (
                    CombatEvent::LootAwarded { .. },
                    CombatEvent::LootAwarded {
                        player,
                        item,
                        props: got,
                        index,
                        ..
                    },
                ) => {
                    assert_eq!(player, client);
                    assert_eq!(item, def.id);
                    assert_eq!((got, index), (props, 1));
                }
                (
                    CombatEvent::LootMailed { .. },
                    CombatEvent::LootMailed {
                        player,
                        item,
                        props: got,
                        index,
                        reason,
                        ..
                    },
                ) => {
                    assert_eq!(player, client);
                    assert_eq!(item, def.id);
                    assert_eq!((got, index), (props, 0));
                    assert_eq!(reason, MailReason::Unclaimed);
                }
                (
                    CombatEvent::MobLoot { .. },
                    CombatEvent::MobLoot {
                        player,
                        item,
                        props: got,
                        ..
                    },
                ) => {
                    assert_eq!((player, item, got), (client, def.id, props));
                }
                (
                    CombatEvent::MobCurrency { .. },
                    CombatEvent::MobCurrency {
                        player,
                        resource,
                        amount,
                        ..
                    },
                ) => {
                    assert_eq!(player, client);
                    assert_eq!((resource, amount), ("aether_shard", 2));
                }
                (sent, got) => panic!("{sent:?} came back as {got:?}"),
            }
        }
        let unknown = WireEvent::LootAwarded {
            player: EntityRef::Player("acct".into()),
            item: "no_such_item".into(),
            quality: 100,
            enhancement: 0,
            broken: false,
            index: 0,
        };
        assert!(decode(&unknown, &resolve, 0).is_none());
        // A resource no loot rule drops does not decode into a `'static` id.
        let strange = WireEvent::MobCurrency {
            player: EntityRef::Player("acct".into()),
            resource: "moon_cheese".into(),
            amount: 1,
        };
        assert!(decode(&strange, &resolve, 0).is_none());
    }

    #[test]
    fn loot_events_from_before_quality_existed_still_read_as_the_legacy_item() {
        let old = r#"{"e":"LootAwarded","player":{"t":"Player","v":"acct"},"item":"head_haste_critical_strike"}"#;
        let wire: WireEvent = serde_json::from_str(old).expect("an old event reads");
        let client = EntityId::new(8, 2);
        let resolve = |_: &EntityRef| Some(client);
        match decode(&wire, &resolve, 1) {
            Some(CombatEvent::LootAwarded { props, index, .. }) => {
                assert_eq!(props, ItemProps::STANDARD);
                assert_eq!(index, 0);
            }
            other => panic!("decoded into {other:?}"),
        }
    }

    #[test]
    fn modifier_events_round_trip_through_json_and_keep_their_clocks() {
        let name = |_: EntityId| EntityRef::None;
        let resolve = |_: &EntityRef| None;
        let events = [
            CombatEvent::ModifierBeamsStarted {
                id: 3 << 30,
                center: Vec3::new(1.0, 0.0, -2.0),
                start_angle: 1.25,
                angular_speed: -0.61,
                radius: 9.0,
                width: 0.9,
                beams: 3,
                warmup_end_time: 123.456_789,
                start_time: 121.456_789,
                tick: 1,
            },
            CombatEvent::ModifierBeamsEnded {
                id: 3 << 30,
                tick: 2,
            },
            CombatEvent::PlagueArmed {
                id: (3 << 30) + 1,
                position: Vec3::new(4.0, 0.0, 5.0),
                radius: 4.0,
                armed_time: 10.0,
                explode_time: 15.0,
                tick: 3,
            },
            CombatEvent::PlagueExploded {
                id: (3 << 30) + 1,
                position: Vec3::new(4.0, 0.0, 5.0),
                radius: 4.0,
                tick: 4,
            },
            CombatEvent::PlagueCloudEnded {
                id: (3 << 30) + 1,
                tick: 5,
            },
        ];
        for event in events {
            let wire = encode(&event, &name).expect("carried");
            let json = serde_json::to_string(&wire).expect("serializes");
            let back: WireEvent = serde_json::from_str(&json).expect("deserializes");
            assert_eq!(back, wire);
            let decoded = decode(&back, &resolve, event.tick()).expect("needs no entity");
            assert_eq!(decoded, event, "the event survived the wire unchanged");
        }
    }

    #[test]
    fn unresolvable_targets_are_skipped() {
        let wire = WireEvent::Death {
            entity: EntityRef::Monster(1),
            killer: EntityRef::None,
        };
        assert!(decode(&wire, &|_| None, 0).is_none());
    }
}
