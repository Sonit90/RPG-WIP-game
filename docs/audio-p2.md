# P2 audio audit

Every P2 player action and event has a cue. All sounds are procedural (`engine-audio` recipes, no
binary assets); a WAV dropped in `assets/audio/<folder>/<name>.wav` still overrides any of them.
"New" means a recipe added to the catalogue in `crates/engine-audio/src/asset.rs` for this pass
(13 sounds, `SOUND_COUNT` 125); everything else reuses an existing sound.

Where each cue is decided:

* **Online front-end events** (`crates/engine-app/src/online_audio.rs`, wired in `online.rs` and
  `app.rs`): pure functions and edge watchers, each with a unit test.
* **In-world combat events** (`crates/engine-presentation/src/audio.rs`): `mechanic_sound`,
  `shield_sound` and the existing event observer, each with a unit test.

| Action / event | Cue | Where | Status |
|---|---|---|---|
| Sharpen success | `SharpenSuccess` (new: stone rasp + steel ring) | `online_audio::sharpen_sound` | added (was silent) |
| Sharpen break | `ItemBreak` (new: crunch + falling tone) | `sharpen_sound` | added (was the generic error tone) |
| Reforge, jackpot (110) | `LootJackpot` | `online_audio::reforge_sound` | existing, now table-driven |
| Reforge, 100+ and better | `LootEpic` | `reforge_sound` | existing |
| Reforge, better | `LootRare` | `reforge_sound` | existing |
| Reforge, worse | `UiError` | `reforge_sound` | existing |
| Reforge, same number | none (the toast reads it) | `reforge_sound` | by design |
| Gem socket | `GemSocket` (new: crystal click) | `gem_slot_sound` | added (was silent) |
| Gem unsocket (costs gold) | `GemUnsocket` (new: falling click) | `gem_slot_sound` | added |
| Gem upgrade success | `GemUpgrade` (new: three rising crystal notes) | `gem_upgrade_sound` | added (was `LootRare`) |
| Gem upgrade failure | `ItemBreak` | `gem_upgrade_sound` | added (was the generic error tone) |
| Book read | `BookRead` (new: page turn + warm note) | `online.rs` `BookRead` | added (was `LootRare`) |
| Book option switch | `BookSwitch` (new: page flick) | `app.rs` `SetBookOption` | added (was `UiSelect`) |
| Set bonus switches on when equipping | `SetBonus` (new: bright rising chord) | `online_audio::SetBonusWatch`, fed by `ApplyLoadout` | added; only a tier gained, never on login/hero change or removal |
| Run timer, 60 s left | `TimerWarning` (new: two low beeps), once | `NetSoundWatch` (`TimerPhase::Running -> Urgent`) | added; not on arriving mid-minute |
| Run timer expires | `ModifierResultWeak` (falling stinger) | `NetSoundWatch` (`-> Overtime`) | existing sound, new trigger |
| Completion, timed | `DungeonComplete` + `RunTimed` (new: bright sting) | `NetSoundWatch` | added |
| Completion, overtime | `DungeonComplete` + `RunOvertime` (new: low falling sting) | `NetSoundWatch` | added |
| Completion, no clock (untimed community map) | `DungeonComplete` only | `NetSoundWatch` | by design |
| Leaderboard personal best | `LootJackpot` + toast, once per run | `online_audio::is_personal_best` on the newest History page | added; best = fastest completed run of that dungeon and key among the page, not a first clear |
| Community map published | `PartyFormed` (warm chord) | `online_community::published_sound` | added |
| Community upload accepted | `ReadyAccept` | `CommunityState::saved` | added |
| Community upload / publish rejected | `UiError` | `saved`, `NetEvent::Failed(Community)` | added |
| Boss phase change (all bosses, incl. the new dungeons) | `BossPhase` | `AudioDirector` `PhaseChanged` | existing |
| Drowning Hymn (cast start) | `Tranquility` over the heavy-cast bed | `mechanic_sound` | added trigger, existing sound |
| Undertow (cast start) | `BossUndertow` (new: surging water) | `mechanic_sound` | added |
| Molten Ladle soak (cast start) | `SunfireCast` (warm sweep); the soak zone itself keeps the telegraph hum and burst | `mechanic_sound` | added trigger, existing sound |
| Conveyor Surge (cast start) | `BossConveyor` (new: grinding rumble) | `mechanic_sound` | added |
| Overheat (cast start) | `BattleFervor` (roar of power) | `mechanic_sound` | added trigger, existing sound |
| Pylon pulse (Ash Tithe, cast start) | `ModifierBeamsStart` (swelling hum) | `mechanic_sound` | added trigger, existing sound |
| Pylons make the boss immune | `SpellReflect` (glass catching light) | `shield_sound(true)` on `StatusApplied(Shielded)` | added |
| Last pylon falls, boss vulnerable | `ShieldBash` (struck metal) | `shield_sound(false)` on `StatusRemoved(Shielded)` | added |
| Last Rite (cast start) | `LastStand` (defiant swell) | `mechanic_sound` | added trigger, existing sound |

Buttons keep their `action_sound` mapping (click/select/cancel); the cues above are heard when the
realm *answers*, so a refusal never sounds like a success.

Known gaps, deliberately left: offline (non-realm) equipping does not chime set bonuses (the
persistent character only exists online); a reforge that rolls the same quality is silent.
