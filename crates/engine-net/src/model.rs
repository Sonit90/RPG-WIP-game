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
use engine_realm::Role;
use engine_realm::chat::Channel;
use engine_realm::combat::{
    CastState, CooldownState, EntityRef, Resources, StatusState, WireEvent,
};
use engine_realm::instance::InstanceState;
use engine_realm::meter::MeterBlob;
use engine_realm::modifiers::{KeyModifier, RerollEvent};
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
    /// The leader's account id, once chosen (whoever inserts the key).
    pub leader: Option<String>,
    /// The account id of whoever inserted the key, once one is in.
    pub key_owner: Option<String>,
    /// The inserted key's level (0 while none is in).
    pub key_level: u32,
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
    /// The dungeon their key is for (empty when they have none).
    pub key_dungeon_id: String,
    /// Their key's level as snapshotted at queue time (0 when they have none).
    pub key_level: u32,
    /// Their key's modifiers as snapshotted at queue time.
    pub key_modifiers: Vec<KeyModifier>,
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
    /// The level of the key the run was started with.
    pub key_level: u32,
    /// The key's modifiers as they stand now, rerolls included.
    pub modifiers: Vec<KeyModifier>,
    /// Rerolls the run has used.
    pub rerolls: u32,
    /// The run's reward bonus, per cent (a reroll sets it).
    pub reward_bonus_pct: u32,
    /// The latest reroll, if there was one; its `seq` grows with every reroll.
    pub last_reroll: Option<RerollEvent>,
}

/// What the local player may spend on a key reroll this run, and has spent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendInfo {
    /// The currency's resource id.
    pub resource: String,
    /// How much the snapshot held when the run began.
    pub balance: u64,
    /// How much has been spent this run.
    pub spent: u64,
}

impl SpendInfo {
    /// What can still be spent.
    #[must_use]
    pub fn available(&self) -> u64 {
        self.balance.saturating_sub(self.spent)
    }
}

/// How the player's last run ended, for the completion screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummaryInfo {
    /// The instance it was.
    pub instance_id: u64,
    /// Which dungeon.
    pub dungeon_id: String,
    /// Its display name.
    pub dungeon_name: String,
    /// Whether the boss fell.
    pub success: bool,
    /// The key level the run used.
    pub key_level: u32,
    /// Whether this player inserted the key.
    pub key_owner: bool,
    /// The owner's key level after the run (equal to `key_level` on failure).
    pub key_level_after: u32,
    /// How long the run took, microseconds.
    pub duration_us: i64,
    /// Experience granted.
    pub experience: u32,
    /// Resources granted, `(resource id, amount)`.
    pub resources: Vec<(String, u32)>,
    /// The items the chest rolled for this player, in roll order (empty before the roll).
    pub loot: Vec<engine_realm::haul::SummaryItem>,
    /// How many items monsters dropped for this player during the run.
    pub mob_drops: u32,
    /// Gold and aether shards this player picked up from monsters, `(resource id, amount)`.
    pub looted: Vec<(String, u32)>,
    /// The run's reward bonus, per cent.
    pub bonus_pct: u32,
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

/// One chat line, as the server stored it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessageInfo {
    /// Message id; larger is newer.
    pub id: u64,
    /// The channel.
    pub channel: Channel,
    /// The sender's account id.
    pub sender_account: String,
    /// The sender's character name, set by the server.
    pub sender_name: String,
    /// A whisper's target account id.
    pub recipient_account: Option<String>,
    /// A whisper's target character name.
    pub recipient_name: Option<String>,
    /// The text.
    pub text: String,
    /// When it was sent, server time.
    pub sent_at: Micros,
}

/// The most chat lines the mirror keeps.
pub const CHAT_HISTORY: usize = 200;

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
    /// The local player's reroll currency row.
    Spend(Option<SpendInfo>),
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
    /// One player's Recount figures inserted or changed.
    MeterUpsert(String, Box<MeterBlob>),
    /// One player's Recount row gone.
    MeterRemoved(String),
    /// One notice arrived.
    NoticeAdded(Notice),
    /// One notice expired or was dismissed.
    NoticeRemoved(u64),
    /// Queue counts per dungeon.
    Population(Vec<(String, Population)>),
    /// The run summary row (inserted, updated or deleted).
    RunSummary(Option<RunSummaryInfo>),
    /// One chat line arrived (or was there when the subscription applied).
    Chat(ChatMessageInfo),
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
    /// What the local player may spend on a key reroll, while in an instance.
    pub spend: Option<SpendInfo>,
    /// The instance's players, by account id.
    pub players: BTreeMap<String, RemotePlayerState>,
    /// The instance's monsters, by key.
    pub monsters: BTreeMap<u64, MonsterInfo>,
    /// Every player's combat state, by account id.
    pub combat: BTreeMap<String, PlayerCombatInfo>,
    /// Combat events not yet taken by the game, oldest first.
    pub events: Vec<WireEvent>,
    /// Recount: every instance member's server-computed figures, by account id.
    pub meter: BTreeMap<String, MeterBlob>,
    /// Counts changes to [`RealmMirror::meter`] only (a few a second at most), so the window
    /// that shows it rebuilds when its data changed rather than on every combat event.
    pub meter_revision: u64,
    /// Unread notices, oldest first.
    pub notices: Vec<Notice>,
    /// Queue counts per dungeon id.
    pub population: BTreeMap<String, Population>,
    /// How the latest run ended, for the completion screen.
    pub run_summary: Option<RunSummaryInfo>,
    /// Chat the player may read, oldest first, at most [`CHAT_HISTORY`].
    pub chat: Vec<ChatMessageInfo>,
    /// Counts every applied update; lets a reader notice change without diffing.
    pub revision: u64,
}

impl RealmMirror {
    /// Whether the account is queued, in a party or in an instance: the realm holds a snapshot
    /// of the character's talents and equipment for the run, so neither may change meanwhile.
    ///
    /// The API enforces the same rule; this is what lets the interface say so before asking.
    #[must_use]
    pub fn is_engaged(&self) -> bool {
        self.ticket.is_some() || self.party.is_some() || self.instance.is_some()
    }

    /// Applies one update.
    pub fn apply(&mut self, update: RealmUpdate) {
        self.revision = self.revision.wrapping_add(1);
        match update {
            RealmUpdate::Reset(mirror) => {
                let (revision, meter_revision) = (self.revision, self.meter_revision);
                *self = *mirror;
                self.revision = revision;
                self.meter_revision = meter_revision.wrapping_add(1);
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
                    self.spend = None;
                    self.players.clear();
                    self.monsters.clear();
                    self.combat.clear();
                    self.events.clear();
                    self.meter.clear();
                    self.meter_revision = self.meter_revision.wrapping_add(1);
                }
                self.instance = instance;
            }
            RealmUpdate::Spend(spend) => self.spend = spend,
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
            RealmUpdate::MeterUpsert(account_id, blob) => {
                self.meter.insert(account_id, *blob);
                self.meter_revision = self.meter_revision.wrapping_add(1);
            }
            RealmUpdate::MeterRemoved(account_id) => {
                self.meter.remove(&account_id);
                self.meter_revision = self.meter_revision.wrapping_add(1);
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
            RealmUpdate::RunSummary(summary) => self.run_summary = summary,
            RealmUpdate::Chat(message) => {
                if self.chat.iter().any(|existing| existing.id == message.id) {
                    return;
                }
                self.chat.push(message);
                self.chat.sort_by_key(|message| message.id);
                let excess = self.chat.len().saturating_sub(CHAT_HISTORY);
                self.chat.drain(..excess);
            }
        }
    }

    /// The local account id, once known.
    #[must_use]
    pub fn my_account_id(&self) -> Option<&str> {
        self.account
            .as_ref()
            .map(|account| account.account_id.as_str())
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

    /// The character name of whoever whispered the player last, for a reply.
    #[must_use]
    pub fn last_whisperer(&self) -> Option<&str> {
        let me = self.my_account_id()?;
        self.chat
            .iter()
            .rev()
            .find(|message| message.channel == Channel::Whisper && message.sender_account != me)
            .map(|message| message.sender_name.as_str())
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

    fn summary(instance_id: u64, experience: u32) -> RunSummaryInfo {
        RunSummaryInfo {
            instance_id,
            dungeon_id: "crypt".into(),
            dungeon_name: "The Crypt".into(),
            success: true,
            key_level: 2,
            key_owner: true,
            key_level_after: 3,
            duration_us: 1_000,
            experience,
            resources: vec![("crypt_dust".into(), 7)],
            loot: Vec::new(),
            mob_drops: 0,
            looted: Vec::new(),
            bonus_pct: 0,
        }
    }

    fn blob(damage: u64) -> MeterBlob {
        MeterBlob {
            overall: engine_realm::meter::Summary {
                damage,
                seconds: 10.0,
                ..Default::default()
            },
            ..MeterBlob::default()
        }
    }

    #[test]
    fn meter_rows_mirror_upserts_and_removals() {
        let mut mirror = RealmMirror::default();
        mirror.apply(RealmUpdate::MeterUpsert("a".into(), Box::new(blob(100))));
        mirror.apply(RealmUpdate::MeterUpsert("b".into(), Box::new(blob(50))));
        mirror.apply(RealmUpdate::MeterUpsert("a".into(), Box::new(blob(300))));
        assert_eq!(mirror.meter.len(), 2);
        assert_eq!(mirror.meter["a"].overall.damage, 300);
        mirror.apply(RealmUpdate::MeterRemoved("b".into()));
        assert_eq!(mirror.meter.keys().collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn leaving_the_instance_clears_the_meter() {
        let mut mirror = RealmMirror::default();
        mirror.apply(RealmUpdate::MeterUpsert("a".into(), Box::new(blob(100))));
        mirror.apply(RealmUpdate::Instance(None));
        assert!(mirror.meter.is_empty());
    }

    #[test]
    fn run_summary_applies_updates_and_clears() {
        let mut mirror = RealmMirror::default();
        mirror.apply(RealmUpdate::RunSummary(Some(summary(1, 100))));
        assert_eq!(mirror.run_summary.as_ref().map(|s| s.experience), Some(100));
        let mut settled = summary(1, 100);
        settled.loot = vec![engine_realm::haul::SummaryItem::new(
            "sword",
            engine_game::character::ItemProps::rolled(90),
        )];
        mirror.apply(RealmUpdate::RunSummary(Some(settled)));
        assert_eq!(
            mirror
                .run_summary
                .as_ref()
                .map(|s| s.loot.first().map(|item| item.item.as_str())),
            Some(Some("sword"))
        );
        mirror.apply(RealmUpdate::RunSummary(None));
        assert!(mirror.run_summary.is_none());
    }

    #[test]
    fn reset_replaces_run_summary_and_key_data() {
        let mut mirror = RealmMirror::default();
        mirror.apply(RealmUpdate::RunSummary(Some(summary(1, 100))));
        mirror.apply(RealmUpdate::Party(Some(PartyInfo {
            id: 1,
            dungeon_id: "crypt".into(),
            state: PartyState::Ready,
            leader: Some("a".into()),
            key_owner: Some("a".into()),
            key_level: 4,
            ready_deadline: 0,
            instance_id: None,
        })));
        mirror.apply(RealmUpdate::Reset(Box::default()));
        assert!(mirror.run_summary.is_none());
        assert!(mirror.party.is_none());
        let fresh = RealmMirror {
            run_summary: Some(summary(2, 5)),
            ..RealmMirror::default()
        };
        mirror.apply(RealmUpdate::Reset(Box::new(fresh)));
        assert_eq!(mirror.run_summary.map(|s| s.instance_id), Some(2));
    }

    fn line(id: u64, channel: Channel, sender: &str) -> ChatMessageInfo {
        ChatMessageInfo {
            id,
            channel,
            sender_account: sender.into(),
            sender_name: sender.to_uppercase(),
            recipient_account: None,
            recipient_name: None,
            text: format!("line {id}"),
            sent_at: 0,
        }
    }

    #[test]
    fn chat_is_deduplicated_ordered_and_bounded() {
        let mut mirror = RealmMirror::default();
        for id in [3, 1, 3, 2] {
            mirror.apply(RealmUpdate::Chat(line(id, Channel::Server, "a")));
        }
        assert_eq!(
            mirror.chat.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        for id in 4..(CHAT_HISTORY as u64 + 10) {
            mirror.apply(RealmUpdate::Chat(line(id, Channel::Server, "a")));
        }
        assert_eq!(mirror.chat.len(), CHAT_HISTORY);
        assert_eq!(
            mirror.chat.last().map(|m| m.id),
            Some(CHAT_HISTORY as u64 + 9)
        );
        assert!(mirror.chat.first().is_some_and(|m| m.id > 1));
    }

    #[test]
    fn chat_survives_leaving_an_instance_and_is_replaced_by_a_reset() {
        let mut mirror = RealmMirror::default();
        mirror.apply(RealmUpdate::Chat(line(1, Channel::Party, "a")));
        mirror.apply(RealmUpdate::Instance(None));
        assert_eq!(mirror.chat.len(), 1);
        let fresh = RealmMirror {
            chat: vec![line(5, Channel::Server, "b")],
            ..RealmMirror::default()
        };
        mirror.apply(RealmUpdate::Reset(Box::new(fresh)));
        assert_eq!(
            mirror.chat.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![5]
        );
    }

    #[test]
    fn a_reply_goes_to_whoever_whispered_last() {
        let mut mirror = RealmMirror::default();
        mirror.apply(RealmUpdate::Account(Some(AccountInfo {
            account_id: "me".into(),
            display_name: "Me".into(),
        })));
        assert_eq!(mirror.last_whisperer(), None);
        mirror.apply(RealmUpdate::Chat(line(1, Channel::Whisper, "ann")));
        mirror.apply(RealmUpdate::Chat(line(2, Channel::Whisper, "bob")));
        // My own whisper and a party line do not change who I reply to.
        mirror.apply(RealmUpdate::Chat(line(3, Channel::Whisper, "me")));
        mirror.apply(RealmUpdate::Chat(line(4, Channel::Party, "cy")));
        assert_eq!(mirror.last_whisperer(), Some("BOB"));
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

    #[test]
    fn the_spend_row_follows_updates_and_leaves_with_the_instance() {
        let mut mirror = RealmMirror::default();
        mirror.apply(RealmUpdate::Spend(Some(SpendInfo {
            resource: "aether_shard".into(),
            balance: 3,
            spent: 1,
        })));
        assert_eq!(mirror.spend.as_ref().map(SpendInfo::available), Some(2));
        mirror.apply(RealmUpdate::Instance(None));
        assert!(mirror.spend.is_none());
        let spent_more = SpendInfo {
            resource: "aether_shard".into(),
            balance: 1,
            spent: 5,
        };
        assert_eq!(spent_more.available(), 0, "never negative");
    }
}
