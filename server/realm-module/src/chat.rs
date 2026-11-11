//! Chat: three channels, every line authored by the server.
//!
//! ```text
//! chat_send(channel, target, text)
//!   │ rate limit ─► account ─► speaker (character name from party / ticket, never the client)
//!   │ sanitise ─► Server: store │ Party: member of a party? store │ Whisper: resolve name
//!   ▼
//! chat_message (private) ──► my_chat view: Server + my party + whispers I sent or received
//! ```
//!
//! The client sends a channel number, a whisper's target name and the text — no sender, no
//! name, no party — so a line cannot claim to come from anyone but the connection that sent
//! it. History is bounded: every send prunes, and the janitor prunes for a quiet realm.

use engine_realm::RealmError;
use engine_realm::chat::{
    self, Channel, Envelope, Reader, Stored, WhisperRefusal, resolve_whisper, speaker_name,
};
use engine_realm::rate_limit::Operation;
use spacetimedb::{Identity, ReducerContext, Table, ViewContext};

use crate::schema::*;
use crate::util::{caller_account, micros, notify, rate_limit};
use crate::views::ChatLine;

/// The character a connected player speaks as, and the party they are in (zero: none).
fn speaker(ctx: &ReducerContext, identity: Identity) -> Option<(String, u64)> {
    let member = ctx.db.party_member().identity().find(identity);
    let ticket = ctx.db.queue_ticket().identity().find(identity);
    let account = ctx.db.account().identity().find(identity);
    let name = speaker_name(
        account.as_ref().map(|a| a.display_name.as_str()),
        member.as_ref().map(|m| m.character_name.as_str()),
        ticket.as_ref().map(|t| t.character_name.as_str()),
    )?
    .to_owned();
    Some((name, member.map_or(0, |m| m.party_id)))
}

/// Every connected player with a character, as whispers see them.
fn online_speakers(ctx: &ReducerContext) -> Vec<(Identity, String)> {
    ctx.db
        .account()
        .iter()
        .filter(|account| account.connections > 0)
        .filter_map(|account| {
            speaker(ctx, account.identity).map(|(name, _)| (account.identity, name))
        })
        .collect()
}

/// Says something. `target` is a whisper's character name and is ignored on the other
/// channels. The sender's name is never an argument: it is the caller's own character.
#[spacetimedb::reducer]
pub fn chat_send(
    ctx: &ReducerContext,
    channel: u8,
    target: String,
    text: String,
) -> Result<(), String> {
    rate_limit(ctx, Operation::Chat)?;
    let account = caller_account(ctx)?;
    let channel = Channel::from_u8(channel)
        .ok_or_else(|| RealmError::protocol("That chat channel does not exist.").to_wire())?;
    let text = chat::sanitize(&text)?;
    let (name, party_id) =
        speaker(ctx, ctx.sender()).ok_or_else(|| chat::no_character().to_wire())?;

    let mut line = ChatMessage {
        id: 0,
        channel: channel.as_u8(),
        sender: ctx.sender(),
        sender_account: account.account_id.clone(),
        sender_name: name,
        party_id: 0,
        recipient: Identity::ZERO,
        recipient_account: String::new(),
        recipient_name: String::new(),
        text,
        sent_at: ctx.timestamp,
    };
    match channel {
        Channel::Server => {}
        Channel::Party => {
            if party_id == 0 {
                return Err(chat::not_in_party().to_wire());
            }
            line.party_id = party_id;
        }
        Channel::Whisper => {
            let online = online_speakers(ctx);
            match resolve_whisper(&ctx.sender(), &target, &online) {
                Ok(&recipient) => {
                    let Some(other) = ctx.db.account().identity().find(recipient) else {
                        return Ok(());
                    };
                    line.recipient = recipient;
                    line.recipient_account = other.account_id;
                    line.recipient_name = online
                        .iter()
                        .find(|(who, _)| *who == recipient)
                        .map(|(_, name)| name.clone())
                        .unwrap_or_default();
                }
                Err(refusal) => {
                    // A notice, and a success: an error would roll the notice back.
                    refuse_whisper(ctx, refusal, &target);
                    return Ok(());
                }
            }
        }
    }
    ctx.db.chat_message().insert(line);
    prune(ctx);
    Ok(())
}

fn refuse_whisper(ctx: &ReducerContext, refusal: WhisperRefusal, target: &str) {
    notify(
        ctx,
        ctx.sender(),
        "whisper_failed",
        NoticeLevel::Warning,
        refusal.message(target),
    );
}

/// Deletes history past the window and Server lines past the cap.
pub fn prune(ctx: &ReducerContext) {
    let stored: Vec<Stored> = ctx
        .db
        .chat_message()
        .iter()
        .filter_map(|row| {
            Some(Stored {
                id: row.id,
                channel: Channel::from_u8(row.channel)?,
                sent_at: micros(row.sent_at),
            })
        })
        .collect();
    for id in chat::prune(&stored, micros(ctx.timestamp)) {
        ctx.db.chat_message().id().delete(id);
    }
}

/// What `ctx.sender()` may read.
pub fn readable_by(ctx: &ViewContext) -> Vec<ChatLine> {
    let me = ctx.sender();
    let party = ctx
        .db
        .party_member()
        .identity()
        .find(me)
        .map(|member| member.party_id);

    let mut rows: Vec<ChatMessage> = ctx
        .db
        .chat_message()
        .channel()
        .filter(Channel::Server.as_u8())
        .collect();
    if let Some(party_id) = party {
        rows.extend(ctx.db.chat_message().party_id().filter(party_id));
    }
    rows.extend(ctx.db.chat_message().sender().filter(me));
    rows.extend(ctx.db.chat_message().recipient().filter(me));
    rows.sort_by_key(|row| row.id);
    rows.dedup_by_key(|row| row.id);

    let reader = Reader { who: me, party };
    rows.into_iter()
        .filter(|row| {
            let Some(channel) = Channel::from_u8(row.channel) else {
                return false;
            };
            let envelope = Envelope {
                channel,
                sender: row.sender,
                recipient: (row.recipient != Identity::ZERO).then_some(row.recipient),
                party: (row.party_id != 0).then_some(row.party_id),
            };
            chat::visible_to(&envelope, &reader)
        })
        .map(|row| {
            let whisper = row.recipient != Identity::ZERO;
            ChatLine {
                id: row.id,
                channel: row.channel,
                sender_account: row.sender_account,
                sender_name: row.sender_name,
                recipient_account: whisper.then_some(row.recipient_account),
                recipient_name: whisper.then_some(row.recipient_name),
                text: row.text,
                sent_at: row.sent_at,
            }
        })
        .collect()
}
