# Manual testing of P1 and P2 on this PC

A hands-on checklist for the P1 and P2 work on `p2-core`, run by a person on this machine. It
focuses on what automated runs have not covered yet: the real client, visible windows, sound,
and actual combat. The automated status is recorded in `docs/P1_FINAL_REPORT.md` §11 and
`docs/P2_FINAL_REPORT.md` "Deferred".

Tick each box as you go. Note anything wrong, with the time and the client log, in a new
"Manual run" section at the bottom of `docs/P2_VERIFICATION.md`.

---

## 0. One-time setup

You need:

- Docker Desktop running, for Postgres
- the `spacetime` CLI (at `%LOCALAPPDATA%\SpacetimeDB\spacetime.exe`)
- the `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`)

Run every command from `C:\Projects\myGame`, in PowerShell.

```powershell
git checkout p2-core
cargo build --release --bin mygame --bin realm-bot
cargo build --release --bin mygame --features editor   # only for section 6 (editor)
```

> The editor build overwrites the same `target\release\mygame.exe`. Build it last, or rebuild
> without `--features editor` before the online sections.

### Start the online stack

```powershell
.\tools\realm-dev.ps1 up        # Postgres, SpacetimeDB, publishes the module, starts the API
.\tools\realm-dev.ps1 status    # all three must say "up": Postgres 5434, SpacetimeDB 3000, API 8080
```

- Don't pipe `up` through `grep` or `Select-String`, because it hangs. Use `status` instead.
- Logs go to `.realm-dev\`.
- If the first login fails with WebSocket 401, wait about 30 seconds and retry. This is a known
  one-off right after `up`.

### Smoke check before testing by hand

```powershell
$env:SPACETIME_CLI = "$env:LOCALAPPDATA\SpacetimeDB\spacetime.exe"
.\target\release\realm-bot.exe scenario all     # expect 14/14 PASS
```

If this fails, fix the stack first. Manual results mean nothing on a broken stack.

### Admin account for the dashboard (optional, sections 3 and 7)

```powershell
cd backend
$env:DATABASE_URL = "postgres://realm:realm-dev-password@127.0.0.1:5434/realm"
cargo run -p generic-auth-api -- create-admin --email=admin@local.mygame.test --password=admin-password-1
cd ..
```

Then open http://127.0.0.1:8080/admin and sign in. If `create-admin` asks for more config, see
`backend/README.md` §create-admin.

---

## 1. Launching clients

A dungeon queue needs **1 Tank, 1 Healer and 1 DPS**. The easiest manual setup is:

- **you** play one client by hand;
- two **autoplay helper** clients fill the other roles.

Autoplay helpers sign in, queue, accept and start on their own. They **do not fight**.

**Your client:**

```powershell
.\target\release\mygame.exe --online --profile me
```

Create an account in the window, create a character, and press **Enter World**.

**Helpers** (the example below is for when you play the DPS; swap the roles to fit your class):

```powershell
$exe = ".\target\release\mygame.exe"
Start-Process $exe -WorkingDirectory target\release -ArgumentList '--online','--profile','tank','--width','960','--height','540','--dev-login','tank@local.mygame.test:local-password-1','--dev-autoplay','warrior:tank'
Start-Process $exe -WorkingDirectory target\release -ArgumentList '--online','--profile','healer','--width','960','--height','540','--dev-login','healer@local.mygame.test:local-password-1','--dev-autoplay','druid:healer'
```

To watch all three as autoplay instead, run `.\tools\run-3-clients.ps1 -Autoplay`.

**Finishing a run without combat.** Use this when a helper party can't kill the boss. It is
owner-only: it kills every monster, and the realm then completes the run with its real chest.
Get the instance id from the `/admin` page or from the client log line `entering instance N`:

```powershell
& "$env:LOCALAPPDATA\SpacetimeDB\spacetime.exe" call --server http://127.0.0.1:3000 realm admin_clear_instance 42
```

### In-game keys (P1/P2 additions)

| Key | Opens |
|---|---|
| C | Character sheet (gear, set bonuses, gems in tooltips) |
| J | Mailbox (loot delivered by mail) |
| O | Recount (damage meter), or the REC button |
| K | Key modifiers window |
| T / V | Trinket 1 / 2 |
| R | Ultimate (Fire Mage: Moving Fire Beam) |
| M | Dungeon map |
| E | Interact (chest, objects) |
| Enter | Chat (`/s`, `/p`, `/w Name`, `/r`) |
| Talent corner button | Talents ↔ Books page |

The hub menu has these entries: Dungeon Finder, Equipment, History, Boards, Community Maps,
Change Character and Log Out. Workshop modes are Gems, Sharpen items, Reforge items, Salvage and
Exchange.

---

## 2. Offline checks (no stack needed)

These are quick visual and audio checks of the new content. Run without `--hidden` and with sound
on.

| ✓ | Check | Command | Expect |
|---|---|---|---|
| ☐ | Ashen Reliquary loads, looks right | `mygame.exe --scene-file assets/scenes/ashen_reliquary.json --party 3` | lit level, packs, boss reachable |
| ☐ | Drowned Catacombs (P2) | `... --scene-file assets/scenes/drowned_catacombs.json --party 3` | **Undertow** pulls you; Waterlogged status icon; Hymn |
| ☐ | Bone Foundry (P2) | `... --scene-file assets/scenes/bone_foundry.json --party 3` | conveyor lane moves you; Ladle soak; Warded/Stoked icons; Cinder Bomber detonates |
| ☐ | Sanctum of Ash (P2) | `... --scene-file assets/scenes/sanctum_of_ash.json --party 3` | 3 shrines needed; mini-boss Censer Warden; Pontiff: Branded stacks, hide behind pillars for the Rite, **pylon immunity shows a "Pylon Shield" aura and tinted bars**, Last Rite enrage |
| ☐ | Modifiers render | `... --scene-file assets/scenes/graveyard_crypt.json --modifiers rotating_beams,plague_explosion,necrotic_pools,bolstering,raging,spiteful_casters` | beam telegraphs, plague explosion on death, pools, buffs on mobs |
| ☐ | Warrior Valor → Battle Fervor | `... --hero warrior` | War Stomp / Colossus Strike / Shield Charge build Valor; Fervor stacks to 3 |
| ☐ | Moving Fire Beam | `... --hero firemage`, press R | beam follows facing, damages dummies |
| ☐ | Druid | `... --hero druid` | Wrath and Nature's Exposure (max 10 stacks) |
| ☐ | Chest + loot tooltips | `... --spawn-chest`, walk to it, E | items with quality and sharpening (+N) in tooltip, no C/B/A/S tiers |
| ☐ | Mail and icons | `... --sample-mail 20 --open-mail` | gem/book/key/dice icons are distinct and readable |
| ☐ | New monsters one at a time | `... --scene monsters` | choose new-dungeon monsters, spawn, fight; telegraphs readable |

`...` means `.\target\release\mygame.exe`. If you are low on health, use `--scene monsters`
(1 M health) instead of the dungeon.

### Sound (P1 53 sounds, P2 13 procedural cues — never auditioned)

`mygame.exe --scene audio` gives one key per sound in the catalogue. Listen for:

- sharpen, reforge, gem socket and upgrade, book use, set bonus;
- run-timer warning, personal best, community publish;
- the boss mechanics.

Sounds are listed in `docs/audio-p2.md`. Note any that are missing, clipped or too loud.

---

## 3. P1 online loop (real client)

| ✓ | Step | Expect |
|---|---|---|
| ☐ | Register, create a character, enter the hub | hub stands; gear score, key, resources shown |
| ☐ | Dungeon Finder → Graveyard Crypt → role → Find Group | match found with the helpers; accept prompt; owner inserts key (helpers log "InsertKey refused", expected) |
| ☐ | Key + modifiers (K) before start | modifiers listed for the key level; reroll available at 50 % kills with currency; free downgrade works |
| ☐ | **Combat by hand** (P1 §11 gap) | abilities hit, monsters fight back, damage numbers, nameplates |
| ☐ | **Talents in combat** (gap) | spend a point on the talent page before the run; effect visible in combat. Talents locked while engaged |
| ☐ | **Trinkets T/V** (gap) | equip a trinket with an active (Rallying Bell, Censer, Hourglass, Aegis); press; party gets the effect; cooldown shown |
| ☐ | **Kill the boss** (gap) | at least once, with real combat, not admin clear |
| ☐ | **Open the chest (E)** (gap) | 2 items per player, quality shown in the UI |
| ☐ | Recount (O) | current fight / overall; damage and healing per player |
| ☐ | Completion screen | grouped rewards; +1 talent point; back to the hub |
| ☐ | Key progression | owner key +1; a helper with a lower key also +1 |
| ☐ | Chat | `/s`, `/p` and `/w` between you and a helper (open a helper window by hand) |
| ☐ | **Reconnect during a boss fight** (gap) | close your client mid-boss and relaunch with the same `--profile`; you return to the same instance and position |
| ☐ | Party disconnect prompt | kill a helper process (Task Manager); you see Wait / Leave Party; Wait hides the prompt; Leave returns you to the hub |
| ☐ | Admin `/admin` | the live run is listed with players, presence, kills and ticks behind ~0–2 |

Known issues, so don't report these:

- at 960×540 the chat overlaps the party frames;
- an autoplay helper's chest is mailed as `unclaimed`.

---

## 4. P2 hub systems (real client)

Do several runs first, using admin clear if needed, so that you have gold, dust, epics, gems and
books.

| ✓ | Feature | Where | Expect |
|---|---|---|---|
| ☐ | **Sharpening** | Equipment → Workshop → Sharpen items | +0→+1 costs once; break chance shown on high levels; result persists after relog |
| ☐ | **Reforge** | Workshop → Reforge items | odds, EV and cost shown **before** confirm; quality changes, sharpening kept; broken items refused |
| ☐ | **Gems** | Workshop → Gems | socket (stat rises in the sheet), unsocket (gold charged once, gem back), upgrade (keep-one on fail) |
| ☐ | **Sets** | wear 2 / 4 / 5 pieces of one set (C) | bonus lines light up in the tooltip/panel; stats rise; set chime (online only) |
| ☐ | **Books** | talent corner button → Books | talent book +1 point; duplicate refused; enhancement book → choose option, switch option |
| ☐ | **Engaged lock** | try any of the above while queued or in a run | refused, with a reason shown on the button |
| ☐ | **Run timer** | inside any dungeon | HUD timer from barrier lift; warning at 60 s left; outcome timed / overtime |
| ☐ | **History** | hub → History | your runs: key, party, modifiers, outcome; opening a run shows details |
| ☐ | **Boards** | hub → Boards | highest key and fastest per key; personal best cue on a new best |
| ☐ | **Eternal keys** | push the key past 12 (admin clear helps) | odd levels add modifiers, even levels add HP/damage |
| ☐ | **New dungeons online** | queue Drowned Catacombs and Bone Foundry | **Undertow and conveyor do not rubber-band** (untested risk); report any snapping |
| ☐ | Sanctum online | queue Sanctum of Ash | pylon shield indicator, LOS Rite |
| ☐ | **Retry safety** | spam-click Sharpen, Reforge and Gem upgrade during lag (or pull the network) | gold, dust and gems change by exactly one operation |
| ☐ | **Persistence across restart** | note your gold, gear and key → `.\tools\realm-dev.ps1 restart` → relog | everything identical |

---

## 5. Community maps (never played in the real client)

| ✓ | Step | Expect |
|---|---|---|
| ☐ | Hub → Community Maps (or Dungeon Finder → Community maps) | browser opens; Back returns to where you came from |
| ☐ | My maps → Upload → type the path of an exported map (section 6), or `assets/templates/community_blank.json` after adding a spawn and monsters | draft listed; validator issues shown as `[code] Title: path - message` |
| ☐ | Publish | status becomes published; listed in Browse |
| ☐ | Browse: search (Enter), sort, difficulty filter, detail view | plays, clears and fastest clear are shown |
| ☐ | Play at level 1–10 → role → queue (helpers must also queue it; otherwise use `realm-bot`) | map loads; you can complete it |
| ☐ | Completion | **no** xp, loot, gold or key change; plays and community boards update; UNTIMED label if no timer |
| ☐ | Starting a map while queued | refused |
| ☐ | Save as new version / Unpublish | version bumps; an unpublished map leaves Browse |
| ☐ | Bad file (missing monster, loot block, timer 5) | upload/publish refused with `unknown_monster`, `loot_not_allowed` or `bad_timer` |

The helpers' `--dev-autoplay` only queues normal dungeons. For a community run, sign in two more
clients by hand, or run `realm-bot scenario community` for the full flow.

Known issue: reconnecting after a map is unpublished mid-run can't fetch the map.

---

## 6. Level editor — community authoring (P2)

```powershell
cargo build --release --bin mygame --features editor
Copy-Item assets/templates/community_blank.json assets/scenes/my_community.json
.\target\release\mygame.exe --editor --scene-file assets/scenes/my_community.json
```

| ✓ | Check | Expect |
|---|---|---|
| ☐ | Inspector tabs: Selection / Dungeon / Listing / Validate | all four present |
| ☐ | Listing → Make Community Map; type a title, description and tags | text accepted (only letters, digits, space, `-` and `.`, which is known) |
| ☐ | Place monsters; Dungeon tab shows Placed vs Needed | warns `impossible_objective` when short |
| ☐ | Mark a boss pack, add a spawn (P), a gate, a patrol waypoint, boss adds | each is one undo step (Ctrl+Z) |
| ☐ | Dungeon tab timer field (300–3600) | out of range → `bad_timer` |
| ☐ | Validate tab | issues update ~0.8 s after you stop editing; clicking an issue selects the object |
| ☐ | Export... with errors | refused |
| ☐ | Export... when clean | file written; upload it in section 5 |
| ☐ | Cross-check | `cargo run -p engine-realm --bin map_check -- assets/scenes/my_community.json` reports the same issues |

---

## 7. Performance spot check (optional)

Use the RTX 5080 baseline. Run headless, and A/B from one directory:

```powershell
.\target\release\mygame.exe --hidden --scene-file assets/scenes/bone_foundry.json --autocast --frames 20000 --report 1 --shadows high
```

Compare with `docs/P2_FINAL_REPORT.md` "Performance". For Bone Foundry that is about 1.2 ms CPU p50
and about 695 FPS.

---

## 8. Shutting down

```powershell
.\tools\realm-dev.ps1 down          # keeps Postgres data
.\tools\realm-dev.ps1 down -Wipe    # also deletes all accounts/characters - irreversible
```

## Reference

- P1 status, gaps and known bugs: `docs/P1_FINAL_REPORT.md`
- P2 status, deferred work and known issues: `docs/P2_FINAL_REPORT.md`
- Automated §46 run (30/30): `docs/P2_VERIFICATION.md`
- Stack details: `docs/networking/SPACETIMEDB_DEPLOYMENT.md`, `docs/networking/ADMIN.md`
- Community format and codes: `docs/community-maps.md`
- Older per-feature checklists (MT-001…MT-034): `docs/manual-tests.md`
