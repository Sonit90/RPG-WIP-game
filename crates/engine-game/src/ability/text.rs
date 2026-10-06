//! What an ability says about itself: its description, its empowered effect and what it does
//! with its class's resource — generated from the table, never written twice.
//!
//! ```text
//! AbilityDef.description  "Stacks up to {max_stacks} times."      template, in the row
//!        │                                                        │
//!        └── write_template ── {max_stacks} ── row column?  ──────┤
//!                                          └─ class tuning? ──────┘ "Stacks up to 5 times."
//!
//! AbilityDef ──► resource_uses()  Cost · Empower · Generate      derived from the columns
//! ```
//!
//! ## Why templates
//!
//! A sentence that states a number is a second copy of that number. The rows hold prose with
//! named tokens instead, and the tokens are filled at the moment a tooltip is built: from the
//! row itself (`{cooldown}`, `{duration}`, `{range}`, …) or from the class's tuning constants
//! (`{heal_per_stack}` is `RECUPERATE_HEAL_PER_STACK`). A balance pass moves the sentence with
//! the number, and `every_template_resolves` fails the build of the tests if a row names a
//! token nobody answers.
//!
//! ## House style
//!
//! Second person, present tense, sentence case, a full stop at the end. Durations are `sec`,
//! distances `m`, shares `%`. Resource names and stat names are capitalised (`Deflection`,
//! `Health`, `Block`). Nothing here is upper case for the old bitmap font: every tooltip is set
//! in the proportional face.

use std::fmt::Write as _;

use super::table::{AbilityDef, FireAbility};
use crate::hero::Hero;

/// A number a template token stands for, with its unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TextValue {
    /// Seconds: `8 sec`, `0.5 sec`.
    Seconds(f32),
    /// Metres: `30 m`.
    Metres(f32),
    /// A fraction shown as a percentage: `0.002` is `0.2%`.
    Percent(f32),
    /// A plain count: `5`.
    Count(f32),
    /// Degrees of arc: `120°`.
    Degrees(f32),
}

impl TextValue {
    /// Writes the value with its unit.
    pub fn write(self, out: &mut String) {
        match self {
            Self::Seconds(value) => {
                write_number(out, value);
                out.push_str(" sec");
            }
            Self::Metres(value) => {
                write_number(out, value);
                out.push_str(" m");
            }
            Self::Percent(value) => {
                write_number(out, value * 100.0);
                out.push('%');
            }
            Self::Count(value) => write_number(out, value),
            Self::Degrees(value) => {
                write_number(out, value);
                out.push('°');
            }
        }
    }
}

/// Writes a number with no more decimals than it needs: `5`, `0.5`, `0.25`.
pub fn write_number(out: &mut String, value: f32) {
    let rounded = (value * 100.0).round() / 100.0;
    if (rounded - rounded.round()).abs() < 1e-4 {
        let _ = write!(out, "{}", rounded.round() as i64);
    } else if ((rounded * 10.0) - (rounded * 10.0).round()).abs() < 1e-4 {
        let _ = write!(out, "{rounded:.1}");
    } else {
        let _ = write!(out, "{rounded:.2}");
    }
}

/// A token a template named that neither the row nor the class could answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownToken(pub String);

/// The value of a token every row can answer from its own columns.
fn row_token(definition: &AbilityDef, key: &str) -> Option<TextValue> {
    Some(match key {
        "cooldown" => TextValue::Seconds(definition.cooldown),
        "duration" => TextValue::Seconds(definition.duration),
        "cast_time" => TextValue::Seconds(definition.cast_time),
        "tick_interval" => TextValue::Seconds(definition.tick_interval),
        "impact_delay" => TextValue::Seconds(definition.impact_delay),
        "range" => TextValue::Metres(definition.range),
        "radius" => TextValue::Metres(definition.radius),
        "width" => TextValue::Metres(definition.width),
        "charges" => TextValue::Count(f32::from(definition.charges)),
        "ignite_stacks" => TextValue::Count(f32::from(definition.fire().ignite_stacks)),
        "ignite_chance" => TextValue::Percent(definition.fire().ignite_chance),
        "ignite_bonus" => TextValue::Percent(definition.fire().ignite_bonus_per_stack),
        "cost" if definition.warrior().is_empowerable() => {
            TextValue::Count(definition.warrior().deflection_cost)
        }
        "threshold" if definition.warrior().is_empowerable() => {
            TextValue::Count(definition.warrior().empower_threshold())
        }
        _ => return None,
    })
}

/// The value of `key` for `ability`: its own row first, then its class's tuning.
#[must_use]
pub fn token(ability: FireAbility, key: &str) -> Option<TextValue> {
    row_token(ability.definition(), key).or_else(|| match ability.hero() {
        Hero::FireMage => crate::firemage::abilities::token(ability, key),
        Hero::Warrior => crate::warrior::abilities::token(ability, key),
        Hero::Druid => crate::druid::abilities::token(ability, key),
    })
}

/// Writes `template` for `ability` into `out`, filling every `{token}`.
///
/// Appends; the caller clears. A token nobody answers is written back as it was and reported,
/// so a tooltip with a typo still reads rather than vanishing.
///
/// # Errors
///
/// [`UnknownToken`] naming the first token that did not resolve.
pub fn write_template(
    out: &mut String,
    ability: FireAbility,
    template: &str,
) -> Result<(), UnknownToken> {
    let mut unknown = None;
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            rest = "";
            break;
        };
        let key = &after[..close];
        match token(ability, key) {
            // "every second", not "every 1 sec".
            Some(TextValue::Seconds(seconds))
                if (seconds - 1.0).abs() < 1e-6 && out.ends_with("every ") =>
            {
                out.push_str("second");
            }
            Some(value) => value.write(out),
            None => {
                out.push('{');
                out.push_str(key);
                out.push('}');
                unknown.get_or_insert_with(|| UnknownToken(key.to_owned()));
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    unknown.map_or(Ok(()), Err)
}

/// The ability's description, filled.
pub fn write_description(out: &mut String, ability: FireAbility) {
    let _ = write_template(out, ability, ability.definition().description);
}

/// What spending the class's resource adds, filled. Writes nothing for an ability nothing
/// empowers.
pub fn write_empowered(out: &mut String, ability: FireAbility) {
    let _ = write_template(out, ability, ability.definition().empowered);
}

/// A class resource, by the name every tooltip uses for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    /// The Warrior's.
    Deflection,
    /// The Druid's spending resource.
    Mana,
    /// The Druid's empowering resource.
    AstralPower,
    /// The FireMage's.
    Heat,
    /// What the Ultimate costs.
    UltimateEnergy,
}

impl Resource {
    /// The name, capitalised as a proper noun.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Deflection => "Deflection",
            Self::Mana => "Mana",
            Self::AstralPower => "Astral Power",
            Self::Heat => "Heat",
            Self::UltimateEnergy => "Ultimate Energy",
        }
    }
}

/// How an ability treats a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceMode {
    /// Required, and consumed: without it the ability is refused.
    Cost,
    /// Optional, and consumed when there is enough: the ability is cast either way, and
    /// stronger when it is paid for.
    Empower,
    /// Produced by casting it.
    Generate,
}

/// One thing an ability does with one resource.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResourceUse {
    /// Which resource.
    pub resource: Resource,
    /// What the ability does with it.
    pub mode: ResourceMode,
    /// How much. Zero for "whatever is armed" — the Druid's Astral Power.
    pub amount: f32,
    /// How much must be held before it is spent, when that is more than `amount`.
    pub threshold: f32,
}

impl ResourceUse {
    /// One line for the tooltip's resource section.
    ///
    /// ```text
    /// Costs 300 Mana
    /// Consumes 20 Deflection to empower, if available
    /// Consumes 20 Deflection to empower, at 100 Deflection
    /// Generates 4 Heat
    /// ```
    pub fn write_line(&self, out: &mut String) {
        let name = self.resource.name();
        match self.mode {
            ResourceMode::Cost => {
                out.push_str("Costs ");
                write_number(out, self.amount);
                out.push(' ');
                out.push_str(name);
            }
            ResourceMode::Empower if self.amount <= 0.0 => {
                out.push_str("Consumes armed ");
                out.push_str(name);
                out.push_str(" to empower");
            }
            ResourceMode::Empower => {
                out.push_str("Consumes ");
                write_number(out, self.amount);
                out.push(' ');
                out.push_str(name);
                if self.threshold > self.amount {
                    out.push_str(" to empower, at ");
                    write_number(out, self.threshold);
                    out.push(' ');
                    out.push_str(name);
                } else {
                    out.push_str(" to empower, if available");
                }
            }
            ResourceMode::Generate => {
                out.push_str("Generates ");
                write_number(out, self.amount);
                out.push(' ');
                out.push_str(name);
            }
        }
    }
}

impl AbilityDef {
    /// Everything this ability does with a class resource, derived from its columns.
    ///
    /// Derived rather than a second column, so the tooltip, the cast check and the spend can
    /// never disagree: a row with a Deflection cost *is* an optional empowerment, because that
    /// is what the executor does with one.
    pub fn resource_uses(&self) -> impl Iterator<Item = ResourceUse> + '_ {
        let mut uses: [Option<ResourceUse>; 4] = [None; 4];
        let mut at = 0;
        let mut push = |entry: ResourceUse| {
            if at < uses.len() {
                uses[at] = Some(entry);
                at += 1;
            }
        };
        if self.fire().energy_cost > 0.0 {
            push(ResourceUse {
                resource: Resource::UltimateEnergy,
                mode: ResourceMode::Cost,
                amount: self.fire().energy_cost,
                threshold: self.fire().energy_cost,
            });
        }
        if self.druid().mana_cost > 0.0 {
            push(ResourceUse {
                resource: Resource::Mana,
                mode: ResourceMode::Cost,
                amount: self.druid().mana_cost,
                threshold: self.druid().mana_cost,
            });
        }
        if self.warrior().is_empowerable() {
            push(ResourceUse {
                resource: Resource::Deflection,
                mode: ResourceMode::Empower,
                amount: self.warrior().deflection_cost,
                threshold: self.warrior().empower_threshold(),
            });
        }
        if self.druid().astral_empowerable {
            push(ResourceUse {
                resource: Resource::AstralPower,
                mode: ResourceMode::Empower,
                amount: 0.0,
                threshold: 0.0,
            });
        }
        if self.fire().heat > 0.0 {
            push(ResourceUse {
                resource: Resource::Heat,
                mode: ResourceMode::Generate,
                amount: self.fire().heat,
                threshold: 0.0,
            });
        }
        if self.druid().astral_generation > 0.0 {
            push(ResourceUse {
                resource: Resource::AstralPower,
                mode: ResourceMode::Generate,
                amount: self.druid().astral_generation,
                threshold: 0.0,
            });
        }
        uses.into_iter().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(ability: FireAbility, template: &str) -> String {
        let mut out = String::new();
        if let Err(UnknownToken(key)) = write_template(&mut out, ability, template) {
            panic!("{ability:?} names {{{key}}}, which nothing answers: {template:?}");
        }
        out
    }

    #[test]
    fn every_template_resolves() {
        for ability in FireAbility::ALL {
            let definition = ability.definition();
            filled(ability, definition.description);
            filled(ability, definition.empowered);
        }
    }

    #[test]
    fn every_description_reads_as_a_sentence() {
        for ability in FireAbility::ALL {
            let mut text = String::new();
            write_description(&mut text, ability);
            assert!(!text.is_empty(), "{ability:?} says nothing");
            assert!(
                text.chars().next().is_some_and(char::is_uppercase),
                "{ability:?}: {text}"
            );
            assert!(
                text.ends_with('.'),
                "{ability:?} does not end a sentence: {text}"
            );
            // Sentence case, not the bitmap font's capitals: no shouted words.
            let shouted = text
                .split(|c: char| !c.is_alphabetic())
                .any(|word| word.len() > 2 && word.chars().all(char::is_uppercase));
            assert!(!shouted, "{ability:?} is upper case: {text}");
            let mut empowered = String::new();
            write_empowered(&mut empowered, ability);
            assert!(
                empowered.is_empty() || empowered.ends_with('.'),
                "{ability:?}"
            );
        }
    }

    #[test]
    fn a_number_in_a_sentence_is_the_number_in_the_table() {
        let mut text = String::new();
        write_description(&mut text, FireAbility::Recuperate);
        assert_eq!(
            text,
            "Strike your target and gain a stack of Recuperate, restoring 0.2% of your maximum \
             Health every second for 10 sec. Stacks up to 5 times."
        );
        text.clear();
        write_description(&mut text, FireAbility::Taunt);
        assert!(text.contains("30 m"), "{text}");
    }

    #[test]
    fn every_empowerable_ability_says_what_the_empowerment_does() {
        for ability in FireAbility::ALL {
            let definition = ability.definition();
            let empowerable = definition
                .resource_uses()
                .any(|entry| entry.mode == ResourceMode::Empower);
            assert_eq!(
                empowerable,
                !definition.empowered.is_empty(),
                "{ability:?}: empowerable {empowerable}, text {:?}",
                definition.empowered
            );
        }
    }

    #[test]
    fn resource_lines_say_cost_empower_and_generate_differently() {
        let mut line = String::new();
        let stomp: Vec<_> = FireAbility::WarStomp.definition().resource_uses().collect();
        assert_eq!(stomp.len(), 1);
        stomp[0].write_line(&mut line);
        assert_eq!(line, "Consumes 20 Deflection to empower, if available");

        line.clear();
        let recuperate: Vec<_> = FireAbility::Recuperate
            .definition()
            .resource_uses()
            .collect();
        recuperate[0].write_line(&mut line);
        assert_eq!(line, "Consumes 20 Deflection to empower, at 100 Deflection");

        line.clear();
        let rejuvenation: Vec<_> = FireAbility::Rejuvenation
            .definition()
            .resource_uses()
            .collect();
        rejuvenation[0].write_line(&mut line);
        assert!(
            line.starts_with("Costs ") && line.ends_with(" Mana"),
            "{line}"
        );

        line.clear();
        let lance: Vec<_> = FireAbility::FlameLance
            .definition()
            .resource_uses()
            .collect();
        lance[0].write_line(&mut line);
        assert_eq!(line, "Generates 4 Heat");
    }

    #[test]
    fn numbers_print_with_only_the_decimals_they_need() {
        let mut out = String::new();
        for (value, expected) in [
            (TextValue::Seconds(8.0), "8 sec"),
            (TextValue::Seconds(0.5), "0.5 sec"),
            (TextValue::Percent(0.002), "0.2%"),
            (TextValue::Percent(0.3), "30%"),
            (TextValue::Metres(30.0), "30 m"),
            (TextValue::Degrees(120.0), "120°"),
        ] {
            out.clear();
            value.write(&mut out);
            assert_eq!(out, expected);
        }
    }
}
