//! Active trinkets: the four effects, their cooldowns and refusals, and who decides.

#![allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]

use engine_core::math::Vec3;
use engine_game::ability::AbilityError;
use engine_game::arena::{self, ArenaConfig};
use engine_game::character::{
    CatalogItem, EquipSlot, ItemFamily, ItemIcon, ItemProps, ItemQuality, ItemStack, Stat,
    item_by_key, register_catalog_items,
};
use engine_game::combat::DamageType;
use engine_game::command::PlayerCommand;
use engine_game::entity::EntityId;
use engine_game::event::CombatEvent;
use engine_game::firemage::talent::TalentEffects;
use engine_game::hero::Hero;
use engine_game::sim::Simulation;
use engine_game::status::StatusKind;
use engine_game::trinket::{TrinketActive, TrinketEffect};
use engine_game::world::EntityDesc;

fn active(
    effect: TrinketEffect,
    cooldown: f32,
    radius: f32,
    duration: f32,
    magnitude: f32,
) -> TrinketActive {
    TrinketActive {
        effect,
        cooldown,
        radius,
        duration,
        magnitude,
    }
}

fn register() {
    let row = |key: &str, active| CatalogItem {
        key: key.to_owned(),
        name: key.to_owned(),
        family: ItemFamily::Trinket,
        quality: ItemQuality::Rare,
        item_level: 8,
        icon: ItemIcon::Trinket,
        stats: vec![(Stat::Stamina, 5.0)],
        active,
    };
    register_catalog_items(vec![
        row(
            "t_ward",
            Some(active(
                TrinketEffect::GroupDamageReduction,
                120.0,
                30.0,
                8.0,
                0.15,
            )),
        ),
        row(
            "t_dispel",
            Some(active(TrinketEffect::MassDispel, 90.0, 12.0, 0.0, 0.0)),
        ),
        row(
            "t_haste",
            Some(active(
                TrinketEffect::GroupCooldownReduction,
                120.0,
                30.0,
                0.0,
                0.10,
            )),
        ),
        row(
            "t_absorb",
            Some(active(TrinketEffect::GroupAbsorb, 120.0, 30.0, 10.0, 0.20)),
        ),
        row("t_plain", None),
    ]);
}

fn wear(sim: &mut Simulation, key: &str, slot: EquipSlot, props: ItemProps) {
    register();
    let id = item_by_key(key).expect("registered").id;
    let hero = sim.hero();
    sim.roster_mut()
        .sheet_mut(hero)
        .equipment_mut()
        .equip(slot, ItemStack::with_props(id, props))
        .expect("a trinket fits a trinket slot");
    sim.refresh_character();
}

struct Group {
    sim: Simulation,
    user: EntityId,
    near: EntityId,
    far: EntityId,
    foe: EntityId,
}

/// A user, an ally 5 m away, an ally 50 m away, and a hostile dummy 4 m away.
fn group(key: &str) -> Group {
    register();
    let config = ArenaConfig::with_group(0);
    let mut sim = arena::training_arena(&config);
    let user = sim.player().entity;
    let origin = config.player_spawn;
    let near = sim
        .world_mut()
        .spawn(EntityDesc::player(origin + Vec3::new(5.0, 0.0, 0.0)));
    let far = sim
        .world_mut()
        .spawn(EntityDesc::player(origin + Vec3::new(50.0, 0.0, 0.0)));
    let foe = sim
        .world_mut()
        .spawn(EntityDesc::dummy(origin + Vec3::new(0.0, 0.0, -4.0), 1.0e7));
    sim.add_player(near, Hero::Warrior);
    sim.add_player(far, Hero::Druid);
    wear(&mut sim, key, EquipSlot::Trinket1, ItemProps::STANDARD);
    sim.step_once();
    Group {
        sim,
        user,
        near,
        far,
        foe,
    }
}

fn health(sim: &Simulation, entity: EntityId) -> f32 {
    sim.world().health(entity).map_or(0.0, |h| h.current())
}

fn max_health(sim: &Simulation, entity: EntityId) -> f32 {
    sim.world().health(entity).map_or(0.0, |h| h.max())
}

/// Hits `target` for `amount` fire damage from the dummy and answers the health it lost.
fn hit(group: &mut Group, target: EntityId, amount: f32) -> f32 {
    let before = health(&group.sim, target);
    let foe = group.foe;
    group.sim.apply_damage(
        foe,
        target,
        amount,
        engine_game::AbilityId::NONE,
        DamageType::Fire,
    );
    before - health(&group.sim, target)
}

fn has(sim: &Simulation, entity: EntityId, kind: StatusKind) -> bool {
    sim.world().statuses(entity).is_some_and(|s| s.has(kind))
}

#[test]
fn the_ward_takes_fifteen_per_cent_off_for_party_members_in_range_only() {
    let mut g = group("t_ward");
    assert_eq!(g.sim.use_trinket(0), Ok(()));
    g.sim.step_once();
    let (user, near, far) = (g.user, g.near, g.far);
    assert!((hit(&mut g, user, 100.0) - 85.0).abs() < 0.01);
    assert!((hit(&mut g, near, 100.0) - 85.0).abs() < 0.01);
    assert!(
        (hit(&mut g, far, 100.0) - 100.0).abs() < 0.01,
        "out of range"
    );
    assert!(has(&g.sim, near, StatusKind::TrinketWard));
    assert!(!has(&g.sim, far, StatusKind::TrinketWard));

    // Eight seconds, then it is over.
    g.sim.step_many(8 * 60 + 5);
    assert!(!has(&g.sim, user, StatusKind::TrinketWard));
    assert!((hit(&mut g, user, 100.0) - 100.0).abs() < 0.01);
}

#[test]
fn a_trinket_is_off_the_global_cooldown_and_has_a_cooldown_of_its_own() {
    let mut g = group("t_ward");
    g.sim.mage_mut().global_cooldown.start(1.5);
    assert!(!g.sim.mage().off_global_cooldown());
    assert_eq!(
        g.sim.use_trinket(0),
        Ok(()),
        "the global cooldown is not asked"
    );
    assert!(
        (g.sim.mage().global_cooldown.remaining() - 1.5).abs() < 1e-3,
        "and it is not started"
    );
    assert!((g.sim.trinket_remaining(0) - 120.0).abs() < 1e-3);
    assert_eq!(g.sim.use_trinket(0), Err(AbilityError::TrinketNotReady));
    g.sim.step_many(120 * 60);
    assert_eq!(g.sim.trinket_remaining(0), 0.0);
    assert_eq!(g.sim.use_trinket(0), Ok(()));
}

#[test]
fn a_trinket_is_refused_with_nothing_to_use_or_when_dead() {
    let mut g = group("t_plain");
    assert_eq!(g.sim.use_trinket(0), Err(AbilityError::NoTrinketActive));
    assert_eq!(g.sim.use_trinket(1), Err(AbilityError::NoTrinketActive));
    assert_eq!(g.sim.use_trinket(5), Err(AbilityError::NoTrinketActive));
    wear(
        &mut g.sim,
        "t_ward",
        EquipSlot::Trinket2,
        ItemProps::STANDARD,
    );
    g.sim.step_once();
    g.sim.step_many(31 * 60);
    g.sim.kill_player();
    g.sim.step_once();
    assert_eq!(g.sim.use_trinket(1), Err(AbilityError::CasterDead));
    assert_eq!(g.sim.trinket_remaining(1), 0.0, "a refusal costs nothing");
}

#[test]
fn mass_dispel_cleans_allies_in_range_and_strips_enemy_buffs_but_not_the_rest() {
    let mut g = group("t_dispel");
    let (user, near, far, foe) = (g.user, g.near, g.far, g.foe);
    let spot = g.sim.player_position() + Vec3::new(0.0, 0.0, -20.0);
    let far_foe = g.sim.world_mut().spawn(EntityDesc::dummy(spot, 1.0e7));
    // Allies: a curse (harmful, dispellable), a bleed (harmful, not), a buff.
    assert!(
        g.sim
            .apply_effect(user, StatusKind::CurseOfWeakness, 30.0, 1, foe)
    );
    assert!(g.sim.apply_effect(user, StatusKind::Bleeding, 30.0, 1, foe));
    assert!(
        g.sim
            .apply_effect(near, StatusKind::WitheringPlague, 30.0, 1, foe)
    );
    assert!(
        g.sim
            .apply_effect(near, StatusKind::Fortitude, 30.0, 1, user)
    );
    assert!(g.sim.apply_effect(far, StatusKind::Poisoned, 30.0, 1, foe));
    // Enemies: a buff (strippable), a debuff, and a buff out of range.
    assert!(g.sim.apply_effect(foe, StatusKind::Bloodlust, 30.0, 1, foe));
    assert!(
        g.sim
            .apply_effect(foe, StatusKind::CurseOfWeakness, 30.0, 1, user)
    );
    assert!(
        g.sim
            .apply_effect(far_foe, StatusKind::Bloodlust, 30.0, 1, far_foe)
    );

    assert_eq!(g.sim.use_trinket(0), Ok(()));
    assert!(!has(&g.sim, user, StatusKind::CurseOfWeakness));
    assert!(
        has(&g.sim, user, StatusKind::Bleeding),
        "a wound is not dispellable"
    );
    assert!(!has(&g.sim, near, StatusKind::WitheringPlague));
    assert!(
        has(&g.sim, near, StatusKind::Fortitude),
        "allies keep their buffs"
    );
    assert!(has(&g.sim, far, StatusKind::Poisoned), "out of range");
    assert!(!has(&g.sim, foe, StatusKind::Bloodlust));
    assert!(
        has(&g.sim, foe, StatusKind::CurseOfWeakness),
        "enemy debuffs stay"
    );
    assert!(has(&g.sim, far_foe, StatusKind::Bloodlust), "out of range");
    let dispels = g
        .sim
        .events()
        .iter()
        .filter(|e| matches!(e, CombatEvent::Dispelled { .. }))
        .count();
    assert_eq!(dispels, 3);
    assert!(g.sim.events().iter().any(|e| matches!(
        e,
        CombatEvent::TrinketUsed {
            effect: TrinketEffect::MassDispel,
            slot: 0,
            ..
        }
    )));
}

fn first_cooling(sim: &mut Simulation, entity: EntityId) -> (engine_game::FireAbility, f32) {
    sim.with_player(entity, |s| {
        let effects = TalentEffects::default();
        let ability = s
            .hero()
            .abilities()
            .iter()
            .copied()
            .find(|a| {
                s.mage_mut().abilities.spend(*a, &effects)
                    && s.mage().abilities.recharge_remaining(*a) > 1.0
            })
            .expect("an ability with a real cooldown");
        (ability, s.mage().abilities.recharge_remaining(ability))
    })
    .expect("a player")
}

#[test]
fn cooldown_reduction_takes_a_tenth_of_what_is_left_from_each_ally_in_range() {
    let mut g = group("t_haste");
    let (user, near, far) = (g.user, g.near, g.far);
    let (a_user, before_user) = first_cooling(&mut g.sim, user);
    let (a_near, before_near) = first_cooling(&mut g.sim, near);
    let (a_far, before_far) = first_cooling(&mut g.sim, far);
    g.sim.mage_mut().global_cooldown.start(1.0);

    assert_eq!(g.sim.use_trinket(0), Ok(()));
    let left = |sim: &mut Simulation, e, a| {
        sim.with_player(e, |s| s.mage().abilities.recharge_remaining(a))
            .expect("a player")
    };
    assert!((left(&mut g.sim, user, a_user) - before_user * 0.9).abs() < 1e-3);
    assert!((left(&mut g.sim, near, a_near) - before_near * 0.9).abs() < 1e-3);
    assert!(
        (left(&mut g.sim, far, a_far) - before_far).abs() < 1e-3,
        "out of range"
    );
    assert!(
        (g.sim.mage().global_cooldown.remaining() - 1.0).abs() < 1e-3,
        "the global cooldown is not an ability"
    );
    assert!(
        (g.sim.trinket_remaining(0) - 120.0).abs() < 1e-3,
        "nor is the trinket itself"
    );
}

#[test]
fn the_absorb_shields_a_fifth_of_maximum_health_first_and_then_expires() {
    let mut g = group("t_absorb");
    let (user, near, far) = (g.user, g.near, g.far);
    assert_eq!(g.sim.use_trinket(0), Ok(()));
    g.sim.step_once();
    let max = max_health(&g.sim, near);
    assert!((g.sim.absorb_of(near) - max * 0.2).abs() < 0.01);
    assert!((g.sim.absorb_of(user) - max_health(&g.sim, user) * 0.2).abs() < 0.01);
    assert_eq!(g.sim.absorb_of(far), 0.0);
    assert!(has(&g.sim, near, StatusKind::TrinketAbsorb));

    // A hit smaller than the shield costs no health; a bigger one costs only the excess.
    assert!(hit(&mut g, near, max * 0.1).abs() < 0.01);
    assert!((g.sim.absorb_of(near) - max * 0.1).abs() < 0.01);
    assert!((hit(&mut g, near, max * 0.3) - max * 0.2).abs() < 0.01);
    assert_eq!(g.sim.absorb_of(near), 0.0);
    assert!(
        !has(&g.sim, near, StatusKind::TrinketAbsorb),
        "spent, so gone"
    );

    // Unspent, it lapses after ten seconds.
    g.sim.step_many(10 * 60 + 5);
    assert_eq!(g.sim.absorb_of(user), 0.0);
    assert!(!has(&g.sim, user, StatusKind::TrinketAbsorb));
    let damage = hit(&mut g, user, 50.0);
    assert!((damage - 50.0).abs() < 0.01);
}

#[test]
fn quality_scales_the_passive_stats_and_never_the_active() {
    let mut g = group("t_absorb");
    wear(
        &mut g.sim,
        "t_absorb",
        EquipSlot::Trinket1,
        ItemProps::new(50, 0, true),
    );
    assert!(g.sim.character_stats().get(Stat::Stamina) > 0.0);
    let user = g.user;
    assert_eq!(g.sim.use_trinket(0), Ok(()));
    g.sim.step_once();
    assert!((g.sim.absorb_of(user) - max_health(&g.sim, user) * 0.2).abs() < 0.01);
}

#[test]
fn putting_a_trinket_on_starts_a_short_cooldown_so_swapping_is_not_a_second_use() {
    let mut g = group("t_ward");
    assert_eq!(g.sim.use_trinket(0), Ok(()));
    // Move it to the other slot mid-cooldown: that slot's copy is not free either.
    wear(
        &mut g.sim,
        "t_ward",
        EquipSlot::Trinket2,
        ItemProps::STANDARD,
    );
    g.sim.step_once();
    assert!(g.sim.trinket_remaining(1) > 29.0);
    assert_eq!(g.sim.use_trinket(1), Err(AbilityError::TrinketNotReady));
}

#[test]
fn a_client_under_a_remote_authority_only_asks() {
    let mut g = group("t_ward");
    g.sim.set_remote_authority(true);
    g.sim.push_command(PlayerCommand::UseTrinket(0));
    g.sim.step_once();
    let user = g.user;
    assert!(!has(&g.sim, user, StatusKind::TrinketWard));
    assert_eq!(g.sim.trinket_remaining(0), 0.0);
    assert_eq!(
        g.sim.take_remote_requests(),
        vec![PlayerCommand::UseTrinket(0)]
    );
    // What the authority says is what is shown, and it runs down between rows.
    g.sim.present_trinkets([50.0, 0.0]);
    g.sim.step_many(60);
    assert!((g.sim.trinket_remaining(0) - 49.0).abs() < 0.05);
}

#[test]
fn the_command_uses_the_trinket_in_a_tick() {
    let mut g = group("t_ward");
    g.sim.push_command(PlayerCommand::UseTrinket(0));
    g.sim.step_once();
    assert!(has(&g.sim, g.user, StatusKind::TrinketWard));
}

fn transcript() -> Vec<String> {
    let mut g = group("t_ward");
    let mut out = Vec::new();
    g.sim.push_command(PlayerCommand::UseTrinket(0));
    for _ in 0..600 {
        g.sim.step_once();
        out.extend(g.sim.events().iter().map(|e| format!("{e:?}")));
    }
    out
}

#[test]
fn using_a_trinket_is_deterministic() {
    let first = transcript();
    assert!(first.iter().any(|line| line.contains("TrinketUsed")));
    assert_eq!(first, transcript());
}
