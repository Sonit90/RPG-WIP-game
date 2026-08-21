# The Character Sheet

Seven stats, fifteen equipment slots, a bag of thirty-two cells, and one drag that does all
four things a player can do with an item. Opened with `C` or the button beside the talent
button; the game keeps running behind it.

## What a character is

**One slice of one item table, plus a hero.**

```text
CharacterSheet ─┬─ hero          which hero this sheet is for
                ├─ equipment     15 slots, each holding an ItemStack or nothing
                └─ inventory     32 cells, likewise

                stats() = hero.base_stats() + every worn item's stats
                        (+ the Spirit buff, through stats_with)
```

`engine-game/src/character/`. Three sheets exist — one per hero, held in a `Roster` — because
swapping to the Warrior is becoming a different *character*, not putting a different weapon on
the same one. The panel is written once against "the sheet of the hero the player currently
is", so a fourth hero adds no branch anywhere.

## Stats

| Group | Stat | What it is |
| --- | --- | --- |
| Primary | **Intellect** | Main stat of the FireMage and the Druid. Is spell power. |
| Primary | **Strength** | Main stat of the Warrior. |
| Primary | **Stamina** | Maximum health, at ten health a point. |
| Secondary | Critical Strike | Chance to critically strike. |
| Secondary | Haste | Faster damage-over-time ticks, shorter global cooldown. |
| Secondary | Versatility | More damage and healing done, less damage taken. |
| Secondary | Spirit | Chance at an Ultimate point and a stacking main-stat buff. |

Every hero starts at 100 main stat and 50 Stamina. **Every secondary stat is zero, on every
hero and on every item**, and one test asserts that no item in the game grants one — which is
the cheapest possible guarantee that displaying Spirit on a panel cannot make Spirit happen.

### One block, not seven fields

`StatBlock` is `[f32; 7]` indexed by the `Stat` enum. A struct with seven named fields makes
adding an eighth stat a change in the item table, the sheet, the tooltip and the panel; a
block makes it a variant and a row of text, and the summing code never changes. The
aggregation is written *once*, in `CharacterSheet::stats`, and no interface component adds up
a stat — the panel asks for the finished block and prints it.

Nothing caches the total. It is recomputed on every call, which is the rule
`firemage/stats.rs` already follows and for the same reason: the moment gear can change
mid-fight, a stored total is a character whose health bar and whose damage disagree about what
they are wearing.

### Where the numbers go

```text
Stamina ──────► Health::set_max, keeping the fraction
main stat ────► FireMage::gear.spell_power  (base main stat == base spell power,
haste rating ─► FireMage::gear.haste         so gear is exactly the difference)
crit rating ──► FireMage::gear.crit_chance
```

`Simulation::refresh_character` is the one place that pushes. It runs when an item moves, a
hero swaps, a save loads, or a Spirit stack falls off — not every tick, because on almost
every tick nothing has changed.

Health preserves its *fraction* across a change of Stamina, so putting a trinket on is not a
free heal and taking one off is not a way to die.

Five hundred health did not move; it stopped being a constant and started being a consequence.
`BASE_STAMINA * HEALTH_PER_STAMINA == tuning::player::MAX_HEALTH`, held together by a test, so
changing either without the other is a failure rather than a silently rebalanced enemy roster.

## Spirit

Defined in full, and inert.

```text
a proc ──► +1 Ultimate point
       └─► +1 stack: +2 % main stat, for 10 seconds
           1 stack +2 %   ·   2 stacks +4 %   ·   3 stacks +6 %
```

Each stack keeps its own clock — a burst of four leaves one at a time, ten seconds after each
was gained — so `SpiritSurge` is an array of remaining times rather than a count and one
timer. `proc_chance(0.0)` is zero and the roll is strictly less-than, so at the current Spirit
rating of zero the mechanic cannot fire. Turning it on means putting Spirit on an item.

## The fifteen slots

```text
        HEAD                              BACK
    SHOULDER      ┌──────────────┐    NECKLACE
       CHEST      │  the         │       WRIST
       HANDS      │  character   │        FEET
        LEGS      └──────────────┘

            RING1 RING2 TRINKET1..4
```

Fifteen, not fourteen: Head, Shoulder, Chest, Back, Hands, Legs, Feet, Wrist, Necklace, two
Rings and four Trinkets is a list fifteen entries long.

An item carries an `ItemFamily`, not a slot. A ring is a ring, and `ItemFamily::accepts` says
which of the two fingers will take it. **That one comparison is the whole equipment
restriction** — there is no second path by which a cloak reaches a ring finger, including
through a hand-edited save.

## Items

```text
ItemDef  id · name · key · family · quality · item level · icon · StatBlock · max_stack
```

`ITEMS` is a static table; a slot holds an `ItemStack` — an id and a quantity. So a saved
character is a list of names, moving an item copies four bytes, and the interface never names
a row of the table.

Every character starts wearing a white, zero-stat piece in **Chest, Legs, Wrist and Feet**,
and carrying the white pieces for Head, Shoulder, Back, Ring and Trinket so there is something
to drag the first time the panel is opened. All of them grant nothing, and the tooltip says
`GRANTS NO COMBAT STATS`.

One item in the table grants anything at all: `REINFORCED IRON BAND`, uncommon, +10 Stamina,
which no character starts with. It exists so that "equipping ten Stamina raises maximum health
by a hundred" is something the test suite asserts against the live simulation rather than a
property of arithmetic nothing has ever run.

Item art goes through the existing icon system: eleven new SVGs under
`engine-renderer/assets/icons`, rasterized into the UI atlas at build time like every ability
icon. `engine-game` names `ItemIcon::Helm` and never learns there is a texture.

## The one drag

Equipping, unequipping, swapping two rings and rearranging the bag are the **same operation**:

```text
CharacterSheet::move_item(from: ItemSlot, to: ItemSlot)
```

where `ItemSlot` is either a worn slot or a bag cell. Both ends must accept what they would
receive — a ring dropped on a worn cloak would send the cloak to a finger, so the whole
exchange is refused rather than half-applied. A refused move changes **nothing**, which is
what lets the panel drag an item onto a slot, be told no, and put the icon back without having
recorded anything.

Because a move exchanges what is already there, an item is never duplicated and never lost —
a property of one function rather than a rule four of them have to keep. A test drives every
worn slot against every bag cell in both directions and counts the items after each.

The panel emits a request; the simulation decides:

```text
drag ──► CharacterMenuOutcome::Move ──► PlayerCommand::MoveItem ──► Simulation
                                                                        │
   next frame's UiSnapshot ◄────────────────────────────────────────────┘
```

So the same drag is reproducible in a replay, and the panel is always drawn from the sheet as
it actually is rather than from what the pointer thinks it did. The drag itself — where the
item was picked up and whether the last drop was refused — is the only state the menu keeps,
because both are questions about the pointer, which gameplay has no opinion about.

## The panel

`engine-presentation/src/ui/character_menu.rs`, built like the talent menu: a layout module of
design pixels, a `CharacterMetrics` that converts them once, hit tests and painting reading the
*same* rectangle functions, and a `CharacterTheme` of colours. The whole panel is 1120×704
design pixels, which fits inside 1280×720 at an interface scale of one — a sheet taller than
the screen is a sheet whose bag cannot be reached.

Left column: the main stat, printed large, then Stamina, then all four secondary ratings with
the percentage each is worth. Right column: health, spell power, crit chance, haste and the
current global cooldown — every one of them a number some system actually reads. Middle: the
character, with their gear around them and the rings and trinkets underneath. Below: the bag.

The character in the middle is a silhouette built from rectangles, tinted by class, at the
proportions of the shared humanoid rig. It is a placeholder for the model and deliberately a
readable one: rendering the skinned model into a texture is a renderer feature, and a panel
with a hole where the character should be reads as broken.

## Persistence

`character.cfg`, JSON, beside the executable — a third file for the reason `hud.cfg` is a
second one: it is rewritten every time an item is dragged, and a bag corrupted by a bad hand
edit should not cost the player their camera sensitivity.

```text
character.cfg  <->  CharacterConfig  <->  Roster
                    (names)              (items)
```

`engine-settings` links no engine crate, so it stores *names*: `"warrior"`, `"ring1"`,
`"simple_cloth_chestpiece"`. `engine-presentation/src/character_save.rs` is the only place that
knows both vocabularies. Bag entries carry their own index, so a gap left in the third row is
in the third row afterwards.

Loading never fails. A missing file means a fresh roster in its starting gear; an unparseable
one means the same; an item this build does not have resolves to an empty slot; an item saved
into a slot it does not fit is dropped. The **roster** is the authority and the file is a copy
of it — nothing reads the panel to decide what was saved, which is what keeps a half-finished
drag out of the save.
