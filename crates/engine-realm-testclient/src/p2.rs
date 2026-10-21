//! The P2 final verification (brief section 46) against a running stack: `p2_full` plays the
//! account, run, progression and hub steps with real bots over the real network layer and the
//! real API, writes a snapshot of what the server holds, and `p2_after` compares that snapshot
//! with what the server says after the stack was restarted.
//!
//! Two things here are test tooling rather than game paths, and say so where they are used:
//! the hub phase seeds a character's loadout straight into Postgres (the way a long play
//! session would have filled it: epics, gems, books, gold), through `docker exec psql`; and the
//! disconnect probes write a raw HTTP request and hang up before the answer.
//!
//! Needs the realm stack of `tools/realm-dev.ps1`, the owner's `spacetime` CLI (`SPACETIME_CLI`)
//! and the dev Postgres container (`P2_PG_CONTAINER`, default `mygame-dev-postgres-1`).

use std::io::Write;
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use engine_game::character::{Stat, item_by_key};
use engine_net::api::ApiClient;
use engine_net::{ConnectionStatus, NetCommand, RealmEndpoints};
use engine_realm::api::CharacterDetail;
use engine_realm::instance::InstanceState;
use engine_realm::loadout::{BagItem, EquippedItem, Loadout, SlotRef};
use engine_realm::outcome::RunOutcome;
use serde_json::{Value, json};

use crate::bot::Bot;
use crate::scenarios::{
    Outcome, all_active, check, clear_instance, drive, fetch_details, key_level_of, refetch,
    resource_of, run_tag, trio_for,
};

/// Where `p2_full` leaves what the server held, for `p2_after`.
fn snapshot_path() -> PathBuf {
    std::env::var("P2_SNAPSHOT").map_or_else(|_| PathBuf::from("p2_snapshot.json"), PathBuf::from)
}

/// Runs `sql` in the dev Postgres container.
fn psql(sql: &str) -> Result<String, String> {
    let container =
        std::env::var("P2_PG_CONTAINER").unwrap_or_else(|_| "mygame-dev-postgres-1".into());
    let mut child = std::process::Command::new("docker")
        .args([
            "exec",
            "-i",
            &container,
            "psql",
            "-U",
            "realm",
            "-d",
            "realm",
            "-v",
            "ON_ERROR_STOP=1",
            "-At",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run docker: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("no stdin")?
        .write_all(sql.as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "psql failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A request id for action `n` of this run (a UUID-shaped string).
fn rid(tag: &str, n: u32) -> String {
    let digits: String = tag.chars().filter(char::is_ascii_digit).collect();
    format!("{n:08x}-0000-4000-8000-{digits:0>12}")
}

/// What a run left on the clients.
struct Played {
    key_level: u32,
    modifiers: usize,
    summaries: Vec<engine_net::model::RunSummaryInfo>,
}

/// Queues everyone, waits for the active run, calls `during`, then lets the server's own tick
/// complete the run once every monster is dead (`admin_clear_instance`, as the loot scenario
/// does), and returns what the clients were told.
fn play(
    bots: &mut Vec<Bot>,
    endpoints: &RealmEndpoints,
    label: &str,
    during: impl FnOnce(&mut Vec<Bot>, u64) -> Outcome,
) -> Result<Played, String> {
    for bot in bots.iter_mut() {
        bot.config.queue = true;
    }
    check(
        drive(bots, Duration::from_secs(90), all_active),
        &format!("{label}: the party formed, the key went in, the run became active"),
    )?;
    let instance = bots[0]
        .mirror
        .instance
        .clone()
        .ok_or("no instance on the first client")?;
    during(bots, instance.id)?;
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        clear_instance(endpoints, instance.id)?;
        let done = drive(bots, Duration::from_secs(1), |bots| {
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
            return Err(format!("FAILED: {label}: the run did not complete"));
        }
    }
    let summaries = bots
        .iter()
        .filter_map(|b| b.mirror.run_summary.clone())
        .collect::<Vec<_>>();
    check(
        summaries.len() == bots.len() && summaries.iter().all(|s| s.success),
        &format!("{label}: every client got a successful run summary"),
    )?;
    Ok(Played {
        key_level: instance.key_level,
        modifiers: instance.modifiers.len(),
        summaries,
    })
}

/// How many of a summary's chest lines are equipment (the chest may also hold a gem or a book).
fn equipment_lines(summary: &engine_net::model::RunSummaryInfo) -> usize {
    engine_realm::loot::ensure_items_registered();
    summary
        .loot
        .iter()
        .filter(|line| {
            item_by_key(&line.item)
                .is_some_and(|def| engine_game::character::ItemFamily::ALL.contains(&def.family))
        })
        .count()
}

/// `(item, quality, sharpening)` of the dungeon loot a character holds in its bag and mailbox.
fn held_full(bot: &Bot) -> Vec<(String, u8, u8)> {
    engine_realm::loot::ensure_items_registered();
    let Some(detail) = bot.detail.as_ref() else {
        return Vec::new();
    };
    let catalog_item = |key: &str| {
        item_by_key(key).is_some_and(|def| def.id.0 >= engine_game::character::CATALOG_ID_BASE)
    };
    let mut held: Vec<(String, u8, u8)> = detail
        .inventory
        .iter()
        .filter(|e| catalog_item(&e.item))
        .map(|e| (e.item.clone(), e.quality, e.sharpening))
        .collect();
    held.extend(
        detail
            .mail
            .iter()
            .filter(|m| catalog_item(&m.item))
            .map(|m| (m.item.clone(), m.quality, m.sharpening)),
    );
    held.sort();
    held
}

/// Everything the loot delivery check compares.
struct Before {
    gold: Vec<u64>,
    held: Vec<usize>,
    points: Vec<u32>,
    xp: Vec<(u32, u64)>,
}

fn before_of(bots: &mut [Bot]) -> Before {
    fetch_details(bots, Duration::from_millis(1500));
    Before {
        gold: bots.iter().map(|b| resource_of(b, "gold")).collect(),
        held: bots.iter().map(|b| held_full(b).len()).collect(),
        points: bots
            .iter()
            .map(|b| b.detail.as_ref().map_or(0, |d| d.talent_points_total))
            .collect(),
        xp: bots
            .iter()
            .map(|b| {
                b.detail
                    .as_ref()
                    .map_or((0, 0), |d| (d.summary.level, d.summary.experience))
            })
            .collect(),
    }
}

/// Leaves the finished run, waits for the bridge, and checks the loot, the gold and the key
/// arrived exactly once. Returns the number of chest items that arrived pre-sharpened.
fn settle(
    bots: &mut [Bot],
    before: &Before,
    played: &Played,
    key_before: u32,
    label: &str,
) -> Result<usize, String> {
    for bot in bots.iter_mut() {
        bot.config.queue = false;
        bot.send(NetCommand::LeaveParty);
    }
    check(
        drive(bots, Duration::from_secs(8), |bots| {
            bots.iter()
                .all(|b| b.mirror.party.is_none() && b.mirror.instance.is_none())
        }),
        &format!("{label}: everyone is back in the hub"),
    )?;
    let looted: Vec<u64> = played
        .summaries
        .iter()
        .map(|s| {
            s.looted
                .iter()
                .find(|(id, _)| id == "gold")
                .map_or(0, |(_, n)| u64::from(*n))
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        refetch(bots, Duration::from_millis(700));
        fetch_details(bots, Duration::from_millis(900));
        let landed = bots.iter().enumerate().all(|(i, b)| {
            resource_of(b, "gold") >= before.gold[i] + looted[i]
                && held_full(b).len() >= before.held[i] + played.summaries[i].loot.len()
        }) && key_level_of(&bots[0]) > key_before;
        if landed {
            break;
        }
        if Instant::now() > deadline {
            return Err(format!(
                "FAILED: {label}: loot, gold or key progress were not delivered"
            ));
        }
    }
    let mut presharpened = 0;
    for (i, bot) in bots.iter().enumerate() {
        check(
            resource_of(bot, "gold") == before.gold[i] + looted[i],
            &format!(
                "{label}: bot {i} holds exactly the gold the summary said (+{})",
                looted[i]
            ),
        )?;
        let held = held_full(bot);
        for line in &played.summaries[i].loot {
            check(
                held.contains(&(line.item.clone(), line.quality, line.sharpening)),
                &format!(
                    "{label}: bot {i} holds {} q{} +{} as the chest rolled it",
                    line.item, line.quality, line.sharpening
                ),
            )?;
            presharpened += usize::from(line.sharpening > 0);
        }
        check(
            played.summaries[i].experience > 0,
            &format!("{label}: bot {i} earned experience"),
        )?;
    }
    check(
        key_level_of(&bots[0]) == key_before + 1,
        &format!(
            "{label}: the key owner's key rose from {key_before} to {}",
            key_before + 1
        ),
    )?;
    // Exactly once: more bridge passes change nothing.
    let snapshot: Vec<(u64, usize)> = bots
        .iter()
        .map(|b| (resource_of(b, "gold"), held_full(b).len()))
        .collect();
    drive(bots, Duration::from_secs(8), |_| false);
    fetch_details(bots, Duration::from_millis(1500));
    let after: Vec<(u64, usize)> = bots
        .iter()
        .map(|b| (resource_of(b, "gold"), held_full(b).len()))
        .collect();
    check(
        after == snapshot,
        &format!("{label}: loot and gold are delivered exactly once"),
    )?;
    Ok(presharpened)
}

/// Writes `loadout` over a character's stored one. Test tooling: the hub phase needs a
/// character as a long play session would leave it, and this is the fast way to get one.
fn seed(character: &str, loadout: &Loadout) -> Outcome {
    let json = serde_json::to_string(loadout).map_err(|e| e.to_string())?;
    if json.contains('\'') {
        return Err("seed json contains a quote".into());
    }
    psql(&format!(
        "UPDATE game.characters SET loadout = '{json}'::jsonb WHERE id = '{character}';"
    ))?;
    Ok(())
}

fn loadout_of_detail(detail: &CharacterDetail) -> Loadout {
    Loadout {
        equipment: detail.summary.equipment.clone(),
        inventory: detail.inventory.clone(),
        mail: detail.mail.clone(),
        resources: detail.summary.resources.iter().cloned().collect(),
        ..Loadout::default()
    }
}

fn worn(slot: &str, item: &str, quality: u8) -> EquippedItem {
    let mut entry = EquippedItem::new(slot, item);
    entry.quality = quality;
    entry
}

fn bagged(index: u32, item: &str, quantity: u16, quality: u8) -> BagItem {
    let mut entry = BagItem::new(index, item, quantity);
    entry.quality = quality;
    entry
}

fn stat_of(detail: &CharacterDetail, key: &str) -> f32 {
    detail
        .stats
        .iter()
        .find(|(name, _)| name == key)
        .map_or(0.0, |(_, value)| *value)
}

fn gold_of(detail: &CharacterDetail) -> u64 {
    detail
        .summary
        .resources
        .iter()
        .find(|(id, _)| id == "gold")
        .map_or(0, |(_, n)| *n)
}

fn count_in_bag(detail: &CharacterDetail, item: &str) -> u32 {
    detail
        .inventory
        .iter()
        .filter(|e| e.item == item)
        .map(|e| u32::from(e.quantity))
        .sum()
}

/// The persisted character as JSON, for exact comparisons.
fn json_of(detail: &CharacterDetail) -> Value {
    serde_json::to_value(detail).unwrap_or(Value::Null)
}

/// Sends a POST and hangs up without reading the answer: the client was killed mid-request.
/// `linger` is how long the connection stays open after the request was written.
fn fire_and_hang_up(
    endpoints: &RealmEndpoints,
    token: &str,
    path: &str,
    body: &Value,
    linger: Duration,
) -> Outcome {
    let authority = endpoints
        .persistence_api_url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();
    let body = body.to_string();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(&authority).map_err(|e| e.to_string())?;
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    std::thread::sleep(linger);
    drop(stream);
    Ok(())
}

fn token_of(bot: &mut Bot, endpoints: &RealmEndpoints) -> Result<String, String> {
    bot.acquire_probe_token(endpoints).map_err(|e| e.message)
}

/// The character in full, through the API.
fn detail_of(api: &ApiClient, token: &str, id: &str) -> Result<CharacterDetail, String> {
    api.character(token, id).map_err(|e| e.message)
}

/// Steps 16 to 21 on the Fire Mage: sharpening, gems, sets, books, reforging, and requests
/// that were cut off. Every action is replayed with its request id and must change nothing.
#[allow(clippy::too_many_lines, reason = "one linear script, read top to bottom")]
fn hub_phase(endpoints: &RealmEndpoints, tag: &str, bot: &mut Bot) -> Result<Value, String> {
    log::info!("p2: hub phase");
    let api = ApiClient::new(endpoints);
    let token = token_of(bot, endpoints)?;
    let id = bot.character().map(|c| c.id.clone()).ok_or("no character")?;
    let mut n = 100u32;
    let mut next = || {
        n += 1;
        rid(tag, n)
    };
    let base = detail_of(&api, &token, &id)?;

    // Seed 0: nothing worn, so the sheet is the bare character's.
    let mut loadout = loadout_of_detail(&base);
    loadout.equipment.clear();
    loadout.inventory.clear();
    loadout.mail.clear();
    seed(&id, &loadout)?;
    let bare = detail_of(&api, &token, &id)?;

    // Seed 1: a full Emberwake Regalia worn, and a bag of things to work on.
    loadout.equipment = vec![
        worn("head", "emberwake_hood", 100),
        worn("shoulder", "emberwake_mantle", 100),
        worn("back", "emberwake_shroud", 100),
        worn("wrist", "emberwake_cuffs", 100),
        worn("hands", "emberwake_gauntlets", 100),
    ];
    loadout.inventory = vec![
        bagged(0, "gem_main_stat_1", 20, 100),
        bagged(1, "gem_stamina_1", 1, 100),
        bagged(2, "crown_of_the_hollow_steward", 1, 90),
        bagged(3, "tome_of_cinders", 1, 100),
        bagged(4, "grimoire_of_the_pyre", 1, 100),
        bagged(5, "tome_of_cinders", 1, 100),
        bagged(6, "stewards_funerary_raiment", 1, 90),
        bagged(7, "ashgraves_seal", 1, 90),
    ];
    loadout.resources.insert("gold".into(), 100_000);
    loadout.resources.insert("crypt_dust".into(), 5000);
    loadout.resources.insert("steward_ember".into(), 20);
    seed(&id, &loadout)?;
    let start = detail_of(&api, &token, &id)?;
    check(
        start.inventory.len() == 8 && start.summary.equipment.len() == 5,
        "the seeded character is read back by the API as written",
    )?;

    // 18: sets. The server's sheet carries the 2 piece and 4 piece bonuses of the five worn
    // pieces: haste is the bare character's, plus the pieces' own, plus 4 (2 pc) and 2 (4 pc).
    engine_realm::loot::ensure_items_registered();
    let pieces: f32 = [
        "emberwake_hood",
        "emberwake_mantle",
        "emberwake_shroud",
        "emberwake_cuffs",
        "emberwake_gauntlets",
    ]
    .iter()
    .filter_map(|key| item_by_key(key))
    .map(|def| {
        engine_game::character::ItemStack::one(def.id)
            .stats()
            .get(Stat::Haste)
    })
    .sum();
    let haste_bare = stat_of(&bare, "haste");
    let haste_set = stat_of(&start, "haste");
    log::info!(
        "  haste: bare {haste_bare}, worn {haste_set}, pieces alone {pieces}, expected {}",
        haste_bare + pieces + 6.0
    );
    check(
        (haste_set - (haste_bare + pieces + 6.0)).abs() < 0.01,
        "the server sheet holds the five pieces' haste plus the 2 piece (+4) and 4 piece (+2) bonuses",
    )?;
    check(
        (stat_of(&start, "critical_strike") - stat_of(&bare, "critical_strike")) >= 6.0,
        "the 2 piece set bonus (+6 critical strike) is on the server sheet",
    )?;

    // 16: sharpening. A replay with the same request id changes nothing.
    let crown = SlotRef::Inventory { index: 2 };
    let sharpen_id = next();
    let first = api
        .sharpen(&token, &id, &crown, &sharpen_id)
        .map_err(|e| e.message)?;
    check(
        first.new_level == 1,
        "sharpening the crown from +0 gave +1",
    )?;
    let after_first = json_of(&detail_of(&api, &token, &id)?);
    let replay = api
        .sharpen(&token, &id, &crown, &sharpen_id)
        .map_err(|e| e.message)?;
    check(
        replay.new_level == first.new_level
            && json_of(&detail_of(&api, &token, &id)?) == after_first,
        "replaying the sharpen request id returned the first answer and charged nothing",
    )?;
    check(
        detail_of(&api, &token, &id)?
            .inventory
            .iter()
            .any(|e| e.index == 2 && e.sharpening == 1),
        "the persisted crown is +1",
    )?;

    // 17: gems. Socket, unsocket, upgrade, each replayed.
    let worn_head = SlotRef::Equipment {
        slot: "head".into(),
    };
    let stamina_before = stat_of(&detail_of(&api, &token, &id)?, "stamina");
    let socket_id = next();
    let socketed = api
        .socket_gem(&token, &id, &worn_head, 0, 1, &socket_id)
        .map_err(|e| e.message)?;
    check(
        socketed.gem == "gem_stamina_1"
            && socketed.character.summary.equipment.iter().any(|e| {
                e.slot == "head" && e.gems.first().is_some_and(|g| g == "gem_stamina_1")
            }),
        "the gem is in the worn hood's socket",
    )?;
    check(
        count_in_bag(&socketed.character, "gem_stamina_1") == 0,
        "the gem left the bag",
    )?;
    let stamina_after = stat_of(&socketed.character, "stamina");
    log::info!("  stamina {stamina_before} -> {stamina_after}");
    check(
        stamina_after > stamina_before,
        "the socketed gem raised the server sheet's stamina",
    )?;
    let after_socket = json_of(&detail_of(&api, &token, &id)?);
    api.socket_gem(&token, &id, &worn_head, 0, 1, &socket_id)
        .map_err(|e| e.message)?;
    check(
        json_of(&detail_of(&api, &token, &id)?) == after_socket,
        "replaying the socket request id changed nothing",
    )?;
    let gold_before = gold_of(&detail_of(&api, &token, &id)?);
    let unsocket_id = next();
    let unsocketed = api
        .unsocket_gem(&token, &id, &worn_head, 0, &unsocket_id)
        .map_err(|e| e.message)?;
    check(
        unsocketed.cost > 0
            && unsocketed.gold_left == gold_before - unsocketed.cost
            && count_in_bag(&unsocketed.character, "gem_stamina_1") == 1,
        "unsocketing cost gold once and returned the gem to the bag",
    )?;
    let after_unsocket = json_of(&detail_of(&api, &token, &id)?);
    api.unsocket_gem(&token, &id, &worn_head, 0, &unsocket_id)
        .map_err(|e| e.message)?;
    check(
        json_of(&detail_of(&api, &token, &id)?) == after_unsocket,
        "replaying the unsocket request id charged nothing more",
    )?;
    let ruby_cell = 0;
    let before_upgrade = detail_of(&api, &token, &id)?;
    let upgrade_id = next();
    let upgraded = api
        .upgrade_gem(&token, &id, ruby_cell, &upgrade_id)
        .map_err(|e| e.message)?;
    let gems_before = count_in_bag(&before_upgrade, "gem_main_stat_1");
    check(
        count_in_bag(&upgraded.character, "gem_main_stat_1")
            == gems_before - u32::from(upgraded.consumed) + u32::from(upgraded.returned)
            && count_in_bag(&upgraded.character, "gem_main_stat_2")
                == count_in_bag(&before_upgrade, "gem_main_stat_2")
                    + u32::from(upgraded.success)
            && upgraded.gold_left == gold_of(&before_upgrade) - upgraded.cost,
        &format!(
            "the gem upgrade (success: {}) consumed {}, returned {}, charged {} gold, and the bag adds up",
            upgraded.success, upgraded.consumed, upgraded.returned, upgraded.cost
        ),
    )?;
    let after_upgrade = json_of(&detail_of(&api, &token, &id)?);
    let again = api
        .upgrade_gem(&token, &id, ruby_cell, &upgrade_id)
        .map_err(|e| e.message)?;
    check(
        again.success == upgraded.success
            && json_of(&detail_of(&api, &token, &id)?) == after_upgrade,
        "replaying the upgrade request id returned the first roll and changed nothing",
    )?;

    // 19: books. Read a talent book, read an enhancement book, set its active option.
    let points_before = detail_of(&api, &token, &id)?.talent_points_total;
    let book_id = next();
    let book_slot = SlotRef::Inventory { index: 3 };
    let read = api
        .use_book(&token, &id, &book_slot, &book_id)
        .map_err(|e| e.message)?;
    check(
        read.book == "tome_of_cinders"
            && read.character.books.has("tome_of_cinders")
            && read.character.talent_points_total == points_before + 1,
        "reading the talent book learned it and gave a talent point",
    )?;
    let after_read = json_of(&detail_of(&api, &token, &id)?);
    api.use_book(&token, &id, &book_slot, &book_id)
        .map_err(|e| e.message)?;
    check(
        json_of(&detail_of(&api, &token, &id)?) == after_read,
        "replaying the read request id consumed nothing more",
    )?;
    let duplicate = SlotRef::Inventory { index: 5 };
    check(
        api.use_book(&token, &id, &duplicate, &next()).is_err()
            && count_in_bag(&detail_of(&api, &token, &id)?, "tome_of_cinders") == 1,
        "a second copy of a learned book is refused and stays in the bag",
    )?;
    let grimoire = api
        .use_book(&token, &id, &SlotRef::Inventory { index: 4 }, &next())
        .map_err(|e| e.message)?;
    check(
        grimoire.character.books.has("grimoire_of_the_pyre"),
        "the enhancement book is learned",
    )?;
    check(
        api.set_active_book(&token, &id, "grimoire_of_the_pyre", "no_such_option", &next())
            .is_err(),
        "an option the book does not have is refused",
    )?;
    let active_id = next();
    let active = api
        .set_active_book(&token, &id, "grimoire_of_the_pyre", "twin_pyre", &active_id)
        .map_err(|e| e.message)?;
    check(
        active
            .books
            .active
            .get("grimoire_of_the_pyre")
            .is_some_and(|o| o == "twin_pyre"),
        "the active option is set and stored",
    )?;
    let switched = api
        .set_active_book(&token, &id, "grimoire_of_the_pyre", "ricochet_pyre", &next())
        .map_err(|e| e.message)?;
    check(
        switched
            .books
            .active
            .get("grimoire_of_the_pyre")
            .is_some_and(|o| o == "ricochet_pyre"),
        "the active option can be switched in the hub",
    )?;

    // 20: gambling. Reforge replaces the quality, charges once, replays free.
    let before_reforge = detail_of(&api, &token, &id)?;
    let reforge_id = next();
    let reforged = api
        .reforge(&token, &id, &crown, &reforge_id)
        .map_err(|e| e.message)?;
    check(
        reforged.old_quality == 90
            && reforged.cost > 0
            && reforged.gold_left == gold_of(&before_reforge) - reforged.cost
            && reforged.character.inventory.iter().any(|e| {
                e.index == 2 && e.quality == reforged.new_quality && e.sharpening == 1
            }),
        &format!(
            "reforge: quality 90 -> {} for {} gold, sharpening kept",
            reforged.new_quality, reforged.cost
        ),
    )?;
    let after_reforge = json_of(&detail_of(&api, &token, &id)?);
    let replayed = api
        .reforge(&token, &id, &crown, &reforge_id)
        .map_err(|e| e.message)?;
    check(
        replayed.new_quality == reforged.new_quality
            && json_of(&detail_of(&api, &token, &id)?) == after_reforge,
        "replaying the reforge request id returned the first roll and charged nothing",
    )?;

    // 21: the client dies mid-request. The request is written, the connection dropped before
    // any answer; the retry with the same id must leave exactly one effect.
    let seal = SlotRef::Inventory { index: 7 };
    for (label, linger_ms) in [("at once", 0u64), ("after 40 ms", 40)] {
        let before = detail_of(&api, &token, &id)?;
        let reforge_cut = next();
        fire_and_hang_up(
            endpoints,
            &token,
            &format!("/api/v1/characters/{id}/reforge"),
            &json!({"slot": {"kind": "inventory", "index": 7}, "request_id": reforge_cut}),
            Duration::from_millis(linger_ms),
        )?;
        std::thread::sleep(Duration::from_millis(600));
        let retry = api
            .reforge(&token, &id, &seal, &reforge_cut)
            .map_err(|e| e.message)?;
        let retry2 = api
            .reforge(&token, &id, &seal, &reforge_cut)
            .map_err(|e| e.message)?;
        let now = detail_of(&api, &token, &id)?;
        check(
            gold_of(&before) - gold_of(&now) == retry.cost
                && retry.new_quality == retry2.new_quality
                && now
                    .inventory
                    .iter()
                    .any(|e| e.index == 7 && e.quality == retry.new_quality),
            &format!(
                "reforge cut off {label}: after two retries gold fell by exactly one charge ({}) and the quality is the one answer ({})",
                retry.cost, retry.new_quality
            ),
        )?;
    }
    let before = detail_of(&api, &token, &id)?;
    let upgrade_cut = next();
    fire_and_hang_up(
        endpoints,
        &token,
        &format!("/api/v1/characters/{id}/gems/upgrade"),
        &json!({"gem_slot": 0, "request_id": upgrade_cut}),
        Duration::from_millis(10),
    )?;
    std::thread::sleep(Duration::from_millis(600));
    let retry = api
        .upgrade_gem(&token, &id, 0, &upgrade_cut)
        .map_err(|e| e.message)?;
    let retry2 = api
        .upgrade_gem(&token, &id, 0, &upgrade_cut)
        .map_err(|e| e.message)?;
    let now = detail_of(&api, &token, &id)?;
    check(
        gold_of(&before) - gold_of(&now) == retry.cost
            && retry.success == retry2.success
            && count_in_bag(&now, "gem_main_stat_1")
                == count_in_bag(&before, "gem_main_stat_1") - u32::from(retry.consumed)
                    + u32::from(retry.returned)
            && count_in_bag(&now, "gem_main_stat_2")
                == count_in_bag(&before, "gem_main_stat_2") + u32::from(retry.success),
        "gem upgrade cut off: after two retries the gems and gold moved by exactly one upgrade",
    )?;
    let before = detail_of(&api, &token, &id)?;
    let sharpen_cut = next();
    let raiment = SlotRef::Inventory { index: 6 };
    fire_and_hang_up(
        endpoints,
        &token,
        &format!("/api/v1/characters/{id}/sharpen"),
        &json!({"slot": {"kind": "inventory", "index": 6}, "request_id": sharpen_cut}),
        Duration::from_millis(10),
    )?;
    std::thread::sleep(Duration::from_millis(600));
    let retry = api
        .sharpen(&token, &id, &raiment, &sharpen_cut)
        .map_err(|e| e.message)?;
    api.sharpen(&token, &id, &raiment, &sharpen_cut)
        .map_err(|e| e.message)?;
    let now = detail_of(&api, &token, &id)?;
    check(
        retry.new_level == 1
            && now
                .inventory
                .iter()
                .any(|e| e.index == 6 && e.sharpening == 1)
            && resource_after_one_sharpen(&before, &now),
        "sharpen cut off: after two retries the item is +1 and the dust was paid once",
    )?;

    // Leave the character with something worth finding after the restart.
    let final_state = detail_of(&api, &token, &id)?;
    Ok(json!({
        "character": id,
        "detail": json_of(&final_state),
        "gold": gold_of(&final_state),
        "books": final_state.books.unlocked,
        "set_haste": haste_set,
    }))
}

/// Whether `now` paid exactly the +1 sharpening price in dust relative to `before`.
fn resource_after_one_sharpen(before: &CharacterDetail, now: &CharacterDetail) -> bool {
    let dust = |d: &CharacterDetail| {
        d.summary
            .resources
            .iter()
            .find(|(id, _)| id == "crypt_dust")
            .map_or(0, |(_, n)| *n)
    };
    dust(before) - dust(now) == 10
}

/// Steps 4 to 22 and the state snapshot for 24.
pub fn p2_full(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: p2_full");
    let tag = run_tag();
    let mut bots = trio_for(&tag, "p2", endpoints, false, true, "graveyard_crypt", None)?;
    for bot in &mut bots {
        bot.config.queue = false;
    }

    // 4-6: an account is created (or signed in), its characters listed, the connection up.
    check(
        drive(&mut bots, Duration::from_secs(30), |bots| {
            bots.iter().all(|b| {
                b.session.is_some()
                    && b.character().is_some()
                    && b.status == ConnectionStatus::Connected
            })
        }),
        "steps 4-6: registered, signed in, a character per account selected, connected to the hub",
    )?;
    check(
        bots.iter().all(|b| {
            b.characters.len() == 3
                && b.mirror.party.is_none()
                && b.mirror.ticket.is_none()
                && b.mirror.instance.is_none()
        }),
        "step 6: each account has its three starters and stands in the hub, queued for nothing",
    )?;
    let first = before_of(&mut bots);

    // 7-9: queue, run, loot. Run 1 at key 1.
    for bot in &mut bots {
        bot.config.queue = true;
    }
    check(
        drive(&mut bots, Duration::from_secs(30), |bots| {
            bots.iter()
                .all(|b| b.mirror.ticket.is_some() || b.mirror.party.is_some())
        }),
        "step 7: all three queued for the Graveyard Crypt",
    )?;
    let key_1 = key_level_of(&bots[0]);
    let run1 = play(&mut bots, endpoints, "run 1", |_, _| Ok(()))?;
    check(
        run1.key_level == 1 && run1.modifiers == 0,
        "step 8: run 1 completed at key 1 with no modifiers",
    )?;
    let sharp_1 = settle(&mut bots, &first, &run1, key_1, "run 1")?;
    check(
        run1.summaries.iter().all(|s| equipment_lines(s) == 2),
        "step 9: every player received two chest items, delivered to the bag or mailbox",
    )?;

    // 10-12: the key rises, a higher key runs.
    let second = before_of(&mut bots);
    let key_2 = key_level_of(&bots[0]);
    check(key_2 == 2, "steps 10-11: the key owner's key is now level 2")?;
    let run2 = play(&mut bots, endpoints, "run 2", |_, _| Ok(()))?;
    check(
        run2.key_level == 2,
        "step 12: run 2 ran at the higher key (2)",
    )?;
    let sharp_2 = settle(&mut bots, &second, &run2, key_2, "run 2")?;

    // 13 and 22: a key with a modifier; a client drops out mid-run and comes back.
    let third = before_of(&mut bots);
    let key_3 = key_level_of(&bots[0]);
    check(key_3 == 3, "the key owner's key is now level 3")?;
    let run3 = play(&mut bots, endpoints, "run 3", |bots, instance| {
        check(
            bots[0]
                .mirror
                .instance
                .as_ref()
                .is_some_and(|i| i.modifiers.len() == 1),
            "step 13: the level 3 run carries one modifier",
        )?;
        check(
            bots.iter().all(|b| {
                b.mirror
                    .members
                    .iter()
                    .any(|m| m.key_level == 3 && m.key_modifiers.len() == 1)
            }),
            "step 13: every client sees the key's modifier",
        )?;
        // 22: the healer's client dies and a new one signs in.
        let gone = bots.remove(1);
        let account = gone
            .session
            .as_ref()
            .map(|s| s.account_id.clone())
            .unwrap_or_default();
        let config = gone.config.clone();
        let position = gone.mirror.me().map(|p| p.position);
        drop(gone);
        check(
            drive(bots, Duration::from_secs(10), |bots| {
                bots.iter()
                    .all(|b| b.mirror.players.get(&account).is_some_and(|p| !p.present))
            }),
            "step 22: the party sees the dropped client as disconnected",
        )?;
        let mut back = Bot::start(
            crate::bot::BotConfig {
                queue: false,
                ..config
            },
            endpoints,
        )
        .map_err(|e| e.message)?;
        let resumed = drive(std::slice::from_mut(&mut back), Duration::from_secs(25), |b| {
            b[0].mirror.instance.as_ref().map(|i| i.id) == Some(instance)
                && b[0].mirror.me().is_some()
        });
        check(
            resumed,
            "step 22: the new client resumed in the same instance",
        )?;
        check(
            matches!(
                (position, back.mirror.me().map(|p| p.position)),
                (Some(a), Some(b)) if (a - b).length() < 2.0
            ),
            "step 22: at the authoritative position it left",
        )?;
        bots.insert(1, back);
        check(
            drive(bots, Duration::from_secs(8), |bots| {
                bots.iter()
                    .all(|b| b.mirror.players.get(&account).is_some_and(|p| p.present))
            }),
            "step 22: the party sees it present again",
        )?;
        Ok(())
    })?;
    check(
        run3.key_level == 3 && run3.modifiers == 1,
        "step 13: run 3 completed at key 3 with its modifier",
    )?;
    let sharp_3 = settle(&mut bots, &third, &run3, key_3, "run 3")?;
    log::info!("pre-sharpened chest drops over the three runs: {sharp_1} {sharp_2} {sharp_3}");

    // 10: progression upgraded: talent points and experience, from the server.
    let done = before_of(&mut bots);
    check(
        done.points
            .iter()
            .zip(&first.points)
            .all(|(now, then)| now >= &(then + 3)),
        &format!(
            "step 10: talent points rose by a point per completion or more ({:?} -> {:?})",
            first.points, done.points
        ),
    )?;
    check(
        done.xp.iter().zip(&first.xp).all(|(now, then)| now > then),
        &format!(
            "step 10: level and experience advanced ({:?} -> {:?})",
            first.xp, done.xp
        ),
    )?;
    check(
        key_level_of(&bots[0]) == 4 && bots[1..].iter().all(|b| key_level_of(b) >= 2),
        "step 11: the key owner's key reached 4; the participants' keys rose as participants'",
    )?;

    // 14-15: history and leaderboards, for the three runs.
    let api = ApiClient::new(endpoints);
    let token = token_of(&mut bots[0], endpoints)?;
    let tank = bots[0]
        .character()
        .map(|c| c.id.clone())
        .ok_or("no tank character")?;
    let deadline = Instant::now() + Duration::from_secs(40);
    let page = loop {
        let page = api
            .history(&token, &tank, None, 10)
            .map_err(|e| e.message)?;
        if page.runs.len() >= 3 {
            break page;
        }
        if Instant::now() > deadline {
            return Err("FAILED: the three runs never reached the history".into());
        }
        drive(&mut bots, Duration::from_secs(1), |_| false);
    };
    check(
        page.runs.len() == 3
            && page.runs.iter().all(|r| {
                r.outcome == RunOutcome::Timed
                    && r.dungeon_id == "graveyard_crypt"
                    && r.party.len() == 3
                    && r.run_uid.len() == 32
            }),
        "step 14: the tank's history lists the three runs, timed, with their parties",
    )?;
    let keys: Vec<u32> = page.runs.iter().map(|r| r.key_level).collect();
    check(
        keys == [3, 2, 1],
        &format!("step 14: newest first, at keys {keys:?}"),
    )?;
    check(
        page.runs[0].modifiers.len() == 1 && page.runs[1].modifiers.is_empty(),
        "step 14: the key 3 run recorded its modifier, the key 2 run none",
    )?;
    let detail = api
        .run(&token, &page.runs[0].run_uid)
        .map_err(|e| e.message)?;
    check(
        detail.you.is_some() && detail.party.len() == 3,
        "step 14: a participant opens the run (the stranger refusal is the history scenario's)",
    )?;
    let highest = api
        .leaderboard(&token, "highest", "graveyard_crypt", None, 0)
        .map_err(|e| e.message)?;
    check(
        page.runs
            .iter()
            .all(|r| highest.rows.iter().any(|row| row.run_uid == r.run_uid)),
        "step 15: the 'highest key' board lists all three runs",
    )?;
    let mut fastest_rows = 0;
    for run in &page.runs {
        let fastest = api
            .leaderboard(&token, "fastest", "graveyard_crypt", Some(run.key_level), 0)
            .map_err(|e| e.message)?;
        fastest_rows += fastest
            .rows
            .iter()
            .filter(|row| row.run_uid == run.run_uid)
            .count();
    }
    check(
        fastest_rows == 3,
        "step 15: each run is on the 'fastest' board of its key level, once",
    )?;

    // 16-21: the hub.
    let hub = hub_phase(endpoints, &tag, &mut bots[2])?;

    // 24 prep: what the server holds, for after the restart.
    for bot in &mut bots {
        bot.config.queue = false;
    }
    drive(&mut bots, Duration::from_secs(10), |_| false);
    let mut characters = Vec::new();
    for bot in &mut bots {
        let token = token_of(bot, endpoints)?;
        let id = bot
            .character()
            .map(|c| c.id.clone())
            .ok_or("no character")?;
        let detail = detail_of(&api, &token, &id)?;
        let history = api.history(&token, &id, None, 10).map_err(|e| e.message)?;
        characters.push(json!({
            "email": bot.config.email,
            "password": bot.config.password,
            "definition": bot.config.definition,
            "id": id,
            "detail": json_of(&detail),
            "key_level": detail.summary.key.as_ref().map_or(0, |k| k.level),
            "history": history.runs.iter().map(|r| r.run_uid.clone()).collect::<Vec<_>>(),
        }));
    }
    let snapshot = json!({
        "tag": tag,
        "characters": characters,
        "hub": hub,
        "highest_rows": highest.rows.len(),
        "run_uids": page.runs.iter().map(|r| r.run_uid.clone()).collect::<Vec<_>>(),
    });
    std::fs::write(
        snapshot_path(),
        serde_json::to_string_pretty(&snapshot).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("could not write the snapshot: {e}"))?;
    log::info!("snapshot written to {}", snapshot_path().display());
    Ok(())
}

/// Step 24: after the stack was restarted, every account signs in again and the server holds
/// exactly what the snapshot recorded.
pub fn p2_after(endpoints: &RealmEndpoints) -> Outcome {
    log::info!("scenario: p2_after");
    let text = std::fs::read_to_string(snapshot_path())
        .map_err(|e| format!("no snapshot (run p2_full first): {e}"))?;
    let snapshot: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let characters = snapshot["characters"].as_array().ok_or("no characters")?;
    let api = ApiClient::new(endpoints);
    for entry in characters {
        let email = entry["email"].as_str().ok_or("email")?;
        let password = entry["password"].as_str().ok_or("password")?;
        let id = entry["id"].as_str().ok_or("id")?;
        let pair = api.login(email, password).map_err(|e| e.message)?;
        let token = pair.access_token;
        let list = api.characters(&token).map_err(|e| e.message)?;
        check(
            list.iter().any(|c| c.id == id) && list.len() == 3,
            &format!("{email}: signs in after the restart and sees its three characters"),
        )?;
        let detail = detail_of(&api, &token, id)?;
        check(
            json_of(&detail) == entry["detail"],
            &format!(
                "{email}: the {} in full (equipment, bag, mail, resources, talents, books, key, stats) equals the snapshot",
                entry["definition"].as_str().unwrap_or("character")
            ),
        )?;
        let history = api.history(&token, id, None, 10).map_err(|e| e.message)?;
        let uids: Vec<String> = history.runs.iter().map(|r| r.run_uid.clone()).collect();
        let expected: Vec<String> = entry["history"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        check(
            uids == expected,
            &format!("{email}: the run history ({} runs) equals the snapshot", uids.len()),
        )?;
    }
    let first = characters.first().ok_or("no characters")?;
    let pair = api
        .login(
            first["email"].as_str().ok_or("email")?,
            first["password"].as_str().ok_or("password")?,
        )
        .map_err(|e| e.message)?;
    let board = api
        .leaderboard(&pair.access_token, "highest", "graveyard_crypt", None, 0)
        .map_err(|e| e.message)?;
    let uids = snapshot["run_uids"].as_array().ok_or("run_uids")?;
    check(
        uids.iter()
            .all(|uid| board.rows.iter().any(|row| Some(row.run_uid.as_str()) == uid.as_str())),
        "the leaderboard still lists the three runs",
    )?;
    Ok(())
}
