//! Keys to commands. The only place in the project that knows which key does what.
//!
//! ```text
//! InputState -> Bindings::write_commands -> [PlayerCommand] -> Simulation
//! ```
//!
//! This is the top of the gameplay stack, not the bottom of the platform stack:
//! `engine-core::input` deliberately has no `Jump` and no `Fireball`, because a binding is
//! a gameplay decision. Keeping the translation here means the simulation can be driven by
//! a recording or by a future network client without a keyboard existing at all.
//!
//! Nothing here mutates gameplay state. It reads input and appends commands.

use engine_core::input::{InputState, KeyCode, MouseButton};

use crate::command::PlayerCommand;
use crate::firemage::ABILITY_SLOTS;
use crate::hero::Hero;

/// Which key does what.
///
/// A struct rather than constants so a future options screen can rebind without touching
/// the translation, and so a test can drive an unusual layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bindings {
    /// Move away from the camera.
    pub forward: KeyCode,
    /// Move towards the camera.
    pub back: KeyCode,
    /// Strafe left.
    pub left: KeyCode,
    /// Strafe right.
    pub right: KeyCode,
    /// Jump.
    pub jump: KeyCode,
    /// Select the next hostile target.
    pub cycle_target: KeyCode,
    /// Select the player's own character.
    pub select_self: KeyCode,
    /// Aim the Ultimate. It is not on the bar, so it has a key of its own.
    ///
    /// Kept here beside the rest even though the application is what acts on it: a key that
    /// lived somewhere else is a key nothing can check for a collision - which is exactly
    /// how it ended up sharing `R` with the developer reset and silently wiping the energy
    /// bar every time the Ultimate was pressed.
    pub ultimate: KeyCode,
    /// Restore the dummies and clear the combat session. A developer control.
    pub reset_dummies: KeyCode,
    /// The mouse button held to turn the camera, which also turns the character.
    pub look: MouseButton,
}

impl Default for Bindings {
    fn default() -> Self {
        Self {
            forward: KeyCode::W,
            back: KeyCode::S,
            left: KeyCode::A,
            right: KeyCode::D,
            jump: KeyCode::Space,
            cycle_target: KeyCode::Tab,
            // F1, which is where the genre has put "target yourself" for twenty years.
            select_self: KeyCode::F1,
            ultimate: KeyCode::R,
            // Not `R`: that is the Ultimate. A developer control has no business taking a
            // key the class needs.
            reset_dummies: KeyCode::Backspace,
            look: MouseButton::Right,
        }
    }
}

impl Bindings {
    /// Every key this binding set claims, for a collision check.
    ///
    /// The ability bar's digits are added by [`Bindings::ability_key`]; these are the named
    /// ones. A test walks the whole list, which is what stops two actions from quietly
    /// sharing a key again.
    #[must_use]
    pub fn named_keys(&self) -> [KeyCode; 9] {
        [
            self.forward,
            self.back,
            self.left,
            self.right,
            self.jump,
            self.cycle_target,
            self.select_self,
            self.ultimate,
            self.reset_dummies,
        ]
    }

    /// What `key` already does, if this binding set claims it.
    ///
    /// The one question a rebinding interface has to be able to ask before it writes a
    /// binding down. It lives here because this module is the only place that knows which
    /// key does what, and an interface that kept its own list of "keys you may not take"
    /// would be a second list to forget to update.
    ///
    /// Escape is included even though it is not a field: it is what closes a menu, and a
    /// player who bound it to an action bar slot would have bound their way out of the
    /// interface that could unbind it.
    #[must_use]
    pub fn reserved(&self, key: KeyCode) -> Option<&'static str> {
        if key == KeyCode::Escape {
            return Some("THE MENU");
        }
        let named = [
            (self.forward, "MOVE FORWARD"),
            (self.back, "MOVE BACK"),
            (self.left, "STRAFE LEFT"),
            (self.right, "STRAFE RIGHT"),
            (self.jump, "JUMP"),
            (self.cycle_target, "CYCLE TARGET"),
            (self.select_self, "TARGET YOURSELF"),
            (self.ultimate, "THE ULTIMATE"),
            (self.reset_dummies, "RESET DUMMIES"),
        ];
        named
            .into_iter()
            .find(|(bound, _)| *bound == key)
            .map(|(_, what)| what)
    }

    /// The key that triggers ability `slot`.
    ///
    /// Slots 0 to 8 are the digits 1 to 9 and slot 9 is `0`, which is the layout every
    /// action bar in the genre uses.
    #[must_use]
    pub fn ability_key(slot: usize) -> Option<KeyCode> {
        if slot >= ABILITY_SLOTS {
            return None;
        }
        // Slot 9 is the `0` key, which is where every action bar in the genre puts it.
        KeyCode::digit_key(((slot as u8) + 1) % 10)
    }

    /// Appends this frame's commands to `out`.
    ///
    /// `camera_yaw` is where the view is pointing, in radians about +Y; movement is
    /// expressed relative to it so W means "away from the camera".
    ///
    /// Held keys produce a [`PlayerCommand::Move`] every frame — including a neutral one
    /// when nothing is held, so releasing the keys is itself an instruction to stop rather
    /// than the absence of one. Everything else is edge-triggered from
    /// [`InputState::was_key_pressed`], so holding `1` casts once.
    pub fn write_commands(
        &self,
        hero: Hero,
        input: &InputState,
        camera_yaw: f32,
        out: &mut Vec<PlayerCommand>,
    ) {
        self.write_world_commands(input, camera_yaw, out);
        self.write_bar_commands(hero, input, out);
    }

    /// Appends everything except the ability bar: movement, jump, targeting, developer keys.
    ///
    /// What the game itself uses. The bar is left out because its keys are the player's to
    /// arrange — eight bars of ten slots, each with a key of its own, saved in `hud.cfg` —
    /// and a fixed digit row issuing commands beside them would cast twice on `1` and not at
    /// all on a slot the player rebound. The arranged bar lives in `engine-presentation`;
    /// what a slot does when it is pressed is still a `PlayerCommand`, built here in
    /// [`Bindings::write_bar_commands`]'s shape.
    pub fn write_world_commands(
        &self,
        input: &InputState,
        camera_yaw: f32,
        out: &mut Vec<PlayerCommand>,
    ) {
        self.write_movement_commands(input, camera_yaw, out);

        if input.was_key_pressed(self.cycle_target) {
            out.push(PlayerCommand::CycleTarget);
        }
        if input.was_key_pressed(self.select_self) {
            out.push(PlayerCommand::SelectSelf);
        }
        if input.was_key_pressed(self.reset_dummies) {
            out.push(PlayerCommand::ResetDummies);
        }
    }

    /// Appends `hero`'s digit-row bar: slot 1 on `1`, through slot 10 on `0`.
    ///
    /// The layout every action bar in the genre starts with, and the one a headless test or
    /// a replay drives the class with when there is no interface to arrange one.
    pub fn write_bar_commands(&self, hero: Hero, input: &InputState, out: &mut Vec<PlayerCommand>) {
        // An *aimed* ability - one placed on the ground, or a cone pointed with the mouse -
        // is deliberately not cast from here: its key shows the preview, and the application
        // sends the command when the player clicks.
        for slot in 0..ABILITY_SLOTS {
            let (Some(key), Some(ability)) = (Self::ability_key(slot), hero.ability_at(slot))
            else {
                continue;
            };
            if input.was_key_pressed(key) && !ability.definition().target.is_aimed() {
                out.push(PlayerCommand::UseAbility(slot));
            }
        }
    }

    /// Appends only this frame's movement and jump.
    ///
    /// What the collision demo scene uses. That scene has no abilities, no targets and no
    /// dummies, and its number keys stand the character at a test station instead — so
    /// issuing `UseAbility` there would be issuing commands nothing can act on and taking
    /// the digits away from the thing that needs them.
    pub fn write_movement_commands(
        &self,
        input: &InputState,
        camera_yaw: f32,
        out: &mut Vec<PlayerCommand>,
    ) {
        let axis = |positive: KeyCode, negative: KeyCode| -> f32 {
            f32::from(u8::from(input.is_key_down(positive)))
                - f32::from(u8::from(input.is_key_down(negative)))
        };

        out.push(PlayerCommand::Move {
            forward: axis(self.forward, self.back),
            right: axis(self.right, self.left),
            yaw: camera_yaw,
            // Held right button: the camera is steering, so the character faces the camera
            // rather than its own feet. Read here rather than passed in, because which
            // button steers is a binding and bindings live in this module.
            face_camera: input.is_mouse_button_down(self.look),
        });

        if input.was_key_pressed(self.jump) {
            out.push(PlayerCommand::Jump);
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;
    use engine_core::input::InputEvent;

    /// Applies one frame of key presses, the way the engine loop does.
    fn frame(state: &mut InputState, down: &[KeyCode], up: &[KeyCode]) {
        state.begin_frame();
        for key in down {
            state.apply(InputEvent::KeyDown {
                key: *key,
                repeat: false,
            });
        }
        for key in up {
            state.apply(InputEvent::KeyUp { key: *key });
        }
    }

    fn commands(input: &InputState, yaw: f32) -> Vec<PlayerCommand> {
        let mut out = Vec::new();
        Bindings::default().write_commands(Hero::default(), input, yaw, &mut out);
        out
    }

    fn movement(commands: &[PlayerCommand]) -> Option<PlayerCommand> {
        commands
            .iter()
            .find(|command| !command.is_discrete())
            .copied()
    }

    #[test]
    fn wasd_becomes_one_movement_command_per_frame() {
        let mut input = InputState::new();
        frame(&mut input, &[KeyCode::W, KeyCode::D], &[]);

        let issued = commands(&input, 0.0);
        assert_eq!(
            issued.iter().filter(|c| !c.is_discrete()).count(),
            1,
            "movement is one held intent, not one command per key"
        );
        assert_eq!(
            movement(&issued),
            Some(PlayerCommand::Move {
                forward: 1.0,
                right: 1.0,
                yaw: 0.0,
                face_camera: false
            })
        );
    }

    #[test]
    fn opposite_keys_cancel_rather_than_fighting() {
        let mut input = InputState::new();
        frame(
            &mut input,
            &[KeyCode::W, KeyCode::S, KeyCode::A, KeyCode::D],
            &[],
        );
        assert_eq!(
            movement(&commands(&input, 0.0)).map(PlayerCommand::move_direction),
            Some(engine_core::math::Vec3::ZERO)
        );
    }

    #[test]
    fn releasing_everything_still_issues_a_neutral_movement_command() {
        // The absence of a command must not mean "keep running": a dropped frame of input
        // would then send the character off across the arena.
        let mut input = InputState::new();
        frame(&mut input, &[], &[]);
        assert_eq!(
            movement(&commands(&input, 1.0)),
            Some(PlayerCommand::Move {
                forward: 0.0,
                right: 0.0,
                yaw: 1.0,
                face_camera: false
            })
        );
    }

    #[test]
    fn the_camera_yaw_travels_with_the_movement_command() {
        let mut input = InputState::new();
        frame(&mut input, &[KeyCode::W], &[]);
        let direction = movement(&commands(&input, std::f32::consts::FRAC_PI_2))
            .expect("a movement command was issued")
            .move_direction();
        assert!((direction.x + 1.0).abs() < 1e-4, "{direction:?}");
    }

    #[test]
    fn jump_fires_once_per_physical_press() {
        let mut input = InputState::new();
        frame(&mut input, &[KeyCode::Space], &[]);
        assert!(commands(&input, 0.0).contains(&PlayerCommand::Jump));

        // Still held on the next frame: the key is down, but it was not pressed again.
        frame(&mut input, &[], &[]);
        assert!(!commands(&input, 0.0).contains(&PlayerCommand::Jump));
        assert!(input.is_key_down(KeyCode::Space));
    }

    #[test]
    fn holding_an_ability_key_casts_once() {
        let mut input = InputState::new();
        let firebolt = PlayerCommand::UseAbility(0);

        frame(&mut input, &[KeyCode::Digit1], &[]);
        assert!(commands(&input, 0.0).contains(&firebolt));
        frame(&mut input, &[], &[]);
        assert!(!commands(&input, 0.0).contains(&firebolt));
    }

    #[test]
    fn every_ability_slot_has_a_key_and_they_are_all_different() {
        let keys: Vec<KeyCode> = (0..ABILITY_SLOTS)
            .map(|slot| Bindings::ability_key(slot).expect("every slot has a key"))
            .collect();
        assert_eq!(keys[0], KeyCode::Digit1);
        assert_eq!(keys[8], KeyCode::Digit9);
        assert_eq!(keys[9], KeyCode::Digit0, "the tenth slot is the 0 key");

        let mut unique = keys.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), keys.len(), "two slots share a key");
        assert_eq!(Bindings::ability_key(ABILITY_SLOTS), None);
    }

    #[test]
    fn no_two_actions_share_a_key() {
        // The Ultimate and the developer reset both used to be `R`, so pressing the Ultimate
        // reset the arena - which cleared the energy bar and left the cast refused.
        let bindings = Bindings::default();
        let mut claimed: Vec<KeyCode> = bindings.named_keys().to_vec();
        for slot in 0..ABILITY_SLOTS {
            if let Some(key) = Bindings::ability_key(slot) {
                claimed.push(key);
            }
        }
        for (index, key) in claimed.iter().enumerate() {
            assert!(
                !claimed[index + 1..].contains(key),
                "{key:?} is bound to two different actions"
            );
        }
    }

    #[test]
    fn every_ability_key_produces_its_own_slot() {
        for slot in 0..ABILITY_SLOTS {
            let ability = Hero::default()
                .ability_at(slot)
                .expect("every slot of the mage's bar has an ability");
            // An aimed ability shows its preview instead of casting from the key; the
            // application sends the command when the player clicks.
            if ability.definition().target.is_aimed() {
                continue;
            }
            let key = Bindings::ability_key(slot).expect("every slot has a key");
            let mut input = InputState::new();
            frame(&mut input, &[key], &[]);

            let expected = PlayerCommand::UseAbility(slot);
            let issued = commands(&input, 0.0);
            assert!(issued.contains(&expected), "slot {slot} did not fire");
            assert_eq!(
                issued
                    .iter()
                    .filter(|command| matches!(command, PlayerCommand::UseAbility(_)))
                    .count(),
                1,
                "slot {slot} fired more than one ability"
            );
        }
    }

    #[test]
    fn the_developer_keys_are_edge_triggered_too() {
        let mut input = InputState::new();
        frame(&mut input, &[KeyCode::Tab, KeyCode::Backspace], &[]);
        let issued = commands(&input, 0.0);
        assert!(issued.contains(&PlayerCommand::CycleTarget));
        assert!(issued.contains(&PlayerCommand::ResetDummies));

        frame(&mut input, &[], &[]);
        let held = commands(&input, 0.0);
        assert!(!held.contains(&PlayerCommand::ResetDummies));
    }

    #[test]
    fn the_movement_only_mapping_issues_nothing_the_collision_scene_cannot_act_on() {
        let mut input = InputState::new();
        frame(
            &mut input,
            &[KeyCode::W, KeyCode::Space, KeyCode::Digit1, KeyCode::Tab],
            &[],
        );

        let mut out = Vec::new();
        Bindings::default().write_movement_commands(&input, 0.0, &mut out);
        assert!(out.contains(&PlayerCommand::Jump));
        assert_eq!(
            movement(&out),
            Some(PlayerCommand::Move {
                forward: 1.0,
                right: 0.0,
                yaw: 0.0,
                face_camera: false
            })
        );
        assert_eq!(
            out.len(),
            2,
            "an ability and a target cycle leaked in: {out:?}"
        );
    }

    #[test]
    fn an_idle_frame_costs_one_command() {
        // The frame path appends into a reused buffer; a neutral frame must not push five
        // commands nobody acts on.
        let input = InputState::new();
        assert_eq!(commands(&input, 0.0).len(), 1);
    }

    #[test]
    fn every_key_the_game_claims_reports_what_it_is_for() {
        let bindings = Bindings::default();
        for key in bindings.named_keys() {
            assert!(
                bindings.reserved(key).is_some(),
                "{key:?} is bound but reports nothing"
            );
        }
        // Escape is not a field, and is reserved anyway: it is the way out of every menu.
        assert!(bindings.reserved(KeyCode::Escape).is_some());
        // The digit row is not reserved. It is the default action bar, and the player is
        // allowed to take it apart.
        for slot in 0..ABILITY_SLOTS {
            let key = Bindings::ability_key(slot).expect("every slot has a default key");
            assert_eq!(bindings.reserved(key), None, "{key:?} cannot be rebound");
        }
        // F1 is claimed now: it is the self-target key, and a slot that took it would take
        // the one selection a player can always make.
        assert_eq!(bindings.reserved(KeyCode::F1), Some("TARGET YOURSELF"));
        assert_eq!(bindings.reserved(KeyCode::F2), None);
    }

    #[test]
    fn the_world_commands_are_everything_but_the_bar() {
        let mut state = InputState::new();
        frame(
            &mut state,
            &[KeyCode::W, KeyCode::Space, KeyCode::Tab, KeyCode::Digit1],
            &[],
        );

        let mut world = Vec::new();
        Bindings::default().write_world_commands(&state, 0.0, &mut world);
        assert!(
            !world
                .iter()
                .any(|command| matches!(command, PlayerCommand::UseAbility(_))),
            "the bar reached the world commands: {world:?}"
        );
        assert!(world.contains(&PlayerCommand::Jump));
        assert!(world.contains(&PlayerCommand::CycleTarget));

        let mut bar = Vec::new();
        Bindings::default().write_bar_commands(Hero::default(), &state, &mut bar);
        assert_eq!(bar, vec![PlayerCommand::UseAbility(0)]);

        // Together they are exactly what the whole translation produces.
        let mut both = Vec::new();
        Bindings::default().write_commands(Hero::default(), &state, 0.0, &mut both);
        world.extend(bar);
        assert_eq!(both, world);
    }
}
