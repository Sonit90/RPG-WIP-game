//! Recount: combat statistics computed by the realm from the server's own combat events.
//!
//! ```text
//! server Simulation ─► WireEvents (per fixed tick) ─► InstanceMeter ─┬─► instance_meter rows (≤ 1 Hz)
//!                                                                    └─► DamageReport (at finish)
//! ```
//!
//! The meter is an observer of the events the server already produces; nothing a client sends
//! reaches it, so the numbers (and the persisted report the operator dashboard reads) cannot
//! be forged. Time is the fight's own clock (ticks / 60), so the figures are the same on every
//! machine and never depend on how late a pass ran.
//!
//! ## What is measured
//!
//! Per player (by account): damage, healing, and per ability the damage, healing, hits,
//! crits and casts, plus the damage done to each target. Two scopes:
//!
//! * **Overall** — the whole run, from the moment the instance went Active.
//! * **Current fight** — starts when the party deals or takes damage after at least
//!   [`FIGHT_IDLE_SECONDS`] of quiet (or the boss is engaged), ends when
//!   [`FIGHT_IDLE_SECONDS`] pass with no damage dealt or taken by anyone in the party, when the
//!   boss encounter resets, or when the dungeon completes. The last finished fight stays as
//!   the "current" one until the next starts.
//!
//! Cost: O(events) with no allocation once a player has met their abilities and targets.

use std::collections::BTreeMap;

use engine_game::ability::FireAbility;
use serde::{Deserialize, Serialize};

use crate::combat::{AbilityRef, EntityRef, WireEvent};
use crate::{Micros, SECOND};

/// Seconds without party damage after which a fight is over.
pub const FIGHT_IDLE_SECONDS: f64 = 5.0;

/// The shortest window a rate is computed over, so one hit is not an infinite DPS.
pub const MINIMUM_RATE_WINDOW: f64 = 1.0;

/// Targets kept per player in a snapshot.
pub const TOP_TARGETS: usize = 8;

/// The longest gap between two of a player's actions that still counts as active time.
const ACTIVE_GAP_SECONDS: f64 = 2.5;

/// What one ability did, in a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AbilityStat {
    /// The ability's index in `FireAbility::ALL` (the wire index).
    pub ability: u16,
    /// Damage dealt.
    pub damage: u64,
    /// Healing done.
    #[serde(default)]
    pub healing: u64,
    /// Damage instances.
    pub hits: u32,
    /// Of which critical strikes.
    pub crits: u32,
    /// Times it was used.
    pub casts: u32,
}

/// What one target took from one player, in a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetStat {
    /// The monster's key in the instance.
    pub target: u64,
    /// Damage it took.
    pub damage: u64,
}

/// One player's figures for one scope (a fight or the whole run).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Summary {
    /// Total damage.
    pub damage: u64,
    /// Total healing.
    pub healing: u64,
    /// The scope's length in seconds: what DPS divides by.
    pub seconds: f32,
    /// Seconds the player was actually acting in it.
    pub active: f32,
    /// Per ability, most damage first (healers: most healing).
    pub abilities: Vec<AbilityStat>,
    /// Top targets by damage.
    pub targets: Vec<TargetStat>,
}

impl Summary {
    /// Damage per second over the scope.
    #[must_use]
    pub fn dps(&self) -> f32 {
        self.damage as f32 / self.seconds.max(MINIMUM_RATE_WINDOW as f32)
    }

    /// Healing per second over the scope.
    #[must_use]
    pub fn hps(&self) -> f32 {
        self.healing as f32 / self.seconds.max(MINIMUM_RATE_WINDOW as f32)
    }
}

/// What one `instance_meter` row carries for one player.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MeterBlob {
    /// The whole run.
    pub overall: Summary,
    /// The current (or last finished) fight.
    pub fight: Summary,
    /// Whether that fight is still going.
    pub in_fight: bool,
}

impl MeterBlob {
    /// The blob as the row stores it.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads a row's blob; `None` for text this version cannot read.
    #[must_use]
    pub fn from_json(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }
}

/// The display name of the ability at wire index `index`.
#[must_use]
pub fn ability_name(index: u16) -> &'static str {
    FireAbility::ALL
        .get(usize::from(index))
        .map_or("Unknown", |ability| ability.definition().name)
}

/// The stable key of the ability at wire index `index`, for the persisted report.
#[must_use]
pub fn ability_key(index: u16) -> String {
    FireAbility::ALL
        .get(usize::from(index))
        .map_or_else(|| "unknown".to_owned(), |ability| format!("{ability:?}"))
}

#[derive(Debug, Clone, Default)]
struct AbilityAcc {
    ability: u16,
    damage: f64,
    healing: f64,
    hits: u32,
    crits: u32,
    casts: u32,
}

#[derive(Debug, Clone, Default)]
struct Tally {
    damage: f64,
    healing: f64,
    abilities: Vec<AbilityAcc>,
    targets: BTreeMap<u64, f64>,
    active: f64,
    last_action: Option<f64>,
}

impl Tally {
    fn ability(&mut self, ability: u16) -> &mut AbilityAcc {
        if let Some(index) = self.abilities.iter().position(|a| a.ability == ability) {
            return &mut self.abilities[index];
        }
        self.abilities.push(AbilityAcc {
            ability,
            ..AbilityAcc::default()
        });
        let last = self.abilities.len() - 1;
        &mut self.abilities[last]
    }

    fn act(&mut self, now: f64) {
        if let Some(last) = self.last_action {
            self.active += (now - last).clamp(0.0, ACTIVE_GAP_SECONDS);
        }
        self.last_action = Some(now);
    }

    fn load(&mut self, summary: &Summary) {
        self.damage = summary.damage as f64;
        self.healing = summary.healing as f64;
        self.abilities = summary
            .abilities
            .iter()
            .map(|a| AbilityAcc {
                ability: a.ability,
                damage: a.damage as f64,
                healing: a.healing as f64,
                hits: a.hits,
                crits: a.crits,
                casts: a.casts,
            })
            .collect();
        self.targets = summary
            .targets
            .iter()
            .map(|t| (t.target, t.damage as f64))
            .collect();
        self.active = f64::from(summary.active);
        self.last_action = None;
    }

    fn clear(&mut self) {
        self.damage = 0.0;
        self.healing = 0.0;
        self.abilities.clear();
        self.targets.clear();
        self.active = 0.0;
        self.last_action = None;
    }

    fn summary(&self, seconds: f64) -> Summary {
        let mut abilities: Vec<AbilityStat> = self
            .abilities
            .iter()
            .map(|a| AbilityStat {
                ability: a.ability,
                damage: whole(a.damage),
                healing: whole(a.healing),
                hits: a.hits,
                crits: a.crits,
                casts: a.casts,
            })
            .collect();
        abilities.sort_by(|a, b| {
            (b.damage + b.healing)
                .cmp(&(a.damage + a.healing))
                .then(a.ability.cmp(&b.ability))
        });
        let mut targets: Vec<TargetStat> = self
            .targets
            .iter()
            .map(|(target, damage)| TargetStat {
                target: *target,
                damage: whole(*damage),
            })
            .collect();
        targets.sort_by(|a, b| b.damage.cmp(&a.damage).then(a.target.cmp(&b.target)));
        targets.truncate(TOP_TARGETS);
        Summary {
            damage: whole(self.damage),
            healing: whole(self.healing),
            seconds: tenths(seconds),
            active: tenths(self.active),
            abilities,
            targets,
        }
    }
}

fn whole(value: f64) -> u64 {
    value.max(0.0).round() as u64
}

fn tenths(seconds: f64) -> f32 {
    ((seconds.max(0.0) * 10.0).round() / 10.0) as f32
}

#[derive(Debug, Clone)]
struct PlayerAcc {
    account: String,
    /// Left the run: still counted in the figures so far, no longer listed.
    departed: bool,
    overall: Tally,
    fight: Tally,
    /// The ability cast most recently, with its time and whether the fight tally saw it: an
    /// ability is announced before its damage lands, so the cast that opens a fight arrives
    /// while no fight exists yet and is credited when the fight starts on the same tick.
    last_cast: Option<(u16, f64, bool)>,
}

#[derive(Debug, Clone, Copy)]
struct Fight {
    start: f64,
    last_activity: f64,
}

/// One instance's statistics.
#[derive(Debug, Clone, Default)]
pub struct InstanceMeter {
    players: Vec<PlayerAcc>,
    now: f64,
    active_from: Option<f64>,
    finished_at: Option<f64>,
    fight: Option<Fight>,
    /// The last finished fight's span, kept until the next starts.
    last_fight: Option<(f64, f64)>,
    boss_engaged: bool,
    boss_engaged_at: Option<f64>,
    boss_killed_at: Option<f64>,
    dirty: bool,
}

impl InstanceMeter {
    /// A meter for these accounts.
    #[must_use]
    pub fn new<I, S>(accounts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            players: accounts
                .into_iter()
                .map(|account| PlayerAcc {
                    account: account.into(),
                    departed: false,
                    overall: Tally::default(),
                    fight: Tally::default(),
                    last_cast: None,
                })
                .collect(),
            ..Self::default()
        }
    }

    /// Marks the instance Active at fight time `now`: what the overall rates divide from.
    pub fn activate(&mut self, now: f64) {
        self.now = self.now.max(now);
        if self.active_from.is_none() {
            self.active_from = Some(now);
            self.dirty = true;
        }
    }

    /// Ends the run at `now`: the overall length freezes and later events are ignored.
    pub fn finish(&mut self, now: f64) {
        if self.finished_at.is_some() {
            return;
        }
        self.advance_to(now);
        self.end_fight(self.fight.map_or(now, |f| f.last_activity));
        self.finished_at = Some(now);
        self.dirty = true;
    }

    /// Whether [`InstanceMeter::finish`] has run.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished_at.is_some()
    }

    /// Moves the clock to `now`, ending a fight that has been quiet long enough.
    pub fn advance_to(&mut self, now: f64) {
        if now > self.now {
            self.now = now;
        }
        if let Some(fight) = self.fight
            && self.now - fight.last_activity >= FIGHT_IDLE_SECONDS
        {
            self.end_fight(fight.last_activity);
        }
    }

    /// Tells the meter whether the boss encounter is engaged. Engaging starts a fight; a reset
    /// (engaged to not engaged before the boss died) ends it.
    pub fn set_boss_engaged(&mut self, now: f64, engaged: bool) {
        if self.finished_at.is_some() || engaged == self.boss_engaged {
            return;
        }
        self.boss_engaged = engaged;
        if engaged {
            self.boss_engaged_at = Some(now);
            self.boss_killed_at = None;
            self.touch_fight(now);
        } else if self.boss_killed_at.is_none() {
            self.boss_engaged_at = None;
            self.end_fight(now);
        }
        self.dirty = true;
    }

    /// Whether a fight is going.
    #[must_use]
    pub fn in_fight(&self) -> bool {
        self.fight.is_some()
    }

    /// Whether anything changed since [`InstanceMeter::clear_dirty`].
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// Marks the current state as projected.
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    /// Whether a projection could differ from the last one: something changed, or a fight's
    /// length is still growing.
    #[must_use]
    pub fn needs_projection(&self) -> bool {
        self.dirty || self.fight.is_some()
    }

    /// Reads one pass's events, all of which happened at fight time `now`.
    pub fn observe(&mut self, now: f64, events: &[WireEvent]) {
        if self.finished_at.is_some() {
            return;
        }
        self.advance_to(now);
        for event in events {
            match event {
                WireEvent::Damage {
                    source,
                    target,
                    amount,
                    ability,
                    critical,
                    ..
                } if *amount > 0.0 => {
                    self.damage(now, source, target, *amount, *ability, *critical)
                }
                WireEvent::Healing {
                    source,
                    amount,
                    ability,
                    ..
                } if *amount > 0.0 => self.healing(now, source, f64::from(*amount), *ability),
                WireEvent::AbilityUsed {
                    source, ability, ..
                } => self.cast(now, source, *ability),
                WireEvent::DungeonCompleted { .. } => {
                    if self.boss_engaged_at.is_some() && self.boss_killed_at.is_none() {
                        self.boss_killed_at = Some(now);
                    }
                    self.end_fight(self.fight.map_or(now, |f| f.last_activity.max(now)));
                    self.dirty = true;
                }
                _ => {}
            }
        }
    }

    fn player_mut(&mut self, source: &EntityRef) -> Option<&mut PlayerAcc> {
        let EntityRef::Player(account) = source else {
            return None;
        };
        self.players.iter_mut().find(|p| p.account == *account)
    }

    fn damage(
        &mut self,
        now: f64,
        source: &EntityRef,
        target: &EntityRef,
        amount: f32,
        ability: AbilityRef,
        critical: bool,
    ) {
        match (source, target) {
            // The party hits a monster.
            (EntityRef::Player(_), EntityRef::Monster(key)) => {
                self.touch_fight(now);
                let amount = f64::from(amount);
                let in_fight = self.fight.is_some();
                let Some(player) = self.player_mut(source) else {
                    return;
                };
                for (scope, counts) in [(&mut player.overall, true), (&mut player.fight, in_fight)]
                {
                    if !counts {
                        continue;
                    }
                    scope.act(now);
                    scope.damage += amount;
                    *scope.targets.entry(*key).or_default() += amount;
                    if let AbilityRef::Player(index) = ability {
                        let stat = scope.ability(index);
                        stat.damage += amount;
                        stat.hits += 1;
                        stat.crits += u32::from(critical);
                    }
                }
                self.dirty = true;
            }
            // A monster hits the party: it keeps the fight alive and starts one.
            (EntityRef::Monster(_), EntityRef::Player(_)) => {
                self.touch_fight(now);
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn healing(&mut self, now: f64, source: &EntityRef, amount: f64, ability: AbilityRef) {
        // Healing from no ability is regeneration on the walk between pulls.
        let AbilityRef::Player(index) = ability else {
            return;
        };
        let in_fight = self.fight.is_some();
        let Some(player) = self.player_mut(source) else {
            return;
        };
        for (scope, counts) in [(&mut player.overall, true), (&mut player.fight, in_fight)] {
            if !counts {
                continue;
            }
            scope.act(now);
            scope.healing += amount;
            scope.ability(index).healing += amount;
        }
        self.dirty = true;
    }

    fn cast(&mut self, now: f64, source: &EntityRef, ability: AbilityRef) {
        let AbilityRef::Player(index) = ability else {
            return;
        };
        let in_fight = self.fight.is_some();
        let Some(player) = self.player_mut(source) else {
            return;
        };
        player.overall.ability(index).casts += 1;
        if in_fight {
            player.fight.ability(index).casts += 1;
        }
        player.last_cast = Some((index, now, in_fight));
        self.dirty = true;
    }

    /// Records party damage activity at `now`, starting a fight if none is going.
    fn touch_fight(&mut self, now: f64) {
        match &mut self.fight {
            Some(fight) => fight.last_activity = now,
            None => {
                self.fight = Some(Fight {
                    start: now,
                    last_activity: now,
                });
                self.last_fight = None;
                for player in &mut self.players {
                    player.fight.clear();
                    // The cast that opened this fight was announced on this same tick.
                    if let Some((index, at, false)) = player.last_cast
                        && (at - now).abs() < f64::EPSILON
                    {
                        player.fight.ability(index).casts += 1;
                        player.last_cast = Some((index, at, true));
                    }
                }
            }
        }
        self.dirty = true;
    }

    fn end_fight(&mut self, at: f64) {
        if let Some(fight) = self.fight.take() {
            self.last_fight = Some((fight.start, at.max(fight.start)));
            self.dirty = true;
        }
    }

    /// Restores one player's figures from the row a previous module instance wrote, after the
    /// fight was rebuilt from the tables. The fight itself is not resumed: it shows as the last
    /// finished one. Boss timing is lost.
    pub fn seed(&mut self, account: &str, blob: &MeterBlob) {
        let now = self.now;
        let Some(player) = self.players.iter_mut().find(|p| p.account == account) else {
            return;
        };
        player.overall.load(&blob.overall);
        player.fight.load(&blob.fight);
        let from = now - f64::from(blob.overall.seconds);
        self.active_from = Some(self.active_from.map_or(from, |current| current.min(from)));
        let fight_seconds = f64::from(blob.fight.seconds);
        if self.fight.is_none() {
            self.last_fight = Some((now - fight_seconds, now));
        }
        self.dirty = true;
    }

    /// Seconds the current (or last finished) fight lasts.
    #[must_use]
    pub fn fight_seconds(&self) -> f64 {
        match (self.fight, self.last_fight) {
            (Some(fight), _) => (self.now - fight.start).max(0.0),
            (None, Some((start, end))) => (end - start).max(0.0),
            (None, None) => 0.0,
        }
    }

    /// Seconds from the instance going Active to now while a fight is going, to the end of the
    /// last fight between fights (so a quiet minute neither dilutes the rates nor makes the
    /// figures change), and to the end of the run once it is finished.
    #[must_use]
    pub fn overall_seconds(&self) -> f64 {
        let Some(from) = self.active_from else {
            return 0.0;
        };
        let to = match (self.finished_at, self.fight, self.last_fight) {
            (Some(end), _, _) => end,
            (None, Some(_), _) => self.now,
            (None, None, Some((_, end))) => end,
            (None, None, None) => from,
        };
        (to - from).max(0.0)
    }

    /// Seconds from the boss being engaged to its death, if both happened.
    #[must_use]
    pub fn boss_kill_seconds(&self) -> Option<f64> {
        Some((self.boss_killed_at? - self.boss_engaged_at?).max(0.0))
    }

    /// Marks `account` as having left the run. What they did stays in [`InstanceMeter::blob`]
    /// and the persisted report; they are no longer in [`InstanceMeter::accounts`] or
    /// [`InstanceMeter::blobs`], so nothing lists them as present or projects them again.
    pub fn depart(&mut self, account: &str) {
        if let Some(player) = self.players.iter_mut().find(|p| p.account == account) {
            player.departed = true;
            self.dirty = true;
        }
    }

    /// The accounts being measured and present, in the order they were given.
    pub fn accounts(&self) -> impl Iterator<Item = &str> {
        self.players
            .iter()
            .filter(|p| !p.departed)
            .map(|p| p.account.as_str())
    }

    /// One player's blob.
    #[must_use]
    pub fn blob(&self, account: &str) -> Option<MeterBlob> {
        let player = self.players.iter().find(|p| p.account == account)?;
        Some(MeterBlob {
            overall: player.overall.summary(self.overall_seconds()),
            fight: player.fight.summary(self.fight_seconds()),
            in_fight: self.fight.is_some(),
        })
    }

    /// Every present player's blob, in the order they were given.
    #[must_use]
    pub fn blobs(&self) -> Vec<(String, MeterBlob)> {
        self.players
            .iter()
            .filter(|p| !p.departed)
            .filter_map(|p| Some((p.account.clone(), self.blob(&p.account)?)))
            .collect()
    }

    /// The persisted report for `account` over the whole run. `party` is who played.
    #[must_use]
    pub fn damage_report(&self, account: &str, party: Vec<ReportMember>) -> Option<DamageReport> {
        let blob = self.blob(account)?;
        let seconds = self.overall_seconds();
        let total = blob.overall.damage;
        let abilities = self
            .players
            .iter()
            .find(|p| p.account == account)?
            .overall
            .summary(seconds)
            .abilities
            .into_iter()
            .filter(|a| a.damage > 0 || a.casts > 0)
            .map(|a| ReportAbility {
                ability: ability_key(a.ability),
                damage: a.damage,
                casts: a.casts,
            })
            .collect();
        Some(DamageReport {
            party,
            total_damage: total,
            dps: (total as f64 / seconds.max(MINIMUM_RATE_WINDOW)) as f32,
            duration_seconds: seconds as f32,
            boss_kill_seconds: self.boss_kill_seconds().map(|s| s as f32),
            abilities,
        })
    }
}

/// Decides when and what to write to the `instance_meter` rows: at most once per
/// [`PROJECTION_INTERVAL`], only when the meter has something new, and only the rows whose text
/// differs from what was last written.
#[derive(Debug, Clone, Default)]
pub struct MeterProjector {
    last: Option<Micros>,
    written: BTreeMap<String, String>,
}

/// The least time between two projections.
pub const PROJECTION_INTERVAL: Micros = SECOND;

impl MeterProjector {
    /// The rows to write at `now` as `(account, JSON)`; empty when it is too soon, nothing
    /// changed or every row already says so. `force` skips the time and dirtiness checks (the
    /// end of the run) but still never reports an unchanged row.
    pub fn project(
        &mut self,
        now: Micros,
        meter: &mut InstanceMeter,
        force: bool,
    ) -> Vec<(String, String)> {
        if !force {
            if self
                .last
                .is_some_and(|last| now - last < PROJECTION_INTERVAL)
            {
                return Vec::new();
            }
            if !meter.needs_projection() {
                return Vec::new();
            }
        }
        self.last = Some(now);
        meter.clear_dirty();
        let mut changed = Vec::new();
        for (account, blob) in meter.blobs() {
            let json = blob.to_json();
            if self.written.get(&account) != Some(&json) {
                self.written.insert(account.clone(), json.clone());
                changed.push((account, json));
            }
        }
        changed
    }
}

/// A party member in a persisted report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportMember {
    /// The account id.
    pub account_id: String,
    /// The character's name.
    pub character_name: String,
    /// Its class.
    pub class: String,
    /// The role it queued as.
    #[serde(default)]
    pub role: String,
}

/// One ability in a persisted report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportAbility {
    /// The ability's stable key.
    pub ability: String,
    /// Damage dealt.
    pub damage: u64,
    /// Times used.
    pub casts: u32,
}

/// What the realm persists about one player's run. Aggregates only; never raw events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DamageReport {
    /// Who played.
    pub party: Vec<ReportMember>,
    /// The player's total damage.
    pub total_damage: u64,
    /// Damage per second over the run.
    pub dps: f32,
    /// The run's length in seconds (from Active to the last boss falling).
    pub duration_seconds: f32,
    /// Seconds from the boss being engaged to its death, if known.
    #[serde(default)]
    pub boss_kill_seconds: Option<f32>,
    /// Per ability.
    pub abilities: Vec<ReportAbility>,
}

impl DamageReport {
    /// The report as the outbox stores it.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads an outbox report; `None` for empty or unreadable text.
    #[must_use]
    pub fn from_json(text: &str) -> Option<Self> {
        if text.trim().is_empty() {
            return None;
        }
        serde_json::from_str(text).ok()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    fn player(account: &str) -> EntityRef {
        EntityRef::Player(account.to_owned())
    }

    fn hit(account: &str, monster: u64, amount: f32, ability: u16, critical: bool) -> WireEvent {
        WireEvent::Damage {
            source: player(account),
            target: EntityRef::Monster(monster),
            amount,
            overkill: 0.0,
            ability: AbilityRef::Player(ability),
            damage_type: 0,
            killed: false,
            critical,
            periodic: false,
            position: [0.0; 3],
        }
    }

    fn taken(account: &str, monster: u64, amount: f32) -> WireEvent {
        WireEvent::Damage {
            source: EntityRef::Monster(monster),
            target: player(account),
            amount,
            overkill: 0.0,
            ability: AbilityRef::Monster(0),
            damage_type: 1,
            killed: false,
            critical: false,
            periodic: false,
            position: [0.0; 3],
        }
    }

    fn heal(account: &str, amount: f32, ability: AbilityRef) -> WireEvent {
        WireEvent::Healing {
            source: player(account),
            target: player(account),
            amount,
            ability,
        }
    }

    fn used(account: &str, ability: u16) -> WireEvent {
        WireEvent::AbilityUsed {
            source: player(account),
            ability: AbilityRef::Player(ability),
            targets: 1,
            facing: 0.0,
        }
    }

    fn meter() -> InstanceMeter {
        let mut meter = InstanceMeter::new(["a", "b"]);
        meter.activate(0.0);
        meter
    }

    #[test]
    fn damage_is_aggregated_per_ability_target_and_crit() {
        let mut m = meter();
        m.observe(
            1.0,
            &[
                used("a", 0),
                hit("a", 7, 100.0, 0, false),
                hit("a", 7, 200.0, 0, true),
                hit("a", 8, 50.0, 1, false),
            ],
        );
        let blob = m.blob("a").expect("present");
        assert_eq!(blob.overall.damage, 350);
        assert_eq!(blob.fight.damage, 350);
        assert_eq!(blob.overall.abilities[0].ability, 0);
        assert_eq!(blob.overall.abilities[0].damage, 300);
        assert_eq!(blob.overall.abilities[0].hits, 2);
        assert_eq!(blob.overall.abilities[0].crits, 1);
        assert_eq!(blob.overall.abilities[0].casts, 1);
        assert_eq!(
            blob.overall.targets[0],
            TargetStat {
                target: 7,
                damage: 300
            }
        );
        assert_eq!(
            blob.overall.targets[1],
            TargetStat {
                target: 8,
                damage: 50
            }
        );
    }

    #[test]
    fn damage_is_attributed_to_the_right_player() {
        let mut m = meter();
        m.observe(
            1.0,
            &[hit("a", 1, 100.0, 0, false), hit("b", 1, 40.0, 2, false)],
        );
        assert_eq!(m.blob("a").expect("present").overall.damage, 100);
        assert_eq!(m.blob("b").expect("present").overall.damage, 40);
        // Somebody who is not in the party is ignored.
        m.observe(1.0, &[hit("stranger", 1, 999.0, 0, false)]);
        assert!(m.blob("stranger").is_none());
        assert_eq!(m.blob("a").expect("present").overall.damage, 100);
    }

    #[test]
    fn healing_counts_but_regeneration_does_not() {
        let mut m = meter();
        m.observe(
            1.0,
            &[
                hit("a", 1, 10.0, 0, false),
                heal("b", 80.0, AbilityRef::Player(26)),
                heal("b", 500.0, AbilityRef::None),
            ],
        );
        let blob = m.blob("b").expect("present");
        assert_eq!(blob.overall.healing, 80);
        assert_eq!(blob.fight.healing, 80);
        assert_eq!(blob.overall.abilities[0].healing, 80);
        assert_eq!(blob.overall.damage, 0);
    }

    #[test]
    fn a_fight_starts_on_damage_dealt_or_taken_and_ends_after_idle() {
        let mut m = meter();
        assert!(!m.in_fight());
        m.observe(10.0, &[taken("a", 1, 30.0)]);
        assert!(m.in_fight());
        m.observe(12.0, &[hit("a", 1, 100.0, 0, false)]);
        m.advance_to(16.9);
        assert!(
            m.in_fight(),
            "4.9 s after the last damage is still the fight"
        );
        m.advance_to(17.0);
        assert!(!m.in_fight());
        // Ended at the last damage: 10 to 12.
        assert!((m.fight_seconds() - 2.0).abs() < 1e-9);
        assert_eq!(m.blob("a").expect("present").fight.damage, 100);
        assert!(!m.blob("a").expect("present").in_fight);
    }

    #[test]
    fn the_last_fight_stays_visible_until_the_next_starts() {
        let mut m = meter();
        m.observe(1.0, &[hit("a", 1, 100.0, 0, false)]);
        m.advance_to(30.0);
        assert!(!m.in_fight());
        assert_eq!(m.blob("a").expect("present").fight.damage, 100);
        m.observe(40.0, &[hit("a", 2, 30.0, 0, false)]);
        let blob = m.blob("a").expect("present");
        assert_eq!(blob.fight.damage, 30, "a new fight starts from zero");
        assert_eq!(
            blob.overall.damage, 130,
            "overall accumulates across fights"
        );
    }

    #[test]
    fn damage_after_idle_starts_a_new_fight_even_without_a_clock_call() {
        let mut m = meter();
        m.observe(1.0, &[hit("a", 1, 100.0, 0, false)]);
        m.observe(20.0, &[hit("a", 1, 7.0, 0, false)]);
        assert_eq!(m.blob("a").expect("present").fight.damage, 7);
    }

    #[test]
    fn healing_between_fights_goes_to_overall_only() {
        let mut m = meter();
        m.observe(1.0, &[hit("a", 1, 100.0, 0, false)]);
        m.advance_to(10.0);
        m.observe(11.0, &[heal("b", 50.0, AbilityRef::Player(26))]);
        let blob = m.blob("b").expect("present");
        assert_eq!(blob.overall.healing, 50);
        assert_eq!(blob.fight.healing, 0);
    }

    #[test]
    fn the_opening_cast_is_credited_to_the_fight_it_started() {
        let mut m = meter();
        m.observe(5.0, &[used("a", 0), hit("a", 1, 100.0, 0, false)]);
        let blob = m.blob("a").expect("present");
        assert_eq!(blob.fight.abilities[0].casts, 1);
        assert_eq!(blob.overall.abilities[0].casts, 1);
        // An earlier cast is not.
        let mut m = meter();
        m.observe(5.0, &[used("a", 0)]);
        m.observe(6.0, &[hit("a", 1, 100.0, 0, false)]);
        assert_eq!(m.blob("a").expect("present").fight.abilities[0].casts, 0);
    }

    #[test]
    fn dps_is_damage_over_the_fight_and_over_the_run() {
        let mut m = meter();
        m.observe(10.0, &[hit("a", 1, 100.0, 0, false)]);
        m.observe(14.0, &[hit("a", 1, 300.0, 0, false)]);
        m.advance_to(19.0);
        let blob = m.blob("a").expect("present");
        // Fight: 10 to 14.
        assert!((blob.fight.seconds - 4.0).abs() < 1e-6);
        assert!((blob.fight.dps() - 100.0).abs() < 1e-3);
        // Overall: since Active at 0, to the end of the last fight (14).
        assert!((blob.overall.seconds - 14.0).abs() < 1e-6);
        assert!((blob.overall.dps() - 400.0 / 14.0).abs() < 0.01);
    }

    #[test]
    fn a_single_hit_is_not_an_infinite_dps() {
        let mut m = meter();
        m.observe(5.0, &[hit("a", 1, 100.0, 0, false)]);
        m.advance_to(11.0);
        let blob = m.blob("a").expect("present");
        assert!((blob.fight.dps() - 100.0).abs() < 1e-3);
    }

    #[test]
    fn engaging_the_boss_starts_a_fight_and_a_reset_ends_it() {
        let mut m = meter();
        m.set_boss_engaged(30.0, true);
        assert!(m.in_fight());
        m.set_boss_engaged(33.0, false);
        assert!(!m.in_fight());
        assert!(m.boss_kill_seconds().is_none());
    }

    #[test]
    fn the_boss_kill_time_runs_from_engage_to_the_dungeon_completing() {
        let mut m = meter();
        m.set_boss_engaged(100.0, true);
        m.observe(110.0, &[hit("a", 1, 100.0, 0, false)]);
        m.observe(145.5, &[WireEvent::DungeonCompleted { seconds: 145.5 }]);
        assert!(!m.in_fight());
        let seconds = m.boss_kill_seconds().expect("present");
        assert!((seconds - 45.5).abs() < 1e-9);
    }

    #[test]
    fn finishing_freezes_the_run() {
        let mut m = meter();
        m.observe(5.0, &[hit("a", 1, 100.0, 0, false)]);
        m.finish(20.0);
        m.observe(25.0, &[hit("a", 1, 900.0, 0, false)]);
        m.advance_to(500.0);
        let blob = m.blob("a").expect("present");
        assert_eq!(blob.overall.damage, 100);
        assert!((m.overall_seconds() - 20.0).abs() < 1e-9);
    }

    #[test]
    fn active_time_skips_long_pauses() {
        let mut m = meter();
        m.observe(1.0, &[hit("a", 1, 1.0, 0, false)]);
        m.observe(2.0, &[hit("a", 1, 1.0, 0, false)]);
        m.observe(5.0, &[hit("a", 1, 1.0, 0, false)]);
        let active = m.blob("a").expect("present").overall.active;
        assert!((active - 3.5).abs() < 1e-6, "{active}");
    }

    #[test]
    fn targets_keep_only_the_top_few() {
        let mut m = meter();
        let events: Vec<WireEvent> = (0..20)
            .map(|k| hit("a", k, 10.0 + k as f32, 0, false))
            .collect();
        m.observe(1.0, &events);
        let targets = m.blob("a").expect("present").overall.targets;
        assert_eq!(targets.len(), TOP_TARGETS);
        assert_eq!(targets[0].target, 19);
    }

    #[test]
    fn the_report_is_aggregates_with_stable_keys() {
        let mut m = meter();
        m.observe(10.0, &[used("a", 0), hit("a", 1, 600.0, 0, false)]);
        m.set_boss_engaged(11.0, true);
        m.observe(41.0, &[WireEvent::DungeonCompleted { seconds: 41.0 }]);
        m.finish(41.0);
        let party = vec![ReportMember {
            account_id: "a".into(),
            character_name: "Ash".into(),
            class: "Fire Mage".into(),
            role: "dps".into(),
        }];
        let report = m.damage_report("a", party).expect("present");
        assert_eq!(report.total_damage, 600);
        assert!((report.dps - 600.0 / 41.0).abs() < 0.01);
        assert!((report.duration_seconds - 41.0).abs() < 1e-4);
        assert_eq!(report.boss_kill_seconds, Some(30.0));
        assert_eq!(report.abilities[0].ability, "Firebolt");
        assert_eq!(report.abilities[0].casts, 1);
        let again = DamageReport::from_json(&report.to_json()).expect("present");
        assert_eq!(again, report);
        assert!(DamageReport::from_json("").is_none());
        assert!(m.damage_report("nobody", Vec::new()).is_none());
    }

    #[test]
    fn the_projector_writes_at_most_once_a_second_and_only_changes() {
        let mut m = meter();
        let mut projector = MeterProjector::default();
        m.observe(1.0, &[hit("a", 1, 100.0, 0, false)]);
        m.advance_to(10.0);
        let first = projector.project(10 * SECOND, &mut m, false);
        assert_eq!(first.len(), 2, "both players' rows are new");
        // Too soon, even though the meter changed.
        m.observe(10.2, &[heal("b", 50.0, AbilityRef::Player(26))]);
        assert!(
            projector
                .project(10 * SECOND + SECOND / 2, &mut m, false)
                .is_empty()
        );
        // A second on: only b's row differs from what was written.
        let next = projector.project(11 * SECOND, &mut m, false);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].0, "b");
    }

    #[test]
    fn the_projector_is_silent_when_nothing_changed() {
        let mut m = meter();
        let mut projector = MeterProjector::default();
        m.observe(1.0, &[hit("a", 1, 100.0, 0, false)]);
        m.advance_to(10.0);
        assert!(!projector.project(10 * SECOND, &mut m, false).is_empty());
        // The fight is over and nothing is dirty: no work at all.
        for second in 11..60 {
            assert!(projector.project(second * SECOND, &mut m, false).is_empty());
        }
        // A forced projection of unchanged rows still writes nothing.
        assert!(projector.project(61 * SECOND, &mut m, true).is_empty());
    }

    #[test]
    fn a_seeded_meter_reproduces_its_rows() {
        let mut m = meter();
        m.observe(10.0, &[used("a", 0), hit("a", 3, 250.0, 0, true)]);
        m.advance_to(20.0);
        let blob = m.blob("a").expect("present");
        let mut rebuilt = InstanceMeter::new(["a", "b"]);
        rebuilt.activate(20.0);
        rebuilt.advance_to(20.0);
        rebuilt.seed("a", &blob);
        let again = rebuilt.blob("a").expect("present");
        assert_eq!(again.overall, blob.overall);
        assert_eq!(again.fight, blob.fight);
    }

    #[test]
    fn the_blob_round_trips_and_is_stable_when_nothing_happens() {
        let mut m = meter();
        m.observe(1.0, &[hit("a", 1, 100.0, 0, true)]);
        m.advance_to(30.0);
        let first = m.blob("a").expect("present").to_json();
        m.advance_to(60.0);
        assert_eq!(
            first,
            m.blob("a").expect("present").to_json(),
            "idle time changes nothing"
        );
        assert_eq!(MeterBlob::from_json(&first), m.blob("a"));
        assert!(MeterBlob::from_json("nope").is_none());
    }
}
