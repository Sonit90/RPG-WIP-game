//! What the interface is allowed to know about the fight.
//!
//! ```text
//! Simulation --> UiSnapshot --> game_ui / ability bar / tooltips / talent menu
//! ```
//!
//! The UI reads this and never the simulation. That is the rule the whole interface is built
//! on: a widget cannot decide that an ability is ready, cannot recompute a cooldown, and
//! cannot invent a number the game does not have. Everything it draws was measured here,
//! once, from the one system that owns the answer.
//!
//! Rebuilt in place every frame. Nothing here allocates after the first one: the vectors are
//! cleared and refilled, so a snapshot is a copy of a few hundred bytes rather than a fresh
//! object graph.

use engine_core::input::Modifiers;
use engine_game::EntityId;
use engine_game::ability::AbilityId;
use engine_game::character::equipment::INVENTORY_SLOTS;
use engine_game::character::{EquipSlot, ItemSlot, ItemStack, Stat, StatBlock};
use engine_game::druid::tuning::{ASTRAL_MAXIMUM, ASTRAL_SPEND};
use engine_game::enemy::EnemyAbility;
use engine_game::enemy::brain::AiState;
use engine_game::firemage::{
    ABILITY_SLOTS, AbilityError, ENERGY_MAXIMUM, FireAbility, FireMastery, HEAT_HOT_THRESHOLD,
    HEAT_MAXIMUM, HEAT_WARM_THRESHOLD, HeatTier, TALENTS, TalentId, ULTIMATE_ENABLED,
};
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::status::StatusKind;
use engine_game::warrior::{DEFLECTION_MAXIMUM, DEFLECTION_SPEND};
use engine_game::world::EntityKind;

/// Reads everything the Warrior's readouts show.
///
/// A free function beside [`character_view`], for the same reason: building one is a
/// question for the simulation, and the view is the answer. Every hero but the Warrior gets
/// the inert default, so a frame drawing it asks one question rather than branching on who
/// the player is in five places.
fn warrior_view(simulation: &Simulation) -> WarriorView {
    if simulation.hero() != Hero::Warrior {
        return WarriorView::default();
    }
    let warrior = simulation.warrior();
    let profile = simulation.defense_profile();
    WarriorView {
        active: true,
        deflection: warrior.deflection.value(),
        max_health_bonus: warrior.deflection.max_health_bonus(),
        armor_bonus: warrior.deflection.armor_bonus(),
        shield_block: warrior.shield_block.remaining(),
        shield_block_empowered: warrior.shield_block_empowered,
        battle_trance: warrior.battle_trance.remaining(),
        last_stand: warrior.last_stand.remaining(),
        spell_reflect: warrior.spell_reflect.remaining(),
        spell_reflect_empowered: warrior.spell_reflect_empowered,
        adrenaline: warrior.adrenaline.remaining(),
        recovery_pool: warrior.recovery_pool,
        absorb: warrior.absorb.remaining(),
        absorb_seconds: warrior.absorb.seconds_left(),
        charging: warrior.charge.is_some(),
        war_stomp_armor: warrior.war_stomp_armor,
        defensive_chance: profile.defensive_chance(),
        armor: profile.armor,
    }
}

/// Reads the current character sheet.
///
/// A free function rather than a method on the view, because building one is a question for
/// the simulation and the view is the answer.
fn character_view(simulation: &Simulation) -> CharacterView {
    let sheet = simulation.sheet();
    let mut equipment = [None; EquipSlot::COUNT];
    for slot in EquipSlot::ALL {
        equipment[slot.index()] = sheet.equipment().get(slot);
    }
    let mut inventory = [None; INVENTORY_SLOTS];
    for (index, cell) in sheet.inventory().cells().iter().enumerate() {
        inventory[index] = *cell;
    }
    let total = simulation.character_stats();
    CharacterView {
        hero: sheet.hero(),
        main_stat: sheet.main_stat(),
        base: sheet.base_stats(),
        gear: sheet.gear_stats(),
        total,
        max_health: engine_game::character::stat::health_from_stamina(total.get(Stat::Stamina)),
        equipment,
        inventory,
        spirit_stacks: simulation.spirit().stacks(),
    }
}

/// One status on an entity, as the UI sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatusView {
    /// Which status.
    pub kind: StatusKind,
    /// How many stacks are on the bearer. One for a status that does not stack.
    ///
    /// Carried so a plate can print "SUNDERED 8" rather than "SUNDERED": War Stomp's eighth
    /// stack is a condition the Warrior is watching for, and a plate that cannot count is a
    /// plate that cannot tell them they have reached it.
    pub stacks: u8,
    /// Seconds left.
    pub remaining: f32,
    /// How much of its duration is left, in `0.0..=1.0`.
    pub fraction: f32,
}

/// A fixed-capacity list of statuses.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct StatusList {
    entries: [Option<StatusView>; Self::CAPACITY],
}

impl StatusList {
    /// How many statuses one entity can show.
    pub const CAPACITY: usize = 4;

    /// The statuses, in the order they were added.
    pub fn iter(&self) -> impl Iterator<Item = StatusView> + '_ {
        self.entries.iter().filter_map(|entry| *entry)
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds one, if there is room.
    fn push(&mut self, view: StatusView) {
        if let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) {
            *slot = Some(view);
        }
    }
}

/// **The Warrior's own readouts.**
///
/// Everything the interface needs to say what Deflection is worth right now, and what the
/// class currently has running. Every field is read from the simulation rather than derived
/// here, which is the rule the rest of this module already follows: a bar that works out its
/// own bonus is a bar that can disagree with the damage pipeline.
///
/// `active` is false for every other hero, and the frame draws its own resources instead.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct WarriorView {
    /// Whether the player is the Warrior at all.
    pub active: bool,
    /// Deflection banked, in `0..=100`.
    pub deflection: f32,
    /// The maximum health bonus Deflection is currently worth, as a fraction.
    pub max_health_bonus: f32,
    /// The armour bonus Deflection is currently worth, as a fraction.
    pub armor_bonus: f32,
    /// Seconds of Shield Block left.
    pub shield_block: f32,
    /// Whether the running Shield Block was empowered.
    pub shield_block_empowered: bool,
    /// Seconds of Battle Trance left.
    pub battle_trance: f32,
    /// Seconds of Last Stand left.
    pub last_stand: f32,
    /// Seconds of Spell Reflect left.
    pub spell_reflect: f32,
    /// Whether the next spell reflected will be empowered.
    pub spell_reflect_empowered: bool,
    /// Seconds of Adrenaline left.
    pub adrenaline: f32,
    /// What Adrenaline has banked so far, in points of health.
    pub recovery_pool: f32,
    /// Puncture Blood's remaining absorb, in points of damage.
    pub absorb: f32,
    /// Seconds the absorb has left.
    pub absorb_seconds: f32,
    /// Whether a Shield Charge is running.
    pub charging: bool,
    /// Whether something in the world carries a full stack of War Stomp.
    pub war_stomp_armor: bool,
    /// The chance an attack is answered by a Block, Parry or Dodge, as a fraction.
    pub defensive_chance: f32,
    /// Armour, after every multiplier.
    pub armor: f32,
}

impl WarriorView {
    /// Deflection as a fraction, for a bar.
    #[must_use]
    pub fn deflection_fraction(&self) -> f32 {
        (self.deflection / DEFLECTION_MAXIMUM).clamp(0.0, 1.0)
    }

    /// Whether there is enough banked to empower an ability.
    #[must_use]
    pub fn can_empower(&self) -> bool {
        self.deflection + f32::EPSILON >= DEFLECTION_SPEND
    }

    /// Whether the bar is full, which is where the passive bonuses are worth nothing.
    #[must_use]
    pub fn is_capped(&self) -> bool {
        self.deflection >= DEFLECTION_MAXIMUM
    }
}

/// **The Druid's own readouts.**
///
/// Everything the interface needs to draw two resources and say what the next cast will do.
/// Every field is read from the simulation rather than derived here, which is the rule the
/// rest of this module follows: a bar that works out its own state is a bar that can
/// disagree with the executor.
///
/// `active` is false for every other hero, and the frame draws its own resources instead.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct DruidView {
    /// Whether the player is the Druid at all.
    pub active: bool,
    /// Mana available.
    pub mana: f32,
    /// The size of the Mana pool.
    pub max_mana: f32,
    /// Astral Power banked, in `0..=100`.
    pub astral: f32,
    /// **Whether the next compatible spell will be empowered.**
    ///
    /// The one fact the whole resource is about, carried as a fact rather than as something
    /// the bar works out from the value: a full bar is not armed, and an armed bar that has
    /// just spent itself is not full.
    pub armed: bool,
    /// Whether the button can be pressed right now.
    pub can_arm: bool,
    /// Seconds of Astral Wrath left. Zero when it is not running.
    pub astral_wrath: f32,
    /// Seconds of Wild Rush left. Zero when it is not running.
    pub wild_rush: f32,
    /// Whether Tranquility is channelling.
    pub channelling: bool,
    /// Whether the running channel was empowered.
    pub channel_empowered: bool,
}

impl DruidView {
    /// Mana as a fraction, for a bar.
    #[must_use]
    pub fn mana_fraction(&self) -> f32 {
        if self.max_mana <= 0.0 {
            return 0.0;
        }
        (self.mana / self.max_mana).clamp(0.0, 1.0)
    }

    /// Astral Power as a fraction, for a bar.
    #[must_use]
    pub fn astral_fraction(&self) -> f32 {
        (self.astral / ASTRAL_MAXIMUM).clamp(0.0, 1.0)
    }

    /// What one press of the button costs the next compatible spell.
    #[must_use]
    pub fn astral_cost(&self) -> f32 {
        ASTRAL_SPEND
    }
}

/// Reads everything the Druid's readouts show.
///
/// A free function beside [`warrior_view`], and for the same reason.
fn druid_view(simulation: &Simulation) -> DruidView {
    if simulation.hero() != Hero::Druid {
        return DruidView::default();
    }
    let druid = simulation.druid();
    let player = simulation.player().entity;
    let seconds = |kind: StatusKind| {
        simulation
            .world()
            .statuses(player)
            .map_or(0.0, |statuses| statuses.remaining(kind))
    };
    DruidView {
        active: true,
        mana: druid.mana.current(),
        max_mana: druid.mana.max(),
        astral: druid.astral.value(),
        armed: druid.astral.is_armed(),
        can_arm: druid.astral.can_arm(),
        astral_wrath: seconds(StatusKind::AstralWrath),
        wild_rush: seconds(StatusKind::WildRush),
        channelling: druid.channel.active,
        channel_empowered: druid.channel.empowered,
    }
}

/// The player's own state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerView {
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Damage absorbed before health moves: every shield on them, from
    /// [`Simulation::absorb_of`]. Drawn as a layer of its own on every health bar.
    pub absorb: f32,
    /// Current Heat.
    pub heat: f32,
    /// Which band Heat is in, which is what the bar colours itself from.
    pub heat_tier: HeatTier,
    /// Seconds of Inferno State left. Zero when it is not running.
    pub inferno_remaining: f32,
    /// Current Ultimate Energy.
    pub energy: f32,
    /// Whether the Ultimate can be cast.
    pub ultimate_ready: bool,
    /// Whether Flame Shield is up.
    pub shielded: bool,
    /// Whether the class is hasted above its base.
    pub hasted: bool,
    /// Which stat Fire Mastery specialises into.
    pub mastery: FireMastery,
    /// Whether Fire Mastery's active is running.
    pub mastery_active: bool,
    /// Statuses on the player.
    pub statuses: StatusList,
    /// Every buff and debuff on the player, class effects included: what the buff bar and
    /// the debuffs under the player frame draw.
    pub auras: crate::aura::AuraList,
    /// Whether the player is alive.
    ///
    /// Its own field rather than `health > 0`: what "dead" means is the simulation's to
    /// decide, and a future downed-but-not-out state would be a change there and nowhere in
    /// the interface.
    pub alive: bool,
    /// Damage taken this fight, which is a different number from the damage dealt.
    pub damage_taken: f32,
    /// The Warrior's readouts. Inert for every other hero.
    pub warrior: WarriorView,
    /// The Druid's readouts. Inert for every other hero.
    pub druid: DruidView,
}

impl PlayerView {
    /// Health as a fraction, for a bar.
    #[must_use]
    pub fn health_fraction(&self) -> f32 {
        if self.max_health <= 0.0 {
            return 0.0;
        }
        (self.health / self.max_health).clamp(0.0, 1.0)
    }

    /// Heat as a fraction, for a bar.
    #[must_use]
    pub fn heat_fraction(&self) -> f32 {
        (self.heat / HEAT_MAXIMUM).clamp(0.0, 1.0)
    }

    /// Whether Heat has reached the second band, where it hands over a stat.
    #[must_use]
    pub fn heat_is_high(&self) -> bool {
        self.heat >= HEAT_HOT_THRESHOLD
    }

    /// Whether the Inferno State is running.
    #[must_use]
    pub fn in_inferno(&self) -> bool {
        matches!(self.heat_tier, HeatTier::Inferno)
    }

    /// Where the two band markers sit on the bar, as fractions of its width.
    ///
    /// Computed here so the bar cannot draw a threshold the simulation does not have.
    #[must_use]
    pub fn heat_thresholds(&self) -> [f32; 2] {
        [
            HEAT_WARM_THRESHOLD / HEAT_MAXIMUM,
            HEAT_HOT_THRESHOLD / HEAT_MAXIMUM,
        ]
    }

    /// Ultimate Energy as a fraction, for a bar.
    #[must_use]
    pub fn energy_fraction(&self) -> f32 {
        (self.energy / ENERGY_MAXIMUM).clamp(0.0, 1.0)
    }
}

/// The selected enemy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetView {
    /// Which entity.
    pub entity: EntityId,
    /// What it is.
    pub kind: EntityKind,
    /// What it is called, as its table spells it: a monster's own name, or the entity kind for
    /// anything without one. Upper case; the frames print it in title case.
    pub name: &'static str,
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Damage absorbed before health moves: every shield on them, from
    /// [`Simulation::absorb_of`]. Drawn as a layer of its own on every health bar.
    pub absorb: f32,
    /// Metres away.
    pub distance: f32,
    /// Whether it is alive.
    pub alive: bool,
    /// Ignite stacks on it.
    pub ignite_stacks: u8,
    /// Seconds of Ignite left.
    pub ignite_remaining: f32,
    /// Whether it is at the current maximum - the Detonate requirement.
    pub ignite_at_maximum: bool,
    /// Statuses on it.
    pub statuses: StatusList,
    /// What it is casting, if it is an enemy and it is casting.
    ///
    /// The player's cast bar and this one are the same shape, because they are the same
    /// question asked about two actors.
    pub casting: Option<EnemyCastView>,
    /// What its AI is doing, for the developer readout. `None` if it has no brain.
    pub ai_state: Option<AiState>,
    /// Which phase it is in and what that phase is called, if it has more than one.
    ///
    /// `None` for everything that does not change while it is being fought, which is most of
    /// the roster — so the frame draws a phase line for a boss and an elite and for nothing
    /// else, without any widget knowing which monsters those are.
    pub phase: Option<(usize, &'static str)>,
    /// Whether it is a boss, so a frame can say so without knowing the roster.
    pub boss: bool,
    /// Every buff and debuff on it, stacks and clocks included: what the target's buff and
    /// debuff blocks draw, read the same way its nameplate reads them.
    pub auras: crate::aura::AuraList,
}

/// An enemy's cast, for its cast bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnemyCastView {
    /// Which attack.
    pub ability: EnemyAbility,
    /// Its name.
    pub name: &'static str,
    /// Seconds spent.
    pub elapsed: f32,
    /// Seconds it takes.
    pub duration: f32,
    /// Whether the player could interrupt it.
    pub interruptible: bool,
}

impl EnemyCastView {
    /// How far through it is, in `0.0..=1.0`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        (self.elapsed / self.duration).clamp(0.0, 1.0)
    }

    /// Seconds left.
    #[must_use]
    pub fn remaining(&self) -> f32 {
        (self.duration - self.elapsed).max(0.0)
    }
}

impl TargetView {
    /// Health as a fraction, for a bar.
    #[must_use]
    pub fn health_fraction(&self) -> f32 {
        if self.max_health <= 0.0 {
            return 0.0;
        }
        (self.health / self.max_health).clamp(0.0, 1.0)
    }

    /// Health as a whole percentage, for a readout.
    #[must_use]
    pub fn health_percent(&self) -> u32 {
        (self.health_fraction() * 100.0).round() as u32
    }
}

/// Why an ability button looks the way it does.
///
/// The UI picks a colour from this and never works it out itself, which is what stops a
/// button from claiming an ability is ready when the simulation would refuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotAvailability {
    /// Ready to cast.
    Ready,
    /// Cooling down, or out of charges.
    Cooling,
    /// Not enough Ultimate Energy.
    NoResource,
    /// The target cannot be hit, or is not there.
    InvalidTarget,
    /// Ready *and* worth pressing right now: Detonate on a full burn.
    Empowered,
    /// Switched off entirely: the Ultimate, which has no design yet.
    Disabled,
}

impl SlotAvailability {
    /// Whether the ability can be cast right now.
    #[must_use]
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready | Self::Empowered)
    }
}

/// One ability button's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AbilitySlotView {
    /// Which ability.
    pub ability: FireAbility,
    /// Its identity in the event stream.
    pub id: AbilityId,
    /// Its name.
    pub name: &'static str,
    /// The key that casts it.
    pub key: &'static str,
    /// What has to be held with that key, when the slot is bound to a combination.
    ///
    /// Kept beside the key's name rather than folded into it because the name is `'static`
    /// and a combination's is not: the two are joined where the slot is drawn, in a buffer
    /// that is already there for the cooldown numbers.
    pub key_modifiers: Modifiers,
    /// Why it looks the way it does.
    pub availability: SlotAvailability,
    /// Charges ready now.
    pub charges: u8,
    /// Charges it holds when full.
    pub max_charges: u8,
    /// Seconds until the next charge.
    pub cooldown_remaining: f32,
    /// How far through the recharge it is, in `0.0..=1.0`. One is ready.
    pub cooldown_fraction: f32,
    /// How far through the global cooldown we are, in `0.0..=1.0`. One means it is not
    /// running.
    ///
    /// The same value on every slot, because the global cooldown *is* the same on every
    /// slot: starting any ability locks the whole bar. It is carried per-slot rather than
    /// once on the snapshot so that a button has everything it needs to draw itself, which
    /// is the rule the rest of this view already follows.
    pub global_cooldown_fraction: f32,
    /// Whether this is the ability currently being cast.
    pub casting: bool,
    /// Whether this is the ability whose area the player is placing.
    ///
    /// Unlike everything else on this view, aiming is not a fact about the simulation — a
    /// placement that has not been confirmed has changed nothing and costs nothing. It is
    /// carried here anyway, because a button that has to be told separately what it is
    /// showing is a button that can be told wrong.
    pub aiming: bool,
    /// Whether the player has to place it with the mouse.
    pub ground_targeted: bool,
    /// What the cast takes right now, haste included. Zero for an instant.
    pub cast_time: f32,
    /// What it hits for right now, before Heat, critical strikes and Ignite scaling.
    ///
    /// Computed from the ability's coefficient and the mage's current spell power, so a
    /// tooltip prints a number the ability will actually deal rather than a percentage the
    /// player has to do arithmetic on.
    pub damage: f32,
    /// Extra damage per Ignite stack on the target, as a fraction. Zero if it has none.
    pub ignite_scaling: f32,
    /// Ignite stacks it applies to everything it hits.
    pub ignite_stacks: u8,
    /// Chance those stacks land, for the abilities that roll for it.
    pub ignite_chance: f32,
    /// Deflection this ability spends to be empowered. Zero if it has no empowered form.
    pub deflection_cost: f32,
    /// Whether pressing it **now** will spend that Deflection.
    ///
    /// Its own field rather than left to the button to work out from the cost and the
    /// resource: "this press will cost you twenty" is a fact about the simulation, and a
    /// button that computes it is a button that can be wrong about it.
    pub spends_deflection: bool,
    /// Mana one cast costs. Zero for everything that costs none.
    pub mana_cost: f32,
    /// Astral Power one successful cast generates.
    pub astral_generation: f32,
    /// Whether Astral Power can empower it at all. What the tooltip's Astral line is for.
    pub astral_empowerable: bool,
    /// **Whether pressing it now will be empowered.**
    ///
    /// Armed, and compatible. The same field `spends_deflection` is for the Warrior, and it
    /// drives the same highlight — one visual language for "this press buys something
    /// extra", whichever class is holding the bar.
    pub spends_astral: bool,
    /// What one healing event of this ability restores right now, at the current spell
    /// power and *without* the empowerment. Zero for an ability that heals nothing.
    pub healing: f32,
}

impl AbilitySlotView {
    /// Whether the button should show a charge counter.
    #[must_use]
    pub fn shows_charges(&self) -> bool {
        self.max_charges > 1
    }

    /// Whether a radial cooldown should be drawn over it.
    #[must_use]
    pub fn is_cooling(&self) -> bool {
        self.cooldown_remaining > 0.0 && self.charges < self.max_charges
    }

    /// Whether the global cooldown is running.
    #[must_use]
    pub fn is_on_global_cooldown(&self) -> bool {
        self.global_cooldown_fraction < 1.0
    }

    /// Whether the button should be framed as the one the player is in the middle of.
    ///
    /// Casting it, or choosing where to put it. Both are "this is the ability happening
    /// right now", and the player has no reason to tell the two states apart on the bar:
    /// aiming a Meteor and casting a Pyroblast are the same answer to "what am I doing".
    #[must_use]
    pub fn shows_active_frame(&self) -> bool {
        self.casting || self.aiming
    }

    /// Whether the button should draw the global cooldown's sweep.
    ///
    /// Only when its own cooldown is not already being drawn. Both are a translucent wash
    /// over the same button, so drawing them together would darken the overlap twice and
    /// make a slot that is merely waiting look different from its neighbours for no reason.
    /// The longer sweep is the one that matters, and it is the one that survives.
    #[must_use]
    pub fn shows_global_cooldown(&self) -> bool {
        self.is_on_global_cooldown() && !self.is_cooling()
    }
}

/// The cast in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CastView {
    /// Which ability.
    pub ability: FireAbility,
    /// Its name.
    pub name: &'static str,
    /// Seconds spent.
    pub elapsed: f32,
    /// Seconds it takes.
    pub duration: f32,
}

impl CastView {
    /// How far through it is, in `0.0..=1.0`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        (self.elapsed / self.duration).clamp(0.0, 1.0)
    }

    /// Seconds left.
    #[must_use]
    pub fn remaining(&self) -> f32 {
        (self.duration - self.elapsed).max(0.0)
    }
}

/// The fight so far.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CombatView {
    /// Damage dealt.
    pub damage: f32,
    /// Damage per second.
    pub dps: f32,
    /// Seconds in combat.
    pub seconds: f32,
}

/// Something the interface should say.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct NotificationView {
    /// The last refusal, if there was one.
    pub error: Option<AbilityError>,
}

/// The talent grid, as the menu sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TalentView {
    /// Which talent.
    pub id: TalentId,
    /// Its name.
    pub name: &'static str,
    /// Which tier it sits in.
    pub tier: u8,
    /// What it costs.
    pub cost: u32,
    /// What it does.
    pub description: &'static str,
    /// Whether it is owned.
    pub purchased: bool,
    /// Whether it could be bought right now.
    pub affordable: bool,
}

/// One party member, as their frame shows them.
///
/// Health and nothing invented. There are no character names in the world yet — an entity
/// has a kind and a body — so a frame prints what the game actually knows rather than a name
/// the gameplay systems could not supply.
#[derive(Debug, Clone, PartialEq)]
pub struct PartyMemberView {
    /// Which entity.
    pub entity: EntityId,
    /// What they are, which is the closest thing to a name the world has.
    pub kind: EntityKind,
    /// Their own name, when the world gave them one (another player).
    pub label: Option<&'static str>,
    /// Current health.
    pub health: f32,
    /// Maximum health.
    pub max_health: f32,
    /// Damage absorbed before health moves: every shield on them, from
    /// [`Simulation::absorb_of`]. Drawn as a layer of its own on every health bar.
    pub absorb: f32,
    /// Whether they are alive.
    pub alive: bool,
    /// Whether this is the local player, who is always the first member.
    pub is_player: bool,
    /// What is on them.
    pub statuses: StatusList,
    /// Every buff and debuff on them: the party frame and the raid cells show the debuffs.
    pub auras: crate::aura::AuraList,
}

impl PartyMemberView {
    /// Health as a fraction of maximum, zero when there is no maximum to divide by.
    #[must_use]
    pub fn health_fraction(&self) -> f32 {
        if self.max_health <= 0.0 {
            0.0
        } else {
            (self.health / self.max_health).clamp(0.0, 1.0)
        }
    }

    /// The name the frame prints.
    #[must_use]
    pub fn name(&self) -> &'static str {
        if self.is_player {
            "YOU"
        } else {
            self.label.unwrap_or_else(|| self.kind.as_str())
        }
    }
}

/// The character sheet, as the interface sees it.
///
/// Every number here was computed by [`engine_game::character::CharacterSheet`]; nothing in
/// the panel adds a stat up. That is the same rule the rest of this file follows, and it is
/// what makes "the sheet and combat agree" structural rather than a thing to remember: there
/// is one aggregation and the panel is downstream of it.
///
/// `Copy`, and the two item lists are fixed-width arrays, so refreshing this allocates
/// nothing — a character sheet is a few hundred bytes memcopied once a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CharacterView {
    /// Which hero this sheet is for.
    pub hero: Hero,
    /// Which stat is this hero's main stat.
    pub main_stat: Stat,
    /// The hero's own stats, before anything is worn.
    pub base: StatBlock,
    /// What the worn items add up to.
    pub gear: StatBlock,
    /// **The total**: base, plus gear, plus the Spirit buff. What the panel prints.
    pub total: StatBlock,
    /// Maximum health, derived from Stamina.
    pub max_health: f32,
    /// What is worn, indexed by [`EquipSlot::index`].
    pub equipment: [Option<ItemStack>; EquipSlot::COUNT],
    /// What is carried, in grid order.
    pub inventory: [Option<ItemStack>; INVENTORY_SLOTS],
    /// How many stacks of the Spirit buff are running. Zero on the current roster.
    pub spirit_stacks: u8,
}

impl CharacterView {
    /// What is in `slot`.
    #[must_use]
    pub fn at(&self, slot: ItemSlot) -> Option<ItemStack> {
        match slot {
            ItemSlot::Equipment(worn) => self.equipment[worn.index()],
            ItemSlot::Inventory(index) => self.inventory.get(index).copied().flatten(),
        }
    }

    /// Whether `stack` may be dropped in `slot`.
    ///
    /// The same question [`engine_game::character::CharacterSheet::accepts`] answers, asked
    /// against the same item data. The interface asks it to colour a drop target while the
    /// item is still in the air; the simulation asks it again when the move arrives, and it
    /// is the simulation's answer that decides anything.
    #[must_use]
    pub fn accepts(&self, slot: ItemSlot, stack: ItemStack) -> bool {
        match slot {
            ItemSlot::Equipment(worn) => stack.fits(worn),
            ItemSlot::Inventory(index) => index < self.inventory.len(),
        }
    }

    /// Whether moving what is in `from` to `to` would be allowed.
    ///
    /// Both ends, because a swap sends the displaced item the other way: a ring dropped on a
    /// worn cloak would put the cloak on a finger, and the whole exchange is refused.
    #[must_use]
    pub fn can_move(&self, from: ItemSlot, to: ItemSlot) -> bool {
        if from == to {
            return false;
        }
        let Some(moving) = self.at(from) else {
            return false;
        };
        if !self.accepts(to, moving) {
            return false;
        }
        match self.at(to) {
            Some(displaced) => self.accepts(from, displaced),
            None => true,
        }
    }
}

/// Everything the interface may read, rebuilt every frame.
#[derive(Debug, Clone)]
pub struct UiSnapshot {
    /// The player.
    pub player: PlayerView,
    /// The selected enemy, if any.
    pub target: Option<TargetView>,
    /// What the selected enemy is fighting, if anything.
    pub target_of_target: Option<TargetView>,
    /// The party, local player first. Rebuilt in place; never reallocated after the first
    /// frame that needed the capacity.
    pub party: Vec<PartyMemberView>,
    /// Which hero the player is. What the swap menu ticks, and what the bar is built from.
    pub hero: Hero,
    /// The character sheet: stats, what is worn and what is carried.
    pub character: CharacterView,
    /// The ability bar, in bar order. `None` in a slot the current hero does not fill.
    ///
    /// An option per slot rather than a shorter array: slot four is the `4` key whichever
    /// hero is holding it, and a bar that renumbered itself per class would move a player's
    /// muscle memory every time they swapped.
    pub abilities: [Option<AbilitySlotView>; ABILITY_SLOTS],
    /// The Ultimate, for a hero that has one.
    pub ultimate: Option<AbilitySlotView>,
    /// The buttons beside the bar, for a hero with more abilities than digits.
    ///
    /// Two at most, in the hero's own order, on `F` and `G` — see
    /// [`engine_game::hero::Hero::extras`]. Empty for the FireMage and the Warrior.
    pub extras: [Option<AbilitySlotView>; 2],
    /// The cast in progress.
    pub cast: Option<CastView>,
    /// The fight so far.
    pub combat: CombatView,
    /// Anything the interface should say.
    pub notification: NotificationView,
    /// Talent points left to spend.
    pub talent_points: u32,
    /// The whole talent grid.
    pub talents: Vec<TalentView>,
    /// The mage's current spell power, which every tooltip number is a percentage of.
    pub spell_power: f32,
    /// Current haste, as a fraction.
    pub haste: f32,
    /// Current critical strike chance, as a fraction.
    pub crit_chance: f32,
    /// The current maximum Ignite stacks. Read by every tooltip that mentions it.
    pub ignite_max: u8,
    /// The dungeon run, if the level is a dungeon: what the progress panel draws.
    pub dungeon: Option<DungeonView>,
}

/// A dungeon run, as the progress panel reads it.
///
/// Read off [`engine_game::dungeon_run::DungeonRun`] every frame, never kept: the run is the
/// one truth about how complete the dungeon is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DungeonView {
    /// Its name. Rewritten in place every frame, so it allocates once.
    pub name: String,
    /// Completion as a fraction of the requirement, `[0, 1]`.
    pub fraction: f32,
    /// Completion as a whole percentage, rounded down.
    pub percent: u32,
    /// Whether the requirement is met and the boss's gate is open.
    pub unlocked: bool,
    /// Whether the boss fight is running.
    pub boss_engaged: bool,
    /// How many of the boss's adds are still standing.
    pub adds_alive: usize,
    /// Whether the boss and every add are dead.
    pub completed: bool,
    /// How long the run took, once it is over.
    pub finished_after: Option<f32>,
}

impl UiSnapshot {
    /// The button state of one ability, whichever slot it is in and whether it is the
    /// Ultimate.
    ///
    /// The lookup an action bar does: a slot holds an *ability*, and the view for it is
    /// whatever the snapshot has for that ability. `None` covers both "nothing there" and
    /// "the current hero cannot cast that", which are the same thing to a button — an
    /// assignment left behind by another hero draws as an empty slot rather than as
    /// somebody else's spell.
    #[must_use]
    pub fn ability_view(&self, ability: FireAbility) -> Option<&AbilitySlotView> {
        self.abilities
            .iter()
            .flatten()
            .chain(self.extras.iter().flatten())
            .chain(self.ultimate.iter())
            .find(|view| view.ability == ability)
    }

    /// Reads the simulation.
    #[must_use]
    pub fn build(simulation: &Simulation) -> Self {
        Self::build_aiming(simulation, None)
    }

    /// Reads the simulation, with `aiming` naming the ability whose area is being placed.
    #[must_use]
    pub fn build_aiming(simulation: &Simulation, aiming: Option<FireAbility>) -> Self {
        let mut snapshot = Self {
            player: PlayerView {
                health: 0.0,
                max_health: 0.0,
                absorb: 0.0,
                warrior: WarriorView::default(),
                heat: 0.0,
                heat_tier: HeatTier::Cold,
                inferno_remaining: 0.0,
                energy: 0.0,
                ultimate_ready: false,
                shielded: false,
                hasted: false,
                mastery: FireMastery::Haste,
                mastery_active: false,
                statuses: StatusList::default(),
                auras: crate::aura::AuraList::default(),
                alive: true,
                damage_taken: 0.0,
                druid: DruidView::default(),
            },
            target: None,
            target_of_target: None,
            party: Vec::with_capacity(engine_game::party::MAX_PARTY),
            hero: simulation.hero(),
            character: character_view(simulation),
            abilities: [None; ABILITY_SLOTS],
            ultimate: None,
            extras: [None; 2],
            cast: None,
            combat: CombatView::default(),
            notification: NotificationView::default(),
            talent_points: 0,
            talents: Vec::with_capacity(TALENTS.len()),
            spell_power: 0.0,
            haste: 0.0,
            crit_chance: 0.0,
            ignite_max: 5,
            dungeon: None,
        };
        snapshot.refresh(simulation, aiming);
        snapshot
    }

    /// Rebuilds in place, keeping every allocation.
    ///
    /// `aiming` is the ability whose area the player is currently placing, which is the one
    /// piece of this the simulation cannot answer: an unconfirmed placement has changed
    /// nothing. It is a parameter rather than a field the caller sets afterwards, so it
    /// cannot be left behind from the frame before.
    pub fn refresh(&mut self, simulation: &Simulation, aiming: Option<FireAbility>) {
        let mage = simulation.mage();
        let player = simulation.player();
        let world = simulation.world();

        let health = world.health(player.entity);
        self.player = PlayerView {
            health: health.map_or(0.0, |health| health.current()),
            max_health: health.map_or(0.0, |health| health.max()),
            absorb: simulation.absorb_of(player.entity),
            heat: mage.heat.value(),
            heat_tier: mage.heat.tier(),
            inferno_remaining: mage.heat.inferno_remaining(),
            energy: mage.energy.value(),
            ultimate_ready: mage.energy.is_full() && ULTIMATE_ENABLED,
            shielded: mage.flame_shield.is_active(),
            hasted: mage.haste() > engine_game::firemage::BASE_HASTE,
            mastery: mage.mastery,
            mastery_active: mage.mastery_buff.is_active(),
            statuses: statuses_of(simulation, player.entity),
            auras: crate::aura::auras_of(simulation, player.entity),
            alive: simulation.player_alive(),
            damage_taken: simulation.meter().total_damage_taken() as f32,
            warrior: warrior_view(simulation),
            druid: druid_view(simulation),
        };

        self.target = player
            .target
            .and_then(|entity| target_view(simulation, entity));
        // The target's target, read from whatever the selected enemy has decided to attack.
        // Nothing new in gameplay answers this: an enemy brain already knows who it is
        // fighting, and a chain of two lookups is the whole feature.
        self.target_of_target = player
            .target
            .and_then(|entity| simulation.brain(entity))
            .and_then(engine_game::enemy::brain::EnemyBrain::target)
            .and_then(|entity| target_view(simulation, entity));

        match simulation.dungeon_run() {
            Some(run) => {
                let view = self.dungeon.get_or_insert_with(DungeonView::default);
                view.name.clear();
                view.name.push_str(run.name());
                view.fraction = run.fraction();
                view.percent = run.percent();
                view.unlocked = run.is_unlocked();
                view.boss_engaged = run.is_engaged();
                view.adds_alive = run
                    .adds()
                    .iter()
                    .filter(|add| world.is_alive(**add))
                    .count();
                view.completed = run.is_completed();
                view.finished_after = run.finished_after();
            }
            None => self.dungeon = None,
        }

        self.party.clear();
        for (index, member) in simulation.party().members().iter().enumerate() {
            let Some(health) = world.health(*member) else {
                continue;
            };
            self.party.push(PartyMemberView {
                entity: *member,
                kind: world.kind(*member).unwrap_or(EntityKind::Player),
                label: world.label(*member),
                health: health.current(),
                max_health: health.max(),
                absorb: simulation.absorb_of(*member),
                alive: world.is_alive(*member),
                is_player: index == 0,
                statuses: statuses_of(simulation, *member),
                auras: crate::aura::auras_of(simulation, *member),
            });
        }

        self.hero = simulation.hero();
        self.character = character_view(simulation);
        for (slot, ability) in simulation.hero().bar().into_iter().enumerate() {
            self.abilities[slot] = ability.map(|ability| ability_view(simulation, ability, aiming));
        }
        self.ultimate = simulation
            .hero()
            .ultimate()
            .map(|ability| ability_view(simulation, ability, aiming));
        self.extras = [None; 2];
        for (index, ability) in simulation.hero().extras().enumerate().take(2) {
            self.extras[index] = Some(ability_view(simulation, ability, aiming));
        }

        self.cast = simulation.casting().map(|cast| CastView {
            ability: cast.ability,
            name: cast.ability.definition().name,
            elapsed: cast.elapsed,
            duration: cast.duration,
        });

        let meter = simulation.meter();
        self.combat = CombatView {
            damage: meter.total_damage() as f32,
            dps: meter.dps() as f32,
            seconds: meter.combat_seconds() as f32,
        };
        // The simulation decides how long a refusal is worth showing, not the interface.
        self.notification = NotificationView {
            error: simulation.recent_error(),
        };

        self.talent_points = mage.talents.available_points();
        self.talents.clear();
        for talent in &TALENTS {
            self.talents.push(TalentView {
                id: talent.id,
                name: talent.name,
                tier: talent.tier,
                cost: talent.cost,
                description: talent.description,
                purchased: mage.talents.has(talent.id),
                affordable: mage.talents.can_purchase(talent.id),
            });
        }
        let stats = mage.stats();
        self.spell_power = stats.spell_power;
        self.haste = stats.haste;
        self.crit_chance = stats.crit();
        self.ignite_max = mage.ignite_max();
    }
}

/// One entity as a frame shows it: the target, and the target's target.
///
/// One function for both, because "the thing the frame is looking at" is the same question
/// twice. A target that is the player — an enemy fighting back — reads out of the same world
/// as any other entity, so nothing here needs a special case for it.
fn target_view(simulation: &Simulation, entity: EntityId) -> Option<TargetView> {
    let world = simulation.world();
    let mage = simulation.mage();
    let player = simulation.player();
    let health = world.health(entity)?;
    let burn = mage.ignite.get(entity);
    let kind = world.kind(entity)?;
    Some(TargetView {
        entity,
        kind,
        name: simulation
            .brain(entity)
            .map(|brain| brain.kind().name())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| {
                if entity == player.entity {
                    "YOU"
                } else {
                    world.label(entity).unwrap_or_else(|| kind.as_str())
                }
            }),
        health: health.current(),
        max_health: health.max(),
        absorb: simulation.absorb_of(entity),
        // No distance means the target is gone; a large number reads as "out of range"
        // everywhere it is used rather than as "right next to you".
        distance: world
            .distance_between(player.entity, entity)
            .unwrap_or(f32::MAX),
        alive: world.is_alive(entity),
        ignite_stacks: burn.stacks,
        ignite_remaining: burn.remaining,
        ignite_at_maximum: burn.is_at_maximum(mage.ignite_max()),
        statuses: statuses_of(simulation, entity),
        casting: simulation.brain(entity).and_then(|brain| {
            brain.cast().map(|cast| EnemyCastView {
                ability: cast.ability,
                name: cast.ability.name(),
                elapsed: cast.elapsed,
                duration: cast.duration,
                interruptible: cast.interruptible,
            })
        }),
        ai_state: simulation.brain(entity).map(|brain| brain.state()),
        phase: simulation.brain(entity).and_then(|brain| {
            (brain.archetype().phase_count() > 1).then(|| (brain.phase() + 1, brain.phase_name()))
        }),
        boss: simulation
            .brain(entity)
            .is_some_and(|brain| brain.kind().is_boss()),
        auras: crate::aura::auras_of(simulation, entity),
    })
}

/// One ability's button state, worked out once here rather than in the widget.
fn ability_view(
    simulation: &Simulation,
    ability: FireAbility,
    aiming: Option<FireAbility>,
) -> AbilitySlotView {
    let mage = simulation.mage();
    let definition = ability.definition();
    let effects = mage.effects();
    let stats = mage.stats();
    let charges = mage.abilities.charges(ability);
    let max_charges = definition.charges_with(effects);

    let target = simulation.player().target;
    let target_alive = target.is_some_and(|id| simulation.world().is_alive(id));
    // A friendly selection — an ally, or the player themselves — is a legal thing to have
    // and an illegal thing to point a Firebolt at. The bar says so before the click rather
    // than after it: a button that looks ready and answers "THAT IS NOT AN ENEMY" has taught
    // the player nothing they could not have been told.
    let target_friendly = target.is_some_and(|id| !simulation.is_attackable(id));
    let in_range = target.and_then(|id| {
        simulation
            .world()
            .distance_between(simulation.player().entity, id)
    });

    // Whether pressing this now spends twenty Deflection. Asked of the simulation's own
    // resource, so the highlight on the button and the spend in the executor are the same
    // question answered once.
    let deflection = simulation.warrior().deflection.value();
    let spends_deflection = simulation.hero() == Hero::Warrior
        && definition.warrior().is_empowerable()
        && deflection + f32::EPSILON >= definition.warrior().empower_threshold();

    // Whether pressing this now spends a hundred Astral Power. Asked of the simulation's own
    // resource, exactly as the Warrior's spend is.
    let spends_astral = simulation.astral_armed() && definition.druid().astral_empowerable;
    // Mana, asked of the pool rather than of the number on the row: a spell the player cannot
    // afford is dimmed before it is pressed rather than after.
    let short_of_mana = simulation.hero() == Hero::Druid
        && definition.druid().mana_cost > 0.0
        && !simulation
            .druid()
            .mana
            .can_pay(definition.druid().mana_cost);
    // The Astral Power button has a requirement of its own, and it is not a cooldown: a bar
    // that is not full, or one that is already armed, cannot be pressed.
    let cannot_arm = ability == FireAbility::AstralPower && !simulation.druid().astral.can_arm();

    let availability = if ability.is_disabled() {
        // Cataclysm has no gameplay concept yet. The button stays where it is and says so,
        // rather than disappearing and taking its architecture with it.
        SlotAvailability::Disabled
    } else if (definition.fire().energy_cost > 0.0 && !mage.energy.is_full())
        || short_of_mana
        || cannot_arm
    {
        // Three ways to be short of a resource and one way to say so: the Ultimate's energy,
        // the Druid's Mana, and an Astral Power button that has nothing to arm with.
        SlotAvailability::NoResource
    } else if charges == 0 {
        SlotAvailability::Cooling
    } else if (target_friendly && !ability.usable_on_friendly_target())
        || (definition.target.needs_target()
            && (!target_alive || in_range.is_some_and(|distance| distance > definition.range)))
    {
        // Three ways to have the wrong target and one way to say so: it is friendly and this
        // is not a spell that can ignore that, it is dead or gone, or it is out of reach.
        SlotAvailability::InvalidTarget
    } else if ability == FireAbility::Detonate {
        // The one ability whose readiness is about the *target*: a full burn is what it is
        // for, and the bar says so rather than making the player count stacks.
        match target {
            Some(id) if mage.ignite.is_at_maximum(id, mage.ignite_max()) => {
                SlotAvailability::Empowered
            }
            _ => SlotAvailability::InvalidTarget,
        }
    } else if spends_deflection || spends_astral {
        // **The empowered highlight.** The same state Detonate-on-a-full-burn already uses,
        // reached by a second class for its own reason: it is ready, and pressing it right
        // now buys something extra. One state, one colour on the bar, and no second visual
        // language for the Warrior.
        SlotAvailability::Empowered
    } else {
        SlotAvailability::Ready
    };

    AbilitySlotView {
        ability,
        id: AbilityId::from_fire(ability),
        name: definition.name,
        key: ability.key_hint(),
        key_modifiers: Modifiers::none(),
        availability,
        charges,
        max_charges,
        cooldown_remaining: mage.abilities.recharge_remaining(ability),
        cooldown_fraction: mage.abilities.recharge_fraction(ability, effects),
        global_cooldown_fraction: mage.global_cooldown_fraction_for(ability),
        casting: simulation
            .casting()
            .is_some_and(|cast| cast.ability == ability),
        aiming: aiming == Some(ability),
        ground_targeted: definition.target.is_ground_targeted(),
        cast_time: definition.cast_time_with(&stats),
        damage: if definition.target.is_friendly() {
            0.0
        } else if definition.coefficient > 0.0 {
            definition.base_damage(&stats)
        } else {
            definition.base_tick_damage(&stats)
        },
        ignite_scaling: definition.fire().ignite_bonus_per_stack,
        ignite_stacks: definition.fire().ignite_stacks,
        ignite_chance: definition.fire().ignite_chance,
        deflection_cost: definition.warrior().deflection_cost,
        spends_deflection,
        mana_cost: definition.druid().mana_cost,
        astral_generation: definition.druid().astral_generation,
        astral_empowerable: definition.druid().astral_empowerable,
        spends_astral,
        // Healing and damage are the same coefficient read against the same spell power;
        // which of the two a row *is* is decided by who it is pointed at, so the interface
        // never has to guess whether a number is a heal.
        healing: if definition.target.is_friendly() {
            let direct = definition.base_damage(&stats);
            if direct > 0.0 {
                direct
            } else {
                definition.base_tick_damage(&stats)
            }
        } else {
            0.0
        },
    }
}

/// The statuses on one entity, capped at what a plate can show.
fn statuses_of(simulation: &Simulation, entity: EntityId) -> StatusList {
    let mut list = StatusList::default();
    if let Some(statuses) = simulation.world().statuses(entity) {
        for status in statuses.iter() {
            list.push(StatusView {
                kind: status.kind,
                stacks: status.stacks,
                remaining: status.remaining,
                fraction: status.fraction(),
            });
        }
    }
    list
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::arena::{self, ArenaConfig};

    fn arena() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation.select_nearest_target();
        simulation
    }

    /// The view in bar slot `slot`, which the mage's bar fills all ten of.
    fn filled(snapshot: &UiSnapshot, slot: usize) -> AbilitySlotView {
        snapshot.abilities[slot].expect("the mage fills every slot of its bar")
    }

    #[test]
    fn selecting_yourself_dims_exactly_the_spells_that_need_an_enemy() {
        // The bar's answer to a friendly selection, checked ability by ability. What may
        // still be pressed is what does not point at the selection: the aimed ones, the
        // self-buffs and the movement.
        let mut simulation = arena();
        simulation.push_command(engine_game::command::PlayerCommand::SelectSelf);
        simulation.step_once();
        let snapshot = UiSnapshot::build(&simulation);

        let availability = |ability: FireAbility| {
            snapshot
                .ability_view(ability)
                .expect("a bar ability")
                .availability
        };
        for ability in [
            FireAbility::Firebolt,
            FireAbility::FlameLance,
            FireAbility::Pyroblast,
            FireAbility::Detonate,
        ] {
            assert_eq!(
                availability(ability),
                SlotAvailability::InvalidTarget,
                "{} points at the selection and should be dimmed",
                ability.definition().name
            );
        }
        for ability in [
            FireAbility::Blink,
            FireAbility::FlameShield,
            FireAbility::FireMastery,
            FireAbility::Meteor,
            FireAbility::Firestorm,
            FireAbility::FlameCone,
        ] {
            assert_ne!(
                availability(ability),
                SlotAvailability::InvalidTarget,
                "{} does not point at the selection and should stay usable",
                ability.definition().name
            );
        }
    }

    #[test]
    fn an_enemy_selection_leaves_the_single_target_spells_alone() {
        // The control for the test above: the same bar, with a dummy selected instead.
        let snapshot = UiSnapshot::build(&arena());
        assert_ne!(
            snapshot
                .ability_view(FireAbility::Firebolt)
                .expect("a bar ability")
                .availability,
            SlotAvailability::InvalidTarget
        );
    }

    #[test]
    fn the_druids_bar_is_its_own_and_its_resources_reach_the_interface() {
        let mut simulation = arena();
        assert!(simulation.set_hero(Hero::Druid));
        let snapshot = UiSnapshot::build(&simulation);

        let names: Vec<&str> = snapshot
            .abilities
            .iter()
            .flatten()
            .map(|slot| slot.name)
            .collect();
        assert_eq!(
            names,
            [
                "MOONFIRE",
                "SUNFIRE",
                "WRATH",
                "REJUVENATION",
                "REGROWTH",
                "LIFEBLOOM",
                "WILD GROWTH",
                "SWIFTMEND",
                "NOURISH",
                "NATURES CLEANSE",
            ]
        );
        let druid = snapshot.player.druid;
        assert!(druid.active);
        assert!((druid.mana_fraction() - 1.0).abs() < 1e-6);
        assert_eq!(druid.astral, 0.0);
        assert!(!druid.armed);
        assert!(!druid.can_arm);
        // And the Warrior's readouts are inert, which is what keeps one frame drawing both.
        assert!(!snapshot.player.warrior.active);
    }

    #[test]
    fn arming_astral_power_lights_up_exactly_the_compatible_abilities() {
        let mut simulation = arena();
        assert!(simulation.set_hero(Hero::Druid));
        simulation
            .druid_mut()
            .astral
            .gain(engine_game::druid::tuning::ASTRAL_MAXIMUM);
        // Full, and not yet pressed: nothing is empowered.
        let snapshot = UiSnapshot::build(&simulation);
        assert!(snapshot.player.druid.can_arm);
        assert!(!snapshot.player.druid.armed);
        assert!(
            !snapshot
                .ability_view(FireAbility::Moonfire)
                .expect("a bar ability")
                .spends_astral
        );

        simulation
            .cast(FireAbility::AstralPower)
            .expect("a full bar arms");
        let snapshot = UiSnapshot::build(&simulation);
        assert!(snapshot.player.druid.armed);
        for ability in [
            FireAbility::Moonfire,
            FireAbility::Sunfire,
            FireAbility::Wrath,
            FireAbility::Rejuvenation,
            FireAbility::Tranquility,
        ] {
            let view = snapshot.ability_view(ability).expect("a druid ability");
            assert!(view.spends_astral, "{ability:?} did not light up");
            assert_eq!(
                view.availability,
                SlotAvailability::Empowered,
                "{ability:?}"
            );
        }
        for ability in [FireAbility::NaturesCleanse, FireAbility::WildRush] {
            let view = snapshot.ability_view(ability).expect("a druid ability");
            assert!(!view.spends_astral, "{ability:?} claimed an empowerment");
        }
    }

    #[test]
    fn a_druid_with_no_mana_sees_the_spells_it_cannot_afford() {
        let mut simulation = arena();
        assert!(simulation.set_hero(Hero::Druid));
        simulation
            .druid_mut()
            .mana
            .pay(engine_game::druid::tuning::MANA_MAXIMUM);
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(
            snapshot
                .ability_view(FireAbility::Rejuvenation)
                .expect("a bar ability")
                .availability,
            SlotAvailability::NoResource
        );
        // Wrath costs nothing and is unaffected, which is the whole point of Wrath.
        assert_ne!(
            snapshot
                .ability_view(FireAbility::Wrath)
                .expect("a bar ability")
                .availability,
            SlotAvailability::NoResource
        );
    }

    #[test]
    fn the_bar_is_the_ten_designed_abilities_and_the_ultimate_is_separate() {
        let snapshot = UiSnapshot::build(&arena());
        let names: Vec<&str> = snapshot
            .abilities
            .iter()
            .flatten()
            .map(|slot| slot.name)
            .collect();
        assert_eq!(
            names,
            [
                "FIREBOLT",
                "FLAME LANCE",
                "FLAME CONE",
                "METEOR",
                "PYROBLAST",
                "FIRESTORM",
                "DETONATE",
                "FIRE MASTERY",
                "FLAME SHIELD",
                "BLINK",
            ]
        );
        assert_eq!(
            snapshot.ultimate.expect("the mage has an Ultimate").name,
            "CATACLYSM"
        );
        assert!(
            !names.contains(&"CATACLYSM"),
            "the ultimate is on the regular bar"
        );
    }

    #[test]
    fn charges_are_reported_only_where_there_are_several() {
        let snapshot = UiSnapshot::build(&arena());
        let lance = filled(&snapshot, 1);
        assert_eq!((lance.charges, lance.max_charges), (2, 2));
        assert!(lance.shows_charges());

        let firebolt = filled(&snapshot, 0);
        assert!(!firebolt.shows_charges(), "firebolt has one charge");
    }

    #[test]
    fn a_spent_ability_reports_its_cooldown_for_the_overlay() {
        let mut simulation = arena();
        simulation
            .cast(FireAbility::Blink)
            .expect("blink is instant");
        simulation.step_many(60);

        let snapshot = UiSnapshot::build(&simulation);
        let blink = filled(&snapshot, 9);
        assert_eq!(blink.charges, 0);
        assert!(blink.is_cooling());
        assert!(blink.cooldown_remaining > 0.0);
        assert!((0.0..1.0).contains(&blink.cooldown_fraction));
        assert_eq!(blink.availability, SlotAvailability::Cooling);
    }

    #[test]
    fn detonate_lights_up_only_against_a_full_burn() {
        let mut simulation = arena();
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(
            filled(&snapshot, 6).availability,
            SlotAvailability::InvalidTarget,
            "it offered itself against a cold target"
        );

        let target = simulation.player().target.expect("a dummy");
        let maximum = simulation.mage().ignite_max();
        let duration = simulation.mage().ignite_duration();
        simulation
            .mage_mut()
            .ignite
            .apply(target, maximum, maximum, duration);

        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(
            filled(&snapshot, 6).availability,
            SlotAvailability::Empowered
        );
        assert!(filled(&snapshot, 6).availability.is_ready());
        assert!(snapshot.target.expect("a target").ignite_at_maximum);
    }

    #[test]
    fn the_ultimate_reports_its_energy_while_it_is_switched_off() {
        let mut simulation = arena();
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(
            snapshot
                .ultimate
                .expect("the mage has an Ultimate")
                .availability,
            SlotAvailability::Disabled
        );
        assert!(!snapshot.player.ultimate_ready);

        simulation.mage_mut().gain_energy(100.0);
        let snapshot = UiSnapshot::build(&simulation);
        assert!((snapshot.player.energy_fraction() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_heat_bar_reports_its_bands_and_the_inferno_state() {
        let mut simulation = arena();
        simulation.mage_mut().gain_heat(40.0);
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(snapshot.player.heat_tier, HeatTier::Warm);
        assert!(!snapshot.player.heat_is_high());
        assert!((snapshot.player.heat_fraction() - 0.4).abs() < 1e-6);

        simulation.mage_mut().gain_heat(40.0);
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(snapshot.player.heat_tier, HeatTier::Hot);
        assert!(snapshot.player.heat_is_high());

        // The top of the bar spends it: the bar then reports the state, not the value.
        simulation.mage_mut().gain_heat(20.0);
        let snapshot = UiSnapshot::build(&simulation);
        assert!(snapshot.player.in_inferno());
        assert_eq!(snapshot.player.heat, 0.0);
        assert!(snapshot.player.inferno_remaining > 0.0);

        // Both band markers are on the bar, in order, and neither is at an end of it.
        let [warm, hot] = snapshot.player.heat_thresholds();
        assert!(0.0 < warm && warm < hot && hot < 1.0);
    }

    #[test]
    fn the_ultimate_button_is_present_and_switched_off() {
        // The design keeps the architecture and takes away the button's use, so the slot
        // must still exist and must never read as ready.
        let mut simulation = arena();
        simulation.mage_mut().gain_energy(100.0);
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(
            snapshot.ultimate.expect("the mage has an Ultimate").ability,
            FireAbility::Cataclysm
        );
        assert_eq!(
            snapshot
                .ultimate
                .expect("the mage has an Ultimate")
                .availability,
            SlotAvailability::Disabled
        );
        assert!(
            !snapshot
                .ultimate
                .expect("the mage has an Ultimate")
                .availability
                .is_ready()
        );
        assert!(!snapshot.player.ultimate_ready);
    }

    #[test]
    fn the_target_view_carries_the_ignite_stacks_and_the_current_maximum() {
        let mut simulation = arena();
        let target = simulation.player().target.expect("a dummy");
        let duration = simulation.mage().ignite_duration();
        simulation.mage_mut().ignite.apply(target, 3, 5, duration);

        let snapshot = UiSnapshot::build(&simulation);
        let view = snapshot.target.expect("a target");
        assert_eq!(view.ignite_stacks, 3);
        assert!(!view.ignite_at_maximum);
        assert_eq!(snapshot.ignite_max, 5);

        // And the maximum follows the talent, which is what the tooltips print.
        simulation
            .mage_mut()
            .purchase_talent(engine_game::firemage::TalentId::InfernalCapacity)
            .expect("testing mode pays");
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(snapshot.ignite_max, 6);
    }

    #[test]
    fn the_talent_grid_reports_what_is_owned_and_what_can_be_bought() {
        let mut simulation = arena();
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(snapshot.talents.len(), TALENTS.len());
        assert!(snapshot.talents.iter().all(|talent| !talent.purchased));
        assert_eq!(
            snapshot.talent_points,
            simulation.mage().talents.available_points()
        );

        let first = snapshot.talents[0].id;
        simulation
            .mage_mut()
            .purchase_talent(first)
            .expect("affordable");
        let snapshot = UiSnapshot::build(&simulation);
        assert!(snapshot.talents[0].purchased);
    }

    #[test]
    fn a_cast_in_progress_is_reported_with_its_progress() {
        let mut simulation = arena();
        simulation.cast(FireAbility::Firebolt).expect("in range");
        simulation.step_many(30);

        let snapshot = UiSnapshot::build(&simulation);
        let cast = snapshot.cast.expect("a cast is running");
        assert_eq!(cast.name, "FIREBOLT");
        assert!(cast.fraction() > 0.0 && cast.fraction() < 1.0);
        assert!(cast.remaining() > 0.0);
    }

    #[test]
    fn a_refusal_reaches_the_interface() {
        let mut simulation = arena();
        let _ = simulation.cast(FireAbility::Detonate);
        let snapshot = UiSnapshot::build(&simulation);
        assert_eq!(
            snapshot.notification.error,
            Some(AbilityError::NotEnoughIgnite)
        );
    }

    #[test]
    fn refreshing_reuses_the_snapshot_rather_than_rebuilding_it() {
        let simulation = arena();
        let mut snapshot = UiSnapshot::build(&simulation);
        let capacity = snapshot.talents.capacity();
        for _ in 0..100 {
            snapshot.refresh(&simulation, None);
        }
        assert_eq!(snapshot.talents.capacity(), capacity, "the snapshot grew");
        assert_eq!(snapshot.talents.len(), TALENTS.len());
    }
}
