//! The realm as the game sees it: plain data, no SpacetimeDB types.
//!
//! ```text
//! SDK callbacks (network thread) ─► RealmUpdate ─► channel ─► RealmMirror (game thread)
//!                                                                │
//!                                       front-end screens, world session, HUD read it
//! ```
//!
//! The mirror is the client's copy of its own slice of the realm, updated only by what the
//! server sent. Nothing on the client writes to it otherwise: a player's HP, their party's
//! state, a remote player's position are what the server last said they were.

use std::collections::BTreeMap;
use std::time::Instant;

use engine_core::math::Vec3;
use engine_realm::Micros;
use engine_realm::combat::{CastState, CooldownState, EntityRef, Resources, StatusState, WireEvent};
use engine_realm::Role;
use engine_realm::instance::InstanceState;
use engine_realm::party::{PartyState, ReadyResponse};

/// This account, as the realm knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountInfo {
    /// The account id.
    pub account_id: String,
    /// What other players see.
    pub display_name: String,
}

/// The local player's queue ticket.
#[derive(Debug, Clone, PartialEq)]
pub struct TicketInfo {
    /// Which dungeon.
    pub dungeon_id: String,
    /// As which role.
    pub role: Role,
    /// The character queued.
    pub character_name: String,
    /// Its gear score, as the server recorded it.
    pub gear_score: u32,
    /// When it was queued, server time.
    pub enqueued_at: Micros,
    /// Whether a party was found and a ready check is open.
    pub matched: bool,
}

/// The local player's party.
#[derive(Debug, Clone, PartialEq)]
pub struct PartyInfo {
    /// Party id.
    pub id: u64,
    /// The dungeon it formed for.
    pub dungeon_id: String,
    /// Where it is in its life.
    pub state: PartyState,
    /// The leader's account id, once chosen.
    pub leader: Option<String>,
    /// When the ready check expires, server time.
    pub ready_deadline: Micros,
    /// Its instance, once started.
    pub instance_id: Option<u64>,
}

/// One party member.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberInfo {
    /// Their account id.
    pub account_id: String,
    /// Their display name.
    pub display_name: String,
    /// The character they brought.
    pub character_name: String,
    /// Its catalog definition.
    pub definition_id: String,
    /// Its role.
    pub role: Role,
    /// Its level.
    pub level: u32,
    /// Its gear score.
    pub gear_score: u32,
    /// Their ready-check answer.
    pub ready: ReadyResponse,
    /// Whether they are connected.
    pub present: bool,
    /// Party order.
    pub slot: u8,
}

/// The local player's dungeon instance.
#[derive(Debug, Clone, PartialEq)]
pub struct InstanceInfo {
    /// Instance id.
    pub id: u64,
    /// Which dungeon.
    pub dungeon_id: String,
    /// Where it is in its life.
    pub state: InstanceState,
    /// When the countdown ends, server time.
    pub countdown_ends_at: Option<Micros>,
    /// Whether the starting barrier is open.
    pub barrier_open: bool,
}

/// One player in the instance, as the server last described them.
#[derive(Debug, Clone, PartialEq)]
pub struct RemotePlayerState {
    /// Their account id.
    pub account_id: String,
    /// The character's name.
    pub name: String,
    /// Its catalog definition (decides the model).
    pub definition_id: String,
    /// Its role.
    pub role: Role,
    /// Feet position.
    pub position: Vec3,
    /// Velocity.
    pub velocity: Vec3,
    /// Surface normal under the feet.
    pub ground_normal: Vec3,
    /// Facing, radians.
    pub yaw: f32,
    /// On the ground.
    pub grounded: bool,
    /// The last of this player's input frames the server applied.
    pub last_seq: u32,
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Connected or not.
    pub present: bool,
}

/// One monster in the instance.
#[derive(Debug, Clone, PartialEq)]
pub struct MonsterInfo {
    /// Its key in the fight: how events and actions name it.
    pub key: u64,
    /// Row id.
    pub id: u64,
    /// The archetype key.
    pub archetype: String,
    /// Display name.
    pub name: String,
    /// Feet position.
    pub position: Vec3,
    /// Facing.
    pub yaw: f32,
    /// Size multiplier.
    pub scale: f32,
    /// Level.
    pub level: u32,
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Alive.
    pub alive: bool,
    /// Whom it is fighting.
    pub target: EntityRef,
    /// What it is casting, as of `received`.
    pub cast: Option<CastState>,
    /// Statuses on it, as of `received`.
    pub statuses: Vec<StatusState>,
    /// When this row arrived, so timers can run down until the next.
    pub received: Instant,
}

/// One player's combat state beyond health and position.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerCombatInfo {
    /// Their account.
    pub account_id: String,
    /// Alive.
    pub alive: bool,
    /// Their selection.
    pub target: EntityRef,
    /// Their cast, as of `received`.
    pub cast: Option<CastState>,
    /// Their resources.
    pub resources: Resources,
    /// Their class's cooldowns, as of `received`.
    pub cooldowns: Vec<CooldownState>,
    /// Statuses on them, as of `received`.
    pub statuses: Vec<StatusState>,
    /// When this row arrived.
    pub received: Instant,
}

impl PlayerCombatInfo {
    /// Seconds since the row arrived.
    #[must_use]
    pub fn age(&self) -> f32 {
        self.received.elapsed().as_secs_f32()
    }

    /// `ability`'s cooldown now: charges, and seconds to the next.
    #[must_use]
    pub fn cooldown(&self, ability: u16) -> Option<(u8, f32)> {
        let state = self.cooldowns.iter().find(|c| c.ability == ability)?;
        Some((state.charges, (state.recharge - self.age()).max(0.0)))
    }
}

/// How a notice should be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    /// Neutral.
    Info,
    /// Notable.
    Warning,
    /// Something went wrong.
    Error,
}

/// A message from the server for this player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Notice id.
    pub id: u64,
    /// Stable code.
    pub code: String,
    /// How to show it.
    pub level: NoticeLevel,
    /// What to show.
    pub message: String,
}

/// How many are searching for one dungeon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Population {
    /// Tanks.
    pub tanks: u32,
    /// Healers.
    pub healers: u32,
    /// DPS.
    pub dps: u32,
}

/// One change to the mirror, as the network thread reports it.
#[derive(Debug, Clone, PartialEq)]
pub enum RealmUpdate {
    /// The subscription (re)applied: everything is replaced.
    Reset(Box<RealmMirror>),
    /// The account row.
    Account(Option<AccountInfo>),
    /// The queue ticket.
    Ticket(Option<TicketInfo>),
    /// The party row.
    Party(Option<PartyInfo>),
    /// The whole member list.
    Members(Vec<MemberInfo>),
    /// The instance row.
    Instance(Option<InstanceInfo>),
    /// One player inserted or changed.
    PlayerUpsert(RemotePlayerState),
    /// One player gone.
    PlayerRemoved(String),
    /// One monster inserted or changed.
    MonsterUpsert(MonsterInfo),
    /// One monster gone, by key.
    MonsterRemoved(u64),
    /// One player's combat row inserted or changed.
    CombatUpsert(PlayerCombatInfo),
    /// One player's combat row gone.
    CombatRemoved(String),
    /// What happened in one combat pass, in order.
    CombatEvents(Vec<WireEvent>),
    /// One notice arrived.
    NoticeAdded(Notice),
    /// One notice expired or was dismissed.
    NoticeRemoved(u64),
    /// Queue counts per dungeon.
    Population(Vec<(String, Population)>),
}

/// The client's copy of its slice of the realm.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RealmMirror {
    /// This account.
    pub account: Option<AccountInfo>,
    /// The queue ticket.
    pub ticket: Option<TicketInfo>,
    /// The party.
    pub party: Option<PartyInfo>,
    /// The party's members, in slot order.
    pub members: Vec<MemberInfo>,
    /// The instance.
    pub instance: Option<InstanceInfo>,
    /// The instance's players, by account id.
    pub players: BTreeMap<String, RemotePlayerState>,
    /// The instance's monsters, by key.
    pub monsters: BTreeMap<u64, MonsterInfo>,
    /// Every player's combat state, by account id.
    pub combat: BTreeMap<String, PlayerCombatInfo>,
    /// Combat events not yet taken by the game, oldest first.
    pub events: Vec<WireEvent>,
    /// Unread notices, oldest first.
    pub notices: Vec<Notice>,
    /// Queue counts per dungeon id.
    pub population: BTreeMap<String, Population>,
    /// Counts every applied update; lets a reader notice change without diffing.
    pub revision: u64,
}

impl RealmMirror {
    /// Applies one update.
    pub fn apply(&mut self, update: RealmUpdate) {
        self.revision = self.revision.wrapping_add(1);
        match update {
            RealmUpdate::Reset(mirror) => {
                let revision = self.revision;
                *self = *mirror;
                self.revision = revision;
            }
            RealmUpdate::Account(account) => self.account = account,
            RealmUpdate::Ticket(ticket) => self.ticket = ticket,
            RealmUpdate::Party(party) => self.party = party,
            RealmUpdate::Members(mut members) => {
                members.sort_by_key(|member| member.slot);
                self.members = members;
            }
            RealmUpdate::Instance(instance) => {
                if instance.is_none() {
                    self.players.clear();
                    self.monsters.clear();
                    self.combat.clear();
                    self.events.clear();
                }
                self.instance = instance;
            }
            RealmUpdate::PlayerUpsert(player) => {
                self.players.insert(player.account_id.clone(), player);
            }
            RealmUpdate::PlayerRemoved(account_id) => {
                self.players.remove(&account_id);
            }
            RealmUpdate::MonsterUpsert(monster) => {
                self.monsters.insert(monster.key, monster);
            }
            RealmUpdate::MonsterRemoved(key) => {
                self.monsters.remove(&key);
            }
            RealmUpdate::CombatUpsert(combat) => {
                self.combat.insert(combat.account_id.clone(), combat);
            }
            RealmUpdate::CombatRemoved(account_id) => {
                self.combat.remove(&account_id);
            }
            RealmUpdate::CombatEvents(events) => {
                // Bounded: a game that stopped reading (a minimised client) must not grow
                // this forever. What is dropped is old effects, never state.
                self.events.extend(events);
                let excess = self.events.len().saturating_sub(4096);
                self.events.drain(..excess);
            }
            RealmUpdate::NoticeAdded(notice) => {
                if !self.notices.iter().any(|existing| existing.id == notice.id) {
                    self.notices.push(notice);
                    self.notices.sort_by_key(|notice| notice.id);
                }
            }
            RealmUpdate::NoticeRemoved(id) => self.notices.retain(|notice| notice.id != id),
            RealmUpdate::Population(rows) => {
                self.population = rows.into_iter().collect();
            }
        }
    }

    /// The local account id, once known.
    #[must_use]
    pub fn my_account_id(&self) -> Option<&str> {
        self.account.as_ref().map(|account| account.account_id.as_str())
    }

    /// The local player's runtime state in the instance.
    #[must_use]
    pub fn me(&self) -> Option<&RemotePlayerState> {
        self.players.get(self.my_account_id()?)
    }

    /// The local player's party membership.
    #[must_use]
    pub fn my_membership(&self) -> Option<&MemberInfo> {
        let me = self.my_account_id()?;
        self.members.iter().find(|member| member.account_id == me)
    }

    /// Whether the local player leads their party.
    #[must_use]
    pub fn i_lead(&self) -> bool {
        match (&self.party, self.my_account_id()) {
            (Some(party), Some(me)) => party.leader.as_deref() == Some(me),
            _ => false,
        }
    }

    /// Takes the combat events that arrived since the last call.
    pub fn take_events(&mut self) -> Vec<WireEvent> {
        std::mem::take(&mut self.events)
    }

    /// The local player's combat state.
    #[must_use]
    pub fn my_combat(&self) -> Option<&PlayerCombatInfo> {
        self.combat.get(self.my_account_id()?)
    }

    /// Everyone else in the instance.
    pub fn others(&self) -> impl Iterator<Item = &RemotePlayerState> {
        let me = self.my_account_id().map(str::to_owned);
        self.players
            .values()
            .filter(move |player| Some(player.account_id.as_str()) != me.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(account: &str, x: f32) -> RemotePlayerState {
        RemotePlayerState {
            account_id: account.into(),
            name: account.into(),
            definition_id: "warrior".into(),
            role: Role::Tank,
            position: Vec3::new(x, 0.0, 0.0),
            velocity: Vec3::ZERO,
            ground_normal: Vec3::Y,
            yaw: 0.0,
            grounded: true,
            last_seq: 0,
            health: 1.0,
            max_health: 1.0,
            present: true,
        }
    }

    #[test]
    fn updates_apply_and_count() {
        let mut mirror = RealmMirror::default();
        mirror.apply(RealmUpdate::Account(Some(AccountInfo {
            account_id: "me".into(),
            display_name: "Me".into(),
        })));
        mirror.apply(RealmUpdate::PlayerUpsert(player("me", 1.0)));
        mirror.apply(RealmUpdate::PlayerUpsert(player("you", 2.0)));
        mirror.apply(RealmUpdate::PlayerUpsert(player("you", 3.0)));
        assert_eq!(mirror.revision, 4);
        assert_eq!(mirror.players.len(), 2);
        assert!((mirror.me().map_or(0.0, |p| p.position.x) - 1.0).abs() < f32::EPSILON);
        assert_eq!(mirror.others().count(), 1);
        mirror.apply(RealmUpdate::PlayerRemoved("you".into()));
        assert_eq!(mirror.others().count(), 0);
    }

    #[test]
    fn leaving_the_instance_clears_its_entities() {
        let mut mirror = RealmMirror::default();
        mirror.apply(RealmUpdate::PlayerUpsert(player("me", 1.0)));
        mirror.apply(RealmUpdate::Instance(None));
        assert!(mirror.players.is_empty());
    }

    #[test]
    fn notices_are_deduplicated_and_ordered() {
        let mut mirror = RealmMirror::default();
        for id in [3, 1, 3] {
            mirror.apply(RealmUpdate::NoticeAdded(Notice {
                id,
                code: "x".into(),
                level: NoticeLevel::Info,
                message: String::new(),
            }));
        }
        assert_eq!(
            mirror.notices.iter().map(|n| n.id).collect::<Vec<_>>(),
            vec![1, 3]
        );
    }
}
