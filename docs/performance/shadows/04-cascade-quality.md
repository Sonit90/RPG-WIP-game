# Milestone 5 — cascade count, resolution and shadow distance

## What changed

The quality tiers were already implemented — cascade count, resolution, split distances,
filter, local-view budget and contact shadows all come from `ShadowPreset::settings()`, which
is the one place a quality level becomes numbers, and `ShadowSettings` carries every one of
them as a field. Nothing was hard-coded in a shader before this milestone and nothing is now.
What changed is three of the values, and one rule that was preventing a fourth.

| | Before | After |
| --- | --- | --- |
| Low, cascade distances | 15 / 50 m | **10 / 35 m** |
| High, resolution | 2048 | **1536** |
| High, contact range | 25 m | 15 m *(Milestone 3)* |
| resolution validation | rounded up to a power of two | **rounded up to a multiple of 256** |

The validation rule is the interesting one. `validated()` rounded any resolution up to the next
power of two, with a comment claiming the snapping arithmetic needed it. It does not: the atlas
lays tiles out by multiplication, the 512-texel local tiles divide every allowed cascade size
an integer number of times, and the texel-snapping in `cascade.rs` divides by the resolution
rather than shifting by it. The rule's only effect was to make 1536 impossible — and 1536 is
exactly what the default preset wants: 2.25× the texels of 1024 for 2.25× the memory, where the
next power of two is four times both for a difference nobody can see at three cascades.

## The tiers

| | Off | Low | Medium | **High (default)** | Ultra |
| --- | --- | --- | --- | --- | --- |
| cascades | – | 2 | 3 | 3 | 4 |
| resolution | – | 1024 | 1024 | **1536** | 2048 |
| shadow distance | – | **35 m** | 150 m | 150 m | 320 m |
| splits | – | 10 / 35 | 15 / 50 / 150 | 15 / 50 / 150 | 15 / 50 / 150 / 320 |
| filter | – | 1 tap | 4-tap disc | 4-tap disc | 6-tap disc |
| local shadow views | – | 0 | 4, spots only | 8 | 16 |
| contact shadows | – | off | off | 8 steps, 15 m, ½ res | 8 steps, 20 m, ½ res |
| atlas | 0 | 2048×1024, 8 MiB | 2048×2048, 16 MiB | **3072×3072, 36 MiB** | 4096×5120, 80 MiB |

Low's short range is a deliberate redistribution rather than a cut: two cascades over 35 m
instead of 50 puts 40 % more texels on the ground the player is standing on, which is where a
cheap configuration should spend what it has. Past 35 m there are no shadows at all, and at
this quality level that reads as a choice.

High's 36 MiB atlas is 28 MiB less than the 64 it used to take.

## Method

Identical to the earlier milestones. Every "before" row is the Milestone 1 binary — the last
one with the original presets — run in the same interleaved session.

## Results

GPU p50, three interleaved repetitions.

| Resolution | Preset | Before | After | Difference | Improvement | FPS after |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1280×720 | Off | – | 0.146 | – | – | 3378 |
| 1280×720 | Low | 0.098 | 0.104 | +0.006 | −6 % | 5525 |
| 1280×720 | Medium | 0.116 | 0.127 | +0.011 | −9 % | 4808 |
| 1280×720 | **High** | 0.224 | **0.183** | −0.041 | **18 %** | 4219 |
| 1280×720 | Ultra | 0.302 | 0.215 | −0.087 | 29 % | 3717 |
| 1920×1080 | Off | – | 0.180 | – | – | 3145 |
| 1920×1080 | Low | 0.187 | 0.193 | +0.006 | −3 % | 4237 |
| 1920×1080 | Medium | 0.217 | 0.233 | +0.016 | −7 % | 3636 |
| 1920×1080 | **High** | 0.448 | **0.346** | −0.102 | **23 %** | 2564 |
| 1920×1080 | Ultra | 0.600 | 0.393 | −0.207 | 35 % | 2288 |
| 2560×1440 | Off | – | 0.204 | – | – | 4049 |
| 2560×1440 | Low | 0.312 | 0.319 | +0.007 | −2 % | 2762 |
| 2560×1440 | Medium | 0.361 | 0.385 | +0.024 | −7 % | 2336 |
| 2560×1440 | **High** | 0.757 | **0.570** | −0.187 | **25 %** | 1626 |
| 2560×1440 | Ultra | 1.014 | 0.635 | −0.379 | 37 % | 1466 |
| 3840×2160 | Off | – | 0.425 | – | – | 2132 |
| 3840×2160 | Low | 0.672 | 0.667 | −0.005 | 1 % | 1422 |
| 3840×2160 | Medium | 0.772 | 0.812 | +0.040 | −5 % | 1153 |
| 3840×2160 | **High** | 1.613 | **1.216** | −0.397 | **25 %** | 766 |
| 3840×2160 | Ultra | 2.469 | 1.347 | −1.122 | **45 %** | 698 |

And the shape of the ladder afterwards, at 4K — each tier's cost over the one below it:

| Step | GPU p50 | Added |
| --- | ---: | ---: |
| Off | 0.425 | – |
| Low | 0.667 | +0.242 |
| Medium | 0.812 | +0.145 |
| High | 1.216 | +0.404 |
| Ultra | 1.347 | +0.131 |

The presets are monotonic and the steps are of a sensible size. High is the largest step
because it is the one that turns contact shadows on.

## The two regressions, named

**Low and Medium are 2–9 % slower than they were.** Neither is a cascade change; both are the
cost of Milestones 1 and 4 landing on presets that get nothing back from Milestones 2 and 3,
because they have no contact shadows to make cheaper. Attributed at 4K Medium, measured:

| Configuration | GPU p50 |
| --- | ---: |
| Milestone 1 binary (3×3 grid, no light grid) | 0.771 |
| now, forced to the 3×3 grid | 0.791 |
| now, as it ships (4-tap disc) | 0.813 |
| now, forced to a single tap | 0.685 |

So of Medium's +0.042 ms, +0.020 is the clustered light grid and +0.022 is the disc kernel
costing slightly more than the nine-tap grid it replaced — even at fewer fetches, because the
per-pixel rotation is not free and four taps is too few for the fetch saving to cover it.

That second one is a trade taken with the numbers in hand rather than an oversight. On the same
scene the four-tap disc gives a 6-pixel penumbra against the 3×3's 4, a third less local noise,
and 37 % less drift when the camera moves. Medium pays 3 % for a filter that is better on every
axis measured. Low pays for the light grid alone and gets a shorter, denser shadow range for it.

## Visual quality

* **High at 1536 against 2048.** Three cascades over 150 m: the near cascade's texel goes from
  17 mm to 23 mm. On the arena and the dungeon that is not a difference the eye finds — the
  contact shadows resolve the seam the cascade no longer does, which is the whole reason High
  runs them. Where it *is* visible is a long straight shadow edge at 30–50 m, in the second
  cascade, where the staircase is a texel wider. Judged worth 28 MiB and 25 % of the frame.
* **Low's 35 m range.** The far edge is a fade over the last tenth of the range, not a line;
  the arena's boundary wall stops casting at about 35 m, which is a visible change from 50 and
  a defensible one for the cheapest tier. The ground the player is on is noticeably better than
  it was, which is the trade.
* **The ladder reads correctly.** Off → Low is the largest visual step, Low → Medium adds a
  cascade and spot-light shadows, Medium → High adds contact shadows and 2.25× the texels,
  High → Ultra adds the fourth cascade, the wider filter and point-light shadows.
* One caster-LOD threshold moved: at 1536 a 4 cm pebble in the near cascade now covers fewer
  than `cull_texels` (2) texels and is dropped where at 2048 it was drawn. That is the LOD rule
  working as designed against a coarser map, and the unit test that asserted the old behaviour
  now asserts the threshold explicitly rather than a size that happened to sit above it.

## Regressions

1. Low −2 % to −6 %, Medium −5 % to −9 %, attributed above.
2. `ShadowSettings::validated` no longer guarantees a power-of-two resolution. Nothing depended
   on it; the test that asserted it now asserts a multiple of 256 in range.

## Conclusion

**Keep it.** The default preset — the one almost every player will run — got 25 % faster and
28 MiB smaller for a difference that has to be looked for. Ultra is 45 % faster than it was at
4K. The two cheap tiers gave up a few per cent to changes that made their filtering better, and
they are the tiers where a few per cent of a 0.2 ms frame is not the constraint.

## Reproducing

```powershell
powershell -ExecutionPolicy Bypass -File tools/bench/shadow_matrix.ps1 `
  -Configs "off|off||;Low|low||;Medium|medium||;High|high||;Ultra|ultra||;HEAD Low|low||target/release/mygame-head.exe|;HEAD Medium|medium||target/release/mygame-head.exe|;HEAD High|high||target/release/mygame-head.exe|;HEAD Ultra|ultra||target/release/mygame-head.exe|"
```
