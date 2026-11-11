//! What happened in combat, as data.
//!
//! Every change combat makes to the world is announced here:
//!
//! ```text
//! Ability system ──┐
//! Status system  ──┼─> CombatEvent ─┬─> health (already applied by the emitter)
//! Death checks   ──┘                ├─> telemetry / the damage meter
//!                                   ├─> future VFX and floating numbers
//!                                   └─> future network replication
//! ```
//!
//! The architectural requirement of Milestone 10 is that damage is **observable without
//! the damage code knowing who is observing**. Nothing in the ability system mentions the
//! meter; the meter reads a slice of events. Adding a second observer — a combat log, a
//! sound cue, a replication layer — costs one more read of the same slice and no change
//! here.
//!
//! Events are values, not callbacks: a callback would run inside the ability system, and an
//! observer that mutated the world from there would make the order of two abilities in one
//! tick depend on who registered first.

use engine_core::math::Vec3;

use crate::ability::AbilityId;
use crate::character::DefenseOutcome;
use crate::combat::DamageType;
use crate::enemy::TelegraphShape;
use crate::entity::EntityId;
use crate::firemage::ZoneShape;
use crate::sim::CastEnd;
use crate::status::{DispelSchool, StatusEnd, StatusKind};
use crate::trinket::TrinketEffect;

/// One instance of damage landing.
///
/// Carries everything an observer could need, because an observer cannot ask the world:
/// by the time it reads this, the target may be dead and its slot reused.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageEvent {
    /// Who dealt it.
    pub source: EntityId,
    /// Who took it.
    pub target: EntityId,
    /// How much health was actually lost.
    pub amount: f32,
    /// How much of the hit exceeded the health that was there.
    pub overkill: f32,
    /// Which ability is credited. [`AbilityId::NONE`] for damage from no ability.
    pub ability: AbilityId,
    /// What kind of damage it was.
    pub damage_type: DamageType,
    /// Whether this hit killed the target.
    pub killed: bool,
    /// Whether it critically struck.
    ///
    /// Carried rather than derived: the roll happens inside the executor against the crit
    /// chance of that moment, and an observer that tried to infer it from the amount would
    /// have to re-implement every multiplier that produced the number.
    pub critical: bool,
    /// Whether this is one tick of damage over time rather than a hit.
    ///
    /// Carried for the same reason `critical` is: a burn's third tick and a bolt can deal the
    /// same number, and a reader that wants to show the two differently — the combat text
    /// draws a tick smaller and quieter — cannot tell them apart from the amount or the
    /// ability, since a status ticks under the ability that applied it.
    pub periodic: bool,
    /// Where the target was standing when it landed, at its feet.
    ///
    /// On the event rather than looked up afterwards, because an observer reading this may
    /// be reading it after the target died and its slot was reused — and because a hit is a
    /// thing that happened at a place, not a thing that happened to a row in a table.
    pub position: Vec3,
    /// The simulation tick it happened on.
    pub tick: u64,
}

/// Something that happened in combat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CombatEvent {
    /// An ability was successfully used. Emitted once per use, before its damage.
    AbilityUsed {
        /// Who used it.
        source: EntityId,
        /// Which ability.
        ability: AbilityId,
        /// How many entities it affected.
        targets: u32,
        /// Which way it was aimed, in radians about +Y.
        ///
        /// The yaw the executor collected its hit list with, not the caster's facing now:
        /// a cone clicked to one side while the camera steers the character somewhere else
        /// must be *drawn* where it *hit*. Meaningless for an ability that covers no
        /// ground, and zero there.
        facing: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// Damage landed.
    Damage(DamageEvent),
    /// Health was restored.
    Healing {
        /// Who healed.
        source: EntityId,
        /// Who was healed.
        target: EntityId,
        /// How much health was actually restored.
        amount: f32,
        /// Which ability is credited.
        ability: AbilityId,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A status was applied to an entity.
    StatusApplied {
        /// Who applied it.
        source: EntityId,
        /// Who now has it.
        target: EntityId,
        /// Which status.
        status: StatusKind,
        /// How long it will last, seconds.
        duration: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A status ended.
    StatusRemoved {
        /// Who had it.
        target: EntityId,
        /// Which status.
        status: StatusKind,
        /// Why it ended.
        reason: StatusEnd,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A cast started. Its effect lands when `CastEnded { end: Completed }` follows.
    ///
    /// Announced rather than exposed as state so a cast bar, a future animation selector and
    /// a network client all read the same stream everything else does.
    CastStarted {
        /// Who is casting.
        source: EntityId,
        /// Which ability.
        ability: AbilityId,
        /// How long the cast takes, seconds.
        duration: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A cast ended, whether or not it landed.
    CastEnded {
        /// Who was casting.
        source: EntityId,
        /// Which ability.
        ability: AbilityId,
        /// Why it ended.
        end: CastEnd,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// An entity was moved somewhere it did not travel to.
    ///
    /// Blink is the one that emits it today. It exists because presentation cannot tell a
    /// teleport from a very fast run by watching positions: both are a large change between
    /// two ticks, and the camera has to lag one and not the other. Carrying both ends means
    /// an observer can measure the jump without having kept last tick's world.
    Teleported {
        /// Who moved.
        entity: EntityId,
        /// Where they were.
        from: Vec3,
        /// Where they are now, after the geometry had its say.
        to: Vec3,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A multi-phase enemy entered a new phase.
    ///
    /// Announced rather than exposed as state for the same reason a cast is: a boss frame, a
    /// visual change and a future encounter log all read the one stream. Emitted exactly
    /// once per transition, on the tick the threshold was crossed.
    PhaseChanged {
        /// Which enemy.
        entity: EntityId,
        /// Which phase it entered. Zero is the first, so this is always one or more.
        phase: usize,
        /// The phase's name, for a readout that should not have to know the table.
        name: &'static str,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// An entity's health reached zero.
    Death {
        /// Who died.
        entity: EntityId,
        /// Who killed them, or [`EntityId::INVALID`] if nobody did.
        killer: EntityId,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A single-target ability reached its victim.
    ///
    /// **The damage has already been applied when this is emitted.** Every targeted ability
    /// in the class is hitscan and stays hitscan — making a bolt a real travelling object
    /// would change what "in range" means, when a target can dodge and what a dead target
    /// does to a cast in flight, and all three are gameplay. What this announces is the
    /// *line the effect travelled along*, so presentation can fly something down it and play
    /// an impact at the far end. A missing bolt costs a picture, never a hit.
    ProjectileLaunched {
        /// Who cast it.
        source: EntityId,
        /// Who it hit.
        target: EntityId,
        /// Where the caster was standing.
        from: Vec3,
        /// Where the target was standing.
        to: Vec3,
        /// Which ability.
        ability: AbilityId,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// Something is falling and will land at a known place after a known delay.
    ///
    /// Meteor, and the Ultimate when it is enabled. Announced when the cast lands rather
    /// than when the rock does, which is what lets the interface show an anticipation ring
    /// for exactly the delay the gameplay is counting down.
    ImpactScheduled {
        /// Who cast it.
        source: EntityId,
        /// Which ability.
        ability: AbilityId,
        /// Where it will land.
        position: Vec3,
        /// The radius it will cover, in metres.
        radius: f32,
        /// Seconds until it lands.
        delay: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A scheduled impact arrived.
    ImpactLanded {
        /// Who cast it.
        source: EntityId,
        /// Which ability.
        ability: AbilityId,
        /// Where it landed.
        position: Vec3,
        /// The radius it covered, in metres.
        radius: f32,
        /// How many entities were inside that radius.
        targets: u32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A lasting area of effect started burning.
    ///
    /// Carries the *gameplay* shape — the same [`ZoneShape`] the containment test uses — so
    /// a visual built from it cannot claim a different footprint from the one that deals
    /// the damage.
    AreaEffectStarted {
        /// Names this area, so its end can be matched to its start.
        id: u32,
        /// Who created it.
        source: EntityId,
        /// Which ability is credited.
        ability: AbilityId,
        /// Where it is anchored: a circle's centre, a line's near end.
        position: Vec3,
        /// The ground it covers.
        shape: ZoneShape,
        /// Seconds it will last.
        duration: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A lasting area of effect burned out.
    AreaEffectEnded {
        /// Which area, matching an earlier [`CombatEvent::AreaEffectStarted`].
        id: u32,
        /// Where it was anchored.
        position: Vec3,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A character left the ground under its own power.
    Jumped {
        /// Who jumped.
        entity: EntityId,
        /// Where they were standing.
        position: Vec3,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// An attack has been announced and will land on a known shape after a known delay.
    ///
    /// **The shape is the gameplay one.** The same [`TelegraphShape`] travels here and into
    /// the hit test that resolves the attack, so what is drawn on the floor and what takes
    /// damage cannot drift apart - there is one value, not two.
    ///
    /// Distinct from [`CombatEvent::ImpactScheduled`], which is the player's own Meteor: that
    /// one is always a circle and carries a radius, and rewriting it would change an event
    /// three observers already read.
    TelegraphStarted {
        /// Names this telegraph, so its end can be matched to its start.
        id: u32,
        /// Who is casting it.
        source: EntityId,
        /// Which ability.
        ability: AbilityId,
        /// Where the shape is anchored: a circle's centre, a cone's apex.
        anchor: Vec3,
        /// The ground it will cover.
        shape: TelegraphShape,
        /// Seconds until it lands.
        delay: f32,
        /// The entity the shape moves with, for a mark that follows its victim.
        ///
        /// `None` for everything else, which is nearly everything: a telegraph that moved
        /// would be impossible to walk out of. A mark is the one exception, and it is drawn on
        /// the victim because stepping out of it is not the answer — taking it away from the
        /// party is.
        follow: Option<EntityId>,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A telegraphed attack stopped being pending, whether or not it landed.
    TelegraphEnded {
        /// Which telegraph, matching an earlier [`CombatEvent::TelegraphStarted`].
        id: u32,
        /// Where it was anchored.
        anchor: Vec3,
        /// Whether the attack actually went off. `false` means it was interrupted, lost its
        /// target, or its caster died.
        resolved: bool,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// An incoming attack met a Block, a Parry, a Dodge — or none of the three.
    ///
    /// Emitted once per attack, from the one place the roll happens, and **only** when the
    /// outcome was a defensive one. It is what Battle Trance's healing and Spell Reflect's
    /// cooldown are both fed from downstream, which is why there
    /// is exactly one of them per attack: a second event for the same swing would be a
    /// second payment for it.
    AttackDefended {
        /// Who was attacking.
        attacker: EntityId,
        /// Who defended.
        defender: EntityId,
        /// Which of the three it was.
        outcome: DefenseOutcome,
        /// The attack that was answered.
        ability: AbilityId,
        /// Where the defender was standing.
        position: Vec3,
        /// The tick it happened on.
        tick: u64,
    },
    /// Valor was generated by a successful cast of War Stomp, Colossus Strike or Shield Charge.
    ///
    /// One per cast, however many enemies it hit. When it was the fifth stack the bar is
    /// already back at zero and `filled` is set: the Battle Fervor stack it bought follows as
    /// an ordinary [`CombatEvent::StatusApplied`].
    ValorGained {
        /// Whose resource moved.
        entity: EntityId,
        /// What the resource reads now: zero when this filled it.
        value: u8,
        /// Whether this was the fifth stack, which emptied the bar and applied Battle Fervor.
        filled: bool,
        /// Which ability generated it.
        ability: AbilityId,
        /// The tick it happened on.
        tick: u64,
    },
    /// Astral Power was generated by a Druid cast.
    AstralPowerGained {
        /// Whose resource moved.
        entity: EntityId,
        /// How much went in, after the cap.
        amount: f32,
        /// What the resource reads now.
        value: f32,
        /// Which ability generated it.
        ability: AbilityId,
        /// The tick it happened on.
        tick: u64,
    },
    /// The Astral Power button was pressed: the next compatible spell is empowered.
    ///
    /// **Nothing has been spent yet.** The resource is taken by the spell that consumes the
    /// arming, which is announced as [`CombatEvent::AstralPowerSpent`].
    AstralPowerArmed {
        /// Who armed it.
        entity: EntityId,
        /// What the resource reads, which is unchanged by arming.
        value: f32,
        /// The tick it happened on.
        tick: u64,
    },
    /// Astral Power was spent to empower a spell.
    AstralPowerSpent {
        /// Whose resource moved.
        entity: EntityId,
        /// How much was spent.
        amount: f32,
        /// What the resource reads now.
        value: f32,
        /// Which ability was empowered.
        ability: AbilityId,
        /// The tick it happened on.
        tick: u64,
    },
    /// Mana was paid for a cast.
    ManaSpent {
        /// Whose resource moved.
        entity: EntityId,
        /// How much was paid.
        amount: f32,
        /// What the pool reads now.
        value: f32,
        /// Which ability was paid for.
        ability: AbilityId,
        /// The tick it happened on.
        tick: u64,
    },
    /// Mana was given back by an ability rather than by regeneration.
    ManaRestored {
        /// Whose resource moved.
        entity: EntityId,
        /// How much went in, after the pool's cap.
        amount: f32,
        /// What the pool reads now.
        value: f32,
        /// Which ability gave it back.
        ability: AbilityId,
        /// The tick it happened on.
        tick: u64,
    },
    /// A harmful effect was removed from a friendly target by a dispel.
    ///
    /// Beside [`CombatEvent::StatusRemoved`] rather than instead of it: the removal itself is
    /// still announced by the status system, and this says *who did it* and *what school
    /// answered*, which is what a combat log and a floating word need.
    Dispelled {
        /// Who cast the dispel.
        source: EntityId,
        /// Who it was cast on.
        target: EntityId,
        /// Which effect came off.
        status: StatusKind,
        /// Which school it belonged to.
        school: DispelSchool,
        /// Which ability removed it.
        ability: AbilityId,
        /// The tick it happened on.
        tick: u64,
    },
    /// An absorb shield was put up.
    AbsorbApplied {
        /// Who is shielded.
        target: EntityId,
        /// How much damage it will take.
        amount: f32,
        /// Seconds it lasts.
        duration: f32,
        /// Which ability granted it.
        ability: AbilityId,
        /// The tick it happened on.
        tick: u64,
    },
    /// An absorb shield took damage instead of health.
    Absorbed {
        /// Who was shielded.
        target: EntityId,
        /// How much the shield took.
        amount: f32,
        /// How much of the shield is left.
        remaining: f32,
        /// The tick it happened on.
        tick: u64,
    },
    /// An enemy spell was sent back to whoever cast it.
    SpellReflected {
        /// The Warrior who reflected it.
        source: EntityId,
        /// The caster it went back to.
        target: EntityId,
        /// Which enemy ability was reflected.
        ability: AbilityId,
        /// How much damage went back.
        amount: f32,
        /// The tick it happened on.
        tick: u64,
    },
    /// An enemy was stunned, silenced or taunted.
    ControlApplied {
        /// Who did it.
        source: EntityId,
        /// Who it was done to.
        target: EntityId,
        /// Which of the two it was.
        control: ControlKind,
        /// Seconds it lasts.
        seconds: f32,
        /// Which ability did it.
        ability: AbilityId,
        /// The tick it happened on.
        tick: u64,
    },
    /// An enemy changed who it attacks, or somebody is about to make it.
    ///
    /// One event per entity concerned: a switch from A to B is a `Lost` for A and a `Gained`
    /// for B, and a warning is a `Losing` for the holder and a `Gaining` for the challenger,
    /// so a reader interested in one character filters on `entity` and nothing else.
    Aggro {
        /// The enemy whose aggro it is.
        enemy: EntityId,
        /// Who gained, lost, or is about to gain or lose it.
        entity: EntityId,
        /// Which of the four.
        change: AggroChange,
        /// The tick it happened on.
        tick: u64,
    },
    /// A character touched the ground again.
    Landed {
        /// Who landed.
        entity: EntityId,
        /// Where they landed.
        position: Vec3,
        /// How fast they were falling, in metres per second. Always positive.
        speed: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// The dungeon's completion moved.
    ///
    /// Emitted once per kill that was worth something, carrying the new totals, so the
    /// progress panel, a sound and the log all hear the same number.
    DungeonProgress {
        /// Completion credited so far, in the dungeon's own units.
        credited: f32,
        /// The fraction of the requirement that is, clamped to `[0, 1]`.
        fraction: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// The dungeon reached its requirement and the boss's gate opened.
    BossUnlocked {
        /// Where the gate stood, for an effect to be drawn at.
        position: Vec3,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// The boss called an add into the fight.
    AddSummoned {
        /// The boss.
        boss: EntityId,
        /// The add.
        add: EntityId,
        /// Where it appeared.
        position: Vec3,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// The boss and every add it called are dead: the dungeon is done.
    DungeonCompleted {
        /// Seconds from the first pull to the last kill.
        seconds: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// The boss's chest appeared above where it fell and began to drop.
    ChestSpawned {
        /// The chest's identity.
        chest: u32,
        /// Where it comes to rest (on the ground).
        position: Vec3,
        /// How far above `position` it starts, in metres.
        drop_height: f32,
        /// How long it takes to land, in seconds.
        fall_seconds: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A player opened the chest. Emitted once, for the first opener; the lid animation.
    ChestOpened {
        /// The chest's identity.
        chest: u32,
        /// Who opened it.
        by: EntityId,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A player's item was put in their bag.
    LootAwarded {
        /// Who received it.
        player: EntityId,
        /// What they received.
        item: crate::character::ItemId,
        /// Its quality, sharpening and broken state.
        props: crate::character::ItemProps,
        /// Which of the player's chest items this is, in roll order, starting at zero.
        index: u8,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A player's item went to their mailbox instead of their bag: the bag was full when they
    /// opened the chest, or they never opened it. The claim is spent.
    LootMailed {
        /// Whose mailbox received it.
        player: EntityId,
        /// What was mailed.
        item: crate::character::ItemId,
        /// Its quality, sharpening and broken state.
        props: crate::character::ItemProps,
        /// Which of the player's chest items this is, in roll order, starting at zero.
        index: u8,
        /// Why it was mailed.
        reason: crate::character::MailReason,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A normal monster dropped a piece of equipment for a player. Personal loot like the
    /// chest's: the realm persists it to the player's bag or mailbox. Only a simulation with
    /// local authority (the server) emits it.
    MobLoot {
        /// Who it dropped for.
        player: EntityId,
        /// What dropped.
        item: crate::character::ItemId,
        /// Its quality, sharpening and broken state.
        props: crate::character::ItemProps,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A monster dropped currency (the modifier-reroll shard, or gold) for a player. Only a
    /// simulation with local authority (the server) emits it.
    MobCurrency {
        /// Who it dropped for.
        player: EntityId,
        /// The resource's id in the economy catalog.
        resource: &'static str,
        /// How many.
        amount: u32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A player took a letter's item into their bag.
    MailTaken {
        /// Who took it.
        player: EntityId,
        /// What they took.
        item: crate::character::ItemId,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A player tried to take a letter's item with a full bag. The letter is kept.
    MailBagFull {
        /// Who was refused.
        player: EntityId,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A rotating-beam emitter appeared (the Rotating Beams dungeon modifier).
    ///
    /// Carries everything the beams' geometry is a function of: beam `i` of `beams` points
    /// along yaw `start_angle + i · 2π / beams + angular_speed · (t − start_time)`, so a client
    /// draws and tests them every frame without any further message. Times are the
    /// simulation's own seconds; a client that runs a different clock rebases them to the
    /// moment it received the event (see `crate::modifier`).
    ModifierBeamsStarted {
        /// Names this emitter, so its end can be matched to its start.
        id: u32,
        /// Where the beams meet: the engaged pack's centroid when it engaged.
        center: Vec3,
        /// Yaw of beam zero at `start_time`, radians about +Y. Zero faces −Z.
        start_angle: f32,
        /// Radians a second the beams turn; the sign is the direction.
        angular_speed: f32,
        /// How far each beam reaches from the centre, in metres.
        radius: f32,
        /// How wide each beam is, edge to edge, in metres.
        width: f32,
        /// How many beams there are, evenly spaced.
        beams: u8,
        /// When the beams become dangerous. Until then they are only a warning.
        warmup_end_time: f64,
        /// When the emitter appeared.
        start_time: f64,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A rotating-beam emitter went out: its encounter ended, was reset, or the dungeon is
    /// over.
    ModifierBeamsEnded {
        /// Which emitter, matching an earlier [`CombatEvent::ModifierBeamsStarted`].
        id: u32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A monster died and its corpse began to fester (the Plague Explosion modifier).
    PlagueArmed {
        /// Names this corpse's plague through its fuse, explosion and cloud.
        id: u32,
        /// Where it will burst.
        position: Vec3,
        /// The radius of the burst, in metres.
        radius: f32,
        /// When the fuse was lit.
        armed_time: f64,
        /// When it bursts, in simulation seconds.
        explode_time: f64,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A festering corpse burst; a plague cloud of the same footprint now covers the spot
    /// until [`CombatEvent::PlagueCloudEnded`].
    PlagueExploded {
        /// Matches the earlier [`CombatEvent::PlagueArmed`].
        id: u32,
        /// Where it burst.
        position: Vec3,
        /// The radius of the burst and of the cloud, in metres.
        radius: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A plague cloud thinned out. Also ends a fuse that was dropped before it burst, so a
    /// visual keyed by `id` is always taken down by this event or by
    /// [`CombatEvent::PlagueExploded`] followed by it.
    PlagueCloudEnded {
        /// Matches the earlier [`CombatEvent::PlagueArmed`].
        id: u32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A player used a trinket's button. Announced once, before the effects it produces (the
    /// ordinary [`CombatEvent::StatusApplied`], [`CombatEvent::Dispelled`] and
    /// [`CombatEvent::AbsorbApplied`] that follow).
    TrinketUsed {
        /// Who used it.
        entity: EntityId,
        /// Which trinket button: `0` is the first slot, `1` the second.
        slot: u8,
        /// What it did.
        effect: TrinketEffect,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// A FireMage began channelling the Moving Fire Beam.
    BeamChannelStarted {
        /// The channeller.
        entity: EntityId,
        /// Which way the beam points, radians about +Y.
        yaw: f32,
        /// The simulation time the channel began at, in seconds.
        start_time: f64,
        /// Seconds the channel will last unless it is broken.
        duration: f32,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// The beam's steering target moved.
    ///
    /// Emitted at most ten times a second, and only when the target changed. The beam turns
    /// at a fixed rate, so an observer that knows where it points and where it is turning to
    /// can draw it every frame in between without hearing again.
    BeamYaw {
        /// The channeller.
        entity: EntityId,
        /// Which way the beam points at `time`.
        yaw: f32,
        /// The yaw it is turning towards.
        turn_target_yaw: f32,
        /// The simulation time of the report, in seconds.
        time: f64,
        /// The simulation tick it happened on.
        tick: u64,
    },
    /// The Moving Fire Beam stopped.
    BeamChannelEnded {
        /// The channeller.
        entity: EntityId,
        /// Why.
        reason: crate::firemage::BeamEnd,
        /// The simulation tick it happened on.
        tick: u64,
    },
}

impl CombatEvent {
    /// The tick this event was emitted on.
    #[must_use]
    pub fn tick(&self) -> u64 {
        match *self {
            Self::AbilityUsed { tick, .. }
            | Self::Healing { tick, .. }
            | Self::StatusApplied { tick, .. }
            | Self::StatusRemoved { tick, .. }
            | Self::CastStarted { tick, .. }
            | Self::CastEnded { tick, .. }
            | Self::Teleported { tick, .. }
            | Self::PhaseChanged { tick, .. }
            | Self::Death { tick, .. }
            | Self::ProjectileLaunched { tick, .. }
            | Self::ImpactScheduled { tick, .. }
            | Self::ImpactLanded { tick, .. }
            | Self::AreaEffectStarted { tick, .. }
            | Self::AreaEffectEnded { tick, .. }
            | Self::Jumped { tick, .. }
            | Self::TelegraphStarted { tick, .. }
            | Self::TelegraphEnded { tick, .. }
            | Self::Landed { tick, .. }
            | Self::AttackDefended { tick, .. }
            | Self::ValorGained { tick, .. }
            | Self::AstralPowerGained { tick, .. }
            | Self::AstralPowerArmed { tick, .. }
            | Self::AstralPowerSpent { tick, .. }
            | Self::ManaSpent { tick, .. }
            | Self::ManaRestored { tick, .. }
            | Self::Dispelled { tick, .. }
            | Self::AbsorbApplied { tick, .. }
            | Self::Absorbed { tick, .. }
            | Self::SpellReflected { tick, .. }
            | Self::ControlApplied { tick, .. }
            | Self::Aggro { tick, .. }
            | Self::DungeonProgress { tick, .. }
            | Self::BossUnlocked { tick, .. }
            | Self::AddSummoned { tick, .. }
            | Self::DungeonCompleted { tick, .. }
            | Self::ChestSpawned { tick, .. }
            | Self::ChestOpened { tick, .. }
            | Self::LootAwarded { tick, .. }
            | Self::LootMailed { tick, .. }
            | Self::MobLoot { tick, .. }
            | Self::MobCurrency { tick, .. }
            | Self::MailTaken { tick, .. }
            | Self::MailBagFull { tick, .. }
            | Self::ModifierBeamsStarted { tick, .. }
            | Self::ModifierBeamsEnded { tick, .. }
            | Self::PlagueArmed { tick, .. }
            | Self::PlagueExploded { tick, .. }
            | Self::PlagueCloudEnded { tick, .. }
            | Self::TrinketUsed { tick, .. }
            | Self::BeamChannelStarted { tick, .. }
            | Self::BeamYaw { tick, .. }
            | Self::BeamChannelEnded { tick, .. } => tick,
            Self::Damage(damage) => damage.tick,
        }
    }

    /// The damage this event carries, if it is a damage event.
    #[must_use]
    pub fn as_damage(&self) -> Option<&DamageEvent> {
        match self {
            Self::Damage(damage) => Some(damage),
            _ => None,
        }
    }
}

/// Which kind of control an enemy was put under.
///
/// A stun and a silence are two lengths of the same sentence — "this enemy may not do
/// something for a while" — and separating them into two events would give the interface,
/// the sound and the log two code paths for one idea.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlKind {
    /// It does nothing at all.
    Stun,
    /// It may not start a cast.
    Silence,
    /// It must attack whoever taunted it.
    Taunt,
}

impl ControlKind {
    /// Short uppercase name, for floating text and the log.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stun => "STUNNED",
            Self::Silence => "SILENCED",
            Self::Taunt => "TAUNTED",
        }
    }
}

/// What happened to somebody's standing with one enemy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggroChange {
    /// The enemy turned on them.
    Gained,
    /// The enemy turned away from them.
    Lost,
    /// They are close to pulling it off whoever it is attacking.
    Gaining,
    /// Somebody else is close to pulling it off them.
    Losing,
}

impl AggroChange {
    /// Short uppercase text, for floating combat text and the log.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gained => "AGGRO GAINED",
            Self::Lost => "AGGRO LOST",
            Self::Gaining => "GAINING AGGRO",
            Self::Losing => "LOSING AGGRO",
        }
    }
}

/// The events emitted since the stream was last cleared.
///
/// A growable buffer that is cleared, never freed: after the first few frames the capacity
/// covers the busiest tick and emitting an event is a `push` into memory that already
/// exists. That is the whole allocation strategy for combat, and Milestone 25 measures it.
#[derive(Debug, Default, Clone)]
pub struct EventStream {
    events: Vec<CombatEvent>,
    recorded: u64,
}

impl EventStream {
    /// An empty stream.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty stream with room for `capacity` events before it grows.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            events: Vec::with_capacity(capacity),
            recorded: 0,
        }
    }

    /// Keeps only the events `keep` accepts. A networked client drops the outcomes it
    /// predicted (damage, healing, deaths) because the server's arrive instead.
    pub fn retain(&mut self, keep: impl FnMut(&CombatEvent) -> bool) {
        self.events.retain(keep);
    }

    /// Appends an event.
    pub fn push(&mut self, event: CombatEvent) {
        self.events.push(event);
        self.recorded += 1;
    }

    /// The events currently held, in emission order.
    #[must_use]
    pub fn events(&self) -> &[CombatEvent] {
        &self.events
    }

    /// The events emitted after `cursor`, which is a length previously read from
    /// [`EventStream::len`].
    ///
    /// This is how a per-tick observer reads only what that tick produced without the
    /// stream having to be cleared between ticks — the frame's whole event list stays
    /// available to the application afterwards.
    #[must_use]
    pub fn events_after(&self, cursor: usize) -> &[CombatEvent] {
        self.events.get(cursor..).unwrap_or(&[])
    }

    /// How many events are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether nothing has been emitted since the last clear.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// How many events this stream has ever carried.
    ///
    /// Survives [`EventStream::clear`], so a test can assert that a frame emitted anything
    /// at all without holding on to the events.
    #[must_use]
    pub fn total_recorded(&self) -> u64 {
        self.recorded
    }

    /// How many events the buffer can hold before it grows.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.events.capacity()
    }

    /// Drops the held events, keeping the allocation.
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// Drops the held events and the lifetime counter.
    pub fn reset(&mut self) {
        self.events.clear();
        self.recorded = 0;
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    fn damage(amount: f32, tick: u64) -> CombatEvent {
        CombatEvent::Damage(DamageEvent {
            source: EntityId::new(0, 1),
            target: EntityId::new(1, 1),
            amount,
            overkill: 0.0,
            ability: AbilityId::from_slot(0).expect("slot 0 exists"),
            damage_type: DamageType::Fire,
            killed: false,
            critical: false,
            periodic: false,
            position: Vec3::ZERO,
            tick,
        })
    }

    #[test]
    fn events_are_held_in_emission_order() {
        let mut stream = EventStream::new();
        stream.push(damage(10.0, 1));
        stream.push(damage(20.0, 1));

        let amounts: Vec<f32> = stream
            .events()
            .iter()
            .filter_map(|event| event.as_damage().map(|damage| damage.amount))
            .collect();
        assert_eq!(amounts, vec![10.0, 20.0]);
        assert_eq!(stream.len(), 2);
        assert_eq!(stream.total_recorded(), 2);
    }

    #[test]
    fn a_cursor_reads_only_what_came_after_it() {
        // How the meter observes one tick's events without the frame's list being cleared.
        let mut stream = EventStream::new();
        stream.push(damage(10.0, 1));
        let cursor = stream.len();
        stream.push(damage(20.0, 2));
        stream.push(damage(30.0, 2));

        let recent = stream.events_after(cursor);
        assert_eq!(recent.len(), 2);
        assert!(recent.iter().all(|event| event.tick() == 2));
        assert!(
            stream.events_after(99).is_empty(),
            "a stale cursor is empty"
        );
    }

    #[test]
    fn clearing_keeps_the_allocation_and_the_lifetime_count() {
        let mut stream = EventStream::with_capacity(64);
        for tick in 0..64 {
            stream.push(damage(1.0, tick));
        }
        let capacity = stream.capacity();
        stream.clear();

        assert!(stream.is_empty());
        assert_eq!(stream.total_recorded(), 64, "the counter survives a clear");
        assert_eq!(stream.capacity(), capacity, "the buffer was not released");

        for tick in 0..64 {
            stream.push(damage(1.0, tick));
        }
        assert_eq!(
            stream.capacity(),
            capacity,
            "a steady state must not reallocate"
        );
    }

    #[test]
    fn resetting_forgets_the_lifetime_count_too() {
        let mut stream = EventStream::new();
        stream.push(damage(1.0, 0));
        stream.reset();
        assert_eq!(stream.total_recorded(), 0);
        assert!(stream.is_empty());
    }

    #[test]
    fn every_event_reports_the_tick_it_happened_on() {
        let entity = EntityId::new(3, 1);
        let ability = AbilityId::from_slot(2).expect("slot 2 exists");
        let events = [
            CombatEvent::AbilityUsed {
                source: entity,
                ability,
                targets: 4,
                facing: 0.0,
                tick: 7,
            },
            damage(1.0, 7),
            CombatEvent::Healing {
                source: entity,
                target: entity,
                amount: 5.0,
                ability,
                tick: 7,
            },
            CombatEvent::StatusApplied {
                source: entity,
                target: entity,
                status: StatusKind::Burning,
                duration: 6.0,
                tick: 7,
            },
            CombatEvent::StatusRemoved {
                target: entity,
                status: StatusKind::Burning,
                reason: StatusEnd::Consumed,
                tick: 7,
            },
            CombatEvent::Death {
                entity,
                killer: entity,
                tick: 7,
            },
        ];
        for event in events {
            assert_eq!(event.tick(), 7, "{event:?}");
        }
    }
}
