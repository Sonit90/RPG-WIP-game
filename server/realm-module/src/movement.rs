//! Authoritative movement: input frames in, positions out.
//!
//! The client sends what its player *wants* (axes, camera yaw, jump) for each 60 Hz tick it
//! predicted. This reducer drops anything stale, malformed or beyond the real-time budget,
//! integrates the rest through `engine_realm::movement::apply_input` — the same function the
//! client predicted with — and writes the result with the last sequence number applied. The
//! client rewinds to that row and replays what the server has not seen yet.

use engine_game::movement::{Body, MovementState};
use engine_realm::RealmError;
use engine_realm::instance::InstanceState;
use engine_realm::movement::{InputBudget, InputFrame, accept_frames, apply_input};
use engine_realm::rate_limit::Operation;
use spacetimedb::ReducerContext;

use crate::schema::*;
use crate::util::{micros, rate_limit, timestamp};

/// The server's body for a player row.
pub fn body_of(player: &InstancePlayer) -> Body {
    Body {
        position: player.position.into(),
        velocity: player.velocity.into(),
        grounded: player.grounded,
        ground_normal: player.ground_normal.into(),
        facing_yaw: player.yaw,
        state: MovementState::Idle,
        support_height: 0.0,
    }
}

/// The row with `body` written into it.
pub fn with_body(player: InstancePlayer, body: &Body) -> InstancePlayer {
    InstancePlayer {
        position: body.position.into(),
        velocity: body.velocity.into(),
        ground_normal: body.ground_normal.into(),
        yaw: body.facing_yaw,
        grounded: body.grounded,
        ..player
    }
}

/// Whether every component is a number.
fn finite(v: engine_core::math::Vec3) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.z.is_finite()
}

/// A body that fell out of the level goes back to its start point.
pub fn rescue_from_void(
    body: Body,
    void_height: f32,
    dungeon: &crate::level::Dungeon,
    player: &InstancePlayer,
) -> Body {
    if body.position.y >= void_height && finite(body.position) {
        return body;
    }
    log::warn!(
        "event=void_rescue account_id={} instance_id={}",
        player.account_id,
        player.instance_id
    );
    crate::instance::spawn_body(dungeon, 0)
}

/// Applies a batch of movement input for the caller.
#[spacetimedb::reducer]
pub fn move_input(ctx: &ReducerContext, frames: Vec<NetInputFrame>) -> Result<(), String> {
    rate_limit(ctx, Operation::Movement)?;
    let player = ctx
        .db
        .instance_player()
        .identity()
        .find(ctx.sender())
        .ok_or_else(|| RealmError::dungeon("You are not in a dungeon.").to_wire())?;
    let instance = ctx
        .db
        .dungeon_instance()
        .id()
        .find(player.instance_id)
        .ok_or_else(|| RealmError::dungeon("That dungeon no longer exists.").to_wire())?;
    let state: InstanceState = instance.state.into();
    if !state.accepts_movement() {
        return Err(RealmError::dungeon("You cannot move yet.").into());
    }
    let dungeon = crate::level::dungeon(&instance.dungeon_id)?;
    let geometry = if instance.barrier_open {
        &dungeon.open
    } else {
        &dungeon.closed
    };

    let frames: Vec<InputFrame> = frames
        .into_iter()
        .map(|frame| InputFrame {
            seq: frame.seq,
            forward: frame.forward,
            right: frame.right,
            yaw: frame.yaw,
            face_camera: frame.face_camera,
            jump: frame.jump,
        })
        .collect();
    let now = micros(ctx.timestamp);
    let mut budget = InputBudget {
        tokens: player.budget_tokens,
        refilled_at: micros(player.budget_refilled_at),
    };
    let (accepted, dropped) = accept_frames(&frames, player.last_seq, &mut budget, now);
    if dropped > 0 {
        log::debug!(
            "event=input_dropped account_id={} instance_id={} dropped={dropped}",
            player.account_id,
            player.instance_id
        );
    }

    let mut body = body_of(&player);
    let mut last_seq = player.last_seq;
    crate::level::with_buffers(|buffers| {
        for frame in &accepted {
            apply_input(&mut body, frame, &geometry.rules, &geometry.world, buffers);
            last_seq = frame.seq;
        }
    });
    let body = rescue_from_void(body, geometry.void_height, &dungeon, &player);
    let body = if finite(body.position) {
        body
    } else {
        crate::instance::spawn_body(&dungeon, 0)
    };
    ctx.db.instance_player().identity().update(InstancePlayer {
        last_seq,
        last_input_at: if accepted.is_empty() {
            player.last_input_at
        } else {
            ctx.timestamp
        },
        budget_tokens: budget.tokens,
        budget_refilled_at: timestamp(budget.refilled_at),
        ..with_body(player, &body)
    });
    Ok(())
}
