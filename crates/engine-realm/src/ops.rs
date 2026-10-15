//! The operator's read-only view of the live realm.
//!
//! The module (`server/realm-module/src/admin.rs`, `svc_admin_snapshot`) copies the rows it
//! needs into a [`SnapshotInput`] and calls [`build_snapshot`]; the persistence API parses the
//! JSON it gets back into the same [`AdminSnapshot`] type. The builder is a pure function so
//! the shape is tested natively, and one definition serves both ends of the wire.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Micros;
use crate::loadout::{EquippedItem, Loadout};

/// Microseconds per fight tick (60 Hz); the module's combat clock.
pub const TICK_US: Micros = 16_667;

/// Searching players of one dungeon, per role.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueCount {
    /// The dungeon.
    pub dungeon_id: String,
    /// Tanks searching.
    pub tanks: u32,
    /// Healers searching.
    pub healers: u32,
    /// DPS searching.
    pub dps: u32,
    /// Players already matched into a party that has not started.
    pub matched: u32,
}

/// One instance's row, as the module stores it.
#[derive(Debug, Clone, Default)]
pub struct InstanceRow {
    /// Instance id.
    pub id: u64,
    /// Its party.
    pub party_id: u64,
    /// The dungeon's id.
    pub dungeon_id: String,
    /// The key level it runs at.
    pub key_level: u32,
    /// The state's key (`active`).
    pub state: String,
    /// When it was created.
    pub created_at_us: Micros,
    /// When the countdown ends, which is when the run starts.
    pub run_starts_at_us: Option<Micros>,
    /// When it completed or failed.
    pub finished_at_us: Option<Micros>,
}

/// A player's runtime row.
#[derive(Debug, Clone, Default)]
pub struct PlayerRow {
    /// Account id.
    pub account_id: String,
    /// Character name.
    pub name: String,
    /// Catalog definition (the class).
    pub definition_id: String,
    /// Role key (`tank`).
    pub role: String,
    /// Connected.
    pub present: bool,
    /// Feet X.
    pub x: f32,
    /// Feet Y.
    pub y: f32,
    /// Feet Z.
    pub z: f32,
    /// Health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// The combat row's `alive`, when there is one.
    pub alive: Option<bool>,
}

/// What a party member's row knows about the character that the runtime row does not.
#[derive(Debug, Clone, Default)]
pub struct MemberRow {
    /// Account id.
    pub account_id: String,
    /// Persistent character id.
    pub character_id: String,
    /// Level.
    pub level: u32,
    /// Gear score.
    pub gear_score: u32,
    /// The loadout, as stored JSON.
    pub loadout: String,
}

/// A monster's row.
#[derive(Debug, Clone, Default)]
pub struct MonsterRow {
    /// Its key in the fight.
    pub key: u64,
    /// Archetype id.
    pub kind: String,
    /// Display name.
    pub name: String,
    /// Feet X.
    pub x: f32,
    /// Feet Z.
    pub z: f32,
    /// Alive.
    pub alive: bool,
    /// Whether it is a boss.
    pub is_boss: bool,
}

/// An instance's fight clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct CombatRow {
    /// Ticks run.
    pub tick: u64,
    /// When tick zero was.
    pub origin_us: Micros,
}

/// One instance with everything that belongs to it.
#[derive(Debug, Clone, Default)]
pub struct InstanceInput {
    /// The instance.
    pub row: InstanceRow,
    /// Key modifiers as display text, e.g. `Plague Explosion +17%`.
    pub modifiers: Vec<String>,
    /// Whether its fight is ticking.
    pub live: bool,
    /// Its players.
    pub players: Vec<PlayerRow>,
    /// Its party's members.
    pub members: Vec<MemberRow>,
    /// Its monsters.
    pub monsters: Vec<MonsterRow>,
    /// Its fight clock, if it has one.
    pub combat: Option<CombatRow>,
}

/// Everything [`build_snapshot`] reads.
#[derive(Debug, Clone, Default)]
pub struct SnapshotInput {
    /// Server time now.
    pub now_us: Micros,
    /// Accounts with an open connection.
    pub online_accounts: u32,
    /// Queue counts.
    pub queue: Vec<QueueCount>,
    /// The state key of every party.
    pub party_states: Vec<String>,
    /// Every instance.
    pub instances: Vec<InstanceInput>,
}

/// One player of an instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerView {
    /// Account id.
    pub account_id: String,
    /// Persistent character id (empty when the party row is gone).
    pub character_id: String,
    /// Character name.
    pub name: String,
    /// Class: the catalog definition.
    pub class: String,
    /// `tank`, `healer` or `dps`.
    pub role: String,
    /// `present` or `disconnected`.
    pub presence: String,
    /// Level.
    pub level: u32,
    /// Gear score.
    pub gear_score: u32,
    /// Health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Feet X.
    pub x: f32,
    /// Feet Y.
    pub y: f32,
    /// Feet Z.
    pub z: f32,
    /// Alive.
    pub alive: bool,
    /// The worn items, by stable key. Talents live in the database, not here.
    pub gear: Vec<EquippedItem>,
}

/// One monster of an instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonsterView {
    /// Its key.
    pub key: u64,
    /// Archetype id.
    pub kind: String,
    /// Display name.
    pub name: String,
    /// Feet X.
    pub x: f32,
    /// Feet Z.
    pub z: f32,
    /// Alive.
    pub alive: bool,
    /// Boss.
    pub is_boss: bool,
}

/// Kill progress.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillProgress {
    /// Dead monsters.
    pub killed: u32,
    /// All monsters.
    pub total: u32,
    /// Dead bosses.
    pub bosses_killed: u32,
    /// All bosses.
    pub bosses_total: u32,
}

/// How the fight clock is keeping up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickHealth {
    /// Ticks run.
    pub tick: u64,
    /// When the last tick ran, server microseconds.
    pub last_tick_at_us: Micros,
    /// Ticks that should have run by now.
    pub expected_tick: u64,
    /// `expected_tick - tick`, never negative. Meaningful while `ticking`.
    pub ticks_behind: u64,
    /// Whether the combat tick is advancing this instance.
    pub ticking: bool,
}

/// One instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceView {
    /// Instance id.
    pub id: u64,
    /// Dungeon id.
    pub dungeon_id: String,
    /// Key level.
    pub key_level: u32,
    /// Key modifiers as display text, e.g. `Plague Explosion +17%`.
    pub modifiers: Vec<String>,
    /// State key.
    pub state: String,
    /// When the run started (barrier lifted), server microseconds; absent before.
    pub started_at_us: Option<Micros>,
    /// When it was created.
    pub created_at_us: Micros,
    /// Seconds since the run started (frozen at the finish).
    pub elapsed_seconds: f64,
    /// Kill progress.
    pub kills: KillProgress,
    /// Fight clock.
    pub tick: Option<TickHealth>,
    /// The party.
    pub players: Vec<PlayerView>,
    /// Every monster.
    pub monsters: Vec<MonsterView>,
}

/// The whole snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AdminSnapshot {
    /// Server time of the snapshot, microseconds since the epoch.
    pub generated_at_us: Micros,
    /// Accounts with an open connection.
    pub online_accounts: u32,
    /// Queue counts per dungeon.
    pub queue: Vec<QueueCount>,
    /// Parties per state key.
    pub parties: BTreeMap<String, u32>,
    /// Instances.
    pub instances: Vec<InstanceView>,
    /// The largest `ticks_behind` among ticking instances.
    pub max_ticks_behind: u64,
}

fn elapsed_seconds(row: &InstanceRow, now: Micros) -> f64 {
    let Some(start) = row.run_starts_at_us else {
        return 0.0;
    };
    let end = row.finished_at_us.unwrap_or(now);
    (end - start).max(0) as f64 / 1_000_000.0
}

fn tick_health(input: &InstanceInput, now: Micros) -> Option<TickHealth> {
    let combat = input.combat?;
    let tick_us = u64::try_from(TICK_US).unwrap_or(1);
    let expected = u64::try_from((now - combat.origin_us).max(0)).unwrap_or(0) / tick_us;
    Some(TickHealth {
        tick: combat.tick,
        last_tick_at_us: combat.origin_us + i64::try_from(combat.tick).unwrap_or(0) * TICK_US,
        expected_tick: expected,
        ticks_behind: expected.saturating_sub(combat.tick),
        ticking: input.live,
    })
}

fn player_view(row: &PlayerRow, members: &[MemberRow]) -> PlayerView {
    let member = members.iter().find(|m| m.account_id == row.account_id);
    let gear = member
        .and_then(|m| serde_json::from_str::<Loadout>(&m.loadout).ok())
        .map(|loadout| loadout.equipment)
        .unwrap_or_default();
    PlayerView {
        account_id: row.account_id.clone(),
        character_id: member.map(|m| m.character_id.clone()).unwrap_or_default(),
        name: row.name.clone(),
        class: row.definition_id.clone(),
        role: row.role.clone(),
        presence: if row.present {
            "present"
        } else {
            "disconnected"
        }
        .to_owned(),
        level: member.map_or(0, |m| m.level),
        gear_score: member.map_or(0, |m| m.gear_score),
        health: row.health,
        max_health: row.max_health,
        x: row.x,
        y: row.y,
        z: row.z,
        alive: row.alive.unwrap_or(row.health > 0.0),
        gear,
    }
}

fn instance_view(input: &InstanceInput, now: Micros) -> InstanceView {
    let mut kills = KillProgress::default();
    for monster in &input.monsters {
        kills.total += 1;
        kills.bosses_total += u32::from(monster.is_boss);
        if !monster.alive {
            kills.killed += 1;
            kills.bosses_killed += u32::from(monster.is_boss);
        }
    }
    InstanceView {
        id: input.row.id,
        dungeon_id: input.row.dungeon_id.clone(),
        key_level: input.row.key_level,
        modifiers: input.modifiers.clone(),
        state: input.row.state.clone(),
        started_at_us: input.row.run_starts_at_us,
        created_at_us: input.row.created_at_us,
        elapsed_seconds: elapsed_seconds(&input.row, now),
        kills,
        tick: tick_health(input, now),
        players: input
            .players
            .iter()
            .map(|p| player_view(p, &input.members))
            .collect(),
        monsters: input
            .monsters
            .iter()
            .map(|m| MonsterView {
                key: m.key,
                kind: m.kind.clone(),
                name: m.name.clone(),
                x: m.x,
                z: m.z,
                alive: m.alive,
                is_boss: m.is_boss,
            })
            .collect(),
    }
}

/// Builds the snapshot. Pure: the same rows always give the same document.
#[must_use]
pub fn build_snapshot(input: &SnapshotInput) -> AdminSnapshot {
    let mut parties = BTreeMap::new();
    for state in &input.party_states {
        *parties.entry(state.clone()).or_insert(0) += 1;
    }
    let mut instances: Vec<InstanceView> = input
        .instances
        .iter()
        .map(|i| instance_view(i, input.now_us))
        .collect();
    instances.sort_by_key(|i| i.id);
    let max_ticks_behind = instances
        .iter()
        .filter_map(|i| i.tick.as_ref())
        .filter(|t| t.ticking)
        .map(|t| t.ticks_behind)
        .max()
        .unwrap_or(0);
    AdminSnapshot {
        generated_at_us: input.now_us,
        online_accounts: input.online_accounts,
        queue: input.queue.clone(),
        parties,
        instances,
        max_ticks_behind,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    fn input() -> SnapshotInput {
        let loadout = r#"{"equipment":[{"slot":"head","item":"head_a"}],"inventory":[]}"#;
        SnapshotInput {
            now_us: 100_000_000,
            online_accounts: 3,
            queue: vec![QueueCount {
                dungeon_id: "graveyard_crypt".into(),
                tanks: 1,
                ..QueueCount::default()
            }],
            party_states: vec!["in_dungeon".into(), "ready".into(), "in_dungeon".into()],
            instances: vec![InstanceInput {
                row: InstanceRow {
                    id: 7,
                    dungeon_id: "graveyard_crypt".into(),
                    key_level: 4,
                    state: "active".into(),
                    run_starts_at_us: Some(40_000_000),
                    ..InstanceRow::default()
                },
                live: true,
                players: vec![
                    PlayerRow {
                        account_id: "a".into(),
                        name: "Ash".into(),
                        present: true,
                        health: 10.0,
                        max_health: 20.0,
                        ..PlayerRow::default()
                    },
                    PlayerRow {
                        account_id: "b".into(),
                        present: false,
                        health: 0.0,
                        ..PlayerRow::default()
                    },
                ],
                members: vec![MemberRow {
                    account_id: "a".into(),
                    character_id: "c1".into(),
                    gear_score: 55,
                    loadout: loadout.into(),
                    ..MemberRow::default()
                }],
                monsters: vec![
                    MonsterRow {
                        alive: false,
                        ..MonsterRow::default()
                    },
                    MonsterRow {
                        alive: true,
                        ..MonsterRow::default()
                    },
                    MonsterRow {
                        alive: false,
                        is_boss: true,
                        ..MonsterRow::default()
                    },
                ],
                combat: Some(CombatRow {
                    tick: 3_000,
                    origin_us: 40_000_000,
                }),
                ..InstanceInput::default()
            }],
        }
    }

    #[test]
    fn counts_parties_by_state() {
        let snapshot = build_snapshot(&input());
        assert_eq!(snapshot.parties["in_dungeon"], 2);
        assert_eq!(snapshot.parties["ready"], 1);
        assert_eq!(snapshot.online_accounts, 3);
    }

    #[test]
    fn an_instance_reports_progress_elapsed_and_modifiers() {
        let snapshot = build_snapshot(&input());
        let instance = &snapshot.instances[0];
        assert_eq!(instance.kills.killed, 2);
        assert_eq!(instance.kills.total, 3);
        assert_eq!(instance.kills.bosses_killed, 1);
        assert!((instance.elapsed_seconds - 60.0).abs() < 1e-9);
        assert!(instance.modifiers.is_empty());
    }

    #[test]
    fn players_carry_gear_presence_and_aliveness() {
        let snapshot = build_snapshot(&input());
        let players = &snapshot.instances[0].players;
        assert_eq!(players[0].gear[0].item, "head_a");
        assert_eq!(players[0].gear_score, 55);
        assert_eq!(players[0].presence, "present");
        assert!(players[0].alive);
        assert_eq!(players[1].presence, "disconnected");
        assert!(!players[1].alive);
        assert!(players[1].gear.is_empty());
    }

    #[test]
    fn tick_health_counts_ticks_behind() {
        let snapshot = build_snapshot(&input());
        let tick = snapshot.instances[0].tick.as_ref().unwrap();
        // 60 s after the origin is 3599 ticks due; 3000 have run.
        assert_eq!(tick.expected_tick, 3_599);
        assert_eq!(tick.ticks_behind, 599);
        assert_eq!(snapshot.max_ticks_behind, 599);
    }

    #[test]
    fn the_snapshot_round_trips_as_json() {
        let snapshot = build_snapshot(&input());
        let json = serde_json::to_string(&snapshot).unwrap();
        let back: AdminSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snapshot);
    }
}
