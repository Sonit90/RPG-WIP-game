# Manual test checklist

What the automated suite cannot assert: that a window actually appears, that a resize is
free of artefacts, that input feels connected to what happens on screen, that the overlay
is legible. Everything here needs a human and a real desktop session.

Run the whole list before declaring a phase complete, and the affected entries after any
change to the window, the swap chain, input handling or the frame loop.

Unless an entry says otherwise, build first:

```powershell
cargo build --release
```

Record the result as PASS / FAIL / SKIPPED with the date and the machine. A SKIPPED entry
is not a pass — see MT-009 and MT-010, which need hardware not every machine has.

---

## MT-001 — Window startup, resize and close

**Purpose** — the window opens at the requested size, survives resizing, and closes
cleanly.

**Steps**

1. `.\target\release\mygame.exe`
2. Observe the window and its title bar.
3. Drag one border slowly, then a corner quickly.
4. Maximize, then restore.
5. Close with the X button.

**Expected** — a 1920×1080 window titled "myGame — phase 1". Resizing is continuous with
no stretched, torn or black frames, and the log reports each new size once rather than
hundreds of times. Maximize and restore behave the same. Closing exits with
`clean shutdown` in the log and no crash dialog.

---

## MT-002 — Minimize and restore

**Purpose** — a zero-sized client area does not crash the renderer.

**Steps**

1. `.\target\release\mygame.exe`
2. Minimize the window and wait five seconds.
3. Restore it.
4. Close.

**Expected** — the log shows `minimized` then `restored`. No rendering or presenting
happens while minimized (the frame counter still advances). The window paints correctly
after restoring, and frame times return to their previous range.

---

## MT-003 — Keyboard input

**Purpose** — key presses reach the engine, transitions are correct, auto-repeat does not
re-trigger.

**Steps**

1. `MYGAME_LOG=trace` then `.\target\release\mygame.exe --report 0`
   (PowerShell: `$env:MYGAME_LOG='trace'`)
2. Tap W, A, S, D, Space, each of 0–9, and Escape.
3. Hold W for three seconds, then release.
4. Press and hold Shift, Ctrl and Alt in turn.

**Expected** — every key produces a trace line naming it. Holding a key does not produce a
stream of *press* lines; the repeats are marked as repeats. Left and right modifiers are
distinguished. No key is reported as `Unknown` from the set above.

---

## MT-004 — Mouse input

**Purpose** — movement, buttons and the wheel reach the engine with correct coordinates.

**Steps**

1. `$env:MYGAME_LOG='trace'` then `.\target\release\mygame.exe --report 0`
2. Move the cursor to each corner of the client area.
3. Click left, right and middle; use the two side buttons if the mouse has them.
4. Scroll up and down.
5. Drag with a button held from inside the window to outside it.

**Expected** — coordinates are within the client area and increase to the right and
downward; the top-left corner reads near (0, 0). Every button is reported distinctly.
Wheel notches are ±1 per click. Dragging outside the window reports negative or
larger-than-client coordinates rather than wrapping to a huge positive number.

---

## MT-004a — Camera: orbit and steer

**Purpose** — both mouse buttons turn the camera, and they differ in what else they do.

**Steps**

1. `.	arget
elease\mygame.exe`
2. Hold the **left** button and drag left and right, then up and down. Watch the character.
3. Release, then hold the **right** button and drag the same way. Watch the character.
4. Hold the left button, and while still holding it press and release the right button.
5. Press Escape to open the menu, then hold the left button over a menu button and drag.
6. Press 4 to aim an area spell, then try holding each button.
7. Alt-tab away while holding either button.

**Expected** — the left button orbits the camera around the character while the character
keeps facing wherever it was. The right button orbits *and* turns the character to face
where the camera looks. In step 4 the cursor stays hidden throughout: adding and removing
the right button changes what the character does without the cursor flickering back. In the
menu the left button clicks buttons and does not take the cursor away; the right button
still turns the camera. While aiming, the left button confirms the placement and the right
cancels it — neither turns the camera. Alt-tab restores the cursor immediately.

---

## MT-005 — Focus loss releases held input

**Purpose** — a key held while alt-tabbing away does not stay stuck down.

**Steps**

1. `.\target\release\mygame.exe --overlay`
2. Hold W and, while holding it, alt-tab to another window.
3. Release W outside the game.
4. Alt-tab back.

**Expected** — the game reports focus lost and treats every key as released. Returning to
the game leaves nothing held; no movement or repeated action continues on its own.

---

## MT-006 — Debug overlay

**Purpose** — the overlay is legible, correct and toggleable at runtime.

**Steps**

1. `.\target\release\mygame.exe`
2. Press F5 to show the overlay; press it again to hide it; show it again.
3. Compare the FPS and CPU numbers against the `--report 1` log lines.
4. Resize the window and check that the resolution line updates.

**Expected** — the overlay appears immediately on the first press, not a quarter of a
second later. Text is readable against the scene. FPS, CPU ms, GPU ms, triangle and draw
counts, resolution and adapter name all appear, update about ten times a second, and agree
with the log. Hiding it removes its draw call (visible in the reported draw count). See
MT-015 for the combat and simulation lines.

---

## MT-007 — Playable prototype

**Purpose** — the Phase 3 acceptance test: the game is playable, and every combat mechanic
is visible on screen rather than only in a test.

**Steps**

1. `.	arget
elease\mygame.exe --overlay`
2. Left-click to capture the mouse, then move it: the camera orbits the character.
3. Hold W, A, S and D; the character runs in the direction the camera faces.
4. Press SPACE while running.
5. Press 1 several times in a row.
6. Press 2, wait a second, then press 3.
7. Press TAB a few times, then walk into the dummy group and press 3.
8. Press R.
9. Roll the mouse wheel both ways. Press ESC.

**Expected**

* A blue box on a dark green floor, a red dummy ahead and four more to the right, with a
  yellow marker floating above the selected one.
* The camera follows smoothly and orbits without the character sliding across the screen.
* The character jumps once per press, arcs, lands, and cannot double-jump by holding SPACE.
* The first 1 damages the target — its health falls in the overlay — and the next presses
  within 1.5 s do nothing except print `CANNOT CAST: ON COOLDOWN`.
* After 2, the target keeps losing health for six seconds with no further input.
* 3 pressed on a burning target removes far more health than 3 on a cold one.
* 3 next to the group damages all four dummies at once and leaves the lone dummy untouched.
* TAB moves the marker between dummies.
* R restores every dummy to full health and resets DAMAGE DONE and DPS to zero.
* The wheel changes the camera distance between about 2.5 and 25 metres; ESC releases the
  cursor.
* Every dummy carries a nameplate reading `DUMMY` with its health percentage on the right.
  The targeted one — and only that one — has a white outline. After pressing 2 the burning
  dummy's plate grows a second line reading `BURNING` and a countdown. Plates stay above the
  heads as the camera turns, vanish when an enemy is behind the camera, and disappear once an
  enemy is more than 40 metres away (run to the far side of the arena to check).

---

## MT-008 — Fullscreen

**Purpose** — borderless fullscreen covers the monitor at its native resolution.

**Steps**

1. `.\target\release\mygame.exe --fullscreen --overlay`
2. Check the overlay's resolution line against the monitor's native resolution.
3. Alt-tab away and back.
4. Close with Alt+F4.

**Expected** — the window covers the whole monitor with no border, no title bar and no
taskbar overlap. The resolution matches the monitor natively (on a scaled display too —
DPI awareness is set before the window is created). Alt-tab is immediate, with no mode
switch and no black screen. Alt+F4 exits cleanly.

---

## MT-009 — Multi-monitor

**Purpose** — the window behaves on a machine with more than one display.

**Requires** — two monitors, ideally with different resolutions or DPI scaling.

**Steps**

1. `.\target\release\mygame.exe --overlay`
2. Drag the window to the second monitor.
3. Resize it there.
4. Close, then run with `--fullscreen`.

**Expected** — the window renders correctly on either monitor, and the overlay's
resolution line follows the client size. Fullscreen covers the **primary** monitor; that
is the current, documented behaviour, not a bug (see Known limitations in
`docs/phase1.md`).

---

## MT-010 — GPU initialization on a second machine

**Purpose** — startup fails informatively, rather than silently, on hardware without a
D3D12 adapter.

**Requires** — a machine or VM without a D3D12-capable GPU.

**Steps**

1. Run the packaged binary there.
2. Read the error.

**Expected** — a message naming the failure and how many adapters were examined, an
`unsupported feature` category, and a non-zero exit code. No crash dialog, no silent exit.

---

## MT-011 — Validation layer

**Purpose** — a debug build reports GPU misuse; a release build does not pay for it.

**Steps**

1. `cargo run --bin mygame -- --frames 300` (debug)
2. `.\target\release\mygame.exe --frames 300` (release)
3. `.\target\release\mygame.exe --frames 300 --validation`

**Expected** — the debug run enables the validation layer and reports no validation
errors. The plain release run does not enable it and is several times faster per frame.
The third run enables it explicitly in release and is correspondingly slower — that is the
point of the option.

---

## MT-012 — Release package

**Purpose** — the shipped package runs on its own and contains no test infrastructure.

**Steps**

1. `.\tools\package.ps1`
2. Read the verification output.
3. Inspect `dist\mygame\` by hand.
4. Copy the directory to a machine without the Rust toolchain and run it.

**Expected** — the script prints `PASS: package contains no test or benchmark
infrastructure`, and the directory contains `mygame.exe` and `README.md` and nothing else.
No `performance_test.exe`, no `cpu_bench.exe`, no `.pdb` from a test target. The copy runs
on a clean machine with no toolchain installed.

---

## MT-013 — Log levels

**Purpose** — log filtering works and verbose logging is absent from release by default.

**Steps**

1. `.\target\release\mygame.exe --frames 60 --report 0`
2. `$env:MYGAME_LOG='trace'; .\target\release\mygame.exe --frames 60 --report 0`
3. `$env:MYGAME_LOG='error'; .\target\release\mygame.exe --frames 60 --report 0`
4. `Remove-Item Env:\MYGAME_LOG`

**Expected** — the default release run logs startup and shutdown at INFO and nothing
per frame. Setting `trace` does **not** produce trace output in release: those calls are
compiled out. Setting `error` silences everything below ERROR. Each line carries a
timestamp, level, thread name and subsystem.

---

## MT-014 — Long run stability

**Purpose** — nothing accumulates over a long session.

**Steps**

1. `.\target\release\mygame.exe --overlay --report 10 --dummies 500`
2. Leave it running for 30 minutes, interacting occasionally.
3. Watch memory in Task Manager and frame times in the overlay.

**Expected** — memory is flat after the first few seconds. Frame times at the end match
those at the start. No warning about the frame allocator exceeding its budget. The
simulation's tick count keeps pace with elapsed time (roughly 60 per second) and the SIM
line does not grow. Closing at the end is still clean.

---

## MT-015 — Developer overlay: combat and simulation

**Purpose** — the overlay reports what is actually happening, not plausible numbers.

**Steps**

1. `.	arget
elease\mygame.exe --overlay`
2. Note DAMAGE DONE, then cast 1 once and note it again.
3. Watch the SIM line while standing still, then while running.
4. Watch a cooldown on the ability bar after casting.
5. Press R.

**Expected** — DAMAGE DONE rises by roughly the damage of one Fireball and DPS becomes
non-zero. The SIM line shows a sub-millisecond figure, `TICKS 0` or `1` most frames, and
`60 HZ`. A cast ability's bar entry counts down in tenths and returns to READY exactly when
the ability becomes usable again. R zeroes the meter and every cooldown.

---

## MT-016 — Character test scene and animation

**Purpose** — the FireMage looks right, is the right size, and every animation reads. Most
of this is asserted in pixels by `crates/engine-renderer/tests/character_render.rs`; what a
human is still needed for is whether the motion *reads*, which no assertion covers.

**Steps**

1. `.	arget
elease\mygame.exe --scene character`
2. Look at the character beside the training dummy.
3. Walk forwards, then hold shift-free full speed; watch the feet.
4. Jump, and watch the take-off, the fall and the landing.
5. Press 1 through 7 in turn, then 0.
6. Hold the right mouse button and orbit the character while an animation is held.
7. Run 40 metres away from the dummy and back.

**Expected** — a hooded figure with a staff, about the same height as the dummy beside it,
standing on the floor rather than in or above it. Walking and running move the feet at the
speed the character travels, with no sliding. A jump compresses, extends, falls with the
legs trailing and compresses again on landing. Keys 1 to 7 hold the character in one
animation regardless of what it is doing; 0 releases it and the character immediately
reflects gameplay again. Orbiting shows no geometry inside out and no surface lit from the
wrong side. At distance the dummy visibly loses its crossbar and wrap and keeps its
silhouette — a level-of-detail switch, not a pop out of existence.

---

## MT-017 — Combat still works with models

**Purpose** — replacing the placeholder cubes changed no combat behaviour.

**Steps**

1. `.	arget
elease\mygame.exe --dummies 8`
2. Tab to a target, cast 1, and watch the dummy.
3. Cast an area ability into the group.
4. Kill one dummy.
5. Press BACKSPACE.

**Expected** — the targeted dummy glows, flashes white on each hit, darkens as its health
drops, and the target marker floats above it. Damage numbers rise from chest height. An
area ability marks and hits the same set the indicator showed. A killed dummy tints to a
lifeless grey and falls over on the floor rather than through it. BACKSPACE restores every
dummy upright and undamaged.

---

## MT-018 — The combat arena reads as a place

**Purpose** — the environment is visually coherent, and it does not get in the way of the
fight.

**Steps**

1. `.\target\release\mygame.exe`
2. Stand at the spawn and turn a full circle with the right mouse button.
3. Walk to each of the four braziers and look back at the middle.
4. Walk into the gate arch, the kerb, a pillar and a crate.
5. Walk through a fern and a shrub.
6. Look up.
7. Look at the horizon in each direction.
8. Place an area ability at the edge of the fighting space.

**Expected** — a paved arena inside a chest-high stone kerb, punctuated by pillars, with a
broken arch over a gap on one side. Four lit braziers mark the corners of the fighting
space. Past the kerb, a treeline; behind that, distant ridges; above, a sky that grades from
warm at the horizon to blue overhead, with no seam where the ground ends. The kerb, the
pillars, the braziers and the crates stop the character; the ferns and shrubs do not, and
the character walks through them without catching. Nothing stands inside the fighting space,
so an area indicator lies flat on the paving with nothing clipping through it. The floor
does not flicker anywhere.

---

## MT-019 — The environment demo scene

**Purpose** — every asset can be inspected on its own, in the light the game runs.

**Steps**

1. `.\target\release\mygame.exe --scene environment`
2. Walk to the far side of the arena, where two rows of objects stand.
3. Walk along each row, looking at each object from three sides.
4. Back away from the rows to the middle of the arena and watch them.
5. Press F3, then walk into the row again.
6. `.\target\release\mygame.exe --scene environment --env-density 8`

**Expected** — one of every environment asset, each standing on the floor at the size it is
used at, each lit and shaded so its form reads. Backing away, the pines, pillars and larger
rocks visibly simplify without vanishing. F3 draws the collision boxes: the wall, pillar,
brazier and crate exhibits have one each, and the plants have none. At density 8 the treeline
and the scatter are several times denser, the arena itself is unchanged, and the frame rate
in the F5 overlay stays well above the target.

---

## MT-020 — The VFX test bed

**Purpose** — every effect in the catalogue can be played on demand and judged on its own.

**Steps**

1. `.	arget
elease\mygame.exe --scene vfx`
2. Press 1 through 0, then Z, X and C, watching each effect from a few metres back.
3. Press 9 (SHIELD) and 8 (IGNITED) and walk around while they play.
4. Orbit the camera during an explosion (7) and during the ultimate (C).
5. Press C and, while the pillar is up, walk behind it and look at the dummies.
6. `.	arget
elease\mygame.exe --scene vfx --vfx-stress 120` and watch the F5 overlay.

**Expected** — thirteen distinct effects, each starting where the character is facing, each
ending on its own without leaving anything behind. The shield and the burn ride the character
as they walk, and neither hides them: the shield is a ring at the feet and sparks, not a
bubble. Nothing is drawn as an opaque wall — during the ultimate the dummies behind the
pillar stay visible. At a stress of 120 effects a second the frame rate in the overlay stays
far above the target and the log line reports effects being refused rather than the frame
degrading.

---

## MT-021 — Combat reads

**Purpose** — the phase's actual goal: an ability press produces a readable chain of
feedback, and gameplay stays authoritative.

**Steps**

1. `.	arget
elease\mygame.exe`
2. Select a dummy and cast Firebolt (1). Watch the character, the space between you and the
   dummy, and the dummy.
3. Cast Flame Lance (2) and Pyroblast (5) and compare the size of the bolts.
4. Place a Meteor (4) on the group and watch the ring before it lands.
5. Cast Flame Cone (3) and Firestorm (6) at the group; compare where the fire is with which
   dummies take damage.
6. Build a full burn on one dummy and press Detonate (7).
7. Press Flame Shield (9), then Blink (0) while it is up.
8. Watch the damage numbers during a rotation, looking for the orange ones.
9. Press Escape mid-cast and watch the cast effect.

**Expected** — casting shows a gather at the chest for exactly the cast time; a bolt leaves
the caster and an impact plays where it lands; the dummy flashes and a number floats up.
Heavier spells throw visibly bigger bolts. The Meteor's ring closes over its delay and the
explosion lands inside it. The cone and the line of fire cover exactly the dummies that take
damage. Detonate explodes on the target. The shield is visible for its whole duration and
gone the instant it ends; blinking shows a departure and an arrival at both ends of the jump.
Critical strikes are orange and larger, killing blows red and larger still. Cancelling a cast
takes its effect with it on the same frame.

---

## MT-022 — The audio test bed

**Purpose** — every sound in the catalogue can be played on demand, from a chosen distance,
and judged on its own.

**Steps**

1. `.\target\release\mygame.exe --scene audio`
2. Step through the catalogue with Z and X, pressing C for each one. Read the log line to
   see which sound is selected.
3. Press C, then walk away from where you dropped it, then back towards it, then past it.
4. Press C, then orbit the camera all the way around with the left mouse button held.
5. Press V for the same sound and compare it with C.
6. Press N (eight at once) on `hit_normal`, then on `ignite_loop`, and read the log line.
7. Open the menu with Escape and drag each of the four volume sliders while a loop plays.
8. Press F8 twice.

**Expected** — 39 distinct sounds, none of them a click, none of them clipped. A dropped
sound gets quieter with distance and is silent well before the far side of the arena;
walking past it moves it from one speaker to the other; orbiting the camera swings it across
the stereo field, and a sound directly behind is still audible. V plays the same sound dead
centre at full level however far away you stand. Eight at once produces a handful of voices
and a log line reporting the rest as refused — **not** eight copies, and never a wall.
Dragging MASTER, EFFECTS, INTERFACE and AMBIENCE changes only what each names, live, with no
step or click, and the values survive a restart. F8 silences everything and brings it back.

---

## MT-023 — Combat is heard

**Purpose** — the phase's actual goal: an ability press produces a readable chain of sound
that agrees with the picture, and gameplay stays authoritative.

**Steps**

1. `.\target\release\mygame.exe --dummies 8`
2. Select a dummy and cast Firebolt (1). Listen for the cast, the flight and the impact.
3. Cast Flame Lance (2) and Pyroblast (5); listen through the cast and to where it ends.
4. Place a Meteor (4) on the group; listen while it falls and when it lands.
5. Cast Flame Cone (3) and Firestorm (6) at the group, then walk over the burning ground and
   away from it.
6. Build a full burn and press Detonate (7).
7. Press Flame Shield (9), then Blink (0) while it is up. Jump, and jump off something high.
8. Run in a straight line across the arena.
9. Cast Pyroblast and press Escape mid-cast.
10. Pull the whole group, let everything burn at once, and listen for a wall of noise.

**Expected** — the cast, the bolt and the impact are three distinct moments, and the impact
lands on the same frame as the flash. Heavier spells are lower and longer. The Meteor whistles
from where it is going to land and stops the instant it lands. The line of fire is a bed you
can hear yourself walk into and out of, and it stops when the fire does. Detonate is heard on
the target, not on the caster. The shield hums for exactly its duration; Blink is two sounds
at two places; a jump and a hard landing are different sounds. Footsteps keep time with the
legs and stop when you do. Cancelling a cast takes its gather away and replaces it with a
fizzle on the same frame. With the whole pack burning, the fire is *one* fire and individual
hits are still audible over it — critical strikes especially.

---

## MT-024 — The encounter test bed

**Purpose** — enemies notice, fight, telegraph, die and reset; the player can die too.

**Steps**

1. `.\target\release\mygame.exe --scene encounter`
2. Stand still. Watch both enemies for ten seconds.
3. Walk slowly towards the brute until it reacts. Watch it, not the ground.
4. Let it reach you. Watch the cast bar under the target frame and the floor in front of it.
5. Walk out of the cone while it is being cast.
6. Pull the caster from a distance with Firebolt (1) and watch where its circle appears.
7. Stand in the circle once. Then walk out of it once.
8. Press G while it is casting (developer interrupt) and watch the bar and the floor.
9. Kill both. Watch the encounter line in the log.
10. Let them kill you: stand still and do nothing.
11. Press BACKSPACE.
12. Run 25 m away mid-fight and keep running.
13. `.\target\release\mygame.exe --scene encounter --enemies 20` and pull the lot.

**Expected** — idle enemies stand still and face the way they spawned. The brute notices at
about 14 m, pauses visibly, then closes and stops at arm's length rather than walking through
you. Its cleave draws a cone on the floor for its whole cast; walking out of it takes no
damage, standing in it does. The caster keeps its distance and draws a circle *where you are
standing*, not where you will be — walking out of it is always possible. G removes both the
cast bar and the shape on the floor, and no damage lands. Killing them completes the
encounter (in the log) and the corpses do nothing. Dying prints YOU DIED, stops your
abilities, and leaves the world running. BACKSPACE puts everybody back: enemies home, idle
and whole, you alive and at full health. Running away long enough sends them home; they walk,
they do not teleport, and they forget you. Twenty enemies all fight, all telegraph, and the
frame rate stays far above the target.

---

## MT-025 — A wall stops a targeted spell

**Purpose** — a targeted ability that requires line of sight is refused when the level is
between the caster and the target, the refusal says so, and the developer view shows why.

**Steps**

1. `.\target\release\mygame.exe --scene dungeon`
2. Pull a pack, then step back around a corner so a wall stands between you and an enemy
   that is still selected and still well inside 35 m.
3. Press 1 (Firebolt). Read the message.
4. Press F3 and look at the line drawn between you and the target, and at the overlay text.
5. Step back into the doorway so the enemy is visible again and press 1.
6. Start a Pyroblast (5) with the enemy in the open, and while it casts let the enemy walk
   behind a pillar.
7. Repeat step 6, but let the enemy walk back into view before the cast bar fills.
8. Press F3 again to turn the developer view off.

**Expected** — behind the wall, the cast is refused with TARGET IS NOT IN LINE OF SIGHT and
no damage is dealt; the cast bar never starts. With F3 on, a red bar runs from your chest to
the point on the wall where it stopped, the blocking wall is lit red, and the overlay reads
`LOS`, `RANGE OK / REQUIRED YES`, `RESULT BLOCKED` and the blocker's name. Stepping into the
doorway makes the same cast work and turns the bar green all the way to the target. The
Pyroblast that loses its target mid-cast ends early, deals nothing and prints the same
refusal; the one whose target steps back into view lands normally. F3 removes the drawing
and the extra overlay lines together, and the combat HUD never shows any of it.

---

## MT-026 — The shadow course

**Purpose** — shadows are correct, stable and adjustable in the running game, not only in a
pixel test: no acne, no floating objects, no seam where cascades meet, alpha-tested casters
keep their silhouette, and the quality setting does what it says.

**Steps**

1. `.\target\release\mygame.exe --scene shadows`
2. Walk to the far-left bay (one pillar on empty ground). Circle it. Look at the base of
   the pillar and at the small rock beside it.
3. Walk to the middle bay and follow the line of rocks away from you, slowly, then back.
   Watch the ground and the rocks' shadows as you move.
4. Press F7 once (cascade tint) and walk the same line again. Press F7 again (shadow
   factor), again (the atlas), again (caster bounds), then again to turn it off.
5. Walk to the leaf screens on the near left. Look at their shadows on the ground.
6. Walk into the colonnade on the near middle, under the spot lights.
7. Walk into the ring of braziers on the near right and stand among the crates.
8. Press F6 five times, pausing on each level, watching the same view.
9. Press F5 and read the shadow block.
10. Quit, restart with `--scene shadows`, and check the quality is the one you left on.

**Expected** — the pillar's shadow starts *at* its base with no gap and no dark stripes on
the pillar itself; the small rock has a shadow too, at its own scale. Down the line of rocks
the shadows get softer and coarser with distance, which is correct, but nothing shimmers,
crawls or snaps as you move, and there is no visible line drawn across the ground where one
cascade hands over to the next. The cascade tint shows those handovers as a smooth colour
blend rather than a hard edge; the shadow factor view shows clean grey shapes with no
speckle; the atlas view shows the depth maps bottom-left with red tile outlines, and the
tiles that should hold something do. The caster-bounds view puts a yellow sphere around every
crate, rock and pillar and nothing around the ground, and turning it off leaves the scene
exactly as it was. The leaf screens cast a scatter of leaf shapes, not
rectangles. The colonnade's pillars cast under the spot lights; at Medium the far one loses
its shadow and at High it has it back. Standing among the braziers, the crates cast in
several directions at once. F6 changes the picture visibly at each step and Off removes
shadows entirely without any other change to the scene. The overlay's shadow block reports a
plausible atlas size and a sub-millisecond stage. The setting survives the restart.

---

## MT-027 — Shadows in the shipping scenes

**Purpose** — the shadow system works in the scenes a player actually plays, not only in the
bed built for it.

**Steps**

1. `.\target\release\mygame.exe --scene combat` — watch your own character's shadow while
   running, jumping and casting.
2. `.\target\release\mygame.exe --scene dungeon` — walk from the entrance towards the first
   braziers and look at the walls and floor around them.
3. `.\target\release\mygame.exe --scene encounter --enemies 20` — pull the lot and watch the
   frame time in the F5 overlay while twenty enemies move.
4. In each, press F6 to Off and back to High.

**Expected** — the character's shadow follows the animation rather than the bind pose, and
stays attached to the feet. In the dungeon the braziers cast local shadows that move
correctly around the crates and pillars near them, and the sun's cascades still work
outdoors. With twenty enemies the shadow stage stays well under a millisecond and the frame
time does not step up when they come into view. Off and back leaves the scene otherwise
identical.
