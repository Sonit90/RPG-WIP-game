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

1. `.	argetelease\mygame.exe --scene character`
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

1. `.	argetelease\mygame.exe --dummies 8`
2. Tab to a target, cast 1, and watch the dummy.
3. Cast an area ability into the group.
4. Kill one dummy.
5. Press BACKSPACE.

**Expected** — the targeted dummy glows, flashes white on each hit, darkens as its health
drops, and the target marker floats above it. Damage numbers rise from chest height. An
area ability marks and hits the same set the indicator showed. A killed dummy tints to a
lifeless grey and falls over on the floor rather than through it. BACKSPACE restores every
dummy upright and undamaged.
