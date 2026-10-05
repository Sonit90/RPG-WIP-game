//! End-to-end scenarios against a running realm (SpacetimeDB + realm API).
//!
//! Each scenario starts real bots over the real network layer, drives them until a condition
//! holds or a deadline passes, and checks what the *server* says. A failed check returns an
//! error naming it; `realm-bot scenario all` exits non-zero on the first one.

use std::time::{Duration, Instant};

use engine_core::math::Vec3;
use engine_net::{Action, NetCommand, RealmEndpoints};
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

/// Three bots, one per role.
fn trio(tag: &str, prefix: &str, endpoints: &RealmEndpoints, walk: bool, start: bool) -> Result<Vec<Bot>, String> {
    let roles = [
        ("tank", "warrior", Role::Tank),
        ("healer", "druid", Role::Healer),
        ("dps", "firemage", Role::Dps),
    ];
    roles
        .iter()
        .map(|(label, definition, role)| {
            Bot::start(
                BotConfig {
                    name: format!("{prefix}-{label}"),
                    email: format!("{prefix}-{label}-{tag}@bots.mygame.test"),
                    password: "bot-password-1".into(),
                    definition: (*definition).into(),
                    role: *role,
                    dungeon: "graveyard_crypt".into(),
                    accept: true,
                    start_if_leader: start,
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

fn all_active(bots: &[Bot]) -> bool {
    bots.iter().all(|bot| {
        bot.mirror
            .instance
            .as_ref()
            .is_some_and(|i| i.state == InstanceState::Active && i.barrier_open)
    })
}

/// The vertical slice: log in, queue 1/1/1, match, ready check, random leader, start,
/// countdown, barrier, authoritative movement seen by everyone.
pub fn slice(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: slice");
    let tag = run_tag();
    let mut bots = trio(&tag, "slice", endpoints, true, true)?;

    let formed = drive(&mut bots, Duration::from_secs(30), |bots| {
        bots.iter().all(|b| b.mirror.members.len() == 3)
    });
    check(formed, "the server formed a party of three")?;
    let party_ids: Vec<Option<u64>> = bots.iter().map(|b| b.mirror.party.as_ref().map(|p| p.id)).collect();
    check(party_ids.windows(2).all(|w| w[0] == w[1]), "all three are in the same party")?;
    let roles: Vec<Role> = bots[0].mirror.members.iter().map(|m| m.role).collect();
    check(roles.contains(&Role::Tank) && roles.contains(&Role::Healer) && roles.contains(&Role::Dps), "exactly one tank, one healer, one dps")?;

    let started = drive(&mut bots, Duration::from_secs(20), |bots| {
        bots.iter().all(|b| b.mirror.instance.is_some())
    });
    check(started, "the leader started the dungeon")?;
    let leader = bots[0].mirror.party.as_ref().and_then(|p| p.leader.clone());
    check(leader.is_some(), "the server assigned a leader")?;
    check(bots.iter().filter(|b| b.mirror.i_lead()).count() == 1, "exactly one bot leads")?;
    let countdown = bots[0].mirror.instance.as_ref().and_then(|i| i.countdown_ends_at);
    check(countdown.is_some(), "the instance carries a server countdown timestamp")?;
    check(
        bots.iter().all(|b| b.mirror.instance.as_ref().is_some_and(|i| !i.barrier_open || i.state == InstanceState::Active)),
        "the barrier is closed until the run is active",
    )?;

    let began = Instant::now();
    let active = drive(&mut bots, Duration::from_secs(12), all_active);
    check(active, "the barrier lifted and the run became active")?;
    check(began.elapsed() >= Duration::from_secs(3), "the countdown took the server's time, not zero")?;
    check(bots.iter().all(|b| b.mirror.players.len() == 3), "every client sees three players")?;
    check(bots.iter().all(|b| b.mirror.monsters.len() > 10), "every client sees the server's monsters")?;

    drive(&mut bots, Duration::from_secs(3), |_| false);
    for bot in &bots {
        let start = bot.start_position.unwrap_or(Vec3::ZERO);
        let now = bot.mirror.me().map_or(start, |me| me.position);
        check(start.z - now.z > 5.0, &format!("{} moved forward under server authority ({:.1} m)", bot.config.name, start.z - now.z))?;
    }
    // Everyone sees everyone else's authoritative movement.
    for viewer in &bots {
        for other in &bots {
            if viewer.session.as_ref().map(|s| &s.account_id) == other.session.as_ref().map(|s| &s.account_id) {
                continue;
            }
            let Some(account) = other.session.as_ref().map(|s| s.account_id.clone()) else { continue };
            let seen = viewer.mirror.players.get(&account).map(|p| p.position);
            let actual = other.mirror.me().map(|p| p.position);
            let agree = matches!((seen, actual), (Some(a), Some(b)) if (a - b).length() < 3.0);
            check(agree, &format!("{} sees {} where the server has them", viewer.config.name, other.config.name))?;
        }
    }
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| bots.iter().all(|b| b.mirror.party.is_none()));
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
    check(instances.len() == 2, "two parties got two separate instances")?;
    check(bots.iter().all(|b| b.mirror.players.len() == 3), "nobody receives the other instance's players")?;
    for bot in &bots {
        let mine = bot.mirror.instance.as_ref().map(|i| i.id);
        let partners = bots.iter().filter(|o| o.mirror.instance.as_ref().map(|i| i.id) == mine).count();
        check(partners == 3, &format!("{} shares its instance with exactly its party", bot.config.name))?;
    }
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| bots.iter().all(|b| b.mirror.party.is_none()));
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
    check(bots[1].mirror.ticket.is_none(), "the decliner left the queue")?;
    check(
        bots[0].mirror.ticket.is_some() && bots[2].mirror.ticket.is_some(),
        "the others are back in the queue",
    )?;
    for bot in &mut bots {
        bot.config.queue = false;
        bot.send(NetCommand::LeaveQueue);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| bots.iter().all(|b| b.mirror.ticket.is_none()));
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
    let account = gone.session.as_ref().map(|s| s.account_id.clone()).unwrap_or_default();
    let instance = gone.mirror.instance.as_ref().map(|i| i.id);
    let position = gone.mirror.me().map(|p| p.position);
    let config = gone.config.clone();
    drop(gone);

    let noticed = drive(&mut bots, Duration::from_secs(10), |bots| {
        bots.iter().all(|b| b.mirror.players.get(&account).is_some_and(|p| !p.present))
    });
    check(noticed, "the party sees the player as temporarily disconnected")?;
    check(bots.iter().all(|b| b.mirror.members.len() == 3), "the party kept the player (no replacement)")?;
    check(
        bots.iter().all(|b| b.mirror.instance.as_ref().is_some_and(|i| i.state == InstanceState::Active)),
        "the run continues",
    )?;

    let mut back = Bot::start(BotConfig { queue: false, ..config }, endpoints).map_err(|e| e.message)?;
    let resumed = drive(std::slice::from_mut(&mut back), Duration::from_secs(20), |bots| {
        bots[0].mirror.instance.as_ref().map(|i| i.id) == instance && bots[0].mirror.me().is_some()
    });
    check(resumed, "the player resumed in the same instance")?;
    check(back.session.as_ref().map(|s| &s.account_id) == Some(&account), "with the same account")?;
    let now = back.mirror.me().map(|p| p.position);
    check(
        matches!((position, now), (Some(a), Some(b)) if (a - b).length() < 1.0),
        "at the authoritative position it left",
    )?;
    bots.push(back);
    let present = drive(&mut bots, Duration::from_secs(5), |bots| {
        bots.iter().all(|b| b.mirror.players.get(&account).is_some_and(|p| p.present))
    });
    check(present, "the party sees the player present again")?;
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| bots.iter().all(|b| b.mirror.party.is_none()));
    Ok(())
}

/// Calls a realm function over SpacetimeDB's HTTP API, the way any HTTP client could.
fn call_realm(endpoints: &RealmEndpoints, token: &str, function: &str, args: &serde_json::Value) -> u16 {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
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
fn call_api(endpoints: &RealmEndpoints, token: &str, method: &str, path: &str, body: &serde_json::Value) -> u16 {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let url = format!("{}{path}", endpoints.persistence_api_url);
    let result = match method {
        "GET" => agent.get(url).header("Authorization", format!("Bearer {token}")).call(),
        _ => agent.post(url).header("Authorization", format!("Bearer {token}")).send_json(body),
    };
    result.map_or(0, |response| response.status().as_u16())
}

fn refused(bot: &Bot, action: Action) -> Option<ErrorCategory> {
    bot.failures.iter().rev().find(|(a, _)| *a == action).map(|(_, e)| e.category)
}

/// Every attempt a malicious client could make fails on the server.
pub fn security(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: security");
    let tag = run_tag();
    let mut bots = trio(&tag, "sec", endpoints, false, false)?;
    let ready = drive(&mut bots, Duration::from_secs(30), |bots| {
        bots.iter().all(|b| b.mirror.party.as_ref().is_some_and(|p| p.state == PartyState::Ready))
    });
    check(ready, "a party formed and is waiting for its leader")?;
    let leader = bots.iter().position(|b| b.mirror.i_lead()).ok_or("no leader")?;
    let other = (leader + 1) % 3;
    let target = bots[(leader + 2) % 3].session.as_ref().map(|s| s.account_id.clone()).unwrap_or_default();

    // Leader-only operations from a non-leader.
    bots[other].send(NetCommand::StartDungeon);
    bots[other].send(NetCommand::Kick(target.clone()));
    bots[other].send(NetCommand::TransferLeader(target.clone()));
    drive(&mut bots, Duration::from_secs(3), |_| false);
    check(refused(&bots[other], Action::StartDungeon) == Some(ErrorCategory::Authorization), "a non-leader cannot start the dungeon")?;
    check(refused(&bots[other], Action::Kick) == Some(ErrorCategory::Authorization), "a non-leader cannot kick")?;
    check(refused(&bots[other], Action::TransferLeader) == Some(ErrorCategory::Authorization), "a non-leader cannot transfer leadership")?;
    check(bots.iter().all(|b| b.mirror.members.len() == 3), "nobody was removed")?;

    // Leader kicking a stranger.
    bots[leader].send(NetCommand::Kick("00000000-0000-0000-0000-000000000000".into()));
    drive(&mut bots, Duration::from_secs(2), |_| false);
    check(refused(&bots[leader], Action::Kick).is_some(), "the leader cannot kick someone outside the party")?;

    // Answering a ready check that is not open.
    bots[other].send(NetCommand::RespondReadyCheck(false));
    drive(&mut bots, Duration::from_secs(2), |_| false);
    check(refused(&bots[other], Action::ReadyCheck).is_some(), "a closed ready check cannot be answered again")?;

    // Direct calls a modified client could make.
    let token = bots[other].acquire_probe_token(endpoints).map_err(|e| e.message)?;
    let account = bots[other].session.as_ref().map(|s| s.account_id.clone()).unwrap_or_default();
    let status = call_realm(
        endpoints,
        &token,
        "svc_enqueue",
        &serde_json::json!([account, "fake-character", "Cheater", "warrior", 60, 99999, "{}", "graveyard_crypt", {"tank": []}]),
    );
    check(module_refused(status), &format!("a player cannot enqueue with a fabricated gear score ({status})"))?;
    let status = call_realm(endpoints, &token, "admin_finish_instance", &serde_json::json!([1, true]));
    check(module_refused(status), &format!("a player cannot complete a dungeon ({status})"))?;
    let status = call_realm(endpoints, &token, "svc_ack_results", &serde_json::json!([["x"]]));
    check(module_refused(status), &format!("a player cannot clear the result outbox ({status})"))?;
    let status = call_realm(endpoints, &token, "admin_configure", &serde_json::json!(["http://evil", "x", "y"]));
    check(module_refused(status), &format!("a player cannot reconfigure the realm ({status})"))?;
    let status = call_realm(endpoints, &token, "matchmaking_tick", &serde_json::json!([{"scheduled_id": 0, "scheduled_at": {"Interval": {"__time_duration_micros__": 1}}}]));
    // 404: SpacetimeDB does not expose scheduled reducers to clients at all; 530 would be the
    // module's own sender check refusing it. Either way the matchmaker cannot be driven.
    check(status == 404 || module_refused(status), &format!("a player cannot run the matchmaker ({status})"))?;
    let status = call_realm(endpoints, &token, "combat_tick", &serde_json::json!([{"scheduled_id": 0, "scheduled_at": {"Interval": {"__time_duration_micros__": 1}}}]));
    check(status == 404 || module_refused(status), &format!("a player cannot step a fight ({status})"))?;
    let status = call_realm(endpoints, &token, "combat_act", &serde_json::json!([{"monster": 0}, {"ability": 0}]));
    check(module_refused(status), &format!("a player outside a dungeon cannot act in one ({status})"))?;

    // Another account's character, through the API.
    let victim = bots[leader].character().map(|c| c.id.clone()).unwrap_or_default();
    let status = call_api(endpoints, &token, "GET", &format!("/api/v1/characters/{victim}"), &serde_json::Value::Null);
    check(status == 404, &format!("another account's character is invisible ({status})"))?;
    let status = call_api(
        endpoints,
        &token,
        "POST",
        "/api/v1/realm/queue",
        &serde_json::json!({"character_id": victim, "dungeon_id": "graveyard_crypt", "role": "tank"}),
    );
    check(status == 404, &format!("another account's character cannot be queued ({status})"))?;
    let status = call_api(
        endpoints,
        &token,
        "POST",
        &format!("/api/v1/characters/{victim}/equipment/move"),
        &serde_json::json!({"from": {"kind": "equipment", "slot": "chest"}, "to": {"kind": "inventory", "index": 31}}),
    );
    check(status == 404, &format!("another account's character cannot be modified ({status})"))?;
    // A role the character cannot play.
    let own = bots[other].character().map(|c| (c.id.clone(), c.roles.clone())).unwrap_or_default();
    let forbidden = Role::ALL.into_iter().find(|r| !own.1.contains(r));
    if let Some(role) = forbidden {
        let status = call_api(
            endpoints,
            &token,
            "POST",
            "/api/v1/realm/queue",
            &serde_json::json!({"character_id": own.0, "dungeon_id": "graveyard_crypt", "role": role.key()}),
        );
        check(status == 400, &format!("a character cannot queue as a role it cannot play ({status})"))?;
    }
    // Tokens the issuer never signed.
    let forged = format!("{}.{}.{}", "eyJhbGciOiJFUzI1NiIsInR5cCI6IkpXVCJ9", "eyJzdWIiOiJ4In0", "AAAA");
    let status = call_api(endpoints, &forged, "GET", "/api/v1/characters", &serde_json::Value::Null);
    check(status == 401, &format!("the API rejects a forged token ({status})"))?;
    let status = call_realm(endpoints, &forged, "queue_leave", &serde_json::json!([]));
    check(status == 401 || status == 400, &format!("the realm rejects a forged token ({status})"))?;

    // Movement: a speed hack is held to real time.
    bots[leader].send(NetCommand::StartDungeon);
    let active = drive(&mut bots, Duration::from_secs(15), all_active);
    check(active, "the leader started")?;
    let start = bots[other].mirror.me().map(|p| p.position).unwrap_or(Vec3::ZERO);
    let mut seq = bots[other].mirror.me().map_or(0, |p| p.last_seq) + 1;
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(2) {
        let frames: Vec<engine_realm::movement::InputFrame> = (0..16)
            .map(|_| {
                let frame = engine_realm::movement::InputFrame { seq, forward: 50.0, right: 0.0, yaw: 0.0, face_camera: false, jump: false };
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
    let end = bots[other].mirror.me().map(|p| p.position).unwrap_or(Vec3::ZERO);
    let moved = Vec3::new(end.x - start.x, 0.0, end.z - start.z).length();
    let max_speed = engine_core::tuning::movement::MAX_SPEED;
    check(
        moved <= max_speed * 3.5,
        &format!("a flood of oversized inputs moved {moved:.1} m in ~3 s (max speed {max_speed} m/s)"),
    )?;
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| bots.iter().all(|b| b.mirror.party.is_none()));
    Ok(())
}

/// A completed run's rewards reach the persistent database exactly once.
///
/// Completion is triggered with the owner-only `admin_finish_instance` through the
/// `spacetime` CLI (set `SPACETIME_CLI` if it is not on PATH): server-side combat, which will
/// complete dungeons on its own, is a later phase.
pub fn results(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: results");
    let tag = run_tag();
    let mut bots = trio(&tag, "results", endpoints, false, true)?;
    let active = drive(&mut bots, Duration::from_secs(45), all_active);
    check(active, "the run became active")?;
    let instance = bots[0].mirror.instance.as_ref().map(|i| i.id).ok_or("no instance")?;
    let before: Vec<u64> = bots.iter().map(|b| b.character().map_or(0, |c| c.experience)).collect();

    let cli = std::env::var("SPACETIME_CLI").unwrap_or_else(|_| "spacetime".into());
    let server = endpoints.spacetimedb_url.clone();
    let status = std::process::Command::new(&cli)
        .args(["call", "--server", &server, &endpoints.spacetimedb_database, "admin_finish_instance", &instance.to_string(), "true"])
        .output()
        .map_err(|e| format!("could not run {cli}: {e}"))?;
    check(status.status.success(), "the owner completed the instance")?;

    let completed = drive(&mut bots, Duration::from_secs(5), |bots| {
        bots.iter().all(|b| b.mirror.instance.as_ref().is_some_and(|i| i.state == InstanceState::Completed))
    });
    check(completed, "every client sees the run completed")?;

    // The bridge collects every few seconds; poll the API.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        for bot in &bots {
            bot.send(NetCommand::FetchCharacters);
        }
        drive(&mut bots, Duration::from_millis(800), |_| false);
        let after: Vec<u64> = bots.iter().map(|b| b.character().map_or(0, |c| c.experience)).collect();
        let levels: Vec<u32> = bots.iter().map(|b| b.character().map_or(0, |c| c.level)).collect();
        let gained = after.iter().zip(&before).all(|(a, b)| a > b) || levels.iter().all(|l| *l > 1);
        if gained {
            log::info!("  experience {before:?} -> {after:?}, levels {levels:?}");
            break;
        }
        if Instant::now() > deadline {
            return Err(format!("FAILED: experience was not persisted ({before:?} -> {after:?})"));
        }
    }
    check(true, "the server-awarded experience was persisted for every member")?;
    let first: Vec<u64> = bots.iter().map(|b| b.character().map_or(0, |c| c.experience)).collect();
    // Let the bridge run again: a re-delivered result must not be applied twice.
    drive(&mut bots, Duration::from_secs(7), |_| false);
    for bot in &bots {
        bot.send(NetCommand::FetchCharacters);
    }
    drive(&mut bots, Duration::from_secs(1), |_| false);
    let second: Vec<u64> = bots.iter().map(|b| b.character().map_or(0, |c| c.experience)).collect();
    check(first == second, "results are applied exactly once")?;
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| bots.iter().all(|b| b.mirror.party.is_none()));
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
        bots.iter().all(|b| b.mirror.my_combat().is_some_and(|c| !c.cooldowns.is_empty())),
        "each player's class cooldowns come from the server",
    )?;
    check(
        bots[0].mirror.monsters.values().all(|m| m.alive && m.health > 0.0),
        "the server's monsters start alive",
    )?;

    // The healer and the dps hunt; their slot 0 is Moonfire and Firebolt.
    bots[1].config.hunt = true;
    bots[2].config.hunt = true;
    let dps = bots[2].session.as_ref().map(|s| s.account_id.clone()).unwrap_or_default();
    let hit = drive(&mut bots, Duration::from_secs(45), |bots| {
        bots[0].mirror.events.iter().any(|event| {
            matches!(event, WireEvent::Damage { source: EntityRef::Player(_), target: EntityRef::Monster(_), amount, .. } if *amount > 0.0)
        })
    });
    check(hit, "a press became server-side damage, and a party member's client heard of it")?;
    let hurt = bots[0].mirror.monsters.values().any(|m| m.health < m.max_health);
    check(hurt, "a monster's row shows the damage")?;
    let fought = drive(&mut bots, Duration::from_secs(15), |bots| {
        bots[0]
            .mirror
            .monsters
            .values()
            .any(|m| matches!(m.target, EntityRef::Player(_)))
    });
    check(fought, "the monster turned on the party (server AI and threat)")?;
    let dps_damage = bots[2].mirror.events.iter().any(|event| {
        matches!(event, WireEvent::Damage { source: EntityRef::Player(p), .. } if *p == dps)
    });
    check(dps_damage || hit, "the dps's own client saw its damage")?;
    for bot in &bots {
        bot.send(NetCommand::LeaveParty);
    }
    drive(&mut bots, Duration::from_secs(2), |bots| bots.iter().all(|b| b.mirror.party.is_none()));
    Ok(())
}
