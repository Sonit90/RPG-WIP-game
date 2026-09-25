//! Floating combat text: damage, healing, avoidance and aggro, floating up from whoever it
//! happened to.
//!
//! ```text
//! CombatEvent stream --> FloatingNumbers::observe_combat --> entry: kind, amount, lane, age
//!                                                               |
//!                          anchor followed each frame (track) and projected to the screen
//!                                                               v
//!                         GameUi::push_damage_numbers: rise, pop, fade, in screen pixels
//! ```
//!
//! Driven entirely by *observing* the combat event stream, exactly as the damage meter is.
//! Nothing in `engine-game` knows a floating number exists, and no gameplay code is called to
//! produce one — the same arrangement that lets the meter, a future combat log and a network
//! client all read one stream. The authoritative facts — how much, whether it was a critical,
//! whether it was a tick of damage over time — are the event's own; nothing here re-derives
//! them from an amount.
//!
//! ## Screen space, anchored to the world
//!
//! An entry is anchored to a point on its target — chest height — which is followed while the
//! target exists and frozen where it was last seen once it does not. Everything else is in
//! **logical pixels on the screen**: how far a number rises, how far apart two lanes are, how
//! big the text is. A number is therefore the same size and moves the same distance whether
//! the enemy is two metres away or forty, which is what makes it readable at a glance and
//! what a number drawn in world units cannot be.
//!
//! ## Lanes, so a burst does not pile up
//!
//! Every target keeps a small counter, and each new number takes the next lane in a fixed
//! sequence — right, left, further right, further left — so hits arriving together fan out
//! beside one another instead of printing on top of each other. Numbers that arrive in a
//! burst — several hits inside a fifth of a second — also start a step lower each, so the
//! burst reads as a staircase rather than a row of overlapping digits. A critical strike takes
//! the centre, where it is read first; ticks of damage over time take narrower lanes closer
//! in, so a burning pack stays compact. The sequence is deterministic: the same fight lays its
//! numbers out the same way every time, and nothing jitters from frame to frame.
//!
//! Two more rules keep an area fight readable. Ordinary hits from the same source with the
//! same ability on the same target within a few hundredths of a second **merge** into one
//! number — a multi-hit becomes one readable total — while criticals never merge, so each one
//! is still seen. And a target never carries more than [`PER_TARGET_LIMIT`] numbers: past it
//! the oldest ordinary number on that target is hurried to its end.
//!
//! ## Allocation
//!
//! A fixed-capacity list and a fixed-size table of lane counters. A burst of a thousand
//! simultaneous hits costs no allocation and drops the oldest entries rather than growing.
//! The text is formatted into a reusable `String` at draw time, not stored per entry.

use engine_core::math::Vec3;
use engine_game::ability::AbilityId;
use engine_game::character::defense::DefenseOutcome;
use engine_game::entity::EntityId;
use engine_game::event::{AggroChange, CombatEvent};

/// Entries alive at once.
///
/// Far more than a player can read; the cap exists so a thousand-target area hit degrades
/// into "the newest hundred" instead of into an allocation storm.
pub const MAX_FLOATING_NUMBERS: usize = 128;

/// Numbers one target carries before its oldest ordinary one is hurried away.
///
/// Six is two full fans of lanes: enough that a fast rotation reads as a stream, few enough
/// that an enemy under three players' area damage is still an enemy with numbers over it
/// rather than a wall of digits.
pub const PER_TARGET_LIMIT: usize = 6;

/// How long an ordinary hit is visible, in seconds.
pub const NUMBER_LIFETIME: f32 = 1.15;

/// Seconds within which ordinary hits from one source, with one ability, on one target merge.
///
/// About two simulation ticks: a multi-hit or a cleave that reports its pieces separately is
/// one blow to the player, and should read as one number.
pub const MERGE_WINDOW: f32 = 0.08;

/// Seconds over which a number fades in rather than appearing at full strength.
///
/// Long enough that it is not a pop, short enough that it is not a delay.
const FADE_IN: f32 = 0.05;

/// How long an entry hurried off a crowded target takes to fade, in seconds.
pub const HURRY_SECONDS: f32 = 0.18;

/// How long the emphasis — the brief swell as a number appears — lasts, in seconds.
const POP_SECONDS: f32 = 0.16;

/// What an attack was answered with, when it is drawn as a word instead of a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avoidance {
    /// The attack missed entirely.
    Dodge,
    /// The attack was turned aside.
    Parry,
    /// The attack was blocked: it still lands, for less.
    Block,
    /// A shield took some or all of the damage.
    Absorb,
    /// The spell went back to whoever cast it.
    Reflect,
}

impl Avoidance {
    /// The word drawn.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dodge => "Dodge",
            Self::Parry => "Parry",
            Self::Block => "Block",
            Self::Absorb => "Absorb",
            Self::Reflect => "Reflect",
        }
    }
}

/// What a floating entry reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CombatTextKind {
    /// Damage the local player dealt.
    Damage,
    /// One tick of damage over time the local player's effect dealt.
    Periodic,
    /// Damage the local player took.
    Incoming,
    /// Healing the local player did or received.
    Heal,
    /// An attack answered with a word rather than a number.
    Avoided(Avoidance),
    /// The local player gained or lost an enemy's attention, or is about to.
    Aggro(AggroChange),
}

impl CombatTextKind {
    /// Whether this entry prints an amount.
    #[must_use]
    pub fn is_amount(self) -> bool {
        matches!(
            self,
            Self::Damage | Self::Periodic | Self::Incoming | Self::Heal
        )
    }
}

/// Which categories of combat text are drawn: the player's settings, as the observer reads
/// them.
///
/// A plain struct here rather than `engine_settings::CombatTextSettings` itself so the model
/// can be driven by a test without a settings file, and so this module names only what it
/// filters on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CombatTextFilter {
    /// Anything at all.
    pub enabled: bool,
    /// Damage the local player deals.
    pub outgoing: bool,
    /// Damage the local player takes, and attacks they avoid.
    pub incoming: bool,
    /// Healing done and received.
    pub healing: bool,
}

impl Default for CombatTextFilter {
    fn default() -> Self {
        Self {
            enabled: true,
            outgoing: true,
            incoming: true,
            healing: true,
        }
    }
}

impl From<engine_settings::CombatTextSettings> for CombatTextFilter {
    fn from(settings: engine_settings::CombatTextSettings) -> Self {
        Self {
            enabled: settings.enabled,
            outgoing: settings.outgoing_damage,
            incoming: settings.incoming_damage,
            healing: settings.healing,
        }
    }
}

/// Where an entry is in its animation, in screen terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextFrame {
    /// Offset from the projected anchor, in **logical** pixels: right and down are positive.
    pub offset: (f32, f32),
    /// What the entry's design size is multiplied by right now: above one during the swell.
    pub scale: f32,
    /// Opacity, in `[0, 1]`.
    pub alpha: f32,
}

/// One entry in flight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatingNumber {
    /// Who it happened to, so the entry can follow them while they move.
    pub target: EntityId,
    /// Who caused it, for merging.
    pub source: EntityId,
    /// Which ability is credited, for merging.
    pub ability: AbilityId,
    /// The point on the target it floats up from, in world space.
    ///
    /// Followed each frame while the target exists (see [`FloatingNumbers::track`]) and left
    /// where it was once the target is gone, so an entry outlives the entity it describes
    /// without a dangling lookup.
    pub origin: Vec3,
    /// How much it reports. Zero for a word.
    pub amount: f32,
    /// What it reports.
    pub kind: CombatTextKind,
    /// Whether it was a killing blow.
    pub killed: bool,
    /// Whether it critically struck.
    ///
    /// Read from the event rather than guessed from the amount: the same number can be a
    /// critical Firebolt or an ordinary Pyroblast, and a player who cannot tell the two apart
    /// cannot tell whether the build is working.
    pub critical: bool,
    /// Time it was created, in simulated seconds.
    pub born: f32,
    /// How long it lives, in seconds.
    pub lifetime: f32,
    /// Which lane it floats in: zero is the centre, negative is left.
    pub lane: i8,
    /// How many steps below the anchor it starts, for the numbers of a burst.
    pub stagger: u8,
    /// When it last swelled: its birth, or the moment another hit merged into it.
    pub emphasized: f32,
    /// How many events it stands for. One unless hits merged into it.
    pub merged: u16,
    /// When it was hurried off to make room on a crowded target, or infinity if it was not.
    ///
    /// A hurried entry keeps rising where it was and fades out over [`HURRY_SECONDS`], so
    /// making room never makes a number jump or vanish.
    pub hurried: f32,
}

/// The lane sequence ordinary hits take, one step per hit on a target.
const LANES: [i8; 4] = [1, -1, 2, -2];

/// Logical pixels between two lanes for a hit.
const LANE_SPACING: f32 = 40.0;

/// Seconds within which numbers on one target count as one burst.
const BURST_WINDOW: f32 = 0.2;

/// Logical pixels each later number of a burst starts below the one before.
const BURST_STEP: f32 = 22.0;

/// Steps a burst descends before it starts again from the top.
///
/// Three against four lanes, deliberately: the two cycles share no factor, so the lane and
/// the step only repeat together every twelfth number — past the per-target limit — and a
/// burst never prints two numbers in exactly the same place.
const BURST_STEPS: u8 = 3;

/// Logical pixels between two lanes for a tick of damage over time: closer in, so a pack under
/// a burn stays compact.
const PERIODIC_LANE_SPACING: f32 = 22.0;

impl FloatingNumber {
    /// How far through its life the entry is, in `[0, 1]`.
    #[must_use]
    pub fn age_fraction(&self, now: f32) -> f32 {
        if self.lifetime <= 0.0 {
            return 1.0;
        }
        ((now - self.born) / self.lifetime).clamp(0.0, 1.0)
    }

    /// Whether it has finished.
    #[must_use]
    pub fn is_finished(&self, now: f32) -> bool {
        now - self.born >= self.lifetime
    }

    /// Opacity at this moment, in `[0, 1]`.
    ///
    /// A quick fade in, a long hold, then an eased fade over the last part of the life: a
    /// number that started fading immediately would be hardest to read at exactly the moment
    /// it appeared, and one that vanished at full strength would flicker.
    #[must_use]
    pub fn alpha(&self, now: f32) -> f32 {
        let age = (now - self.born).max(0.0);
        let fade_in = (age / FADE_IN).clamp(0.0, 1.0);
        let t = self.age_fraction(now);
        let hold = 0.55;
        let fade_out = if t <= hold {
            1.0
        } else {
            let x = ((t - hold) / (1.0 - hold)).clamp(0.0, 1.0);
            // Ease-in: the fade starts gently and finishes quickly.
            1.0 - x * x
        };
        let hurry = if self.hurried.is_finite() {
            1.0 - ((now - self.hurried) / HURRY_SECONDS).clamp(0.0, 1.0)
        } else {
            1.0
        };
        (fade_in * fade_out * hurry).clamp(0.0, 1.0)
    }

    /// Whether it is still in full view: not yet fading out, and not hurried off.
    #[must_use]
    pub fn is_readable(&self, now: f32) -> bool {
        !self.hurried.is_finite() && self.age_fraction(now) < 0.55
    }

    /// How far it rises over its life, in logical pixels.
    fn rise(&self) -> f32 {
        match self.kind {
            CombatTextKind::Periodic => 46.0,
            CombatTextKind::Aggro(_) => 40.0,
            CombatTextKind::Avoided(_) => 56.0,
            _ if self.critical => 58.0,
            _ => 72.0,
        }
    }

    /// Where it starts relative to its anchor, in logical pixels: ticks a little lower so
    /// they read as belonging under the hits, words a little higher.
    fn start_height(&self) -> f32 {
        match self.kind {
            CombatTextKind::Periodic => 14.0,
            CombatTextKind::Avoided(_) | CombatTextKind::Aggro(_) => -18.0,
            _ => 0.0,
        }
    }

    /// The swell as it appears, in `[1, …]`: how much bigger than its design size it is.
    fn swell(&self, now: f32) -> f32 {
        let peak = match self.kind {
            CombatTextKind::Periodic => 1.08,
            CombatTextKind::Aggro(_) => 1.10,
            _ if self.critical => 1.55,
            _ => 1.22,
        };
        let age = (now - self.emphasized).max(0.0);
        if age >= POP_SECONDS {
            return 1.0;
        }
        let t = age / POP_SECONDS;
        // Ease-out from the peak: fast at first, settling onto the design size.
        let settle = 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t);
        peak + (1.0 - peak) * settle
    }

    /// Where it is and how it looks at `now`, in screen terms relative to its anchor.
    ///
    /// Pure: a function of the entry and the time, never of the previous frame. That is what
    /// makes the motion identical at any frame rate and every rule about it a unit test.
    #[must_use]
    pub fn frame(&self, now: f32, emphasis: bool) -> TextFrame {
        let t = self.age_fraction(now);
        // Ease-out cubic: the number leaves the target quickly and slows as it rises, so it
        // is readable for most of its life rather than streaking past.
        let eased = 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t);
        let spacing = if self.kind == CombatTextKind::Periodic {
            PERIODIC_LANE_SPACING
        } else {
            LANE_SPACING
        };
        // The fan opens as the number rises: it starts close to the target, so it reads as
        // that target's, and spreads out to its lane.
        let x = f32::from(self.lane) * spacing * (0.55 + 0.45 * eased);
        let y = self.start_height() + f32::from(self.stagger) * BURST_STEP - self.rise() * eased;
        TextFrame {
            offset: (x, y),
            scale: if emphasis { self.swell(now) } else { 1.0 },
            alpha: self.alpha(now),
        }
    }

    /// Its size before any swell, in logical pixels per em.
    ///
    /// The typography hierarchy in one table: a critical is unmistakably bigger than a hit, a
    /// tick is smaller but still a number that reads at a glance, and a word sits between.
    /// Without emphasis a critical keeps its colour and a smaller step up in size.
    #[must_use]
    pub fn design_size(&self, emphasis: bool) -> f32 {
        let critical = self.critical;
        match self.kind {
            CombatTextKind::Damage if critical && emphasis => 38.0,
            CombatTextKind::Damage if critical || self.killed => 30.0,
            CombatTextKind::Damage => 27.0,
            CombatTextKind::Periodic => 21.0,
            CombatTextKind::Incoming if critical && emphasis => 32.0,
            CombatTextKind::Incoming => 25.0,
            CombatTextKind::Heal if critical && emphasis => 34.0,
            CombatTextKind::Heal => 25.0,
            CombatTextKind::Avoided(_) => 23.0,
            CombatTextKind::Aggro(_) => 20.0,
        }
    }

    /// Writes what it says into `out`, without allocating.
    ///
    /// Damage prints bare, as the genre does; incoming damage carries a minus and healing a
    /// plus, so the three read apart even without their colours. An absorb prints what the
    /// shield took.
    pub fn write_text(&self, out: &mut String) {
        use std::fmt::Write as _;
        out.clear();
        let amount = self.amount.round().max(0.0) as i64;
        match self.kind {
            CombatTextKind::Damage | CombatTextKind::Periodic => {
                let _ = write!(out, "{amount}");
            }
            CombatTextKind::Incoming => {
                let _ = write!(out, "-{amount}");
            }
            CombatTextKind::Heal => {
                let _ = write!(out, "+{amount}");
            }
            CombatTextKind::Avoided(Avoidance::Absorb) if amount > 0 => {
                let _ = write!(out, "Absorb {amount}");
            }
            CombatTextKind::Avoided(avoided) => out.push_str(avoided.as_str()),
            CombatTextKind::Aggro(change) => out.push_str(aggro_words(change)),
        }
    }

    /// Where it is now, in world space: its anchor. Kept for callers that projected a
    /// number themselves before the motion moved into screen space.
    #[must_use]
    pub fn position(&self, _now: f32) -> Vec3 {
        self.origin
    }
}

/// Aggro text, in the case the proportional font is drawn in.
#[must_use]
pub fn aggro_words(change: AggroChange) -> &'static str {
    match change {
        AggroChange::Gained => "Aggro Gained",
        AggroChange::Lost => "Aggro Lost",
        AggroChange::Gaining => "Gaining Aggro",
        AggroChange::Losing => "Losing Aggro",
    }
}

/// How long an entry of `kind` lives, in seconds.
fn lifetime_of(kind: CombatTextKind, critical: bool) -> f32 {
    match kind {
        CombatTextKind::Periodic => 0.95,
        CombatTextKind::Aggro(_) => 1.5,
        CombatTextKind::Avoided(_) => 1.1,
        CombatTextKind::Heal if critical => 1.35,
        CombatTextKind::Heal => 1.15,
        CombatTextKind::Incoming => 1.2,
        CombatTextKind::Damage if critical => 1.4,
        CombatTextKind::Damage => NUMBER_LIFETIME,
    }
}

/// Lane counters remembered at once, one per recently hit target.
const LANE_SLOTS: usize = 24;

/// One target's lane counter.
#[derive(Debug, Clone, Copy, PartialEq)]
struct LaneSlot {
    target: EntityId,
    next: u32,
    touched: f32,
    /// How many numbers into the current burst on this target.
    burst: u8,
}

/// The entries currently in flight.
///
/// Fixed capacity, cleared by age rather than freed.
#[derive(Debug)]
pub struct FloatingNumbers {
    numbers: Vec<FloatingNumber>,
    /// Each recently hit target's next lane. A fixed table, least recently used replaced.
    lanes: [Option<LaneSlot>; LANE_SLOTS],
    /// Entries dropped because the list was full, for diagnostics.
    dropped: usize,
}

impl Default for FloatingNumbers {
    fn default() -> Self {
        Self::new()
    }
}

impl FloatingNumbers {
    /// An empty set with room for [`MAX_FLOATING_NUMBERS`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            numbers: Vec::with_capacity(MAX_FLOATING_NUMBERS),
            lanes: [None; LANE_SLOTS],
            dropped: 0,
        }
    }

    /// The entries in flight, oldest first.
    #[must_use]
    pub fn numbers(&self) -> &[FloatingNumber] {
        &self.numbers
    }

    /// How many entries have been dropped because the set was full.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped
    }

    /// Removes every entry.
    pub fn clear(&mut self) {
        self.numbers.clear();
        self.lanes = [None; LANE_SLOTS];
    }

    /// Adds a number for one outgoing hit.
    ///
    /// `origin` is where to anchor it: the caller supplies it because presentation knows
    /// where an entity is drawn and gameplay only knows where its feet are.
    pub fn push(
        &mut self,
        target: EntityId,
        origin: Vec3,
        amount: f32,
        killed: bool,
        critical: bool,
        now: f32,
    ) {
        self.spawn(Spawn {
            target,
            source: EntityId::INVALID,
            ability: AbilityId::NONE,
            origin,
            amount,
            kind: CombatTextKind::Damage,
            killed,
            critical,
            now,
        });
    }

    /// Adds aggro text over `enemy`.
    ///
    /// Centred, in no lane: it rises straight up from above the enemy, so it reads as
    /// belonging to that enemy rather than as one more number in the spray around it.
    pub fn push_aggro(&mut self, enemy: EntityId, origin: Vec3, change: AggroChange, now: f32) {
        self.spawn(Spawn {
            target: enemy,
            source: EntityId::INVALID,
            ability: AbilityId::NONE,
            origin,
            amount: 0.0,
            kind: CombatTextKind::Aggro(change),
            killed: false,
            critical: false,
            now,
        });
    }

    /// Adds aggro text for every aggro event about `local`, the player at this screen.
    ///
    /// Only the local player's: a tank does not need to read that an enemy is about to leave
    /// the healer for the mage, it needs to read that the enemy is about to leave *it*.
    /// `position_of` maps the enemy to where its text should appear.
    pub fn observe_aggro(
        &mut self,
        events: &[CombatEvent],
        local: EntityId,
        now: f32,
        mut position_of: impl FnMut(EntityId) -> Option<Vec3>,
    ) {
        for event in events {
            let CombatEvent::Aggro {
                enemy,
                entity,
                change,
                ..
            } = *event
            else {
                continue;
            };
            if entity != local {
                continue;
            }
            let Some(origin) = position_of(enemy) else {
                continue;
            };
            self.push_aggro(enemy, origin, change, now);
        }
    }

    /// Adds a number for every outgoing damage event in `events`, whoever dealt it.
    ///
    /// The unfiltered form, kept for a caller that has no local player — a benchmark, a test.
    /// The game uses [`FloatingNumbers::observe_combat`].
    pub fn observe(
        &mut self,
        events: &[CombatEvent],
        now: f32,
        mut position_of: impl FnMut(EntityId) -> Option<Vec3>,
    ) {
        for event in events {
            let CombatEvent::Damage(damage) = event else {
                continue;
            };
            let Some(origin) = position_of(damage.target) else {
                continue;
            };
            self.spawn(Spawn {
                target: damage.target,
                source: damage.source,
                ability: damage.ability,
                origin,
                amount: damage.amount,
                kind: if damage.periodic {
                    CombatTextKind::Periodic
                } else {
                    CombatTextKind::Damage
                },
                killed: damage.killed,
                critical: damage.critical,
                now,
            });
        }
    }

    /// Adds an entry for every event in `events` the local player should see.
    ///
    /// What the player at this screen is shown, which is the genre's rule: **their** damage
    /// over whatever they hit, the damage **they** take over themselves, healing they did or
    /// received, and attacks on them that were dodged, parried, blocked, absorbed or
    /// reflected. Somebody else's damage on somebody else is not their business, and drawing
    /// it would bury their own numbers in a group fight. `filter` is the player's settings.
    ///
    /// `position_of` maps an entity to its anchor; an entity it cannot find — gone, or never
    /// drawn — produces nothing rather than a number at the origin.
    pub fn observe_combat(
        &mut self,
        events: &[CombatEvent],
        local: EntityId,
        filter: CombatTextFilter,
        now: f32,
        mut position_of: impl FnMut(EntityId) -> Option<Vec3>,
    ) {
        if !filter.enabled {
            return;
        }
        for event in events {
            let Some(spawn) = classify(event, local, filter, now) else {
                continue;
            };
            let Some(origin) = position_of(spawn.target) else {
                continue;
            };
            self.spawn(Spawn { origin, ..spawn });
        }
    }

    /// Moves every entry's anchor to where its target is now.
    ///
    /// Called once a frame, before drawing, with the same lookup the entries were created
    /// with. An entry whose target the lookup no longer finds — dead and removed, despawned,
    /// or its slot reused by something new, which an id carrying a generation cannot match —
    /// keeps the last anchor it had and finishes its life there.
    pub fn track(&mut self, mut position_of: impl FnMut(EntityId) -> Option<Vec3>) {
        for number in &mut self.numbers {
            if let Some(origin) = position_of(number.target) {
                number.origin = origin;
            }
        }
    }

    /// Drops every entry that has finished.
    ///
    /// Retains in place, so the allocation is kept.
    pub fn retire(&mut self, now: f32) {
        self.numbers.retain(|number| !number.is_finished(now));
    }

    /// Creates, merges or drops one entry.
    fn spawn(&mut self, spawn: Spawn) {
        if spawn.kind.is_amount() && (spawn.amount.is_nan() || spawn.amount <= 0.0) {
            // Zero is a legitimate outcome — a hit on a corpse, a fully absorbed blow — and
            // drawing "0" for it is noise. The absorb is reported by its own event.
            return;
        }
        let now = spawn.now;

        // Merge: an ordinary piece of a blow that has only just landed joins it.
        if matches!(
            spawn.kind,
            CombatTextKind::Damage | CombatTextKind::Periodic
        ) && !spawn.critical
            && !spawn.killed
            && let Some(existing) = self.numbers.iter_mut().rev().find(|number| {
                number.target == spawn.target
                    && number.source == spawn.source
                    && number.ability == spawn.ability
                    && number.kind == spawn.kind
                    && !number.critical
                    && !number.killed
                    && spawn.source != EntityId::INVALID
                    && now - number.born <= MERGE_WINDOW
            })
        {
            existing.amount += spawn.amount;
            existing.merged = existing.merged.saturating_add(1);
            existing.emphasized = now;
            return;
        }

        // Words and aggro text neither take a lane nor count towards a burst: they rise from
        // their own height, and a warning must not shift the fan of numbers around it.
        let (step, stagger) = if spawn.kind.is_amount() {
            self.next_lane(spawn.target, now)
        } else {
            (0, 0)
        };
        let lane = match spawn.kind {
            CombatTextKind::Aggro(_) | CombatTextKind::Avoided(_) => 0,
            // The centre belongs to criticals: the number that matters most is where the eye
            // already is.
            _ if spawn.critical => 0,
            _ => LANES[step as usize % LANES.len()],
        };

        self.make_room_on(spawn.target, now);
        if self.numbers.len() >= MAX_FLOATING_NUMBERS {
            // Drop the oldest rather than the newest: the most recent hit is the one the
            // player is looking for.
            self.numbers.remove(0);
            self.dropped += 1;
        }
        self.numbers.push(FloatingNumber {
            target: spawn.target,
            source: spawn.source,
            ability: spawn.ability,
            origin: spawn.origin,
            amount: spawn.amount,
            kind: spawn.kind,
            killed: spawn.killed,
            critical: spawn.critical,
            born: now,
            lifetime: lifetime_of(spawn.kind, spawn.critical),
            lane,
            stagger,
            emphasized: now,
            merged: 1,
            hurried: f32::INFINITY,
        });
    }

    /// Hurries the oldest ordinary entry on `target` to its end when it already carries its
    /// limit, so a pack under area damage stays legible.
    ///
    /// Shortened rather than removed: an entry that vanished outright would flicker, so it is
    /// put into its final fade instead. Criticals are the last to go.
    fn make_room_on(&mut self, target: EntityId, now: f32) {
        let live = self
            .numbers
            .iter()
            .filter(|number| number.target == target && number.is_readable(now))
            .count();
        if live < PER_TARGET_LIMIT {
            return;
        }
        let oldest = self
            .numbers
            .iter_mut()
            .filter(|number| number.target == target && number.is_readable(now))
            .min_by(|a, b| {
                a.critical
                    .cmp(&b.critical)
                    .then_with(|| a.born.total_cmp(&b.born))
            });
        if let Some(number) = oldest {
            number.hurried = now;
            number.lifetime = number.lifetime.min(now - number.born + HURRY_SECONDS);
        }
    }

    /// The next lane step for `target` and how far into a burst it is, advancing both.
    fn next_lane(&mut self, target: EntityId, now: f32) -> (u32, u8) {
        if let Some(slot) = self
            .lanes
            .iter_mut()
            .flatten()
            .find(|slot| slot.target == target)
        {
            let step = slot.next;
            slot.next = slot.next.wrapping_add(1);
            slot.burst = if now - slot.touched <= BURST_WINDOW {
                (slot.burst + 1) % BURST_STEPS
            } else {
                0
            };
            slot.touched = now;
            return (step, slot.burst);
        }
        // A free slot, or the one touched longest ago.
        let index = self
            .lanes
            .iter()
            .position(Option::is_none)
            .unwrap_or_else(|| {
                self.lanes
                    .iter()
                    .enumerate()
                    .min_by(|a, b| {
                        let touched =
                            |slot: &Option<LaneSlot>| slot.map_or(f32::MIN, |s| s.touched);
                        touched(a.1).total_cmp(&touched(b.1))
                    })
                    .map_or(0, |(index, _)| index)
            });
        self.lanes[index] = Some(LaneSlot {
            target,
            next: 1,
            touched: now,
            burst: 0,
        });
        (0, 0)
    }
}

/// One entry about to be created: the event's facts before the anchor is looked up.
#[derive(Debug, Clone, Copy)]
struct Spawn {
    target: EntityId,
    source: EntityId,
    ability: AbilityId,
    origin: Vec3,
    amount: f32,
    kind: CombatTextKind,
    killed: bool,
    critical: bool,
    now: f32,
}

/// What `event` becomes on the local player's screen, if anything.
fn classify(
    event: &CombatEvent,
    local: EntityId,
    filter: CombatTextFilter,
    now: f32,
) -> Option<Spawn> {
    let spawn = |target, source, ability, amount, kind, killed, critical| Spawn {
        target,
        source,
        ability,
        origin: Vec3::ZERO,
        amount,
        kind,
        killed,
        critical,
        now,
    };
    match *event {
        CombatEvent::Damage(damage) => {
            if damage.target == local {
                // What the player takes, including their own cost-of-casting self-damage:
                // it is health they lost, and they should see it go.
                filter.incoming.then(|| {
                    spawn(
                        local,
                        damage.source,
                        damage.ability,
                        damage.amount,
                        CombatTextKind::Incoming,
                        damage.killed,
                        damage.critical,
                    )
                })
            } else if damage.source == local {
                filter.outgoing.then(|| {
                    spawn(
                        damage.target,
                        local,
                        damage.ability,
                        damage.amount,
                        if damage.periodic {
                            CombatTextKind::Periodic
                        } else {
                            CombatTextKind::Damage
                        },
                        damage.killed,
                        damage.critical,
                    )
                })
            } else {
                None
            }
        }
        CombatEvent::Healing {
            source,
            target,
            amount,
            ability,
            ..
        } => {
            // Regeneration is credited to no ability. It is not something the player did, and
            // a "+12" every second of every walk between pulls would be noise.
            if ability == AbilityId::NONE || !filter.healing {
                return None;
            }
            (source == local || target == local).then(|| {
                spawn(
                    target,
                    source,
                    ability,
                    amount,
                    CombatTextKind::Heal,
                    false,
                    false,
                )
            })
        }
        CombatEvent::AttackDefended {
            attacker,
            defender,
            outcome,
            ability,
            ..
        } => {
            let avoided = match outcome {
                DefenseOutcome::Dodged => Avoidance::Dodge,
                DefenseOutcome::Parried => Avoidance::Parry,
                DefenseOutcome::Blocked => Avoidance::Block,
                DefenseOutcome::Hit => return None,
            };
            // Over whoever avoided it: the player's own dodge over the player, an enemy's
            // parry of the player's swing over that enemy.
            let allowed = if defender == local {
                filter.incoming
            } else if attacker == local {
                filter.outgoing
            } else {
                false
            };
            allowed.then(|| {
                spawn(
                    defender,
                    attacker,
                    ability,
                    0.0,
                    CombatTextKind::Avoided(avoided),
                    false,
                    false,
                )
            })
        }
        CombatEvent::Absorbed { target, amount, .. } => {
            (target == local && filter.incoming).then(|| {
                spawn(
                    local,
                    EntityId::INVALID,
                    AbilityId::NONE,
                    amount,
                    CombatTextKind::Avoided(Avoidance::Absorb),
                    false,
                    false,
                )
            })
        }
        CombatEvent::SpellReflected {
            source, ability, ..
        } => (source == local && filter.incoming).then(|| {
            spawn(
                local,
                source,
                ability,
                0.0,
                CombatTextKind::Avoided(Avoidance::Reflect),
                false,
                false,
            )
        }),
        _ => None,
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_game::arena::{self, ArenaConfig};
    use engine_game::combat::DamageType;
    use engine_game::command::PlayerCommand;
    use engine_game::event::DamageEvent;
    use engine_game::sim::Simulation;

    fn entity() -> EntityId {
        EntityId::new(3, 1)
    }

    fn local() -> EntityId {
        EntityId::new(1, 1)
    }

    fn sim() -> Simulation {
        let mut simulation = arena::training_arena(&ArenaConfig::default());
        simulation.step_once();
        simulation
    }

    fn damage(source: EntityId, target: EntityId, amount: f32) -> DamageEvent {
        DamageEvent {
            source,
            target,
            amount,
            overkill: 0.0,
            ability: AbilityId::from_slot(0).expect("slot 0 exists"),
            damage_type: DamageType::Fire,
            killed: false,
            critical: false,
            periodic: false,
            position: Vec3::ZERO,
            tick: 1,
        }
    }

    fn observe(numbers: &mut FloatingNumbers, events: &[CombatEvent], now: f32) {
        numbers.observe_combat(events, local(), CombatTextFilter::default(), now, |_| {
            Some(Vec3::ZERO)
        });
    }

    #[test]
    fn a_damage_event_becomes_a_number() {
        let mut numbers = FloatingNumbers::new();
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(0));
        // Firebolt is a one-second cast; the damage arrives when it lands.
        simulation.step_many(70);

        let time = simulation.time() as f32;
        let local = simulation.player().entity;
        numbers.observe_combat(
            simulation.events(),
            local,
            CombatTextFilter::default(),
            time,
            |_| Some(Vec3::ZERO),
        );
        assert!(!numbers.numbers().is_empty(), "no number for the hit");
        assert!(numbers.numbers()[0].amount > 0.0);
        assert_eq!(numbers.numbers()[0].kind, CombatTextKind::Damage);
    }

    #[test]
    fn an_area_hit_produces_one_number_per_target() {
        let mut numbers = FloatingNumbers::new();
        let mut simulation = arena::training_arena(&ArenaConfig::with_group(4));
        simulation.step_once();
        simulation.select_nearest_target();

        let target = simulation.player().target.expect("a dummy is selected");
        let position = simulation
            .world()
            .position(target)
            .expect("the dummy has a position");
        simulation
            .cast_at(engine_game::firemage::FireAbility::Meteor, position)
            .expect("in range");
        let local = simulation.player().entity;
        let mut targets = Vec::new();
        for _ in 0..200 {
            simulation.step_once();
            let time = simulation.time() as f32;
            numbers.observe_combat(
                simulation.events(),
                local,
                CombatTextFilter::default(),
                time,
                |_| Some(Vec3::ZERO),
            );
            for number in numbers.numbers() {
                if !targets.contains(&number.target) {
                    targets.push(number.target);
                }
            }
        }
        assert!(!targets.is_empty(), "an area hit should produce numbers");
    }

    #[test]
    fn only_what_concerns_the_local_player_is_shown() {
        let mut numbers = FloatingNumbers::new();
        let ally = EntityId::new(2, 1);
        let enemy = entity();
        observe(
            &mut numbers,
            &[
                CombatEvent::Damage(damage(local(), enemy, 40.0)),
                CombatEvent::Damage(damage(enemy, local(), 25.0)),
                // Somebody else's fight is not the local player's to read.
                CombatEvent::Damage(damage(ally, enemy, 90.0)),
                CombatEvent::Damage(damage(enemy, ally, 30.0)),
            ],
            0.0,
        );
        let kinds: Vec<_> = numbers
            .numbers()
            .iter()
            .map(|number| (number.target, number.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                (enemy, CombatTextKind::Damage),
                (local(), CombatTextKind::Incoming)
            ]
        );
    }

    #[test]
    fn a_tick_of_damage_over_time_is_drawn_as_a_tick() {
        let mut numbers = FloatingNumbers::new();
        let mut tick = damage(local(), entity(), 12.0);
        tick.periodic = true;
        observe(&mut numbers, &[CombatEvent::Damage(tick)], 0.0);
        let number = numbers.numbers()[0];
        assert_eq!(number.kind, CombatTextKind::Periodic);
        let hit = FloatingNumber {
            kind: CombatTextKind::Damage,
            ..number
        };
        assert!(
            number.design_size(true) < hit.design_size(true),
            "a tick is smaller"
        );
        assert!(number.lifetime < hit.lifetime.max(NUMBER_LIFETIME));
    }

    #[test]
    fn healing_is_shown_but_regeneration_is_not() {
        let mut numbers = FloatingNumbers::new();
        let ability = AbilityId::from_slot(1).expect("slot 1 exists");
        observe(
            &mut numbers,
            &[
                CombatEvent::Healing {
                    source: local(),
                    target: entity(),
                    amount: 55.0,
                    ability,
                    tick: 1,
                },
                CombatEvent::Healing {
                    source: local(),
                    target: local(),
                    amount: 8.0,
                    ability: AbilityId::NONE,
                    tick: 1,
                },
            ],
            0.0,
        );
        assert_eq!(numbers.numbers().len(), 1);
        let heal = numbers.numbers()[0];
        assert_eq!(heal.kind, CombatTextKind::Heal);
        let mut text = String::new();
        heal.write_text(&mut text);
        assert_eq!(text, "+55");
    }

    #[test]
    fn an_avoided_attack_is_a_word_over_whoever_avoided_it() {
        let mut numbers = FloatingNumbers::new();
        let enemy = entity();
        let ability = AbilityId::from_slot(0).expect("slot 0 exists");
        observe(
            &mut numbers,
            &[
                CombatEvent::AttackDefended {
                    attacker: enemy,
                    defender: local(),
                    outcome: DefenseOutcome::Parried,
                    ability,
                    position: Vec3::ZERO,
                    tick: 1,
                },
                CombatEvent::AttackDefended {
                    attacker: enemy,
                    defender: local(),
                    outcome: DefenseOutcome::Hit,
                    ability,
                    position: Vec3::ZERO,
                    tick: 1,
                },
            ],
            0.0,
        );
        assert_eq!(numbers.numbers().len(), 1, "a plain hit is not a word");
        let parry = numbers.numbers()[0];
        assert_eq!(parry.target, local());
        let mut text = String::new();
        parry.write_text(&mut text);
        assert_eq!(text, "Parry");
    }

    #[test]
    fn every_category_obeys_its_setting() {
        let events = [
            CombatEvent::Damage(damage(local(), entity(), 40.0)),
            CombatEvent::Damage(damage(entity(), local(), 25.0)),
            CombatEvent::Healing {
                source: local(),
                target: local(),
                amount: 30.0,
                ability: AbilityId::from_slot(2).expect("slot 2 exists"),
                tick: 1,
            },
        ];
        let count = |filter: CombatTextFilter| {
            let mut numbers = FloatingNumbers::new();
            numbers.observe_combat(&events, local(), filter, 0.0, |_| Some(Vec3::ZERO));
            numbers.numbers().len()
        };
        let all = CombatTextFilter::default();
        assert_eq!(count(all), 3);
        assert_eq!(
            count(CombatTextFilter {
                enabled: false,
                ..all
            }),
            0
        );
        assert_eq!(
            count(CombatTextFilter {
                outgoing: false,
                ..all
            }),
            2
        );
        assert_eq!(
            count(CombatTextFilter {
                incoming: false,
                ..all
            }),
            2
        );
        assert_eq!(
            count(CombatTextFilter {
                healing: false,
                ..all
            }),
            2
        );
    }

    #[test]
    fn a_number_rises_eases_and_fades_over_its_lifetime() {
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 45.0, false, false, 0.0);
        let number = numbers.numbers()[0];
        let at = |t: f32| number.frame(number.lifetime * t, true);

        assert!(at(0.0).alpha < 0.1, "it fades in rather than popping");
        assert_eq!(at(0.2).alpha, 1.0, "fully opaque once in");
        assert!(at(0.9).alpha < 1.0, "it fades before it ends");
        assert_eq!(at(1.0).alpha, 0.0, "it ends invisible");
        // Up is negative on the screen, and the rise slows down: eased, not linear.
        let (early, middle, late) = (at(0.25).offset.1, at(0.5).offset.1, at(0.75).offset.1);
        assert!(early < 0.0 && middle < early && late < middle, "it rises");
        assert!(
            (early - 0.0).abs() > (late - middle).abs(),
            "the first quarter covers more ground than the third: {early} {middle} {late}"
        );
    }

    #[test]
    fn a_critical_swells_then_settles_and_takes_the_centre() {
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 45.0, false, false, 0.0);
        numbers.push(entity(), Vec3::ZERO, 90.0, false, true, 0.0);
        let (hit, crit) = (numbers.numbers()[0], numbers.numbers()[1]);
        assert_eq!(crit.lane, 0, "the centre is the critical's");
        assert_ne!(hit.lane, 0);
        assert!(crit.design_size(true) >= hit.design_size(true) * 1.3);
        assert!(crit.lifetime > hit.lifetime, "a critical lingers");
        let swell = crit.frame(0.0, true).scale;
        assert!(swell > 1.4, "it appears swollen: {swell}");
        assert_eq!(crit.frame(0.5, true).scale, 1.0, "and settles");
        assert_eq!(
            crit.frame(0.0, false).scale,
            1.0,
            "no swell without emphasis"
        );
        assert!(crit.design_size(false) < crit.design_size(true));
    }

    #[test]
    fn a_burst_on_one_target_fans_out_across_lanes() {
        // Numbers must not occupy exactly the same screen position.
        let mut numbers = FloatingNumbers::new();
        for index in 0..4 {
            numbers.push(entity(), Vec3::ZERO, 10.0 + index as f32, false, false, 0.0);
        }
        let mut lanes: Vec<i8> = numbers.numbers().iter().map(|number| number.lane).collect();
        lanes.sort_unstable();
        lanes.dedup();
        assert_eq!(lanes.len(), 4, "four hits, four lanes: {lanes:?}");
        let offsets: Vec<f32> = numbers
            .numbers()
            .iter()
            .map(|number| number.frame(0.5, true).offset.0)
            .collect();
        for (index, a) in offsets.iter().enumerate() {
            for b in &offsets[index + 1..] {
                assert!((a - b).abs() > 20.0, "two lanes overlap: {offsets:?}");
            }
        }
    }

    #[test]
    fn each_target_has_its_own_lanes_and_the_layout_is_deterministic() {
        let run = || {
            let mut numbers = FloatingNumbers::new();
            for index in 0..6 {
                let target = EntityId::new(10 + index % 2, 1);
                numbers.push(target, Vec3::ZERO, 5.0, false, false, index as f32 * 0.01);
            }
            numbers
                .numbers()
                .iter()
                .map(|number| (number.target, number.lane))
                .collect::<Vec<_>>()
        };
        let first = run();
        assert_eq!(first, run(), "the same fight lays out the same way");
        // The two targets start their sequences independently.
        assert_eq!(first[0].1, first[1].1);
    }

    #[test]
    fn pieces_of_one_blow_merge_but_criticals_never_do() {
        let mut numbers = FloatingNumbers::new();
        let piece = damage(local(), entity(), 10.0);
        observe(
            &mut numbers,
            &[CombatEvent::Damage(piece), CombatEvent::Damage(piece)],
            0.0,
        );
        assert_eq!(numbers.numbers().len(), 1, "two pieces, one number");
        assert_eq!(numbers.numbers()[0].amount, 20.0);
        assert_eq!(numbers.numbers()[0].merged, 2);

        let crit = DamageEvent {
            critical: true,
            ..piece
        };
        observe(
            &mut numbers,
            &[CombatEvent::Damage(crit), CombatEvent::Damage(crit)],
            0.01,
        );
        assert_eq!(numbers.numbers().len(), 3, "every critical is seen");

        // Outside the window, a new number.
        observe(&mut numbers, &[CombatEvent::Damage(piece)], 0.5);
        assert_eq!(numbers.numbers().len(), 4);
    }

    #[test]
    fn a_target_under_area_damage_carries_a_bounded_number_of_numbers() {
        // Hurried, not removed: the entries past the limit fade rather than blink out.
        let mut numbers = FloatingNumbers::new();
        for index in 0..20 {
            numbers.push(
                entity(),
                Vec3::ZERO,
                5.0 + index as f32,
                false,
                false,
                index as f32 * 0.02,
            );
        }
        let now = 19.0 * 0.02;
        let readable = numbers
            .numbers()
            .iter()
            .filter(|number| number.is_readable(now))
            .count();
        assert!(
            readable <= PER_TARGET_LIMIT,
            "{readable} numbers still in full view"
        );
    }

    #[test]
    fn an_entry_follows_its_target_and_stays_where_it_was_once_the_target_is_gone() {
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 45.0, false, false, 0.0);
        numbers.track(|_| Some(Vec3::new(4.0, 0.0, 0.0)));
        assert_eq!(numbers.numbers()[0].origin.x, 4.0, "it follows");
        numbers.track(|_| None);
        assert_eq!(numbers.numbers()[0].origin.x, 4.0, "it stays");
    }

    #[test]
    fn finished_numbers_are_retired_and_the_allocation_is_kept() {
        let mut numbers = FloatingNumbers::new();
        for index in 0..10 {
            numbers.push(EntityId::new(index, 1), Vec3::ZERO, 5.0, false, false, 0.0);
        }
        let capacity = numbers.numbers.capacity();

        numbers.retire(NUMBER_LIFETIME * 0.5);
        assert_eq!(numbers.numbers().len(), 10, "none has expired yet");

        numbers.retire(NUMBER_LIFETIME + 0.1);
        assert!(numbers.numbers().is_empty());
        assert_eq!(numbers.numbers.capacity(), capacity);
    }

    #[test]
    fn the_set_is_bounded_and_keeps_the_newest() {
        // A thousand-target area hit must degrade, not allocate.
        let mut numbers = FloatingNumbers::new();
        for index in 0..MAX_FLOATING_NUMBERS + 50 {
            numbers.push(
                EntityId::new(index as u32, 1),
                Vec3::ZERO,
                index as f32 + 1.0,
                false,
                false,
                0.0,
            );
        }
        assert_eq!(numbers.numbers().len(), MAX_FLOATING_NUMBERS);
        assert_eq!(numbers.dropped(), 50);
        let newest = numbers.numbers().last().expect("the set is not empty");
        assert_eq!(newest.amount, (MAX_FLOATING_NUMBERS + 50) as f32);
    }

    #[test]
    fn zero_damage_draws_nothing() {
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 0.0, false, false, 0.0);
        numbers.push(entity(), Vec3::ZERO, -5.0, false, false, 0.0);
        numbers.push(entity(), Vec3::ZERO, f32::NAN, false, false, 0.0);
        assert!(numbers.numbers().is_empty());
    }

    #[test]
    fn a_killing_blow_is_marked_so_it_can_be_drawn_differently() {
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 999.0, true, false, 0.0);
        assert!(numbers.numbers()[0].killed);
    }

    fn aggro(enemy: EntityId, entity: EntityId, change: AggroChange) -> CombatEvent {
        CombatEvent::Aggro {
            enemy,
            entity,
            change,
            tick: 1,
        }
    }

    #[test]
    fn aggro_about_the_local_player_becomes_text_over_the_enemy() {
        let local = local();
        let enemy = EntityId::new(9, 1);
        let above = Vec3::new(0.0, 3.0, 0.0);
        let mut numbers = FloatingNumbers::new();
        numbers.observe_aggro(
            &[
                aggro(enemy, local, AggroChange::Gaining),
                aggro(enemy, local, AggroChange::Gained),
            ],
            local,
            0.0,
            |id| (id == enemy).then_some(above),
        );
        let drawn: Vec<_> = numbers.numbers().iter().map(|number| number.kind).collect();
        assert_eq!(
            drawn,
            [
                CombatTextKind::Aggro(AggroChange::Gaining),
                CombatTextKind::Aggro(AggroChange::Gained)
            ]
        );
        let first = numbers.numbers()[0];
        assert_eq!(first.target, enemy, "the text belongs to the enemy");
        assert_eq!(first.origin, above);
        assert_eq!(
            first.frame(1.0, true).offset.0,
            0.0,
            "aggro text rises straight up rather than drifting"
        );
    }

    #[test]
    fn aggro_about_somebody_else_is_not_drawn_for_this_player() {
        let local = local();
        let other = EntityId::new(2, 1);
        let enemy = EntityId::new(9, 1);
        let mut numbers = FloatingNumbers::new();
        numbers.observe_aggro(
            &[
                aggro(enemy, other, AggroChange::Losing),
                aggro(enemy, other, AggroChange::Lost),
            ],
            local,
            0.0,
            |_| Some(Vec3::ZERO),
        );
        assert!(numbers.numbers().is_empty());
    }

    #[test]
    fn aggro_text_does_not_disturb_the_lanes_of_damage_numbers() {
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 10.0, false, false, 0.0);
        numbers.push_aggro(entity(), Vec3::ZERO, AggroChange::Lost, 0.0);
        numbers.push(entity(), Vec3::ZERO, 10.0, false, false, 0.0);
        let first = numbers.numbers()[0].lane;
        let second = numbers.numbers()[2].lane;
        assert_eq!(first, -second, "the two numbers still take opposite sides");
    }

    #[test]
    fn a_number_for_an_entity_that_has_gone_is_skipped_rather_than_guessed() {
        let mut numbers = FloatingNumbers::new();
        let mut simulation = sim();
        simulation.push_command(PlayerCommand::UseAbility(0));
        simulation.step_many(70);
        let local = simulation.player().entity;
        numbers.observe_combat(
            simulation.events(),
            local,
            CombatTextFilter::default(),
            simulation.time() as f32,
            |_| None,
        );
        assert!(numbers.numbers().is_empty());
    }

    #[test]
    fn text_is_written_without_a_fresh_string_per_number() {
        let mut out = String::with_capacity(32);
        let capacity = out.capacity();
        let mut numbers = FloatingNumbers::new();
        numbers.push(entity(), Vec3::ZERO, 12_345.4, false, false, 0.0);
        let number = numbers.numbers()[0];
        number.write_text(&mut out);
        assert_eq!(out, "12345");
        let incoming = FloatingNumber {
            kind: CombatTextKind::Incoming,
            ..number
        };
        incoming.write_text(&mut out);
        assert_eq!(out, "-12345");
        assert_eq!(out.capacity(), capacity);
    }

    #[test]
    fn a_burst_on_one_target_descends_a_step_per_number_and_a_pause_resets_it() {
        let mut numbers = FloatingNumbers::new();
        for index in 0..3 {
            numbers.push(
                entity(),
                Vec3::ZERO,
                10.0,
                false,
                false,
                index as f32 * 0.05,
            );
        }
        let staggers: Vec<u8> = numbers
            .numbers()
            .iter()
            .map(|number| number.stagger)
            .collect();
        assert_eq!(staggers, [0, 1, 2]);
        // Lane and step never coincide within a target's limit of numbers.
        let mut burst = FloatingNumbers::new();
        for _ in 0..PER_TARGET_LIMIT {
            burst.push(entity(), Vec3::ZERO, 10.0, false, false, 0.0);
        }
        let mut places: Vec<(i8, u8)> = burst
            .numbers()
            .iter()
            .map(|number| (number.lane, number.stagger))
            .collect();
        places.sort_unstable();
        places.dedup();
        assert_eq!(
            places.len(),
            PER_TARGET_LIMIT,
            "two numbers share a place: {places:?}"
        );
        // At the same moment the later ones sit lower on the screen: a staircase, not a row.
        let now = 0.2;
        let heights: Vec<f32> = numbers
            .numbers()
            .iter()
            .map(|number| number.frame(now, true).offset.1)
            .collect();
        assert!(
            heights[0] < heights[1] && heights[1] < heights[2],
            "{heights:?}"
        );

        numbers.push(entity(), Vec3::ZERO, 10.0, false, false, 2.0);
        assert_eq!(
            numbers.numbers().last().map(|number| number.stagger),
            Some(0)
        );
    }
}
