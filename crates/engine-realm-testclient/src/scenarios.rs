//! End-to-end scenarios against a running realm (SpacetimeDB + realm API).
//!
//! Each scenario starts real bots over the real network layer, drives them until a condition
//! holds or a deadline passes, and checks what the *server* says. A failed check returns an
//! error naming it; `realm-bot scenario all` exits non-zero on the first one.

use std::time::{Duration, Instant};

use engine_core::math::Vec3;
use engine_net::{Action, NetCommand, RealmEndpoints};
use engine_realm::chat::Channel;
use engine_realm::instance::InstanceState;
use engine_realm::party::PartyState;
use engine_realm::{ErrorCategory, Role};

use crate::bot::{Bot, BotConfig};

/// A scenario's outcome.
pub type Outcome = Result<(), String>;

fn check(condition: bool, what: &str) -> Outcome {
    if condition {
        log::info!("  ok: {what}");
        Ok(())
    } else {
        Err(format!("FAILED: {what}"))
    }
}

/// A unique tag for this run's accounts.
fn run_tag() -> String {
    format!("{}", engine_net::clock::local_now() / 1000 % 10_000_000_000)
}

/// Three bots, one per role. With `lead` the tank (index 0) is the designated lead: it inserts
/// its key when the party is choosing one, and then starts the dungeon. Nobody else does, so
/// which bot leads is deterministic.
fn trio(
    tag: &str,
    prefix: &str,
    endpoints: &RealmEndpoints,
    walk: bool,
    lead: bool,
) -> Result<Vec<Bot>, String> {
    let roles = [
        ("tank", "warrior", Role::Tank),
        ("healer", "druid", Role::Healer),
        ("dps", "firemage", Role::Dps),
    ];
    roles
        .iter()
        .enumerate()
        .map(|(index, (label, definition, role))| {
            Bot::start(
                BotConfig {
                    name: format!("{prefix}-{label}"),
                    email: format!("{prefix}-{label}-{tag}@bots.mygame.test"),
                    password: "bot-password-1".into(),
                    definition: (*definition).into(),
                    role: *role,
                    dungeon: "graveyard_crypt".into(),
                    accept: true,
                    insert_key: lead && index == 0,
                    start_if_leader: lead && index == 0,
                    walk,
                    hunt: false,
                    queue: true,
                },
                endpoints,
            )
            .map_err(|e| e.message)
        })
        .collect()
}

/// Ticks every bot until `done` holds or `timeout` passes.
fn drive(bots: &mut [Bot], timeout: Duration, mut done: impl FnMut(&[Bot]) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        for bot in bots.iter_mut() {
            bot.tick();
        }
        if done(bots) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    false
}

/// Completes `instance` with the owner-only `admin_finish_instance`, through the `spacetime`
/// CLI (set `SPACETIME_CLI` if it is not on PATH).
fn finish_instance(endpoints: &RealmEndpoints, instance: u64, success: bool) -> Outcome {
    let cli = std::env::var("SPACETIME_CLI").unwrap_or_else(|_| "spacetime".into());
    let output = std::process::Command::new(&cli)
        .args([
            "call",
            "--server",
            &endpoints.spacetimedb_url,
            &endpoints.spacetimedb_database,
            "admin_finish_instance",
            &instance.to_string(),
            if success { "true" } else { "false" },
        ])
        .output()
        .map_err(|e| format!("could not run {cli}: {e}"))?;
    check(output.status.success(), "the owner completed the instance")
}

/// Asks the API for every bot's characters again and lets the answers arrive.
fn refetch(bots: &mut [Bot], wait: Duration) {
    for bot in bots.iter() {
        bot.send(NetCommand::FetchCharacters);
    }
    drive(bots, wait, |_| false);
}

fn key_level_of(bot: &Bot) -> u32 {
    bot.character()
        .and_then(|c| c.key.as_ref())
        .map_or(0, |k| k.level)
}

fn resource_of(bot: &Bot, id: &str) -> u64 {
    // The freshly fetched character first: the list is only as new as the last sign-in.
    let fetched = bot
        .detail
        .as_ref()
        .filter(|detail| Some(&detail.summary.id) == bot.character().map(|c| &c.id))
        .map(|detail| &detail.summary);
    fetched
        .or_else(|| bot.character())
        .and_then(|c| c.resources.iter().find(|(r, _)| r == id))
        .map_or(0, |(_, amount)| *amount)
}

fn all_active(bots: &[Bot]) -> bool {
    bots.iter().all(|bot| {
        bot.mirror
            .instance
            .as_ref()
            .is_some_and(|i| i.state == InstanceState::Active && i.barrier_open)
    })
}

/// The vertical slice: log in, queue 1/1/1, match, ready check, key insertion (the tank's key
/// makes it the leader), start, countdown, barrier, authoritative movement seen by everyone.
pub fn slice(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: slice");
    let tag = run_tag();
    let mut bots = trio(&tag, "slice", endpoints, true, true)?;

    let formed = drive(&mut bots, Duration::from_secs(30), |bots| {
        bots.iter().all(|b| b.mirror.members.len() == 3)
    });
    check(formed, "the server formed a party of three")?;
    let party_ids: Vec<Option<u64>> = bots
        .iter()
        .map(|b| b.mirror.party.as_ref().map(|p| p.id))
        .collect();
    check(
        party_ids.windows(2).all(|w| w[0] == w[1]),
        "all three are in the same party",
    )?;
    let roles: Vec<Role> = bots[0].mirror.members.iter().map(|m| m.role).collect();
    check(
        roles.contains(&Role::Tank) && roles.contains(&Role::Healer) && roles.contains(&Role::Dps),
        "exactly one tank, one healer, one dps",
    )?;

    let started = drive(&mut bots, Duration::from_secs(20), |bots| {
        bots.iter().all(|b| b.mirror.instance.is_some())
    });
    check(started, "the leader started the dungeon")?;
    let leader = bots[0].mirror.party.as_ref().and_then(|p| p.leader.clone());
    check(leader.is_some(), "the server assigned a leader")?;
    let tank = bots[0].session.as_ref().map(|s| s.account_id.clone());
    check(
        leader == tank,
        "the designated bot, which inserted its key, is the leader",
    )?;
    check(
        bots[0]
            .mirror
            .party
            .as_ref()
            .is_some_and(|p| p.key_owner == tank && p.key_level >= 1),
        "the party records the key's owner and level",
    )?;
    check(
        bots.iter().filter(|b| b.mirror.i_lead()).count() == 1,
        "exactly one bot leads",
    )?;
    let countdown = bots[0]
        .mirror
        .instance
        .as_ref()
        .and_then(|i| i.countdown_ends_at);
    check(
        countdown.is_some(),
        "the instance carries a server countdown timestamp",
    )?;
    check(
        bots.iter().all(|b| {
            b.mirror
                .instance
                .as_ref()
                .is_some_and(|i| !i.barrier_open || i.state == InstanceState::Active)
        }),
        "the barrier is closed until the run is active",
    )?;

    let began = Instant::now();
    let active = drive(&mut bots, Duration::from_secs(12), all_active);
    check(active, "the barrier lifted and the run became active")?;
    check(
        began.elapsed() >= Duration::from_secs(3),
        "the countdown took the server's time, not zero",
    )?;
    check(
        bots.iter().all(|b| b.mirror.players.len() == 3),
        "every client sees three players",
    )?;
    check(
        bots.iter().all(|b| b.mirror.monsters.len() > 10),
        "every client sees the server's monsters",
    )?;

    drive(&mut bots, Duration::from_secs(3), |_| false);
    for bot in &bots {
        let start = bot.start_position.unwrap_or(Vec3::ZERO);
        let now = bot.mirror.me().map_or(start, |me| me.position);
        check(
            start.z - now.z > 5.0,
            &format!(
                "{} moved forward under server authority ({:.1} m)",
                bot.config.name,
                start.z - now.z
            ),
        )?;
    }
    // Everyone sees everyone else's authoritative movement.
    for viewer in &bots {
        for other in &bots {
            if viewer.session.as_ref().map(|s| &s.account_id)
                == other.session.as_ref().map(|s| &s.account_id)
            {
                continue;
            }
            let Some(account) = other.session.as_ref().map(|s| s.account_id.clone()) else {
                continue;
            };
            let seen = viewer.mirror.players.get(&account).map(|p| p.position);
            let actual = other.mirror.me().map(|p| p.position);
            let agree = matches!((seen, actual), (Some(a), Some(b)) if (a - b).length() < 3.0);
            check(
                agree,
                &format!(
                    "{} sees {} where the server has them",
                    viewer.config.name, other.config.name
                ),
            )?;
        }
    }
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| {
        bots.iter().all(|b| b.mirror.party.is_none())
    });
    Ok(())
}

/// Two parties at once: separate instances, and neither sees the other.
pub fn isolation(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: isolation");
    let tag = run_tag();
    let mut bots = trio(&tag, "iso-a", endpoints, false, true)?;
    bots.extend(trio(&tag, "iso-b", endpoints, false, true)?);
    let active = drive(&mut bots, Duration::from_secs(45), all_active);
    check(active, "both parties reached an active instance")?;
    let instances: std::collections::BTreeSet<u64> = bots
        .iter()
        .filter_map(|b| b.mirror.instance.as_ref().map(|i| i.id))
        .collect();
    check(
        instances.len() == 2,
        "two parties got two separate instances",
    )?;
    check(
        bots.iter().all(|b| b.mirror.players.len() == 3),
        "nobody receives the other instance's players",
    )?;
    for bot in &bots {
        let mine = bot.mirror.instance.as_ref().map(|i| i.id);
        let partners = bots
            .iter()
            .filter(|o| o.mirror.instance.as_ref().map(|i| i.id) == mine)
            .count();
        check(
            partners == 3,
            &format!(
                "{} shares its instance with exactly its party",
                bot.config.name
            ),
        )?;
    }
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| {
        bots.iter().all(|b| b.mirror.party.is_none())
    });
    Ok(())
}

/// A decline cancels the ready check; the others go back to the queue, the decliner leaves it.
pub fn decline(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: decline");
    let tag = run_tag();
    let mut bots = trio(&tag, "decline", endpoints, false, false)?;
    bots[1].config.accept = false;
    let cancelled = drive(&mut bots, Duration::from_secs(30), |bots| {
        bots[1].answered() && bots.iter().all(|b| b.mirror.party.is_none())
    });
    check(cancelled, "the party was cancelled after the decline")?;
    drive(&mut bots, Duration::from_secs(1), |_| false);
    check(
        bots[1].mirror.ticket.is_none(),
        "the decliner left the queue",
    )?;
    check(
        bots[0].mirror.ticket.is_some() && bots[2].mirror.ticket.is_some(),
        "the others are back in the queue",
    )?;
    for bot in &mut bots {
        bot.config.queue = false;
        bot.send(NetCommand::LeaveQueue);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| {
        bots.iter().all(|b| b.mirror.ticket.is_none())
    });
    Ok(())
}

/// A player drops mid-run; the party waits, the run continues, and they come back to the same
/// instance at the same position under the same identity.
pub fn reconnect(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: reconnect");
    let tag = run_tag();
    let mut bots = trio(&tag, "reconnect", endpoints, false, true)?;
    let active = drive(&mut bots, Duration::from_secs(45), all_active);
    check(active, "the run became active")?;
    let gone = bots.remove(2);
    let account = gone
        .session
        .as_ref()
        .map(|s| s.account_id.clone())
        .unwrap_or_default();
    let instance = gone.mirror.instance.as_ref().map(|i| i.id);
    let position = gone.mirror.me().map(|p| p.position);
    let config = gone.config.clone();
    drop(gone);

    let noticed = drive(&mut bots, Duration::from_secs(10), |bots| {
        bots.iter()
            .all(|b| b.mirror.players.get(&account).is_some_and(|p| !p.present))
    });
    check(
        noticed,
        "the party sees the player as temporarily disconnected",
    )?;
    check(
        bots.iter().all(|b| b.mirror.members.len() == 3),
        "the party kept the player (no replacement)",
    )?;
    check(
        bots.iter().all(|b| {
            b.mirror
                .instance
                .as_ref()
                .is_some_and(|i| i.state == InstanceState::Active)
        }),
        "the run continues",
    )?;

    let mut back = Bot::start(
        BotConfig {
            queue: false,
            ..config
        },
        endpoints,
    )
    .map_err(|e| e.message)?;
    let resumed = drive(
        std::slice::from_mut(&mut back),
        Duration::from_secs(20),
        |bots| {
            bots[0].mirror.instance.as_ref().map(|i| i.id) == instance
                && bots[0].mirror.me().is_some()
        },
    );
    check(resumed, "the player resumed in the same instance")?;
    check(
        back.session.as_ref().map(|s| &s.account_id) == Some(&account),
        "with the same account",
    )?;
    let now = back.mirror.me().map(|p| p.position);
    check(
        matches!((position, now), (Some(a), Some(b)) if (a - b).length() < 1.0),
        "at the authoritative position it left",
    )?;
    bots.push(back);
    let present = drive(&mut bots, Duration::from_secs(5), |bots| {
        bots.iter()
            .all(|b| b.mirror.players.get(&account).is_some_and(|p| p.present))
    });
    check(present, "the party sees the player present again")?;
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| {
        bots.iter().all(|b| b.mirror.party.is_none())
    });
    Ok(())
}

/// Everything about a character that a run snapshots, as one comparable string: the API's
/// stored equipment and bag, its talents, and its gear score.
fn configuration_of(bot: &mut Bot, wait: Duration) -> String {
    let Some(id) = bot.character().map(|c| c.id.clone()) else {
        return String::new();
    };
    bot.detail = None;
    bot.send(NetCommand::FetchCharacter(id));
    drive(std::slice::from_mut(bot), wait, |bots| {
        bots[0].detail.is_some()
    });
    bot.detail.as_ref().map_or_else(String::new, |detail| {
        format!(
            "{:?}|{:?}|{:?}|{}",
            detail.summary.equipment, detail.inventory, detail.talents, detail.summary.gear_score
        )
    })
}

/// A party member drops during combat and the others are told; one of them leaves and is
/// safely back in the hub, nothing of theirs touched, the run going on for the rest; the
/// dropped member returns to the same instance with the configuration the run was locked with.
///
/// The boss encounter itself is not scripted: nothing in the realm's admin surface walks a
/// party to the boss (`admin_finish_instance` ends a run, it does not engage it). Presence is a
/// column on the player row and the fight keeps ticking whatever it is doing, so the engaged
/// boss takes the same path; it is checked by hand.
pub fn reconnect_party(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: reconnect_party");
    let tag = run_tag();
    let mut bots = trio(&tag, "rejoin", endpoints, false, true)?;
    drive(&mut bots, Duration::from_secs(45), all_active);
    check(all_active(&bots), "the run became active")?;
    // The baseline: each character's key and currency as the API has them.
    refetch(&mut bots, Duration::from_secs(3));
    let keys: Vec<u32> = bots.iter().map(key_level_of).collect();
    let resources: Vec<Vec<(String, u64)>> = bots
        .iter()
        .map(|b| {
            b.character()
                .map(|c| c.resources.clone())
                .unwrap_or_default()
        })
        .collect();

    // Locked configuration: while engaged the API refuses every change.
    let token = bots[0]
        .acquire_probe_token(endpoints)
        .map_err(|e| e.message)?;
    let id = bots[0]
        .character()
        .map(|c| c.id.clone())
        .unwrap_or_default();
    let head = serde_json::json!({"kind": "equipment", "slot": "head"});
    let locked: [(&str, String, serde_json::Value); 6] = [
        (
            "POST",
            format!("/api/v1/characters/{id}/equipment/move"),
            serde_json::json!({"from": head, "to": {"kind": "inventory", "index": 0}}),
        ),
        (
            "POST",
            format!("/api/v1/characters/{id}/salvage"),
            serde_json::json!({"indices": [0]}),
        ),
        (
            "POST",
            format!("/api/v1/characters/{id}/enhance"),
            serde_json::json!({"slot": head}),
        ),
        (
            "POST",
            format!("/api/v1/characters/{id}/key/downgrade"),
            serde_json::Value::Null,
        ),
        (
            "POST",
            format!("/api/v1/characters/{id}/key/cleanse"),
            serde_json::Value::Null,
        ),
        (
            "PUT",
            format!("/api/v1/characters/{id}/talents"),
            serde_json::json!({"talents": []}),
        ),
    ];
    for (method, path, body) in &locked {
        let status = call_api(endpoints, &token, method, path, body);
        check(
            status == 409,
            &format!("{method} {path} is refused while engaged ({status})"),
        )?;
    }
    let before_dps = configuration_of(&mut bots[2], Duration::from_secs(3));

    // The dps drops mid-run.
    let gone = bots.remove(2);
    let account = gone
        .session
        .as_ref()
        .map(|s| s.account_id.clone())
        .unwrap_or_default();
    let instance = gone.mirror.instance.as_ref().map(|i| i.id);
    let position = gone.mirror.me().map(|p| p.position);
    let max_health = gone.mirror.me().map(|p| p.max_health);
    let gear_score = gone.mirror.my_membership().map(|m| m.gear_score);
    let config = gone.config.clone();
    drop(gone);

    let noticed = drive(&mut bots, Duration::from_secs(10), |bots| {
        bots.iter().all(|b| {
            b.mirror
                .members
                .iter()
                .any(|m| m.account_id == account && !m.present && m.disconnected_at.is_some())
        })
    });
    check(noticed, "the others see the member disconnected, with when")?;
    check(
        bots.iter().all(|b| {
            b.mirror
                .members
                .iter()
                .any(|m| m.account_id == account && m.reconnect_grace > 0)
        }),
        "and how long their place is held",
    )?;

    // The healer chooses Leave.
    let healer = bots[1]
        .session
        .as_ref()
        .map(|s| s.account_id.clone())
        .unwrap_or_default();
    bots[1].send(NetCommand::LeaveParty);
    let left = drive(&mut bots, Duration::from_secs(10), |bots| {
        bots[1].mirror.party.is_none()
            && bots[1].mirror.instance.is_none()
            && bots[0].mirror.members.len() == 2
    });
    check(
        left,
        "the leaver is out of the party and the instance (the hub)",
    )?;
    check(
        bots[0]
            .mirror
            .instance
            .as_ref()
            .is_some_and(|i| i.state == InstanceState::Active),
        "the run continues for the rest",
    )?;
    // Their body left the fight with them: the others see no healer among the instance's
    // players, and no monster is still fighting one.
    let healer_ref = engine_realm::combat::EntityRef::Player(healer.clone());
    let body_gone = drive(&mut bots[..1], Duration::from_secs(5), |bots| {
        bots[0]
            .mirror
            .players
            .values()
            .all(|p| p.account_id != healer)
            && bots[0]
                .mirror
                .combat
                .values()
                .all(|c| c.account_id != healer)
            && bots[0]
                .mirror
                .monsters
                .values()
                .all(|m| m.target != healer_ref)
    });
    check(
        body_gone,
        "the leaver's body is gone from the fight for the rest",
    )?;
    refetch(&mut bots, Duration::from_secs(3));
    check(
        key_level_of(&bots[1]) == keys[1],
        "the leaver's key is untouched",
    )?;
    check(
        bots[1]
            .character()
            .map(|c| c.resources.clone())
            .unwrap_or_default()
            == resources[1],
        "the leaver lost no currency",
    )?;
    check(
        key_level_of(&bots[0]) == keys[0],
        "the key owner's key is untouched",
    )?;

    // The dropped member returns.
    let mut back = Bot::start(
        BotConfig {
            queue: false,
            ..config
        },
        endpoints,
    )
    .map_err(|e| e.message)?;
    let resumed = drive(
        std::slice::from_mut(&mut back),
        Duration::from_secs(20),
        |bots| {
            bots[0].mirror.instance.as_ref().map(|i| i.id) == instance
                && bots[0].mirror.me().is_some()
        },
    );
    check(resumed, "the dropped member resumed in the same instance")?;
    let now = back.mirror.me().map(|p| (p.position, p.max_health));
    check(
        matches!((position, now), (Some(a), Some((b, _))) if (a - b).length() < 1.0),
        "at the position they left",
    )?;
    check(
        now.map(|(_, health)| health) == max_health,
        "with the same maximum health",
    )?;
    check(
        back.mirror.my_membership().map(|m| m.gear_score) == gear_score,
        "and the gear score the run was locked with",
    )?;
    refetch(std::slice::from_mut(&mut back), Duration::from_secs(3));
    check(
        configuration_of(&mut back, Duration::from_secs(3)) == before_dps,
        "equipment, bag and talents are what they were",
    )?;
    bots.push(back);
    // Everyone still in the party: the healer who left sees no members at all.
    let present = drive(&mut bots, Duration::from_secs(5), |bots| {
        bots.iter().filter(|b| b.mirror.party.is_some()).all(|b| {
            b.mirror
                .members
                .iter()
                .find(|m| m.account_id == account)
                .is_some_and(|m| m.present && m.disconnected_at.is_none())
        })
    });
    check(present, "the party sees them present again (prompt closes)")?;
    for bot in bots.iter().filter(|b| b.mirror.party.is_some()) {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| {
        bots.iter().all(|b| b.mirror.party.is_none())
    });
    Ok(())
}

/// Calls a realm function over SpacetimeDB's HTTP API, the way any HTTP client could.
fn call_realm(
    endpoints: &RealmEndpoints,
    token: &str,
    function: &str,
    args: &serde_json::Value,
) -> u16 {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    agent
        .post(format!(
            "{}/v1/database/{}/call/{function}",
            endpoints.spacetimedb_url, endpoints.spacetimedb_database
        ))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .send(args.to_string())
        .map_or(0, |response| response.status().as_u16())
}

/// Whether a realm call was refused *by the module* (it ran and returned an error: 530), as
/// opposed to failing before it reached any rule — which would prove nothing about the rules.
fn module_refused(status: u16) -> bool {
    status == 530
}

/// Calls the realm API directly.
fn call_api(
    endpoints: &RealmEndpoints,
    token: &str,
    method: &str,
    path: &str,
    body: &serde_json::Value,
) -> u16 {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("{}{path}", endpoints.persistence_api_url);
    let result = match method {
        "GET" => agent
            .get(url)
            .header("Authorization", format!("Bearer {token}"))
            .call(),
        "PUT" => agent
            .put(url)
            .header("Authorization", format!("Bearer {token}"))
            .send_json(body),
        _ => agent
            .post(url)
            .header("Authorization", format!("Bearer {token}"))
            .send_json(body),
    };
    result.map_or(0, |response| response.status().as_u16())
}

fn refused(bot: &Bot, action: Action) -> Option<ErrorCategory> {
    bot.failures
        .iter()
        .rev()
        .find(|(a, _)| *a == action)
        .map(|(_, e)| e.category)
}

/// Every attempt a malicious client could make fails on the server.
pub fn security(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: security");
    let tag = run_tag();
    let mut bots = trio(&tag, "sec", endpoints, false, false)?;
    // Hold the third bot's answer so the party stays in its ready check for a moment.
    bots[2].hold_ready = true;
    let checking = drive(&mut bots, Duration::from_secs(30), |bots| {
        bots.iter().all(|b| {
            b.mirror
                .party
                .as_ref()
                .is_some_and(|p| p.state == PartyState::ReadyCheck)
        }) && bots[0].answered()
    });
    check(checking, "a party formed and its ready check is open")?;
    bots[0].send(NetCommand::InsertKey);
    drive(&mut bots, Duration::from_secs(2), |_| false);
    check(
        refused(&bots[0], Action::InsertKey).is_some(),
        "a key cannot be inserted during the ready check",
    )?;
    bots[2].hold_ready = false;
    let choosing = drive(&mut bots, Duration::from_secs(10), |bots| {
        bots.iter().all(|b| {
            b.mirror
                .party
                .as_ref()
                .is_some_and(|p| p.state == PartyState::ChoosingKey)
        })
    });
    check(choosing, "everyone accepted: the party is choosing a key")?;
    check(
        bots.iter()
            .all(|b| b.mirror.party.as_ref().is_some_and(|p| p.leader.is_none())),
        "nobody leads before a key is inserted",
    )?;
    bots[0].send(NetCommand::StartDungeon);
    drive(&mut bots, Duration::from_secs(2), |_| false);
    check(
        refused(&bots[0], Action::StartDungeon).is_some(),
        "the dungeon cannot start while the party is choosing a key",
    )?;
    bots[0].send(NetCommand::InsertKey);
    let ready = drive(&mut bots, Duration::from_secs(10), |bots| {
        bots.iter().all(|b| {
            b.mirror
                .party
                .as_ref()
                .is_some_and(|p| p.state == PartyState::Ready)
        })
    });
    check(
        ready,
        "the key went in and the party is waiting for its leader",
    )?;
    let leader = 0;
    let other = 1;
    check(
        bots[leader].mirror.i_lead(),
        "the key's owner leads the party",
    )?;
    bots[other].send(NetCommand::WithdrawKey);
    drive(&mut bots, Duration::from_secs(2), |_| false);
    check(
        refused(&bots[other], Action::WithdrawKey) == Some(ErrorCategory::Authorization),
        "a non-owner cannot withdraw the key",
    )?;
    bots[other].send(NetCommand::InsertKey);
    drive(&mut bots, Duration::from_secs(2), |_| false);
    check(
        refused(&bots[other], Action::InsertKey).is_some(),
        "a second key cannot be inserted once the party is ready",
    )?;
    check(
        bots.iter().all(|b| {
            b.mirror.party.as_ref().is_some_and(|p| {
                p.state == PartyState::Ready
                    && p.key_owner == bots[0].mirror.my_account_id().map(str::to_owned)
            })
        }),
        "the refused calls changed nothing",
    )?;
    let target = bots[2]
        .session
        .as_ref()
        .map(|s| s.account_id.clone())
        .unwrap_or_default();

    // Leader-only operations from a non-leader.
    bots[other].send(NetCommand::StartDungeon);
    bots[other].send(NetCommand::Kick(target.clone()));
    bots[other].send(NetCommand::TransferLeader(target.clone()));
    drive(&mut bots, Duration::from_secs(3), |_| false);
    check(
        refused(&bots[other], Action::StartDungeon) == Some(ErrorCategory::Authorization),
        "a non-leader cannot start the dungeon",
    )?;
    check(
        refused(&bots[other], Action::Kick) == Some(ErrorCategory::Authorization),
        "a non-leader cannot kick",
    )?;
    check(
        refused(&bots[other], Action::TransferLeader) == Some(ErrorCategory::Authorization),
        "a non-leader cannot transfer leadership",
    )?;
    check(
        bots.iter().all(|b| b.mirror.members.len() == 3),
        "nobody was removed",
    )?;

    // Leader kicking a stranger.
    bots[leader].send(NetCommand::Kick(
        "00000000-0000-0000-0000-000000000000".into(),
    ));
    drive(&mut bots, Duration::from_secs(2), |_| false);
    check(
        refused(&bots[leader], Action::Kick).is_some(),
        "the leader cannot kick someone outside the party",
    )?;

    // Answering a ready check that is not open.
    bots[other].send(NetCommand::RespondReadyCheck(false));
    drive(&mut bots, Duration::from_secs(2), |_| false);
    check(
        refused(&bots[other], Action::ReadyCheck).is_some(),
        "a closed ready check cannot be answered again",
    )?;

    // Direct calls a modified client could make.
    let token = bots[other]
        .acquire_probe_token(endpoints)
        .map_err(|e| e.message)?;
    let account = bots[other]
        .session
        .as_ref()
        .map(|s| s.account_id.clone())
        .unwrap_or_default();
    let status = call_realm(
        endpoints,
        &token,
        "svc_enqueue",
        &serde_json::json!([account, "fake-character", "Cheater", "warrior", 60, 99999, "{}", "graveyard_crypt", {"tank": []}, "graveyard_crypt", 1, "[]", 0, "[]"]),
    );
    check(
        module_refused(status),
        &format!("a player cannot enqueue with a fabricated gear score ({status})"),
    )?;
    let status = call_realm(
        endpoints,
        &token,
        "admin_finish_instance",
        &serde_json::json!([1, true]),
    );
    check(
        module_refused(status),
        &format!("a player cannot complete a dungeon ({status})"),
    )?;
    let status = call_realm(
        endpoints,
        &token,
        "svc_ack_results",
        &serde_json::json!([["x"]]),
    );
    check(
        module_refused(status),
        &format!("a player cannot clear the result outbox ({status})"),
    )?;
    let status = call_realm(
        endpoints,
        &token,
        "admin_configure",
        &serde_json::json!(["http://evil", "x", "y"]),
    );
    check(
        module_refused(status),
        &format!("a player cannot reconfigure the realm ({status})"),
    )?;
    let status = call_realm(
        endpoints,
        &token,
        "matchmaking_tick",
        &serde_json::json!([{"scheduled_id": 0, "scheduled_at": {"Interval": {"__time_duration_micros__": 1}}}]),
    );
    // 404: SpacetimeDB does not expose scheduled reducers to clients at all; 530 would be the
    // module's own sender check refusing it. Either way the matchmaker cannot be driven.
    check(
        status == 404 || module_refused(status),
        &format!("a player cannot run the matchmaker ({status})"),
    )?;
    let status = call_realm(
        endpoints,
        &token,
        "combat_tick",
        &serde_json::json!([{"scheduled_id": 0, "scheduled_at": {"Interval": {"__time_duration_micros__": 1}}}]),
    );
    check(
        status == 404 || module_refused(status),
        &format!("a player cannot step a fight ({status})"),
    )?;
    let status = call_realm(
        endpoints,
        &token,
        "combat_act",
        &serde_json::json!([{"monster": 0}, {"ability": 0}]),
    );
    check(
        module_refused(status),
        &format!("a player outside a dungeon cannot act in one ({status})"),
    )?;

    // Another account's character, through the API.
    let victim = bots[leader]
        .character()
        .map(|c| c.id.clone())
        .unwrap_or_default();
    let status = call_api(
        endpoints,
        &token,
        "GET",
        &format!("/api/v1/characters/{victim}"),
        &serde_json::Value::Null,
    );
    check(
        status == 404,
        &format!("another account's character is invisible ({status})"),
    )?;
    let status = call_api(
        endpoints,
        &token,
        "POST",
        "/api/v1/realm/queue",
        &serde_json::json!({"character_id": victim, "dungeon_id": "graveyard_crypt", "role": "tank"}),
    );
    check(
        status == 404,
        &format!("another account's character cannot be queued ({status})"),
    )?;
    let status = call_api(
        endpoints,
        &token,
        "POST",
        &format!("/api/v1/characters/{victim}/equipment/move"),
        &serde_json::json!({"from": {"kind": "equipment", "slot": "chest"}, "to": {"kind": "inventory", "index": 31}}),
    );
    check(
        status == 404,
        &format!("another account's character cannot be modified ({status})"),
    )?;
    // A role the character cannot play.
    let own = bots[other]
        .character()
        .map(|c| (c.id.clone(), c.roles.clone()))
        .unwrap_or_default();
    let forbidden = Role::ALL.into_iter().find(|r| !own.1.contains(r));
    if let Some(role) = forbidden {
        let status = call_api(
            endpoints,
            &token,
            "POST",
            "/api/v1/realm/queue",
            &serde_json::json!({"character_id": own.0, "dungeon_id": "graveyard_crypt", "role": role.key()}),
        );
        check(
            status == 400,
            &format!("a character cannot queue as a role it cannot play ({status})"),
        )?;
    }
    // Tokens the issuer never signed.
    let forged = format!(
        "{}.{}.{}",
        "eyJhbGciOiJFUzI1NiIsInR5cCI6IkpXVCJ9", "eyJzdWIiOiJ4In0", "AAAA"
    );
    let status = call_api(
        endpoints,
        &forged,
        "GET",
        "/api/v1/characters",
        &serde_json::Value::Null,
    );
    check(
        status == 401,
        &format!("the API rejects a forged token ({status})"),
    )?;
    let status = call_realm(endpoints, &forged, "queue_leave", &serde_json::json!([]));
    check(
        status == 401 || status == 400,
        &format!("the realm rejects a forged token ({status})"),
    )?;

    // Movement: a speed hack is held to real time.
    bots[leader].send(NetCommand::StartDungeon);
    let active = drive(&mut bots, Duration::from_secs(15), all_active);
    check(active, "the leader started")?;
    let start = bots[other]
        .mirror
        .me()
        .map(|p| p.position)
        .unwrap_or(Vec3::ZERO);
    let mut seq = bots[other].mirror.me().map_or(0, |p| p.last_seq) + 1;
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(2) {
        let frames: Vec<engine_realm::movement::InputFrame> = (0..16)
            .map(|_| {
                let frame = engine_realm::movement::InputFrame {
                    seq,
                    forward: 50.0,
                    right: 0.0,
                    yaw: 0.0,
                    face_camera: false,
                    jump: false,
                };
                seq += 1;
                frame
            })
            .collect();
        bots[other].send(NetCommand::Input(frames));
        for bot in &mut bots {
            bot.tick();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drive(&mut bots, Duration::from_secs(1), |_| false);
    let end = bots[other]
        .mirror
        .me()
        .map(|p| p.position)
        .unwrap_or(Vec3::ZERO);
    let moved = Vec3::new(end.x - start.x, 0.0, end.z - start.z).length();
    let max_speed = engine_core::tuning::movement::MAX_SPEED;
    check(
        moved <= max_speed * 3.5,
        &format!(
            "a flood of oversized inputs moved {moved:.1} m in ~3 s (max speed {max_speed} m/s)"
        ),
    )?;
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| {
        bots.iter().all(|b| b.mirror.party.is_none())
    });
    Ok(())
}

/// A completed run's rewards reach the persistent database exactly once, the key owner's key
/// goes up by exactly one level, and everything survives a reconnect.
///
/// Completion is triggered with the owner-only `admin_finish_instance` through the
/// `spacetime` CLI (set `SPACETIME_CLI` if it is not on PATH).
pub fn results(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: results");
    let tag = run_tag();
    let mut bots = trio(&tag, "results", endpoints, false, true)?;
    let active = drive(&mut bots, Duration::from_secs(45), all_active);
    check(active, "the run became active")?;
    let instance = bots[0]
        .mirror
        .instance
        .as_ref()
        .map(|i| i.id)
        .ok_or("no instance")?;
    // Bot 0 is the key owner (it inserted its key to lead).
    let before: Vec<u64> = bots
        .iter()
        .map(|b| b.character().map_or(0, |c| c.experience))
        .collect();
    let levels_before: Vec<u32> = bots
        .iter()
        .map(|b| b.character().map_or(0, |c| c.level))
        .collect();
    let keys_before: Vec<u32> = bots.iter().map(key_level_of).collect();
    let dust_before: Vec<u64> = bots.iter().map(|b| resource_of(b, "crypt_dust")).collect();
    check(keys_before[0] >= 1, "the key owner has a key")?;

    finish_instance(endpoints, instance, true)?;
    let completed = drive(&mut bots, Duration::from_secs(5), |bots| {
        bots.iter().all(|b| {
            b.mirror
                .instance
                .as_ref()
                .is_some_and(|i| i.state == InstanceState::Completed)
        })
    });
    check(completed, "every client sees the run completed")?;
    let summaries = drive(&mut bots, Duration::from_secs(5), |bots| {
        bots.iter().all(|b| b.mirror.run_summary.is_some())
    });
    check(summaries, "every client received its run summary")?;
    let summary = bots[0].mirror.run_summary.clone().ok_or("no summary")?;
    check(summary.success, "the summary reports a success")?;
    check(
        summary.key_owner && summary.key_level == keys_before[0],
        "the owner's summary shows the key level the run used",
    )?;
    check(
        summary.key_level_after == keys_before[0] + 1,
        "the owner's summary shows the key one level higher",
    )?;
    check(
        bots[1..]
            .iter()
            .all(|b| b.mirror.run_summary.as_ref().is_some_and(|s| !s.key_owner)),
        "the other members' summaries are not the key owner's",
    )?;
    let dust_reward = summary
        .resources
        .iter()
        .find(|(id, _)| id == "crypt_dust")
        .map_or(0, |(_, amount)| u64::from(*amount));
    check(dust_reward > 0, "the summary lists a crypt_dust reward")?;

    // The bridge collects every few seconds; poll the API until the key moved.
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        refetch(&mut bots, Duration::from_millis(800));
        if key_level_of(&bots[0]) > keys_before[0] {
            break;
        }
        if Instant::now() > deadline {
            return Err(format!(
                "FAILED: the result was not persisted (key still {})",
                key_level_of(&bots[0])
            ));
        }
    }
    let snapshot = |bots: &[Bot]| -> (Vec<u64>, Vec<u32>, Vec<u32>, Vec<u64>) {
        (
            bots.iter()
                .map(|b| b.character().map_or(0, |c| c.experience))
                .collect(),
            bots.iter()
                .map(|b| b.character().map_or(0, |c| c.level))
                .collect(),
            bots.iter().map(key_level_of).collect(),
            bots.iter().map(|b| resource_of(b, "crypt_dust")).collect(),
        )
    };
    let first = snapshot(&bots);
    check(
        first.2[0] == keys_before[0] + 1,
        "the owner's key went up by exactly one level",
    )?;
    check(
        first.2[1] == keys_before[1] && first.2[2] == keys_before[2],
        "the other members' keys did not change",
    )?;
    check(
        first.0.iter().zip(&before).all(|(a, b)| a > b)
            || first.1.iter().zip(&levels_before).all(|(a, b)| a > b),
        "the server-awarded experience was persisted for every member",
    )?;
    for index in 0..3 {
        if first.1[index] == levels_before[index] {
            check(
                first.0[index] - before[index] == u64::from(summary.experience),
                &format!("bot {index} received the summary's experience exactly"),
            )?;
        }
    }
    check(
        first
            .3
            .iter()
            .zip(&dust_before)
            .all(|(a, b)| a - b == dust_reward),
        "every member received the crypt_dust reward exactly once",
    )?;

    // Let the bridge run again: a re-delivered result must not be applied twice.
    drive(&mut bots, Duration::from_secs(7), |_| false);
    refetch(&mut bots, Duration::from_secs(1));
    check(
        snapshot(&bots) == first,
        "results are applied exactly once (experience, resources, key)",
    )?;

    // A reconnect: a fresh client for the owner sees the same summary and the same character.
    let owner_config = BotConfig {
        queue: false,
        insert_key: false,
        start_if_leader: false,
        ..bots[0].config.clone()
    };
    let mut again = Bot::start(owner_config, endpoints).map_err(|e| e.message)?;
    let back = drive(
        std::slice::from_mut(&mut again),
        Duration::from_secs(20),
        |b| b[0].mirror.run_summary.is_some() && b[0].character().is_some(),
    );
    check(back, "a fresh connection receives the run summary again")?;
    check(
        again.mirror.run_summary.as_ref() == Some(&summary),
        "the run summary is unchanged after reconnecting",
    )?;
    check(
        key_level_of(&again) == first.2[0] && resource_of(&again, "crypt_dust") == first.3[0],
        "the character's key and resources persist after reconnecting",
    )?;
    drop(again);
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| {
        bots.iter().all(|b| b.mirror.party.is_none())
    });
    Ok(())
}

/// The whole loop: three bots run a dungeon to its end, leave, queue again immediately and are
/// matched into a new party that is choosing a key - at the key level the run earned.
pub fn game_loop(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: loop");
    let tag = run_tag();
    let mut bots = trio(&tag, "loop", endpoints, false, true)?;
    let active = drive(&mut bots, Duration::from_secs(45), all_active);
    check(active, "the first run became active")?;
    let first_party = bots[0].mirror.party.as_ref().map(|p| p.id);
    let instance = bots[0]
        .mirror
        .instance
        .as_ref()
        .map(|i| i.id)
        .ok_or("no instance")?;
    let key_before = key_level_of(&bots[0]);
    finish_instance(endpoints, instance, true)?;
    let completed = drive(&mut bots, Duration::from_secs(5), |bots| {
        bots.iter().all(|b| {
            b.mirror
                .instance
                .as_ref()
                .is_some_and(|i| i.state == InstanceState::Completed)
        }) && bots.iter().all(|b| b.mirror.run_summary.is_some())
    });
    check(
        completed,
        "the run completed and every client has its summary",
    )?;

    // Wait for the rewards to land so the next key reflects the run.
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        refetch(&mut bots, Duration::from_millis(800));
        if key_level_of(&bots[0]) > key_before {
            break;
        }
        if Instant::now() > deadline {
            return Err("FAILED: the run's key progression was not persisted".into());
        }
    }
    check(
        key_level_of(&bots[0]) == key_before + 1,
        "the owner's key advanced one level",
    )?;

    // Return to the hub, then queue again at once (the bots queue by themselves when idle).
    bots[0].config.start_if_leader = false;
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    let left = drive(&mut bots, Duration::from_secs(5), |bots| {
        bots.iter()
            .all(|b| b.mirror.party.is_none() && b.mirror.instance.is_none())
    });
    check(left, "everyone left the finished run")?;
    let rematched = drive(&mut bots, Duration::from_secs(45), |bots| {
        bots.iter().all(|b| {
            b.mirror
                .party
                .as_ref()
                .is_some_and(|p| p.state == PartyState::ChoosingKey && Some(p.id) != first_party)
        })
    });
    check(
        rematched,
        "queuing again formed a new party that is choosing a key",
    )?;
    let ready = drive(&mut bots, Duration::from_secs(10), |bots| {
        bots.iter().all(|b| {
            b.mirror
                .party
                .as_ref()
                .is_some_and(|p| p.state == PartyState::Ready)
        })
    });
    check(
        ready,
        "the designated bot inserted its key in the new party",
    )?;
    check(
        bots[0]
            .mirror
            .party
            .as_ref()
            .is_some_and(|p| p.key_level == key_before + 1),
        "the new party's key is the level the last run earned",
    )?;
    for bot in &mut bots {
        bot.config.queue = false;
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(3), |bots| {
        bots.iter().all(|b| b.mirror.party.is_none())
    });
    Ok(())
}

/// Server combat: the realm runs the fight. Every player has combat state; presses become
/// damage the server decides and every client hears about; monsters fight back.
pub fn combat(endpoints: &RealmEndpoints) -> Outcome {
    use engine_realm::combat::{EntityRef, WireEvent};
    log::info!("scenario: combat");
    let tag = run_tag();
    let mut bots = trio(&tag, "combat", endpoints, false, true)?;
    let active = drive(&mut bots, Duration::from_secs(45), all_active);
    check(active, "the party reached an active instance")?;
    let rows = drive(&mut bots, Duration::from_secs(3), |bots| {
        bots.iter().all(|b| b.mirror.combat.len() == 3)
    });
    check(rows, "every client receives every player's combat state")?;
    check(
        bots.iter().all(|b| {
            b.mirror
                .my_combat()
                .is_some_and(|c| !c.cooldowns.is_empty())
        }),
        "each player's class cooldowns come from the server",
    )?;
    check(
        bots[0]
            .mirror
            .monsters
            .values()
            .all(|m| m.alive && m.health > 0.0),
        "the server's monsters start alive",
    )?;

    // The healer and the dps hunt; their slot 0 is Moonfire and Firebolt.
    bots[1].config.hunt = true;
    bots[2].config.hunt = true;
    let dps = bots[2]
        .session
        .as_ref()
        .map(|s| s.account_id.clone())
        .unwrap_or_default();
    let hit = drive(&mut bots, Duration::from_secs(45), |bots| {
        bots[0].mirror.events.iter().any(|event| {
            matches!(event, WireEvent::Damage { source: EntityRef::Player(_), target: EntityRef::Monster(_), amount, .. } if *amount > 0.0)
        })
    });
    check(
        hit,
        "a press became server-side damage, and a party member's client heard of it",
    )?;
    let hurt = bots[0]
        .mirror
        .monsters
        .values()
        .any(|m| m.health < m.max_health);
    check(hurt, "a monster's row shows the damage")?;
    let fought = drive(&mut bots, Duration::from_secs(15), |bots| {
        bots[0]
            .mirror
            .monsters
            .values()
            .any(|m| matches!(m.target, EntityRef::Player(_)))
    });
    check(
        fought,
        "the monster turned on the party (server AI and threat)",
    )?;
    let dps_damage = bots[2].mirror.events.iter().any(|event| {
        matches!(event, WireEvent::Damage { source: EntityRef::Player(p), .. } if *p == dps)
    });
    check(dps_damage || hit, "the dps's own client saw its damage")?;
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| {
        bots.iter().all(|b| b.mirror.party.is_none())
    });
    Ok(())
}

/// Whether `bot` has been shown a chat line with exactly this text.
fn heard<'a>(bot: &'a Bot, text: &str) -> Option<&'a engine_net::model::ChatMessageInfo> {
    bot.mirror.chat.iter().find(|m| m.text == text)
}

/// Whether the server refused `bot`'s chat with a refusal of `category`.
fn chat_refused(bot: &Bot, category: ErrorCategory) -> bool {
    bot.failures
        .iter()
        .any(|(action, error)| *action == Action::Chat && error.category == category)
}

fn said(bot: &Bot, channel: Channel, target: &str, text: &str) {
    bot.send(NetCommand::SendChat {
        channel,
        target: target.to_owned(),
        text: text.to_owned(),
    });
}

fn noticed(bot: &Bot, fragment: &str) -> bool {
    bot.mirror
        .notices
        .iter()
        .any(|notice| notice.message.contains(fragment))
}

/// Chat: a Server line reaches everyone, a Party line only the party, a whisper only its two
/// ends; the sender's name is the server's, a whisper to nobody or oneself is refused with a
/// notice, text is cleaned and bounded, and a flood is rate limited.
///
/// Three bots form a party (and stay in its key selection); a fourth, alone in the queue,
/// is the outsider.
pub fn chat(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: chat");
    let tag = run_tag();
    let mut bots = trio(&tag, "chat", endpoints, false, false)?;
    let formed = drive(&mut bots, Duration::from_secs(40), |bots| {
        bots.iter()
            .all(|b| b.mirror.party.is_some() && b.mirror.members.len() == 3)
    });
    check(formed, "the three bots formed a party")?;
    bots.push(
        Bot::start(
            BotConfig {
                name: "chat-outsider".into(),
                email: format!("chat-outsider-{tag}@bots.mygame.test"),
                password: "bot-password-1".into(),
                definition: "firemage".into(),
                role: Role::Dps,
                dungeon: "graveyard_crypt".into(),
                accept: true,
                insert_key: false,
                start_if_leader: false,
                walk: false,
                hunt: false,
                queue: true,
            },
            endpoints,
        )
        .map_err(|e| e.message)?,
    );
    let queued = drive(&mut bots, Duration::from_secs(30), |bots| {
        bots[3].mirror.ticket.is_some() && bots[3].mirror.party.is_none()
    });
    check(queued, "the outsider is queued alone, with no party")?;
    let settle = |bots: &mut [Bot]| drive(bots, Duration::from_secs(2), |_| false);

    // Server: everyone, under the sender's own account name. Character names are the hero's
    // ("Fire Mage") and shared, so the player's name is the account's display name.
    let server_text = format!("hello realm {tag}");
    said(&bots[3], Channel::Server, "", &server_text);
    let everyone = drive(&mut bots, Duration::from_secs(10), |bots| {
        bots.iter().all(|b| heard(b, &server_text).is_some())
    });
    check(everyone, "a Server message reached all four players")?;
    let line = heard(&bots[0], &server_text);
    let outsider_name = line.map(|m| m.sender_name.clone()).unwrap_or_default();
    check(
        !outsider_name.is_empty()
            && bots
                .iter()
                .all(|b| heard(b, &server_text).is_some_and(|m| m.sender_name == outsider_name)),
        "the sender's name is the server's, the same for every reader",
    )?;
    check(
        line.is_some_and(|m| Some(m.sender_account.as_str()) == bots[3].mirror.my_account_id()),
        "the sender's account is the connection's own",
    )?;

    // Party: only the party.
    let party_text = format!("party line {tag}");
    said(&bots[0], Channel::Party, "", &party_text);
    let party_heard = drive(&mut bots, Duration::from_secs(10), |bots| {
        bots[..3].iter().all(|b| heard(b, &party_text).is_some())
    });
    check(party_heard, "a Party message reached the whole party")?;
    let leader_name = heard(&bots[1], &party_text)
        .map(|m| m.sender_name.clone())
        .unwrap_or_default();
    check(
        !leader_name.is_empty() && leader_name != outsider_name,
        "players speak under distinct names",
    )?;
    settle(&mut bots);
    check(
        heard(&bots[3], &party_text).is_none(),
        "the outsider did not see the Party message",
    )?;
    said(&bots[3], Channel::Party, "", "party? I have none");
    settle(&mut bots);
    check(
        chat_refused(&bots[3], ErrorCategory::Party),
        "a Party message from outside a party was refused",
    )?;
    check(
        bots.iter()
            .all(|b| heard(b, "party? I have none").is_none()),
        "and reached no one",
    )?;

    // Whisper: its two ends.
    let whisper_text = format!("psst {tag}");
    said(
        &bots[0],
        Channel::Whisper,
        &outsider_name.to_uppercase(),
        &whisper_text,
    );
    let whispered = drive(&mut bots, Duration::from_secs(10), |bots| {
        heard(&bots[0], &whisper_text).is_some() && heard(&bots[3], &whisper_text).is_some()
    });
    check(
        whispered,
        "the whisper reached the recipient and the sender",
    )?;
    settle(&mut bots);
    check(
        heard(&bots[1], &whisper_text).is_none() && heard(&bots[2], &whisper_text).is_none(),
        "the sender's party mates did not see the whisper",
    )?;
    check(
        bots[3].mirror.last_whisperer() == Some(leader_name.as_str()),
        "the recipient can reply to the whisperer",
    )?;
    check(
        heard(&bots[0], &whisper_text)
            .is_some_and(|m| m.recipient_name.as_deref() == Some(outsider_name.as_str())),
        "the whisper names its recipient",
    )?;

    // Refused whispers say so in a notice, and are heard by no one.
    let nobody = format!("Nobody{tag}");
    said(&bots[0], Channel::Whisper, &nobody, "anyone there?");
    let told = drive(&mut bots, Duration::from_secs(10), |bots| {
        noticed(&bots[0], &format!("No player named {nobody} is online."))
    });
    check(told, "a whisper to nobody left a notice for the sender")?;
    said(
        &bots[0],
        Channel::Whisper,
        &leader_name.to_uppercase(),
        "me myself",
    );
    let told = drive(&mut bots, Duration::from_secs(10), |bots| {
        noticed(&bots[0], "You cannot whisper yourself.")
    });
    check(told, "a whisper to oneself left a notice")?;
    settle(&mut bots);
    check(
        bots.iter()
            .all(|b| heard(b, "anyone there?").is_none() && heard(b, "me myself").is_none()),
        "refused whispers were stored for no one",
    )?;

    // Text is cleaned and bounded.
    let dirty = format!("a\u{7}b\tc {tag}");
    said(&bots[1], Channel::Server, "", &dirty);
    let clean = format!("abc {tag}");
    let cleaned = drive(&mut bots, Duration::from_secs(10), |bots| {
        heard(&bots[0], &clean).is_some()
    });
    check(cleaned, "control characters were stripped from the text")?;
    said(&bots[1], Channel::Server, "", &"x".repeat(201));
    settle(&mut bots);
    check(
        chat_refused(&bots[1], ErrorCategory::Protocol),
        "a message over 200 characters was refused",
    )?;
    check(
        bots[0].mirror.chat.iter().all(|m| m.text.len() <= 200),
        "and stored for no one",
    )?;

    // A flood is held to the rate.
    for i in 0..12 {
        said(&bots[2], Channel::Server, "", &format!("flood {i} {tag}"));
    }
    settle(&mut bots);
    check(
        chat_refused(&bots[2], ErrorCategory::RateLimited),
        "a flood of Server messages was rate limited",
    )?;
    let landed = (0..12)
        .filter(|i| heard(&bots[0], &format!("flood {i} {tag}")).is_some())
        .count();
    check(landed < 12, "not every flooded message was delivered")?;

    for bot in &mut bots {
        bot.config.queue = false;
        bot.send(NetCommand::LeaveParty);
        bot.send(NetCommand::LeaveQueue);
    }
    drive(&mut bots, Duration::from_secs(2), |_| false);
    Ok(())
}

/// Asks the API for every bot's character in full (bag and mail included).
fn fetch_details(bots: &mut [Bot], wait: Duration) {
    for bot in bots.iter() {
        if let Some(character) = bot.character() {
            bot.send(NetCommand::FetchCharacter(character.id.clone()));
        }
    }
    drive(bots, wait, |_| false);
}

/// Items of `bot`'s stored character that are chest or monster loot, as `(item, quality)`:
/// everything in the bag and mailbox whose key is a dungeon catalog item.
fn loot_held(bot: &Bot) -> Vec<(String, u8)> {
    engine_realm::loot::ensure_items_registered();
    let Some(detail) = bot.detail.as_ref() else {
        return Vec::new();
    };
    let catalog_item = |key: &str| {
        engine_game::character::item_by_key(key)
            .is_some_and(|def| def.id.0 >= engine_game::character::CATALOG_ID_BASE)
    };
    let mut held: Vec<(String, u8)> = detail
        .inventory
        .iter()
        .filter(|entry| catalog_item(&entry.item))
        .map(|entry| (entry.item.clone(), entry.quality))
        .collect();
    held.extend(
        detail
            .mail
            .iter()
            .filter(|mail| catalog_item(&mail.item))
            .map(|mail| (mail.item.clone(), mail.quality)),
    );
    held.sort();
    held
}

/// Calls the owner-only `admin_clear_instance` once.
fn clear_instance(endpoints: &RealmEndpoints, instance: u64) -> Outcome {
    let cli = std::env::var("SPACETIME_CLI").unwrap_or_else(|_| "spacetime".into());
    let output = std::process::Command::new(&cli)
        .args([
            "call",
            "--server",
            &endpoints.spacetimedb_url,
            &endpoints.spacetimedb_database,
            "admin_clear_instance",
            &instance.to_string(),
        ])
        .output()
        .map_err(|e| format!("could not run {cli}: {e}"))?;
    check(output.status.success(), "the owner cleared the instance")
}

/// Loot end to end: the party kills everything (so the run completes with its real chest),
/// every player's summary lists two chest items with qualities, and the character - after the
/// bridge has delivered - holds exactly those items with those qualities, the gold the
/// summary reports, and nothing twice.
pub fn loot(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: loot");
    let tag = run_tag();
    let mut bots = trio(&tag, "loot", endpoints, false, true)?;
    let active = drive(&mut bots, Duration::from_secs(45), all_active);
    check(active, "the run became active")?;
    let instance = bots[0]
        .mirror
        .instance
        .as_ref()
        .map(|i| i.id)
        .ok_or("no instance")?;
    fetch_details(&mut bots, Duration::from_millis(1500));
    let gold_before: Vec<u64> = bots.iter().map(|b| resource_of(b, "gold")).collect();
    let held_before: Vec<usize> = bots.iter().map(|b| loot_held(b).len()).collect();

    // The ordinary ticks complete the run once the monsters are dead.
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        clear_instance(endpoints, instance)?;
        let done = drive(&mut bots, Duration::from_secs(1), |bots| {
            bots.iter().all(|b| {
                b.mirror
                    .instance
                    .as_ref()
                    .is_some_and(|i| i.state == InstanceState::Completed)
                    && b.mirror.run_summary.is_some()
            })
        });
        if done {
            break;
        }
        if Instant::now() > deadline {
            return Err("FAILED: the run did not complete after the monsters were killed".into());
        }
    }
    // Chest items are written at the end of the run; the summary lists them.
    let mut expected: Vec<Vec<(String, u8)>> = Vec::new();
    let mut gold_looted: Vec<u64> = Vec::new();
    for bot in &bots {
        let summary = bot.mirror.run_summary.as_ref().ok_or("no summary")?;
        check(
            summary.loot.len() == 2,
            "the summary lists two chest items per player",
        )?;
        check(
            summary
                .loot
                .iter()
                .all(|line| (50..=110).contains(&line.quality)),
            "every chest item has a rolled quality between 50 and 110",
        )?;
        let mut items: Vec<(String, u8)> = summary
            .loot
            .iter()
            .map(|line| (line.item.clone(), line.quality))
            .collect();
        items.sort();
        expected.push(items);
        let gold = summary
            .looted
            .iter()
            .find(|(id, _)| id == "gold")
            .map_or(0, |(_, amount)| u64::from(*amount));
        check(gold > 0, "the boss and the kills paid gold")?;
        gold_looted.push(gold);
    }

    // Nobody opens the chest: leaving mails it, with the monsters' drops, and the bridge
    // delivers everything.
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        fetch_details(&mut bots, Duration::from_millis(1500));
        let landed = bots.iter().enumerate().all(|(i, b)| {
            resource_of(b, "gold") >= gold_before[i] + gold_looted[i]
                && loot_held(b).len() >= held_before[i] + 2
        });
        if landed {
            break;
        }
        if Instant::now() > deadline {
            return Err("FAILED: the loot and gold were not delivered".into());
        }
    }
    for (index, bot) in bots.iter().enumerate() {
        check(
            resource_of(bot, "gold") == gold_before[index] + gold_looted[index],
            &format!("bot {index} was credited exactly the summary's gold"),
        )?;
        let held = loot_held(bot);
        for item in &expected[index] {
            check(
                held.contains(item),
                &format!("bot {index} holds {item:?} with its rolled quality"),
            )?;
        }
    }
    // Exactly once: more bridge passes change nothing.
    let snapshot: Vec<(u64, usize)> = bots
        .iter()
        .map(|b| (resource_of(b, "gold"), loot_held(b).len()))
        .collect();
    drive(&mut bots, Duration::from_secs(8), |_| false);
    fetch_details(&mut bots, Duration::from_millis(1500));
    let after: Vec<(u64, usize)> = bots
        .iter()
        .map(|b| (resource_of(b, "gold"), loot_held(b).len()))
        .collect();
    check(
        after == snapshot,
        "loot and gold are delivered exactly once",
    )?;
    Ok(())
}

/// Calls an owner-only reducer through the `spacetime` CLI (set `SPACETIME_CLI` if it is not
/// on PATH).
fn admin_call(endpoints: &RealmEndpoints, function: &str, args: &[String]) -> Outcome {
    let cli = std::env::var("SPACETIME_CLI").unwrap_or_else(|_| "spacetime".into());
    let output = std::process::Command::new(&cli)
        .args([
            "call",
            "--server",
            &endpoints.spacetimedb_url,
            &endpoints.spacetimedb_database,
            function,
        ])
        .args(args)
        .output()
        .map_err(|e| format!("could not run {cli}: {e}"))?;
    check(
        output.status.success(),
        &format!("the owner called {function}"),
    )
}

/// Wins the current run (admin finish), waits for the keys to be saved, and sends everyone back
/// to the hub; the bots then queue again by themselves.
fn win_and_requeue(bots: &mut [Bot], endpoints: &RealmEndpoints) -> Outcome {
    let instance = bots[0]
        .mirror
        .instance
        .as_ref()
        .map(|i| i.id)
        .ok_or("no instance")?;
    let before = key_level_of(&bots[0]);
    finish_instance(endpoints, instance, true)?;
    let done = drive(bots, Duration::from_secs(5), |bots| {
        bots.iter().all(|b| b.mirror.run_summary.is_some())
    });
    check(done, "the run completed and every client has its summary")?;
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        refetch(bots, Duration::from_millis(800));
        if key_level_of(&bots[0]) > before {
            break;
        }
        if Instant::now() > deadline {
            return Err("FAILED: the run's key progression was not persisted".into());
        }
    }
    for bot in bots.iter() {
        bot.send(NetCommand::LeaveParty);
    }
    let left = drive(bots, Duration::from_secs(5), |bots| {
        bots.iter()
            .all(|b| b.mirror.party.is_none() && b.mirror.instance.is_none())
    });
    check(left, "everyone left the finished run")
}

/// Key modifiers end to end: keys that carry modifiers, shown to the whole party before the
/// start; the reroll refused before half the dungeon is dead and after the run's one; the
/// currency and the reward bonus; participants' keys rising with the run.
///
/// Needs the realm owner's `spacetime` CLI (like `results`): it clears half the dungeon with
/// `admin_clear_dungeon` and grants the reroll currency with `admin_grant_spend`.
pub fn modifiers(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: modifiers");
    let tag = run_tag();
    let mut bots = trio(&tag, "mods", endpoints, false, true)?;
    // Two wins take the tank's key from level 1 to 3 (the first level with a modifier) and, as
    // participants, lift the others' keys from 1 to 2 (the second run used a level above theirs).
    for run in 0..2 {
        let active = drive(&mut bots, Duration::from_secs(60), all_active);
        check(active, &format!("run {} became active", run + 1))?;
        win_and_requeue(&mut bots, endpoints)?;
    }
    check(
        key_level_of(&bots[0]) == 3,
        "the owner's key reached level 3",
    )?;
    check(
        bots[0]
            .character()
            .and_then(|c| c.key.as_ref())
            .is_some_and(|key| key.modifiers.len() == 1),
        "a level 3 key carries one modifier, rolled and stored by the API",
    )?;
    check(
        bots[1..].iter().all(|b| key_level_of(b) == 2),
        "a participant whose key was lower than the one used rose one level, once",
    )?;

    // The new party: every member sees the tank's key and its modifier before anything starts.
    let seen = drive(&mut bots, Duration::from_secs(60), |bots| {
        bots.iter().all(|b| {
            b.mirror
                .members
                .iter()
                .any(|m| m.key_level == 3 && m.key_modifiers.len() == 1)
        })
    });
    check(
        seen,
        "every member sees the tank's key and its modifiers before the start",
    )?;
    let active = drive(&mut bots, Duration::from_secs(60), all_active);
    check(active, "the third run became active")?;
    let instance = bots[0].mirror.instance.clone().ok_or("no instance")?;
    check(
        instance.modifiers.len() == 1 && instance.rerolls == 0 && instance.reward_bonus_pct == 0,
        "the instance carries the inserted key's modifier, unrolled and with no bonus",
    )?;

    // Before half the dungeon is dead the reroll is refused.
    let failures = bots[1].failures.len();
    bots[1].send(NetCommand::RerollModifier(0));
    drive(&mut bots, Duration::from_secs(2), |_| false);
    check(
        bots[1].failures.len() > failures
            && refused(&bots[1], Action::Reroll) == Some(ErrorCategory::Dungeon),
        "a reroll before half the dungeon is cleared is refused",
    )?;

    // Half the dungeon dies; without currency the reroll is still refused.
    admin_call(
        endpoints,
        "admin_clear_dungeon",
        &[instance.id.to_string(), "50".into()],
    )?;
    let unlocked = drive(&mut bots, Duration::from_secs(10), |bots| {
        let (mut killed, mut total) = (0u32, 0u32);
        // Counted as the realm counts it (and the Modifier panel shows it): placed monsters,
        // the boss left out.
        for m in bots[1].mirror.monsters.values() {
            let boss = engine_game::enemy::EnemyKind::from_id(&m.archetype)
                .is_some_and(engine_game::enemy::EnemyKind::is_boss);
            if m.key >= engine_realm::combat::instance::SUMMONED_KEY_BASE || boss {
                continue;
            }
            total += 1;
            killed += u32::from(!m.alive);
        }
        total > 0 && killed * 2 >= total
    });
    check(unlocked, "half the placed monsters are dead on the clients")?;
    let failures = bots[1].failures.len();
    bots[1].send(NetCommand::RerollModifier(0));
    drive(&mut bots, Duration::from_secs(2), |_| false);
    check(
        bots[1].failures.len() > failures,
        "a reroll without the currency is refused",
    )?;

    // With currency it works once: the server rolls, everyone is told, the bonus is set.
    let healer = bots[1]
        .session
        .as_ref()
        .map(|s| s.account_id.clone())
        .ok_or("no session")?;
    admin_call(
        endpoints,
        "admin_grant_spend",
        &[instance.id.to_string(), healer, "3".into()],
    )?;
    drive(&mut bots, Duration::from_secs(2), |_| false);
    bots[1].send(NetCommand::RerollModifier(0));
    let rerolled = drive(&mut bots, Duration::from_secs(5), |bots| {
        bots.iter().all(|b| {
            b.mirror
                .instance
                .as_ref()
                .is_some_and(|i| i.rerolls == 1 && i.last_reroll.is_some())
        })
    });
    check(
        rerolled,
        "the reroll happened and every client saw its result",
    )?;
    let after = bots[0].mirror.instance.clone().ok_or("no instance")?;
    check(
        after.reward_bonus_pct == 25,
        "a reroll sets the +25% reward bonus for the run",
    )?;
    check(
        after.last_reroll.as_ref().is_some_and(|r| {
            r.delta_pct != 0 && r.magnitude_pct == after.modifiers[0].magnitude_pct
        }),
        "the instance's modifier carries the rolled magnitude the clients animate",
    )?;
    // The spend row arrives through its own view, possibly a beat after the instance's.
    let spent = drive(&mut bots, Duration::from_secs(5), |bots| {
        bots[1].mirror.spend.as_ref().is_some_and(|s| s.spent == 1)
    });
    check(spent, "the roller's spend row says one currency was spent")?;
    check(
        bots.iter().all(|b| {
            b.mirror
                .notices
                .iter()
                .any(|n| n.code == "modifier_rerolled")
        }),
        "the whole party got the reroll notice",
    )?;
    let failures = bots[2].failures.len();
    bots[2].send(NetCommand::RerollModifier(0));
    drive(&mut bots, Duration::from_secs(2), |_| false);
    check(
        bots[2].failures.len() > failures,
        "a second reroll in the same run is refused",
    )?;

    // Winning pays; the participants' keys rise again.
    win_and_requeue(&mut bots, endpoints)?;
    check(
        key_level_of(&bots[0]) == 4,
        "the owner's key rose to level 4",
    )?;
    check(
        bots[1..].iter().all(|b| key_level_of(b) == 3),
        "the participants' keys rose to 3 with the run",
    )?;
    for bot in &mut bots {
        bot.config.queue = false;
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(3), |bots| {
        bots.iter().all(|b| b.mirror.party.is_none())
    });
    Ok(())
}

/// Run history and leaderboards: a completed run appears once in every member's history with
/// the party and the realm's outcome, in the leaderboards, and as a run only its members may
/// open.
pub fn history(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: history");
    let tag = run_tag();
    let mut bots = trio(&tag, "history", endpoints, false, true)?;
    check(
        drive(&mut bots, Duration::from_secs(45), all_active),
        "the run became active",
    )?;
    let instance = bots[0]
        .mirror
        .instance
        .as_ref()
        .map(|i| i.id)
        .ok_or("no instance")?;
    let key_level = key_level_of(&bots[0]);
    finish_instance(endpoints, instance, true)?;
    check(
        drive(&mut bots, Duration::from_secs(5), |bots| {
            bots.iter().all(|b| b.mirror.run_summary.is_some())
        }),
        "every client received its run summary",
    )?;

    // The bridge persists within a few seconds; ask every member for their history until it
    // holds the run.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        for bot in bots.iter_mut() {
            bot.history = None;
            if let Some(character) = bot.character().map(|c| c.id.clone()) {
                bot.send(NetCommand::FetchHistory {
                    character_id: character,
                    before: None,
                    limit: 10,
                });
            }
        }
        drive(&mut bots, Duration::from_millis(900), |_| false);
        if bots
            .iter()
            .all(|b| b.history.as_ref().is_some_and(|h| !h.runs.is_empty()))
        {
            break;
        }
        if Instant::now() > deadline {
            return Err("FAILED: the run never reached the history".into());
        }
    }
    let entries: Vec<_> = bots
        .iter()
        .filter_map(|b| b.history.as_ref().and_then(|h| h.runs.first().cloned()))
        .collect();
    check(
        bots.iter()
            .all(|b| b.history.as_ref().is_some_and(|h| h.runs.len() == 1)),
        "each member's history lists the run once",
    )?;
    let uid = entries[0].run_uid.clone();
    check(
        uid.len() == 32 && entries.iter().all(|e| e.run_uid == uid),
        "all three members share the run's uid",
    )?;
    check(
        entries.iter().all(|e| {
            e.outcome == engine_realm::outcome::RunOutcome::Timed
                && e.key_level == key_level
                && e.dungeon_id == "graveyard_crypt"
                && e.party.len() == 3
        }),
        "the entry is timed, at the key used, in the crypt, with the whole party",
    )?;
    let mut roles: Vec<String> = entries[0].party.iter().map(|m| m.role.clone()).collect();
    roles.sort();
    check(
        roles == ["dps", "healer", "tank"],
        "the party lists a tank, a healer and a DPS",
    )?;

    // The run, for a member; and nothing for a stranger.
    bots[0].send(NetCommand::FetchRun(uid.clone()));
    drive(&mut bots, Duration::from_secs(3), |bots| bots[0].run.is_some());
    check(
        bots[0]
            .run
            .as_ref()
            .is_some_and(|run| run.run_uid == uid && run.party.len() == 3 && run.you.is_some()),
        "a member can open the run",
    )?;
    let mut stranger = Bot::start(
        BotConfig {
            name: "history-stranger".into(),
            email: format!("history-stranger-{tag}@bots.mygame.test"),
            queue: false,
            insert_key: false,
            start_if_leader: false,
            ..bots[0].config.clone()
        },
        endpoints,
    )
    .map_err(|e| e.message)?;
    check(
        drive(std::slice::from_mut(&mut stranger), Duration::from_secs(20), |b| {
            b[0].character().is_some()
        }),
        "the stranger signed in",
    )?;
    let token = stranger
        .acquire_probe_token(endpoints)
        .map_err(|e| e.message)?;
    check(
        call_api(
            endpoints,
            &token,
            "GET",
            &format!("/api/v1/runs/{uid}"),
            &serde_json::Value::Null,
        ) == 404,
        "a stranger cannot open the run",
    )?;
    let other = bots[0].character().map(|c| c.id.clone()).unwrap_or_default();
    check(
        call_api(
            endpoints,
            &token,
            "GET",
            &format!("/api/v1/characters/{other}/history"),
            &serde_json::Value::Null,
        ) == 404,
        "a stranger cannot read another character's history",
    )?;

    // Leaderboards: any signed-in player sees the run on both boards.
    stranger.send(NetCommand::FetchLeaderboard {
        board: "highest".into(),
        dungeon_id: "graveyard_crypt".into(),
        key_level: None,
        page: 0,
    });
    drive(std::slice::from_mut(&mut stranger), Duration::from_secs(5), |b| {
        b[0].board.is_some()
    });
    check(
        stranger
            .board
            .as_ref()
            .is_some_and(|board| board.rows.iter().any(|row| row.run_uid == uid)),
        "the highest board lists the run (one row for the three players)",
    )?;
    stranger.board = None;
    stranger.send(NetCommand::FetchLeaderboard {
        board: "fastest".into(),
        dungeon_id: "graveyard_crypt".into(),
        key_level: Some(key_level),
        page: 0,
    });
    drive(std::slice::from_mut(&mut stranger), Duration::from_secs(5), |b| {
        b[0].board.is_some()
    });
    check(
        stranger.board.as_ref().is_some_and(|board| {
            board.rows.iter().filter(|row| row.run_uid == uid).count() == 1
        }),
        "the fastest board lists the run exactly once",
    )?;
    drop(stranger);
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| {
        bots.iter().all(|b| b.mirror.party.is_none())
    });
    Ok(())
}
