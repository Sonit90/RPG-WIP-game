//! Dungeon modifiers: rules a key adds to a run, as data the simulation runs.
//!
//! ```text
//!  key level + modifiers ──(engine-realm resolves)──▶ Vec<ActiveModifier>
//!                                                          │ Simulation::set_modifiers
//!                                                          ▼
//!  ┌──────────────────────── ModifierRuntime ────────────────────────┐
//!  │  hooks, one per sim phase:                                      │
//!  │    encounter engaged / ended  ─▶ BeamField  (Rotating Beams)    │
//!  │    monster died               ─▶ PlagueField (Plague Explosion) │
//!  │    every tick                 ─▶ both: advance, hit, expire     │
//!  │  and the four that act on the monsters themselves:              │
//!  │    monster died   ─▶ Bolstering (neighbours grow), Necrotic     │
//!  │                      Pools (ground the dead leave behind)       │
//!  │    every tick     ─▶ Raging (the wounded hit harder)            │
//!  │    cast started   ─▶ Spiteful Casters (faster; curse on landing)│
//!  └──────────────────────────────────────────────────────────────────┘
//!                          │ events                    │ damage / statuses
//!                          ▼                           ▼
//!                  presentation, sound      `Simulation::deal_damage`, `apply_status`
//! ```
//!
//! # Where the rules live
//!
//! A modifier is **not** scattered through the dungeon code. Everything it knows is in this
//! module (the pure state: emitters, fuses, clouds and their geometry) and in
//! `crate::sim::modifier_exec` (the glue that asks the world who is standing where and deals
//! the damage). The rest of the simulation makes at most one call per phase: a tick hook, a
//! monster-death hook, and the reset that clears the ground.
//!
//! # Plain data in
//!
//! `engine-game` is serde-free. [`ActiveModifier`] is plain data; `engine-realm` reads the
//! catalog (`assets/catalog/modifiers.json`), applies the key's level scaling and the
//! modifier's reroll magnitude, and hands over the resolved numbers. The defaults here are
//! only fallbacks for a modifier built by hand (a test, the offline `--modifiers` flag).
//!
//! # What a client does
//!
//! A client under a remote authority runs none of this. Its [`ModifierRuntime`] is only a
//! *mirror*: [`ModifierRuntime::observe`] files the server's events away so presentation can
//! ask "where is beam two right now" and "how long until this corpse bursts" every frame.
//! The beams are a pure function of the start event and the clock, so nothing is streamed.
//!
//! # Damage is bounded
//!
//! Every hit is a fraction of the victim's maximum health, scaled by the modifier's magnitude
//! and then **capped** (`max_hit_pct`), and each player takes at most one beam tick or one
//! cloud tick per half second. A modifier can hurt a full-health player; it cannot delete one
//! in a single instant.

pub mod beams;
pub mod bolstering;
pub mod necrotic;
pub mod plague;
pub mod raging;
pub mod spiteful;

pub use beams::{BeamEmitter, BeamField, MAX_BEAM_EMITTERS};
pub use bolstering::{Bolster, BolsterField};
pub use plague::{
    MAX_PLAGUE_CLOUDS, MAX_PLAGUE_FUSES, PlagueCloud, PlagueField, PlagueFuse, PlagueStep,
};

/// Where modifier ids start.
///
/// Area events, telegraphs and hazards name their things with a `u32` too; an observer keyed
/// by id alone would otherwise be handed one id for two different things. The modifier's live
/// in the top quarter, which neither the FireMage's zones (from zero) nor the enemy hazards
/// (from `1 << 31`) can reach.
pub const MODIFIER_ID_BASE: u32 = 3 << 30;

/// Seconds between a modifier's damage ticks (beams and clouds alike).
pub const MODIFIER_TICK_INTERVAL: f32 = 0.5;

/// Which modifier it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModifierKind {
    /// Beams sweep out from a pack's centre for as long as its fight lasts.
    RotatingBeams,
    /// Dead monsters burst after a delay and leave a plague cloud.
    PlagueExplosion,
    /// A dead monster's neighbours gain maximum health and damage, stacking.
    Bolstering,
    /// Dead monsters leave a ground pool that hurts whoever stands in it.
    NecroticPools,
    /// Wounded non-boss monsters enrage and hit harder.
    Raging,
    /// Casters cast faster, and a cast that lands leaves a dispellable curse.
    SpitefulCasters,
}

impl ModifierKind {
    /// Every kind.
    pub const ALL: [Self; 6] = [
        Self::RotatingBeams,
        Self::PlagueExplosion,
        Self::Bolstering,
        Self::NecroticPools,
        Self::Raging,
        Self::SpitefulCasters,
    ];

    /// The kind's stable id, as the catalog writes it.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::RotatingBeams => "rotating_beams",
            Self::PlagueExplosion => "plague_explosion",
            Self::Bolstering => "bolstering",
            Self::NecroticPools => "necrotic_pools",
            Self::Raging => "raging",
            Self::SpitefulCasters => "spiteful_casters",
        }
    }

    /// The kind whose id is `id`.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == id.trim())
    }

    /// The numbers a hand-built modifier of this kind gets.
    #[must_use]
    pub fn default_params(self) -> ModifierParams {
        match self {
            Self::RotatingBeams => ModifierParams::RotatingBeams(BeamParams::default()),
            Self::PlagueExplosion => ModifierParams::PlagueExplosion(PlagueParams::default()),
            Self::Bolstering => ModifierParams::Bolstering(BolsteringParams::default()),
            Self::NecroticPools => ModifierParams::NecroticPools(NecroticParams::default()),
            Self::Raging => ModifierParams::Raging(RagingParams::default()),
            Self::SpitefulCasters => ModifierParams::SpitefulCasters(SpitefulParams::default()),
        }
    }
}

/// The base numbers of Rotating Beams. Percentages are of the victim's maximum health, as a
/// whole number: `5.0` is five per cent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeamParams {
    /// How many beams, evenly spaced around the centre.
    pub beams: u8,
    /// How far each reaches, in metres.
    pub radius: f32,
    /// How wide each is, edge to edge, in metres.
    pub width: f32,
    /// Degrees a second the beams turn. The direction is rolled per emitter.
    pub angular_speed_deg: f32,
    /// Seconds a new emitter only warns before it deals damage.
    pub warmup: f32,
    /// Damage a tick, as a percentage of the victim's maximum health, before magnitude.
    pub damage_pct_max_hp: f32,
    /// The most one tick may take, as a percentage of maximum health, after magnitude.
    pub max_hit_pct: f32,
}

impl Default for BeamParams {
    fn default() -> Self {
        Self {
            beams: 3,
            radius: 9.0,
            width: 0.9,
            angular_speed_deg: 35.0,
            warmup: 2.0,
            damage_pct_max_hp: 5.0,
            max_hit_pct: 9.5,
        }
    }
}

/// The base numbers of Plague Explosion. Percentages are of the victim's maximum health.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlagueParams {
    /// Seconds from a monster's death to its burst.
    pub fuse: f32,
    /// Radius of the burst, in metres.
    pub explosion_radius: f32,
    /// Burst damage as a percentage of maximum health, before magnitude.
    pub explosion_pct_max_hp: f32,
    /// Radius of the cloud it leaves, in metres.
    pub cloud_radius: f32,
    /// Seconds the cloud lasts.
    pub cloud_duration: f32,
    /// Cloud damage a second as a percentage of maximum health, before magnitude.
    pub cloud_pct_max_hp_per_sec: f32,
    /// Fraction of healing the Plagued debuff takes (`0.2` is twenty per cent).
    pub heal_reduction: f32,
    /// Seconds Plagued lingers after the bearer leaves the cloud.
    pub debuff_linger: f32,
    /// The most any one instant of this may take, as a percentage of maximum health, after
    /// magnitude.
    pub max_hit_pct: f32,
}

impl Default for PlagueParams {
    fn default() -> Self {
        Self {
            fuse: 5.0,
            explosion_radius: 4.0,
            explosion_pct_max_hp: 12.0,
            cloud_radius: 4.0,
            cloud_duration: 6.0,
            cloud_pct_max_hp_per_sec: 3.0,
            heal_reduction: 0.2,
            debuff_linger: 3.0,
            max_hit_pct: 30.0,
        }
    }
}

/// The base numbers of Bolstering. Percentages are whole numbers of the monster's *base*.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BolsteringParams {
    /// How far from the dead monster a living one is bolstered, in metres.
    pub radius: f32,
    /// Maximum health each stack adds, in per cent of base, before magnitude.
    pub health_pct: f32,
    /// Damage each stack adds, in per cent of base, before magnitude.
    pub damage_pct: f32,
    /// The most stacks one monster carries.
    pub max_stacks: u8,
}

impl Default for BolsteringParams {
    fn default() -> Self {
        Self {
            radius: 30.0,
            health_pct: 10.0,
            damage_pct: 10.0,
            max_stacks: 5,
        }
    }
}

/// The base numbers of Necrotic Pools. Percentages are of the victim's maximum health.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NecroticParams {
    /// Seconds the ground only warns before the pool hurts.
    pub warning: f32,
    /// Radius of the pool, in metres.
    pub radius: f32,
    /// Seconds the pool lasts once it forms.
    pub duration: f32,
    /// Damage a second inside it as a percentage of maximum health, before magnitude.
    pub pct_max_hp_per_sec: f32,
    /// The most one tick may take, as a percentage of maximum health, after magnitude.
    pub max_hit_pct: f32,
}

impl Default for NecroticParams {
    fn default() -> Self {
        Self {
            warning: 1.5,
            radius: 3.0,
            duration: 8.0,
            pct_max_hp_per_sec: 4.0,
            max_hit_pct: 12.0,
        }
    }
}

/// The base numbers of Raging.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RagingParams {
    /// Below this fraction of maximum health a non-boss monster enrages (`0.3` is 30 %).
    pub threshold: f32,
    /// Damage it gains, in per cent, before magnitude.
    pub damage_pct: f32,
}

impl Default for RagingParams {
    fn default() -> Self {
        Self {
            threshold: 0.30,
            damage_pct: 50.0,
        }
    }
}

/// The base numbers of Spiteful Casters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpitefulParams {
    /// How much faster an interruptible cast is, in per cent, before magnitude.
    pub cast_speed_pct: f32,
    /// Seconds the curse lasts on each victim.
    pub debuff_duration: f32,
    /// Fraction of healing the curse takes (`0.3` is thirty per cent), before magnitude.
    pub heal_reduction: f32,
}

impl Default for SpitefulParams {
    fn default() -> Self {
        Self {
            cast_speed_pct: 30.0,
            debuff_duration: 10.0,
            heal_reduction: 0.30,
        }
    }
}

/// A modifier's base numbers, by kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModifierParams {
    /// See [`BeamParams`].
    RotatingBeams(BeamParams),
    /// See [`PlagueParams`].
    PlagueExplosion(PlagueParams),
    /// See [`BolsteringParams`].
    Bolstering(BolsteringParams),
    /// See [`NecroticParams`].
    NecroticPools(NecroticParams),
    /// See [`RagingParams`].
    Raging(RagingParams),
    /// See [`SpitefulParams`].
    SpitefulCasters(SpitefulParams),
}

impl ModifierParams {
    /// The kind these numbers belong to.
    #[must_use]
    pub fn kind(&self) -> ModifierKind {
        match self {
            Self::RotatingBeams(_) => ModifierKind::RotatingBeams,
            Self::PlagueExplosion(_) => ModifierKind::PlagueExplosion,
            Self::Bolstering(_) => ModifierKind::Bolstering,
            Self::NecroticPools(_) => ModifierKind::NecroticPools,
            Self::Raging(_) => ModifierKind::Raging,
            Self::SpitefulCasters(_) => ModifierKind::SpitefulCasters,
        }
    }
}

/// One modifier in force for a run: what it is, how hard, and its numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActiveModifier {
    /// Which modifier.
    pub kind: ModifierKind,
    /// How strong it is as a multiplier on the damage it deals: `1.0` is the base,
    /// `1.17` is seventeen per cent stronger. Already includes the key level's scaling.
    pub magnitude: f32,
    /// Its base numbers.
    pub params: ModifierParams,
}

impl ActiveModifier {
    /// A modifier of `kind` at `magnitude` with the fallback numbers.
    #[must_use]
    pub fn new(kind: ModifierKind, magnitude: f32) -> Self {
        Self {
            kind,
            magnitude: if magnitude.is_finite() {
                magnitude.max(0.0)
            } else {
                1.0
            },
            params: kind.default_params(),
        }
    }

    /// The same, with these numbers. A mismatch between `params` and the kind is repaired by
    /// taking the kind's defaults, so a malformed catalog row cannot make a modifier do
    /// something other than what it is called.
    #[must_use]
    pub fn with_params(mut self, params: ModifierParams) -> Self {
        self.params = if params.kind() == self.kind {
            params
        } else {
            self.kind.default_params()
        };
        self
    }

    fn beams(&self) -> Option<(BeamParams, f32)> {
        match self.params {
            ModifierParams::RotatingBeams(params) => Some((params, self.magnitude)),
            _ => None,
        }
    }

    fn plague(&self) -> Option<(PlagueParams, f32)> {
        match self.params {
            ModifierParams::PlagueExplosion(params) => Some((params, self.magnitude)),
            _ => None,
        }
    }

    fn bolstering(&self) -> Option<(BolsteringParams, f32)> {
        match self.params {
            ModifierParams::Bolstering(params) => Some((params, self.magnitude)),
            _ => None,
        }
    }

    fn necrotic(&self) -> Option<(NecroticParams, f32)> {
        match self.params {
            ModifierParams::NecroticPools(params) => Some((params, self.magnitude)),
            _ => None,
        }
    }

    fn raging(&self) -> Option<(RagingParams, f32)> {
        match self.params {
            ModifierParams::Raging(params) => Some((params, self.magnitude)),
            _ => None,
        }
    }

    fn spiteful(&self) -> Option<(SpitefulParams, f32)> {
        match self.params {
            ModifierParams::SpitefulCasters(params) => Some((params, self.magnitude)),
            _ => None,
        }
    }
}

/// Every modifier in force and everything they have put on the ground.
#[derive(Debug, Clone)]
pub struct ModifierRuntime {
    active: Vec<ActiveModifier>,
    beams: BeamField,
    plague: PlagueField,
    /// Necrotic Pools: the same disc-on-the-ground state as the plague, kept apart so the two
    /// modifiers can stand together without reading each other's numbers.
    pools: PlagueField,
    bolster: BolsterField,
    next_id: u32,
    /// Seconds until the next beam damage tick, shared by every emitter so a player in two
    /// beams still takes one tick.
    beam_timer: f32,
    /// Seconds until the next cloud tick, shared for the same reason.
    cloud_timer: f32,
    /// Seconds until the next pool tick.
    pool_timer: f32,
    /// Reused: what a plague step produced.
    pub(crate) scratch_step: PlagueStep,
    /// Reused: who a modifier tick is about to hurt, and for what share of their health.
    pub(crate) scratch_hits: Vec<(crate::entity::EntityId, f32)>,
}

impl Default for ModifierRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl ModifierRuntime {
    /// A runtime with no modifiers: every hook returns at its first line.
    #[must_use]
    pub fn new() -> Self {
        Self {
            active: Vec::new(),
            beams: BeamField::new(),
            plague: PlagueField::new(),
            pools: PlagueField::new(),
            bolster: BolsterField::new(),
            next_id: MODIFIER_ID_BASE,
            beam_timer: MODIFIER_TICK_INTERVAL,
            cloud_timer: MODIFIER_TICK_INTERVAL,
            pool_timer: MODIFIER_TICK_INTERVAL,
            scratch_step: PlagueStep::default(),
            scratch_hits: Vec::with_capacity(8),
        }
    }

    /// Replaces the modifiers in force. Anything already on the ground is left for the
    /// caller to clear (see [`ModifierRuntime::clear_into`]).
    pub fn set_active(&mut self, active: Vec<ActiveModifier>) {
        self.active = active;
    }

    /// The modifiers in force.
    #[must_use]
    pub fn active(&self) -> &[ActiveModifier] {
        &self.active
    }

    /// Whether no modifier is in force. A mirror that only observes can still hold ground.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    /// The Rotating Beams modifier, if it is in force: its numbers and magnitude.
    #[must_use]
    pub fn beam_rules(&self) -> Option<(BeamParams, f32)> {
        self.active.iter().find_map(ActiveModifier::beams)
    }

    /// The Plague Explosion modifier, if it is in force: its numbers and magnitude.
    #[must_use]
    pub fn plague_rules(&self) -> Option<(PlagueParams, f32)> {
        self.active.iter().find_map(ActiveModifier::plague)
    }

    /// The Bolstering modifier, if it is in force: its numbers and magnitude.
    #[must_use]
    pub fn bolstering_rules(&self) -> Option<(BolsteringParams, f32)> {
        self.active.iter().find_map(ActiveModifier::bolstering)
    }

    /// The Necrotic Pools modifier, if it is in force: its numbers and magnitude.
    #[must_use]
    pub fn necrotic_rules(&self) -> Option<(NecroticParams, f32)> {
        self.active.iter().find_map(ActiveModifier::necrotic)
    }

    /// The Raging modifier, if it is in force: its numbers and magnitude.
    #[must_use]
    pub fn raging_rules(&self) -> Option<(RagingParams, f32)> {
        self.active.iter().find_map(ActiveModifier::raging)
    }

    /// The Spiteful Casters modifier, if it is in force: its numbers and magnitude.
    #[must_use]
    pub fn spiteful_rules(&self) -> Option<(SpitefulParams, f32)> {
        self.active.iter().find_map(ActiveModifier::spiteful)
    }

    /// Every bolstered monster.
    #[must_use]
    pub fn bolstered(&self) -> &[Bolster] {
        self.bolster.entries()
    }

    /// How many stacks of Bolstering `entity` carries.
    #[must_use]
    pub fn bolster_stacks(&self, entity: crate::entity::EntityId) -> u8 {
        self.bolster.stacks(entity)
    }

    /// Every necrotic pool still warning.
    #[must_use]
    pub fn pool_warnings(&self) -> &[PlagueFuse] {
        self.pools.fuses()
    }

    /// Every necrotic pool on the ground.
    #[must_use]
    pub fn pools(&self) -> &[PlagueCloud] {
        self.pools.clouds()
    }

    /// Every beam emitter standing, in the order they appeared.
    #[must_use]
    pub fn beams(&self) -> &[BeamEmitter] {
        self.beams.emitters()
    }

    /// Every corpse about to burst.
    #[must_use]
    pub fn fuses(&self) -> &[PlagueFuse] {
        self.plague.fuses()
    }

    /// Every plague cloud on the ground.
    #[must_use]
    pub fn clouds(&self) -> &[PlagueCloud] {
        self.plague.clouds()
    }

    pub(crate) fn beam_field(&mut self) -> &mut BeamField {
        &mut self.beams
    }

    pub(crate) fn plague_field(&mut self) -> &mut PlagueField {
        &mut self.plague
    }

    pub(crate) fn pool_field(&mut self) -> &mut PlagueField {
        &mut self.pools
    }

    pub(crate) fn bolster_field(&mut self) -> &mut BolsterField {
        &mut self.bolster
    }

    /// A fresh id, in the modifiers' own space.
    pub(crate) fn fresh_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(MODIFIER_ID_BASE);
        id
    }

    /// Advances the shared damage clock for beams. Returns whether a tick is due.
    pub(crate) fn beam_tick_due(&mut self, step: f32) -> bool {
        Self::tick_due(&mut self.beam_timer, step)
    }

    /// Advances the shared damage clock for clouds. Returns whether a tick is due.
    pub(crate) fn cloud_tick_due(&mut self, step: f32) -> bool {
        Self::tick_due(&mut self.cloud_timer, step)
    }

    /// Advances the shared damage clock for necrotic pools. Returns whether a tick is due.
    pub(crate) fn pool_tick_due(&mut self, step: f32) -> bool {
        Self::tick_due(&mut self.pool_timer, step)
    }

    fn tick_due(timer: &mut f32, step: f32) -> bool {
        *timer -= step;
        if *timer > crate::TIME_EPSILON {
            return false;
        }
        // One tick however long the step was: "never more than one tick per interval".
        *timer = MODIFIER_TICK_INTERVAL;
        true
    }

    /// Puts everything on the ground out, copying the ids into `ended` so they can be
    /// announced. What a reset, a wipe and a finished dungeon run.
    pub fn clear_into(&mut self, ended: &mut Vec<EndedModifier>) {
        for id in self.beams.drain_ids() {
            ended.push(EndedModifier::Beams(id));
        }
        for id in self.plague.drain_ids() {
            ended.push(EndedModifier::Plague(id));
        }
        for id in self.pools.drain_ids() {
            ended.push(EndedModifier::Plague(id));
        }
        self.beam_timer = MODIFIER_TICK_INTERVAL;
        self.cloud_timer = MODIFIER_TICK_INTERVAL;
        self.pool_timer = MODIFIER_TICK_INTERVAL;
    }

    /// Files away what the authority announced, for presentation to draw. A client's only
    /// way to learn about modifier ground.
    ///
    /// `now` is the observer's own clock. The event's times are the authority's, and the
    /// two clocks have nothing in common, so the event is taken to have *just* happened:
    /// every time in it is shifted by the same amount, which keeps the durations
    /// (warm-up, fuse) exactly and puts the phase within a frame or two of the server's.
    pub fn observe(&mut self, event: &crate::event::CombatEvent, now: f64) {
        use crate::event::CombatEvent as E;
        match *event {
            E::ModifierBeamsStarted {
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
            } => {
                let shift = now - start_time;
                self.beams.insert(BeamEmitter {
                    id,
                    encounter: usize::MAX,
                    center,
                    start_angle,
                    angular_speed,
                    radius,
                    width,
                    beams,
                    start_time: now,
                    warmup_end_time: warmup_end_time + shift,
                });
            }
            E::ModifierBeamsEnded { id, .. } => {
                self.beams.remove(id);
            }
            E::PlagueArmed {
                id,
                position,
                radius,
                armed_time,
                explode_time,
                ..
            } => {
                let shift = now - armed_time;
                self.plague.insert_fuse(PlagueFuse {
                    id,
                    position,
                    radius,
                    armed_time: now,
                    explode_time: explode_time + shift,
                });
            }
            E::PlagueExploded {
                id,
                position,
                radius,
                ..
            } => {
                self.plague.remove_fuse(id);
                self.plague.insert_cloud(PlagueCloud {
                    id,
                    position,
                    radius,
                    // Unknown to a mirror; the end event takes it down. The safety bound only
                    // stops a lost end event leaving a cloud on the ground forever.
                    end_time: now + MIRROR_CLOUD_LIMIT,
                });
            }
            E::PlagueCloudEnded { id, .. } => {
                self.plague.remove_fuse(id);
                self.plague.remove_cloud(id);
            }
            _ => {}
        }
    }

    /// Drops mirrored ground whose end event never arrived. Cheap; run once a frame.
    pub fn prune_mirror(&mut self, now: f64) {
        self.plague.prune(now);
    }
}

/// The longest a mirrored cloud is kept without an end event, in seconds.
const MIRROR_CLOUD_LIMIT: f64 = 30.0;

/// Something a clear took off the ground, so its end can be announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndedModifier {
    /// A beam emitter, by id.
    Beams(u32),
    /// A fuse or a cloud, by id.
    Plague(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_through_their_ids() {
        for kind in ModifierKind::ALL {
            assert_eq!(ModifierKind::from_id(kind.id()), Some(kind));
        }
        assert_eq!(ModifierKind::from_id("nonsense"), None);
    }

    #[test]
    fn mismatched_params_fall_back_to_the_kinds_own() {
        let modifier = ActiveModifier::new(ModifierKind::RotatingBeams, 1.0)
            .with_params(ModifierParams::PlagueExplosion(PlagueParams::default()));
        assert!(matches!(modifier.params, ModifierParams::RotatingBeams(_)));
    }

    #[test]
    fn every_kind_carries_its_own_params_and_nobody_elses() {
        for kind in ModifierKind::ALL {
            assert_eq!(kind.default_params().kind(), kind);
            for other in ModifierKind::ALL {
                let modifier = ActiveModifier::new(kind, 1.0).with_params(other.default_params());
                assert_eq!(
                    modifier.params.kind(),
                    kind,
                    "{kind:?} given {other:?}'s numbers"
                );
            }
        }
    }

    #[test]
    fn a_nonsense_magnitude_is_not_stored() {
        assert_eq!(
            ActiveModifier::new(ModifierKind::PlagueExplosion, f32::NAN).magnitude,
            1.0
        );
        assert_eq!(
            ActiveModifier::new(ModifierKind::PlagueExplosion, -2.0).magnitude,
            0.0
        );
    }

    #[test]
    fn the_shared_clock_ticks_once_an_interval() {
        let mut runtime = ModifierRuntime::new();
        let mut ticks = 0;
        for _ in 0..120 {
            if runtime.beam_tick_due(1.0 / 60.0) {
                ticks += 1;
            }
        }
        assert_eq!(ticks, 4, "two seconds at a half-second cadence");
    }
}
