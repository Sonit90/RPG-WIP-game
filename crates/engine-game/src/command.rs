//! What the player asked for, separated from how they asked.
//!
//! ```text
//! keyboard/mouse -> InputState -> PlayerCommand -> Simulation
//!                              ^^^^^^^^^^^^^^^^
//!                              the only thing the simulation consumes
//! ```
//!
//! Nothing below this type knows about keys. That is what makes a replay, an AI-driven
//! character and — eventually — a network client possible without rewriting a gameplay
//! system: each of them produces the same commands from a different source, and the
//! simulation cannot tell the difference.
//!
//! Commands are plain `Copy` values with no allocation and no back-reference to a player.
//! In a networked build the same struct is what a client sends and a server validates.

use engine_core::math::{Quat, Vec3};

use crate::character::ItemSlot;
use crate::entity::EntityId;
use crate::firemage::TalentId;

/// One thing the player wants to happen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerCommand {
    /// Continuous movement intent, in the camera's frame of reference.
    ///
    /// Held rather than consumed: the simulation keeps the most recent one until it is
    /// replaced, so a tick that runs between two frames of input still moves the character.
    Move {
        /// Forward on the camera's axis, in [-1, 1].
        forward: f32,
        /// Right on the camera's axis, in [-1, 1].
        right: f32,
        /// Where the camera is looking, radians about +Y.
        yaw: f32,
        /// Whether the camera is steering the character.
        ///
        /// True while the look button is held. The character then faces where the *camera*
        /// looks instead of where it is walking, which is what makes strafing and
        /// backpedalling keep the enemy in front - and it is the difference between
        /// retreating from something and turning your back on it.
        face_camera: bool,
    },
    /// Jump, if the character is on the ground. Edge-triggered.
    Jump,
    /// Use the ability in a bar slot. Edge-triggered.
    UseAbility(usize),
    /// Use a ground-targeted bar ability at a chosen point. Edge-triggered.
    ///
    /// The placement is part of the command rather than read from the mouse when it lands,
    /// so a replay of the same commands puts the same Meteor in the same place.
    UseAbilityAt {
        /// Which bar slot.
        slot: usize,
        /// Where the player put the circle.
        position: Vec3,
    },
    /// Cast Cataclysm at a chosen point. Edge-triggered.
    UseUltimate {
        /// Where the player put the circle.
        position: Vec3,
    },
    /// Use one of the hero's buttons beside the bar. Edge-triggered.
    ///
    /// A class with more abilities than the bar has digits keeps the rest here — see
    /// [`crate::hero::Hero::extras`]. Indexed rather than named, for the reason
    /// [`PlayerCommand::UseAbility`] is: what sits in the slot is the hero's business, and a
    /// replay that named a spell would cast a different one after a swap.
    UseExtra(usize),
    /// Buy a talent. A menu action, in the command stream so it is ordered with the fight.
    PurchaseTalent(TalentId),
    /// Move an item between two slots: equip, unequip, swap or rearrange.
    ///
    /// A command like a talent purchase, and for the same reason: dragging a ring onto a
    /// finger changes what the character's stats are, so it belongs in the same ordered
    /// stream as the casts those stats decide. A refused move is not an error here — the
    /// simulation applies what it can, exactly as it does with an ability that cannot be
    /// cast.
    MoveItem {
        /// Where the item is now.
        from: ItemSlot,
        /// Where it was dropped.
        to: ItemSlot,
    },
    /// Target the nearest living hostile.
    SelectNearestTarget,
    /// Move to the next living hostile, by distance, within the class's own reach.
    CycleTarget,
    /// Select nobody.
    ///
    /// What Escape asks for. A target is a thing the player chose, so dropping it is also a
    /// choice — and nothing picks a new one afterwards.
    ClearTarget,
    /// Target the player's own character.
    ///
    /// The self-target key. Separate from [`PlayerCommand::SelectTarget`] because the caller
    /// would otherwise have to know which entity the player is to ask for the one selection
    /// that is always available.
    SelectSelf,
    /// Target a named entity: what clicking a nameplate asks for.
    ///
    /// The entity rather than a ray, because a nameplate *is* the entity — the interface
    /// drew a box and wrote which one it belongs to inside it, and re-deriving that from a
    /// ray would be answering a question that was already answered. Gameplay still decides
    /// whether the request is legal, so a caller cannot select a corpse or an ally.
    SelectTarget {
        /// Which entity to select.
        entity: EntityId,
    },
    /// Target whatever a ray passes through: the gameplay half of clicking on something.
    ///
    /// A ray rather than an entity, so the *rules* of selection — who is hostile, who is
    /// alive, what shape a body is, which of two overlapping bodies wins — stay in gameplay.
    /// The caller owns the camera and the cursor and nothing else; it says where the player
    /// pointed, not what they picked.
    ///
    /// A ray that hits nothing changes nothing. Clicking empty floor is not a request to
    /// have no target.
    PickTarget {
        /// Where the ray starts, in world space.
        origin: Vec3,
        /// Which way it goes. Need not be normalised.
        direction: Vec3,
    },
    /// Restore every dummy and clear the combat session. A developer control.
    ResetDummies,
    /// Put the character down at a position, at rest. A developer control.
    ///
    /// A command rather than a direct write to the world, so a teleport is part of the
    /// same ordered stream as everything else: the collision demo's numbered test stations
    /// are reproducible for the same reason a replay is.
    Teleport {
        /// Where the character's feet should end up.
        position: Vec3,
    },
    /// Open the boss's chest and take this player's item from it. Edge-triggered.
    ///
    /// No target: there is at most one chest, and the rules (it has landed, the player is in
    /// range and has not already taken their share) are the simulation's. A command that
    /// fails them does nothing. A full bag mails the item instead.
    OpenChest,
    /// Move a letter's item from the current character's mailbox into their bag.
    ///
    /// A full bag keeps the letter and says so with [`crate::event::CombatEvent::MailBagFull`].
    TakeMail {
        /// Which letter.
        id: u64,
    },
    /// Mark a letter in the current character's mailbox as read.
    MarkMailRead {
        /// Which letter.
        id: u64,
    },
}

impl PlayerCommand {
    /// The yaw a [`PlayerCommand::Move`] locks the character's facing to, if it locks it.
    ///
    /// `None` means "face the way you are going", which is the ordinary case.
    #[must_use]
    pub fn facing_lock(self) -> Option<f32> {
        match self {
            Self::Move {
                yaw,
                face_camera: true,
                ..
            } => Some(yaw),
            _ => None,
        }
    }

    /// The world-space direction a [`PlayerCommand::Move`] asks for.
    ///
    /// Camera-relative, which is what makes W mean "away from the camera" rather than
    /// "towards −Z" — the conversion happens here rather than in the input layer so that a
    /// replayed command produces the same direction as the live one did.
    ///
    /// Returns [`Vec3::ZERO`] for any other command and for a neutral stick.
    #[must_use]
    pub fn move_direction(self) -> Vec3 {
        let Self::Move {
            forward,
            right,
            yaw,
            ..
        } = self
        else {
            return Vec3::ZERO;
        };
        // Local space: −Z is forward, +X is right. Rotating by the camera's yaw puts it in
        // world space. Not normalized here: the character controller does that, and doing
        // it twice would make a half-pressed analogue stick move at full speed.
        let local = Vec3::new(right, 0.0, -forward);
        if local.length_squared() <= f32::EPSILON {
            return Vec3::ZERO;
        }
        Quat::from_yaw_pitch_roll(yaw, 0.0, 0.0).rotate(local)
    }

    /// Whether this command is a one-off rather than a held intent.
    ///
    /// Discrete commands are applied once, on the first simulation tick after they arrive.
    /// Applying a jump on every tick of a catch-up batch would turn one key press into
    /// eight jumps.
    #[must_use]
    pub fn is_discrete(self) -> bool {
        !matches!(self, Self::Move { .. })
    }
}

/// Commands waiting for the next simulation tick.
///
/// A reused `Vec`: the buffer is drained, not freed, so a steady state allocates nothing.
#[derive(Debug, Default, Clone)]
pub struct CommandBuffer {
    commands: Vec<PlayerCommand>,
}

impl CommandBuffer {
    /// An empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty buffer with room for `capacity` commands.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            commands: Vec::with_capacity(capacity),
        }
    }

    /// Queues a command.
    pub fn push(&mut self, command: PlayerCommand) {
        self.commands.push(command);
    }

    /// Queues several commands.
    pub fn extend(&mut self, commands: impl IntoIterator<Item = PlayerCommand>) {
        self.commands.extend(commands);
    }

    /// The queued commands, in arrival order.
    #[must_use]
    pub fn commands(&self) -> &[PlayerCommand] {
        &self.commands
    }

    /// How many commands are queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether nothing is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// How many commands fit before the buffer grows.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.commands.capacity()
    }

    /// Empties the buffer, keeping the allocation.
    pub fn clear(&mut self) {
        self.commands.clear();
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "test code may assert on known-good values"
)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1e-4;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a.x - b.x).abs() < EPSILON && (a.y - b.y).abs() < EPSILON && (a.z - b.z).abs() < EPSILON
    }

    #[test]
    fn forward_with_no_camera_yaw_is_minus_z() {
        let direction = PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: 0.0,
            face_camera: false,
        }
        .move_direction();
        assert!(close(direction, -Vec3::Z), "{direction:?}");
    }

    #[test]
    fn movement_follows_the_camera_rather_than_the_world() {
        // The whole reason yaw is part of the command: W means "away from the camera".
        let direction = PlayerCommand::Move {
            forward: 1.0,
            right: 0.0,
            yaw: std::f32::consts::FRAC_PI_2,
            face_camera: false,
        }
        .move_direction();
        // A quarter turn about +Y sends −Z to −X.
        assert!(close(direction, -Vec3::X), "{direction:?}");
    }

    #[test]
    fn strafing_is_perpendicular_to_forward() {
        let direction = PlayerCommand::Move {
            forward: 0.0,
            right: 1.0,
            yaw: 0.0,
            face_camera: false,
        }
        .move_direction();
        assert!(close(direction, Vec3::X), "{direction:?}");
    }

    #[test]
    fn a_neutral_stick_asks_for_no_movement() {
        assert_eq!(
            PlayerCommand::Move {
                forward: 0.0,
                right: 0.0,
                yaw: 1.2,
                face_camera: false,
            }
            .move_direction(),
            Vec3::ZERO
        );
        assert_eq!(PlayerCommand::Jump.move_direction(), Vec3::ZERO);
    }

    #[test]
    fn only_movement_is_a_held_intent() {
        assert!(
            !PlayerCommand::Move {
                forward: 1.0,
                right: 0.0,
                yaw: 0.0,
                face_camera: false,
            }
            .is_discrete()
        );
        for command in [
            PlayerCommand::Jump,
            PlayerCommand::UseAbility(0),
            PlayerCommand::SelectNearestTarget,
            PlayerCommand::CycleTarget,
            PlayerCommand::ResetDummies,
        ] {
            assert!(command.is_discrete(), "{command:?}");
        }
    }

    #[test]
    fn the_buffer_keeps_arrival_order_and_reuses_its_allocation() {
        let mut buffer = CommandBuffer::with_capacity(8);
        buffer.push(PlayerCommand::Jump);
        buffer.extend([PlayerCommand::CycleTarget, PlayerCommand::ResetDummies]);

        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.commands()[0], PlayerCommand::Jump);
        assert_eq!(buffer.commands()[2], PlayerCommand::ResetDummies);

        let capacity = buffer.capacity();
        buffer.clear();
        assert!(buffer.is_empty());
        assert_eq!(buffer.capacity(), capacity, "the buffer was not released");
    }
}
