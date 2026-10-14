//! Buffs and debuffs, as the interface shows them.
//!
//! ```text
//! StatusSet (engine-game)   ─┐
//! class timers (Flame Shield,│
//!   Shield Block, …)         ├──>  auras_of(entity)  ->  AuraList  ->  icon rows
//! Ignite, Scorched (mage)    │                                         (nameplate, frames,
//! stun, silence, frenzy      ─┘                                         buff/debuff bars)
//! ```
//!
//! The simulation keeps a lasting effect in whichever place suits the mechanic: most live in
//! the entity's [`StatusSet`], but Ignite has its own tracker, a Warrior's cooldowns are
//! timers on the class, and a stun lives on the enemy's mind because what it does is stop
//! the mind. A player does not care where an effect is kept; they see one row of icons. This
//! module is the one place those sources are read and turned into [`Aura`]s, so every row in
//! the interface — under a nameplate, beside the player, on a party frame — shows the same
//! effects with the same icon, name and description.
//!
//! Nothing here decides anything. Whether a debuff is dispellable is
//! [`StatusKind::is_dispellable`]'s answer; how long it has left is the simulation's number.
//!
//! [`StatusSet`]: engine_game::status::StatusSet

use engine_game::EntityId;
use engine_game::firemage::FireAbility;
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::status::{DispelSchool, StatusKind};
use engine_render_data::ui::UiIcon;

/// Every lasting effect the interface can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuraKind {
    /// Anything in the entity's own status set.
    Status(StatusKind),
    /// The FireMage's Ignite, which is kept in its own tracker.
    Ignite,
    /// Molten ground: more fire damage taken while standing in it, and for a moment after.
    Scorched,
    /// Flame Shield's damage reduction.
    FlameShield,
    /// Fire Mastery's active.
    FireMastery,
    /// The Inferno State, Heat's top band.
    Inferno,
    /// Shield Block.
    ShieldBlock,
    /// Battle Trance.
    BattleTrance,
    /// Last Stand.
    LastStand,
    /// Spell Reflect.
    SpellReflect,
    /// Adrenaline, banking damage.
    Adrenaline,
    /// Puncture Blood's absorb.
    BloodWard,
    /// A monster's frenzy.
    Frenzy,
    /// A monster stunned.
    Stunned,
    /// A monster silenced.
    Silenced,
}

impl AuraKind {
    /// The icon the effect is drawn with.
    ///
    /// An effect that is simply an ability's own — Rejuvenation on a target, Shield Block on
    /// the Warrior — wears that ability's icon, so the player sees the button they pressed
    /// sitting on the target it was pressed at. Everything else has an icon of its own in
    /// `assets/icons/aura/`.
    #[must_use]
    pub fn icon(self) -> UiIcon {
        match self {
            Self::Status(kind) => match kind {
                StatusKind::Burning => UiIcon::AuraBurning,
                StatusKind::Bleeding => UiIcon::AuraBleeding,
                StatusKind::Recuperating => UiIcon::Recuperate,
                StatusKind::Recovering => UiIcon::AuraRecovering,
                StatusKind::Sundered => UiIcon::AuraSundered,
                StatusKind::Moonfire => UiIcon::Moonfire,
                StatusKind::Sunfire => UiIcon::Sunfire,
                StatusKind::AstralSunfire => UiIcon::AuraAstralSunfire,
                StatusKind::NaturesExposure => UiIcon::AuraNaturesExposure,
                StatusKind::Rejuvenation => UiIcon::Rejuvenation,
                StatusKind::Regrowth => UiIcon::Regrowth,
                StatusKind::Lifebloom => UiIcon::Lifebloom,
                StatusKind::WildGrowth => UiIcon::WildGrowth,
                StatusKind::AstralWrath => UiIcon::AuraAstralWrath,
                StatusKind::WildRush => UiIcon::WildRush,
                StatusKind::Poisoned => UiIcon::AuraPoisoned,
                StatusKind::RespawnProtection => UiIcon::AuraRespawnProtection,
                StatusKind::Rended => UiIcon::AuraRended,
                StatusKind::Crippled => UiIcon::AuraCrippled,
                StatusKind::Fortitude => UiIcon::AuraFortitude,
                StatusKind::BattleShout => UiIcon::AuraBattleShout,
                StatusKind::ArcaneIntellect => UiIcon::AuraArcaneIntellect,
                StatusKind::Bloodlust => UiIcon::AuraBloodlust,
                StatusKind::Regeneration => UiIcon::AuraRegeneration,
                StatusKind::CurseOfWeakness => UiIcon::AuraCurseOfWeakness,
                StatusKind::WitheringPlague => UiIcon::AuraWitheringPlague,
                StatusKind::MagicVulnerability => UiIcon::AuraMagicVulnerability,
                StatusKind::BattleFervor => UiIcon::BattleFervor,
            },
            Self::Ignite => UiIcon::AuraBurning,
            Self::Scorched => UiIcon::AuraScorched,
            Self::FlameShield => UiIcon::FlameShield,
            Self::FireMastery => UiIcon::FireMastery,
            Self::Inferno => UiIcon::AuraInferno,
            Self::ShieldBlock => UiIcon::ShieldBlock,
            Self::BattleTrance => UiIcon::BattleTrance,
            Self::LastStand => UiIcon::LastStand,
            Self::SpellReflect => UiIcon::SpellReflect,
            Self::Adrenaline => UiIcon::Adrenaline,
            Self::BloodWard => UiIcon::AuraBloodWard,
            Self::Frenzy => UiIcon::AuraFrenzy,
            Self::Stunned => UiIcon::AuraStunned,
            Self::Silenced => UiIcon::AuraSilenced,
        }
    }

    /// The name a tooltip prints.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Status(kind) => kind.display_name(),
            Self::Ignite => "Ignite",
            Self::Scorched => "Scorched",
            Self::FlameShield => "Flame Shield",
            Self::FireMastery => "Fire Mastery",
            Self::Inferno => "Inferno",
            Self::ShieldBlock => "Shield Block",
            Self::BattleTrance => "Battle Trance",
            Self::LastStand => "Last Stand",
            Self::SpellReflect => "Spell Reflect",
            Self::Adrenaline => "Adrenaline",
            Self::BloodWard => "Blood Ward",
            Self::Frenzy => "Frenzy",
            Self::Stunned => "Stunned",
            Self::Silenced => "Silenced",
        }
    }

    /// What it does, in a sentence, for a tooltip.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            // A status says what it does in the effect registry, once, for every reader.
            Self::Status(kind) => kind.description(),
            Self::Ignite => "Burning every tick. Detonate consumes it for a burst.",
            Self::Scorched => "Taking more fire damage from standing in molten ground.",
            Self::FlameShield => "Taking 30% less damage.",
            Self::FireMastery => "More haste or critical strike, whichever you specialise in.",
            Self::Inferno => "Heat is at its peak: every spell burns hotter.",
            Self::ShieldBlock => "Extra block chance. Critical strikes cannot land.",
            Self::BattleTrance => "Blocks, parries and dodges heal you.",
            Self::LastStand => "More maximum health and 10% less damage taken.",
            Self::SpellReflect => "Enemy spells go back to whoever cast them.",
            Self::Adrenaline => "Banking a share of the damage taken, to heal it back later.",
            Self::BloodWard => "Absorbing damage.",
            Self::Frenzy => "Attacking faster.",
            Self::Stunned => "Cannot act.",
            Self::Silenced => "Cannot cast spells.",
        }
    }

    /// Whether the bearer would rather not have it.
    #[must_use]
    pub fn is_harmful(self) -> bool {
        match self {
            Self::Status(kind) => kind.is_harmful(),
            Self::Ignite | Self::Scorched | Self::Stunned | Self::Silenced => true,
            Self::FlameShield
            | Self::FireMastery
            | Self::Inferno
            | Self::ShieldBlock
            | Self::BattleTrance
            | Self::LastStand
            | Self::SpellReflect
            | Self::Adrenaline
            | Self::BloodWard
            | Self::Frenzy => false,
        }
    }

    /// The school a dispel must name to remove it, or `None` if nothing can.
    ///
    /// Only a status can be dispelled: the class timers and a monster's mind are not
    /// something Nature's Cleanse reaches.
    #[must_use]
    pub fn dispel_school(self) -> Option<DispelSchool> {
        match self {
            Self::Status(kind) if kind.is_dispellable() => kind.dispel_school(),
            _ => None,
        }
    }

    /// Whether a dispel can remove it: what the blue border says.
    #[must_use]
    pub fn is_dispellable(self) -> bool {
        self.dispel_school().is_some()
    }
}

/// One effect on one entity, this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aura {
    /// Which effect.
    pub kind: AuraKind,
    /// How many stacks. One for an effect that does not stack.
    pub stacks: u8,
    /// Seconds left.
    pub remaining: f32,
    /// Its full duration in seconds, or zero when the source does not keep one.
    pub duration: f32,
    /// Whether it lasts until something removes it.
    pub indefinite: bool,
    /// What is left of a pool the effect holds — an absorb's points. Zero for anything else.
    pub amount: f32,
}

impl Aura {
    /// An effect with a clock and nothing else.
    #[must_use]
    pub fn timed(kind: AuraKind, remaining: f32, duration: f32) -> Self {
        Self {
            kind,
            stacks: 1,
            remaining: remaining.max(0.0),
            duration: duration.max(0.0),
            indefinite: false,
            amount: 0.0,
        }
    }

    /// How much of its duration is left, in `0..=1`, or `None` when that is not known.
    #[must_use]
    pub fn fraction(&self) -> Option<f32> {
        if self.indefinite || self.duration <= 0.0 {
            return None;
        }
        Some((self.remaining / self.duration).clamp(0.0, 1.0))
    }

    /// Whether the bearer would rather not have it.
    #[must_use]
    pub fn is_harmful(&self) -> bool {
        self.kind.is_harmful()
    }

    /// Whether a dispel can remove it.
    #[must_use]
    pub fn is_dispellable(&self) -> bool {
        self.kind.is_dispellable()
    }
}

/// The effects on one entity, in a fixed array: a list is built for every plate and frame
/// every frame, and none of them may allocate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AuraList {
    entries: [Aura; Self::CAPACITY],
    len: usize,
}

impl Default for AuraList {
    fn default() -> Self {
        Self {
            entries: [Aura::timed(AuraKind::Ignite, 0.0, 0.0); Self::CAPACITY],
            len: 0,
        }
    }
}

impl AuraList {
    /// How many effects one entity can show. More than anything in the game carries at once.
    pub const CAPACITY: usize = 20;

    /// Every effect, in the order it was read: statuses in application order, then the rest.
    pub fn iter(&self) -> impl Iterator<Item = &Aura> + Clone + '_ {
        self.entries[..self.len].iter()
    }

    /// The harmful ones.
    pub fn debuffs(&self) -> impl Iterator<Item = &Aura> + Clone + '_ {
        self.iter().filter(|aura| aura.is_harmful())
    }

    /// The helpful ones.
    pub fn buffs(&self) -> impl Iterator<Item = &Aura> + Clone + '_ {
        self.iter().filter(|aura| !aura.is_harmful())
    }

    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Empties it, keeping the storage.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Adds one, if there is room. Past the capacity the rest are dropped rather than
    /// displacing what is already shown.
    pub fn push(&mut self, aura: Aura) {
        if self.len < Self::CAPACITY {
            self.entries[self.len] = aura;
            self.len += 1;
        }
    }
}

/// Every effect on `entity` right now.
#[must_use]
pub fn auras_of(simulation: &Simulation, entity: EntityId) -> AuraList {
    let mut list = AuraList::default();
    fill_auras(simulation, entity, &mut list);
    list
}

/// [`auras_of`], into a list the caller keeps.
pub fn fill_auras(simulation: &Simulation, entity: EntityId, list: &mut AuraList) {
    list.clear();
    let world = simulation.world();
    if !world.is_alive(entity) {
        // A corpse shows nothing: statuses do not tick on one, and a frame that listed the
        // burn a monster died with would be listing an effect that is not doing anything.
        // Respawn protection is the exception — it is the one thing that runs while dead.
        if let Some(statuses) = world.statuses(entity) {
            for status in statuses.iter() {
                if status.kind == StatusKind::RespawnProtection {
                    list.push(status_aura(status));
                }
            }
        }
        return;
    }

    // Ignite first on an enemy: it is the FireMage's whole readable state on a target.
    let mage = simulation.mage();
    let burn = mage.ignite.get(entity);
    if burn.is_active() {
        list.push(Aura {
            stacks: burn.stacks.max(1),
            ..Aura::timed(
                AuraKind::Ignite,
                burn.remaining,
                engine_game::firemage::ignite::IGNITE_BASE_DURATION,
            )
        });
    }
    let scorch = mage.vulnerability.remaining(entity);
    if scorch > 0.0 {
        list.push(Aura::timed(
            AuraKind::Scorched,
            scorch,
            engine_game::firemage::VULNERABILITY_DURATION,
        ));
    }

    if let Some(statuses) = world.statuses(entity) {
        for status in statuses.iter() {
            list.push(status_aura(status));
        }
    }

    if let Some(brain) = simulation.brain(entity) {
        if brain.is_stunned() {
            list.push(Aura::timed(AuraKind::Stunned, brain.stun_remaining(), 0.0));
        }
        if brain.is_silenced() {
            list.push(Aura::timed(
                AuraKind::Silenced,
                brain.silence_remaining(),
                0.0,
            ));
        }
        if brain.is_frenzied() {
            list.push(Aura::timed(AuraKind::Frenzy, brain.frenzy_remaining(), 0.0));
        }
    }

    if entity == simulation.player().entity {
        push_class_auras(simulation, list);
    }
}

/// One status as an aura.
pub fn status_aura(status: &engine_game::status::Status) -> Aura {
    Aura {
        kind: AuraKind::Status(status.kind),
        stacks: status.stacks.max(1),
        remaining: status.remaining.max(0.0),
        duration: status.duration.max(0.0),
        indefinite: status.indefinite,
        amount: 0.0,
    }
}

/// The player's own class effects: the ones kept as timers on the class, not as statuses.
///
/// Read for the hero being played only. Every class's state exists in the simulation at
/// once, and a FireMage whose idle Warrior half had a Shield Block left running from a test
/// would otherwise show it.
fn push_class_auras(simulation: &Simulation, list: &mut AuraList) {
    let duration = |ability: FireAbility| ability.definition().duration;
    match simulation.hero() {
        Hero::FireMage => {
            let mage = simulation.mage();
            if mage.flame_shield.is_active() {
                list.push(Aura::timed(
                    AuraKind::FlameShield,
                    mage.flame_shield.remaining(),
                    duration(FireAbility::FlameShield),
                ));
            }
            if mage.mastery_buff.is_active() {
                list.push(Aura::timed(
                    AuraKind::FireMastery,
                    mage.mastery_buff.remaining(),
                    duration(FireAbility::FireMastery),
                ));
            }
            let inferno = mage.heat.inferno_remaining();
            if inferno > 0.0 {
                list.push(Aura::timed(
                    AuraKind::Inferno,
                    inferno,
                    engine_game::firemage::resource::INFERNO_DURATION,
                ));
            }
        }
        Hero::Warrior => {
            let warrior = simulation.warrior();
            let timers = [
                (
                    AuraKind::ShieldBlock,
                    warrior.shield_block,
                    FireAbility::ShieldBlock,
                ),
                (
                    AuraKind::BattleTrance,
                    warrior.battle_trance,
                    FireAbility::BattleTrance,
                ),
                (
                    AuraKind::LastStand,
                    warrior.last_stand,
                    FireAbility::LastStand,
                ),
                (
                    AuraKind::SpellReflect,
                    warrior.spell_reflect,
                    FireAbility::SpellReflect,
                ),
                (
                    AuraKind::Adrenaline,
                    warrior.adrenaline,
                    FireAbility::Adrenaline,
                ),
            ];
            for (kind, timer, ability) in timers {
                if timer.is_active() {
                    list.push(Aura::timed(kind, timer.remaining(), duration(ability)));
                }
            }
            if warrior.absorb.is_active() {
                list.push(Aura {
                    amount: warrior.absorb.remaining(),
                    ..Aura::timed(
                        AuraKind::BloodWard,
                        warrior.absorb.seconds_left(),
                        duration(FireAbility::PunctureBlood),
                    )
                });
            }
        }
        // The Druid's own effects are statuses already — Astral Wrath and Wild Rush.
        Hero::Druid => {}
    }
}

/// A duration printed the way an icon has room for: `12`, `2m`, `1.4` under two seconds.
///
/// Written into a buffer the caller reuses, because a row of icons prints one of these per
/// icon per frame.
pub fn write_remaining(out: &mut String, aura: &Aura) {
    use std::fmt::Write as _;
    out.clear();
    if aura.indefinite || aura.remaining <= 0.0 {
        return;
    }
    let seconds = aura.remaining;
    if seconds >= 90.0 {
        let _ = write!(out, "{}m", (seconds / 60.0).round() as u32);
    } else if seconds >= 2.0 {
        let _ = write!(out, "{}", seconds.floor() as u32);
    } else {
        let _ = write!(out, "{seconds:.1}");
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::status::Status;

    const EVERY_STATUS: [StatusKind; 20] = [
        StatusKind::Burning,
        StatusKind::Bleeding,
        StatusKind::Recuperating,
        StatusKind::Recovering,
        StatusKind::Sundered,
        StatusKind::Moonfire,
        StatusKind::Sunfire,
        StatusKind::AstralSunfire,
        StatusKind::NaturesExposure,
        StatusKind::Rejuvenation,
        StatusKind::Regrowth,
        StatusKind::Lifebloom,
        StatusKind::WildGrowth,
        StatusKind::AstralWrath,
        StatusKind::WildRush,
        StatusKind::Poisoned,
        StatusKind::RespawnProtection,
        StatusKind::Rended,
        StatusKind::Crippled,
        StatusKind::BattleFervor,
    ];

    fn every_kind() -> Vec<AuraKind> {
        let mut kinds: Vec<AuraKind> = EVERY_STATUS.into_iter().map(AuraKind::Status).collect();
        kinds.extend([
            AuraKind::Ignite,
            AuraKind::Scorched,
            AuraKind::FlameShield,
            AuraKind::FireMastery,
            AuraKind::Inferno,
            AuraKind::ShieldBlock,
            AuraKind::BattleTrance,
            AuraKind::LastStand,
            AuraKind::SpellReflect,
            AuraKind::Adrenaline,
            AuraKind::BloodWard,
            AuraKind::Frenzy,
            AuraKind::Stunned,
            AuraKind::Silenced,
        ]);
        kinds
    }

    #[test]
    fn every_effect_has_an_icon_a_name_and_a_description() {
        for kind in every_kind() {
            assert!(!kind.name().is_empty(), "{kind:?}");
            assert!(kind.description().ends_with('.'), "{kind:?}");
            // A class ability's own effect wears the ability's icon; everything else must
            // have been given one of the aura icons, never a map glyph or a placeholder.
            let icon = kind.icon();
            assert!(
                UiIcon::ALL.contains(&icon)
                    && !matches!(icon, UiIcon::Monster | UiIcon::Check | UiIcon::Lock),
                "{kind:?} has no icon of its own"
            );
        }
    }

    #[test]
    fn two_different_effects_never_share_an_icon_unless_they_are_the_same_burn() {
        let kinds = every_kind();
        for (index, a) in kinds.iter().enumerate() {
            for b in &kinds[index + 1..] {
                let same_burn = matches!(
                    (a, b),
                    (AuraKind::Status(StatusKind::Burning), AuraKind::Ignite)
                );
                assert!(
                    same_burn || a.icon() != b.icon(),
                    "{a:?} and {b:?} share an icon"
                );
            }
        }
    }

    #[test]
    fn only_a_harmful_status_with_a_school_is_dispellable() {
        for kind in every_kind() {
            let expected = match kind {
                AuraKind::Status(status) => status.is_dispellable(),
                _ => false,
            };
            assert_eq!(kind.is_dispellable(), expected, "{kind:?}");
            if kind.is_dispellable() {
                assert!(kind.is_harmful(), "{kind:?} is a dispellable buff");
            }
        }
        assert!(AuraKind::Status(StatusKind::Poisoned).is_dispellable());
        assert!(!AuraKind::Status(StatusKind::Rended).is_dispellable());
    }

    #[test]
    fn a_status_on_an_enemy_is_read_with_its_clock_and_stacks() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation.select_nearest_target();
        let enemy = simulation
            .player()
            .target
            .expect("the arena has something to target");
        let player = simulation.player().entity;
        let applied = Status::new(
            StatusKind::Poisoned,
            player,
            FireAbility::Firebolt.into(),
            8.0,
            5.0,
        );
        simulation
            .world_mut()
            .statuses_mut(enemy)
            .expect("an enemy carries a status set")
            .apply(applied);
        let auras = auras_of(&simulation, enemy);
        let poison = auras
            .iter()
            .find(|aura| aura.kind == AuraKind::Status(StatusKind::Poisoned))
            .expect("the poison is listed");
        assert!(poison.is_harmful() && poison.is_dispellable());
        assert!((poison.remaining - 8.0).abs() < 1e-3);
        assert_eq!(poison.fraction(), Some(1.0));
        assert_eq!(auras.debuffs().count(), auras.len());
    }

    #[test]
    fn the_player_s_class_timers_are_buffs_on_the_player() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        let slot = simulation
            .hero()
            .bar()
            .into_iter()
            .position(|ability| ability == Some(FireAbility::FlameShield))
            .expect("the FireMage's bar holds Flame Shield");
        simulation.push_command(engine_game::command::PlayerCommand::UseAbility(slot));
        for _ in 0..3 {
            simulation.step_once();
        }
        let auras = auras_of(&simulation, simulation.player().entity);
        let shield = auras
            .iter()
            .find(|aura| aura.kind == AuraKind::FlameShield)
            .expect("Flame Shield is listed once it is cast");
        assert!(!shield.is_harmful() && !shield.is_dispellable());
        assert!(shield.fraction().is_some_and(|fraction| fraction > 0.5));
    }

    #[test]
    fn recuperate_stacks_reach_the_player_s_buffs() {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        assert!(simulation.set_hero(engine_game::hero::Hero::Warrior));
        simulation.select_nearest_target();
        let target = simulation.player().target.expect("a dummy");
        let at = simulation.world().position(target).expect("placed");
        let player = simulation.player().entity;
        if let Some(body) = simulation.world_mut().body_mut(player) {
            body.position = at + engine_core::math::Vec3::new(0.0, 0.0, 2.0);
        }
        for _ in 0..3 {
            simulation.push_command(engine_game::command::PlayerCommand::UseAbility(0));
            for _ in 0..200 {
                simulation.step_once();
            }
        }
        let auras = auras_of(&simulation, player);
        let recuperating = auras
            .iter()
            .find(|aura| aura.kind == AuraKind::Status(StatusKind::Recuperating))
            .expect("Recuperate is listed on the Warrior");
        assert_eq!(recuperating.stacks, 3);
    }

    #[test]
    fn a_remaining_time_fits_an_icon() {
        let mut out = String::new();
        let mut aura = Aura::timed(AuraKind::Ignite, 12.7, 30.0);
        write_remaining(&mut out, &aura);
        assert_eq!(out, "12");
        aura.remaining = 1.44;
        write_remaining(&mut out, &aura);
        assert_eq!(out, "1.4");
        aura.remaining = 300.0;
        write_remaining(&mut out, &aura);
        assert_eq!(out, "5m");
        aura.indefinite = true;
        write_remaining(&mut out, &aura);
        assert!(out.is_empty());
    }

    #[test]
    fn a_full_list_drops_the_rest_rather_than_growing() {
        let mut list = AuraList::default();
        for _ in 0..AuraList::CAPACITY + 5 {
            list.push(Aura::timed(AuraKind::Frenzy, 1.0, 0.0));
        }
        assert_eq!(list.len(), AuraList::CAPACITY);
    }
}
