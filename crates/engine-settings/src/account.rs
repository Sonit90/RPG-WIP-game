//! The settings that follow an account from machine to machine.
//!
//! ```text
//! settings.cfg ─┬─ display, graphics          ── stay on this machine
//!               └─ everything else  ─┐
//! hud.cfg ───────────────────────────┴─► account document ─► persistence API
//! ```
//!
//! A monitor's resolution and how much shadow a graphics card can afford are facts about the
//! *machine*; keybinds, volumes, camera speed, nameplates and the action bars are the
//! *player's*. The account document carries the second kind only, so signing in on a laptop
//! brings the player's bars and keys without dragging a desktop's 4K fullscreen along.

use serde_json::Value;

use crate::GameSettings;
use crate::hud::HudConfig;

/// The document format this build writes.
pub const ACCOUNT_FORMAT: u32 = 1;

/// Settings sections that never leave this machine.
pub const MACHINE_LOCAL: [&str; 2] = ["display", "graphics"];

/// The account document for these settings.
#[must_use]
pub fn account_document(settings: &GameSettings, hud: &HudConfig) -> Value {
    let mut game = serde_json::to_value(settings).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut game {
        for key in MACHINE_LOCAL {
            map.remove(key);
        }
    }
    serde_json::json!({
        "format": ACCOUNT_FORMAT,
        "settings": game,
        "hud": serde_json::to_value(hud).unwrap_or(Value::Null),
    })
}

/// `local` with the account document applied over it: every account section replaced, every
/// machine-local section kept. `None` if the document is not one this build can read.
///
/// Values pass through each type's own validation, so a document edited by hand — or written
/// by a build with wider bounds — produces legal settings or the defaults, never a crash.
#[must_use]
pub fn apply_account_document(
    document: &Value,
    local: &GameSettings,
    local_hud: &HudConfig,
) -> Option<(GameSettings, HudConfig)> {
    if document.get("format").and_then(Value::as_u64) != Some(u64::from(ACCOUNT_FORMAT)) {
        return None;
    }
    let mut merged = serde_json::to_value(local).ok()?;
    if let (Value::Object(target), Some(Value::Object(account))) =
        (&mut merged, document.get("settings"))
    {
        for (key, value) in account {
            if !MACHINE_LOCAL.contains(&key.as_str()) {
                target.insert(key.clone(), value.clone());
            }
        }
    }
    let settings: GameSettings = serde_json::from_value(merged).ok()?;
    let hud = document
        .get("hud")
        .and_then(|hud| serde_json::from_value::<HudConfig>(hud.clone()).ok())
        .map_or_else(|| local_hud.clone(), HudConfig::validated);
    Some((settings.validated(), hud))
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "test code asserts on known-good values")]
mod tests {
    use super::*;

    #[test]
    fn machine_settings_stay_and_account_settings_travel() {
        let mut desktop = GameSettings {
            camera_sensitivity: 1.7,
            master_volume: 0.3,
            ..GameSettings::default()
        };
        desktop.display.resolution_width = 3840;
        let document = account_document(&desktop, &HudConfig::default());
        assert!(document["settings"].get("display").is_none());
        assert!(document["settings"].get("graphics").is_none());

        let mut laptop = GameSettings::default();
        laptop.display.resolution_width = 1366;
        let (merged, _) =
            apply_account_document(&document, &laptop, &HudConfig::default()).expect("readable");
        assert!((merged.camera_sensitivity - 1.7).abs() < 1e-6);
        assert!((merged.master_volume - 0.3).abs() < 1e-6);
        assert_eq!(merged.display.resolution_width, 1366);
    }

    #[test]
    fn unreadable_documents_are_ignored() {
        let local = GameSettings::default();
        let hud = HudConfig::default();
        assert!(apply_account_document(&Value::Null, &local, &hud).is_none());
        assert!(apply_account_document(&serde_json::json!({"format": 99}), &local, &hud).is_none());
    }

    #[test]
    fn out_of_range_values_are_validated() {
        let document = serde_json::json!({
            "format": ACCOUNT_FORMAT,
            "settings": {"master_volume": 50.0},
        });
        let (merged, _) =
            apply_account_document(&document, &GameSettings::default(), &HudConfig::default())
                .expect("readable");
        assert!(merged.master_volume <= 1.0);
    }
}
