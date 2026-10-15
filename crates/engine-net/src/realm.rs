//! The SpacetimeDB connection: the only file in the client that names an SDK type.
//!
//! It subscribes to the caller-scoped views, turns every row change into a plain
//! [`RealmUpdate`] on the event channel, and exposes the module's client reducers as plain
//! methods. Callbacks run on the SDK's own thread; they only convert and send, never block.

use std::sync::mpsc::Sender;
use std::thread::JoinHandle;

use engine_core::math::Vec3;
use engine_realm::chat::Channel;
use engine_realm::combat::{
    AbilityRef, ActionKind, CastState, CombatAction, CooldownState, EntityRef, Resources,
    StatusState, WireEvent,
};
use engine_realm::instance::InstanceState;
use engine_realm::movement::InputFrame;
use engine_realm::party::{PartyState, ReadyResponse};
use engine_realm::{ErrorCategory, RealmError, Role};
use spacetimedb_sdk::{DbContext, Table, TableWithPrimaryKey};

use crate::bindings::{
    self as b, DbConnection, MyAccountTableAccess, MyChatTableAccess, MyCombatBatchesTableAccess,
    MyInstanceMonstersTableAccess, MyInstancePlayersTableAccess, MyInstanceTableAccess,
    MyNoticesTableAccess, MyPartyMembersTableAccess, MyPartyTableAccess, MyPlayerCombatTableAccess,
    MyRunSummaryTableAccess, MyTicketTableAccess, QueuePopulationTableAccess, chat_send,
    combat_act, dismiss_notice, move_input, party_insert_key, party_kick, party_leave, party_start,
    party_transfer_leader, party_withdraw_key, queue_leave, ready_check_respond, server_time,
};
use crate::config::RealmEndpoints;
use crate::model::{
    AccountInfo, ChatMessageInfo, InstanceInfo, MemberInfo, MonsterInfo, Notice, NoticeLevel,
    PartyInfo, PlayerCombatInfo, Population, RealmMirror, RealmUpdate, RemotePlayerState,
    RunSummaryInfo, TicketInfo,
};
use crate::session::{Action, NetEvent, WorkerMessage};

/// What the link tells the network thread about itself.
#[derive(Debug)]
pub enum LinkSignal {
    /// The socket is open and authenticated.
    Connected,
    /// It could not connect.
    ConnectFailed(String),
    /// It closed.
    Disconnected(Option<String>),
    /// The subscription is live; the mirror was reset.
    Subscribed,
    /// One clock exchange finished.
    ServerTime {
        /// Local send time.
        sent: i64,
        /// Local receive time.
        received: i64,
        /// The server's answer.
        server: i64,
    },
}

/// Every view the client subscribes to.
const QUERIES: [&str; 13] = [
    "SELECT * FROM my_account",
    "SELECT * FROM my_ticket",
    "SELECT * FROM my_party",
    "SELECT * FROM my_party_members",
    "SELECT * FROM my_instance",
    "SELECT * FROM my_instance_players",
    "SELECT * FROM my_instance_monsters",
    "SELECT * FROM my_notices",
    "SELECT * FROM queue_population",
    "SELECT * FROM my_player_combat",
    "SELECT * FROM my_combat_batches",
    "SELECT * FROM my_run_summary",
    "SELECT * FROM my_chat",
];

/// An open realm connection.
pub struct RealmLink {
    connection: DbConnection,
    thread: Option<JoinHandle<()>>,
    events: Sender<NetEvent>,
    generation: u64,
}

impl RealmLink {
    /// Opens a connection with `token` and subscribes to the caller's views.
    ///
    /// # Errors
    ///
    /// A connection error if the socket cannot be set up at all; most failures arrive later
    /// as [`LinkSignal::ConnectFailed`].
    pub fn connect(
        endpoints: &RealmEndpoints,
        token: &str,
        events: Sender<NetEvent>,
        worker: Sender<WorkerMessage>,
        generation: u64,
    ) -> Result<Self, RealmError> {
        let uri: spacetimedb_sdk::__codegen::http::Uri =
            endpoints.spacetimedb_url.parse().map_err(|_| {
                RealmError::new(ErrorCategory::Connection, "The realm address is invalid.")
            })?;
        let on_connect = worker.clone();
        let on_error = worker.clone();
        let on_disconnect = worker.clone();
        let connection = DbConnection::builder()
            .with_uri(uri)
            .with_database_name(endpoints.spacetimedb_database.clone())
            .with_token(Some(token.to_owned()))
            .on_connect(move |_, _identity, _token| {
                let _ = on_connect.send(WorkerMessage::Link(generation, LinkSignal::Connected));
            })
            .on_connect_error(move |_, error| {
                let _ = on_error.send(WorkerMessage::Link(
                    generation,
                    LinkSignal::ConnectFailed(error.to_string()),
                ));
            })
            .on_disconnect(move |_, error| {
                let _ = on_disconnect.send(WorkerMessage::Link(
                    generation,
                    LinkSignal::Disconnected(error.map(|error| error.to_string())),
                ));
            })
            .build()
            .map_err(|error| {
                log::warn!("realm: connect failed: {error}");
                RealmError::new(
                    ErrorCategory::Connection,
                    "Cannot reach the game server. Retrying…",
                )
            })?;

        register_callbacks(&connection, &events);
        let applied_events = events.clone();
        let applied_worker = worker.clone();
        let failed_events = events.clone();
        connection
            .subscription_builder()
            .on_applied(move |ctx| {
                let _ = applied_events.send(NetEvent::Realm(RealmUpdate::Reset(Box::new(
                    snapshot(&ctx.db),
                ))));
                let _ =
                    applied_worker.send(WorkerMessage::Link(generation, LinkSignal::Subscribed));
            })
            .on_error(move |_, error| {
                log::error!("realm: subscription failed: {error}");
                let _ = failed_events.send(NetEvent::Failed {
                    action: Action::Connect,
                    error: RealmError::protocol("The game server rejected the session."),
                });
            })
            .subscribe(QUERIES);

        let thread = connection.run_threaded();
        Ok(Self {
            connection,
            thread: Some(thread),
            events,
            generation,
        })
    }

    /// Asks the server for its clock.
    pub fn request_time(&self, worker: Sender<WorkerMessage>) {
        let generation = self.generation;
        let sent = crate::clock::local_now();
        self.connection
            .procedures
            .server_time_then(move |_, result| {
                if let Ok(server) = result {
                    let _ = worker.send(WorkerMessage::Link(
                        generation,
                        LinkSignal::ServerTime {
                            sent,
                            received: crate::clock::local_now(),
                            server,
                        },
                    ));
                }
            });
    }

    /// Sends movement input.
    pub fn send_input(&self, frames: &[InputFrame]) {
        let frames: Vec<b::NetInputFrame> = frames
            .iter()
            .map(|frame| b::NetInputFrame {
                seq: frame.seq,
                forward: frame.forward,
                right: frame.right,
                yaw: frame.yaw,
                face_camera: frame.face_camera,
                jump: frame.jump,
            })
            .collect();
        let result = self
            .connection
            .reducers
            .move_input_then(frames, |_, outcome| {
                if let Ok(Err(message)) = outcome {
                    log::debug!("realm: input refused: {message}");
                }
            });
        if let Err(error) = result {
            log::debug!("realm: input not sent: {error}");
        }
    }

    /// Presses a combat button.
    pub fn act(&self, action: &CombatAction) {
        let target = net_entity(&action.target);
        let aimed = |p: [f32; 3]| b::V3 {
            x: p[0],
            y: p[1],
            z: p[2],
        };
        let kind = match action.kind {
            ActionKind::Target => b::NetAction::Target,
            ActionKind::Ability(slot) => b::NetAction::Ability(slot),
            ActionKind::AbilityAt(slot, p) => b::NetAction::AbilityAt(b::NetAimed {
                slot,
                position: aimed(p),
            }),
            ActionKind::Ultimate(p) => b::NetAction::Ultimate(aimed(p)),
            ActionKind::Extra(index) => b::NetAction::Extra(index),
            ActionKind::OpenChest => b::NetAction::OpenChest,
            ActionKind::ChannelAim(p) => b::NetAction::ChannelAim(aimed(p)),
        };
        let result = self
            .connection
            .reducers
            .combat_act_then(target, kind, |_, outcome| {
                if let Ok(Err(message)) = outcome {
                    log::debug!("realm: action refused: {message}");
                }
            });
        if let Err(error) = result {
            log::debug!("realm: action not sent: {error}");
        }
    }

    /// Leaves the queue.
    pub fn queue_leave(&self) {
        let events = self.events.clone();
        let sent = self
            .connection
            .reducers
            .queue_leave_then(move |_, outcome| report(&events, Action::LeaveQueue, outcome));
        self.report_send(Action::LeaveQueue, sent);
    }

    /// Answers the ready check.
    pub fn ready_check_respond(&self, accept: bool) {
        let events = self.events.clone();
        let sent = self
            .connection
            .reducers
            .ready_check_respond_then(accept, move |_, outcome| {
                report(&events, Action::ReadyCheck, outcome);
            });
        self.report_send(Action::ReadyCheck, sent);
    }

    /// Leaves the party.
    pub fn party_leave(&self) {
        let events = self.events.clone();
        let sent = self
            .connection
            .reducers
            .party_leave_then(move |_, outcome| report(&events, Action::LeaveParty, outcome));
        self.report_send(Action::LeaveParty, sent);
    }

    /// Removes a member.
    pub fn party_kick(&self, account_id: String) {
        let events = self.events.clone();
        let sent = self
            .connection
            .reducers
            .party_kick_then(account_id, move |_, outcome| {
                report(&events, Action::Kick, outcome);
            });
        self.report_send(Action::Kick, sent);
    }

    /// Hands leadership over.
    pub fn party_transfer_leader(&self, account_id: String) {
        let events = self.events.clone();
        let sent =
            self.connection
                .reducers
                .party_transfer_leader_then(account_id, move |_, outcome| {
                    report(&events, Action::TransferLeader, outcome);
                });
        self.report_send(Action::TransferLeader, sent);
    }

    /// Starts the dungeon.
    pub fn party_start(&self) {
        let events = self.events.clone();
        let sent = self
            .connection
            .reducers
            .party_start_then(move |_, outcome| report(&events, Action::StartDungeon, outcome));
        self.report_send(Action::StartDungeon, sent);
    }

    /// Inserts the caller's own key into the party.
    pub fn party_insert_key(&self) {
        let events = self.events.clone();
        let sent = self
            .connection
            .reducers
            .party_insert_key_then(move |_, outcome| report(&events, Action::InsertKey, outcome));
        self.report_send(Action::InsertKey, sent);
    }

    /// Takes the caller's key back out of the party.
    pub fn party_withdraw_key(&self) {
        let events = self.events.clone();
        let sent = self
            .connection
            .reducers
            .party_withdraw_key_then(move |_, outcome| {
                report(&events, Action::WithdrawKey, outcome);
            });
        self.report_send(Action::WithdrawKey, sent);
    }

    /// Says something. The server names the sender; `target` is a whisper's character name.
    pub fn send_chat(&self, channel: Channel, target: String, text: String) {
        let events = self.events.clone();
        let sent = self.connection.reducers.chat_send_then(
            channel.as_u8(),
            target,
            text,
            move |_, outcome| report(&events, Action::Chat, outcome),
        );
        self.report_send(Action::Chat, sent);
    }

    /// Dismisses a notice.
    pub fn dismiss_notice(&self, id: u64) {
        let _ = self.connection.reducers.dismiss_notice(id);
    }

    /// Closes the connection and waits for its thread.
    pub fn close(mut self) {
        let _ = self.connection.disconnect();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn report_send(&self, action: Action, sent: spacetimedb_sdk::Result<()>) {
        if let Err(error) = sent {
            log::warn!("realm: {action:?} not sent: {error}");
            let _ = self.events.send(NetEvent::Failed {
                action,
                error: RealmError::new(
                    ErrorCategory::Connection,
                    "Not connected to the game server.",
                ),
            });
        }
    }
}

/// Turns a reducer outcome into a failure event when it failed.
fn report(
    events: &Sender<NetEvent>,
    action: Action,
    outcome: Result<Result<(), String>, spacetimedb_sdk::__codegen::InternalError>,
) {
    let error = match outcome {
        Ok(Ok(())) => return,
        Ok(Err(message)) => RealmError::from_wire(&message),
        Err(error) => {
            log::warn!("realm: {action:?} failed internally: {error}");
            RealmError::new(ErrorCategory::Connection, "The game server did not answer.")
        }
    };
    let _ = events.send(NetEvent::Failed { action, error });
}

/// Wires every view's changes to the event channel.
fn register_callbacks(connection: &DbConnection, events: &Sender<NetEvent>) {
    let db = &connection.db;

    macro_rules! whole_view {
        ($table:ident, $make:expr) => {{
            let tx = events.clone();
            db.$table().on_insert(move |ctx, _| {
                let _ = tx.send(NetEvent::Realm($make(&ctx.db)));
            });
            let tx = events.clone();
            db.$table().on_delete(move |ctx, _| {
                let _ = tx.send(NetEvent::Realm($make(&ctx.db)));
            });
        }};
    }
    whole_view!(my_account, |db: &b::RemoteTables| RealmUpdate::Account(
        db.my_account().iter().next().map(account)
    ));
    whole_view!(my_ticket, |db: &b::RemoteTables| RealmUpdate::Ticket(
        db.my_ticket().iter().next().map(ticket)
    ));
    whole_view!(my_party, |db: &b::RemoteTables| RealmUpdate::Party(
        party_of(db)
    ));
    whole_view!(my_instance, |db: &b::RemoteTables| RealmUpdate::Instance(
        db.my_instance().iter().next().map(instance)
    ));
    whole_view!(my_party_members, |db: &b::RemoteTables| {
        RealmUpdate::Members(db.my_party_members().iter().map(member).collect())
    });
    whole_view!(queue_population, |db: &b::RemoteTables| {
        RealmUpdate::Population(db.queue_population().iter().map(population).collect())
    });
    {
        let tx = events.clone();
        db.my_party_members().on_update(move |ctx, _, _| {
            let _ = tx.send(NetEvent::Realm(RealmUpdate::Members(
                ctx.db.my_party_members().iter().map(member).collect(),
            )));
        });
        let tx = events.clone();
        db.queue_population().on_update(move |ctx, _, _| {
            let _ = tx.send(NetEvent::Realm(RealmUpdate::Population(
                ctx.db.queue_population().iter().map(population).collect(),
            )));
        });
    }

    {
        let tx = events.clone();
        db.my_run_summary().on_insert(move |ctx, _| {
            let _ = tx.send(NetEvent::Realm(run_summary_update(&ctx.db)));
        });
        let tx = events.clone();
        db.my_run_summary().on_update(move |ctx, _, _| {
            let _ = tx.send(NetEvent::Realm(run_summary_update(&ctx.db)));
        });
        let tx = events.clone();
        db.my_run_summary().on_delete(move |ctx, _| {
            let _ = tx.send(NetEvent::Realm(run_summary_update(&ctx.db)));
        });
    }

    let tx = events.clone();
    db.my_instance_players().on_insert(move |_, row| {
        let _ = tx.send(NetEvent::Realm(RealmUpdate::PlayerUpsert(player(row))));
    });
    let tx = events.clone();
    db.my_instance_players().on_update(move |_, _, row| {
        let _ = tx.send(NetEvent::Realm(RealmUpdate::PlayerUpsert(player(row))));
    });
    let tx = events.clone();
    db.my_instance_players().on_delete(move |_, row| {
        let _ = tx.send(NetEvent::Realm(RealmUpdate::PlayerRemoved(
            row.account_id.clone(),
        )));
    });

    let tx = events.clone();
    db.my_instance_monsters().on_insert(move |_, row| {
        let _ = tx.send(NetEvent::Realm(RealmUpdate::MonsterUpsert(monster(row))));
    });
    let tx = events.clone();
    db.my_instance_monsters().on_update(move |_, _, row| {
        let _ = tx.send(NetEvent::Realm(RealmUpdate::MonsterUpsert(monster(row))));
    });
    let tx = events.clone();
    db.my_instance_monsters().on_delete(move |_, row| {
        let _ = tx.send(NetEvent::Realm(RealmUpdate::MonsterRemoved(row.key)));
    });

    let tx = events.clone();
    db.my_player_combat().on_insert(move |_, row| {
        let _ = tx.send(NetEvent::Realm(RealmUpdate::CombatUpsert(player_combat(
            row,
        ))));
    });
    let tx = events.clone();
    db.my_player_combat().on_update(move |_, _, row| {
        let _ = tx.send(NetEvent::Realm(RealmUpdate::CombatUpsert(player_combat(
            row,
        ))));
    });
    let tx = events.clone();
    db.my_player_combat().on_delete(move |_, row| {
        let _ = tx.send(NetEvent::Realm(RealmUpdate::CombatRemoved(
            row.account_id.clone(),
        )));
    });

    // Each batch once, as it is inserted. Batches already there when the subscription
    // applies are history and are not replayed.
    let tx = events.clone();
    db.my_combat_batches().on_insert(move |ctx, row| {
        if matches!(ctx.event, spacetimedb_sdk::Event::SubscribeApplied) {
            return;
        }
        match serde_json::from_str::<Vec<WireEvent>>(&row.events) {
            Ok(batch) => {
                let _ = tx.send(NetEvent::Realm(RealmUpdate::CombatEvents(batch)));
            }
            Err(error) => log::warn!("realm: unreadable combat batch {}: {error}", row.id),
        }
    });

    let tx = events.clone();
    db.my_chat().on_insert(move |_, row| {
        if let Some(message) = chat_message(row) {
            let _ = tx.send(NetEvent::Realm(RealmUpdate::Chat(message)));
        }
    });

    let tx = events.clone();
    db.my_notices().on_insert(move |_, row| {
        let _ = tx.send(NetEvent::Realm(RealmUpdate::NoticeAdded(notice(row))));
    });
    let tx = events.clone();
    db.my_notices().on_delete(move |_, row| {
        let _ = tx.send(NetEvent::Realm(RealmUpdate::NoticeRemoved(row.id)));
    });
}

/// Everything currently in the cache, as a fresh mirror.
fn snapshot(db: &b::RemoteTables) -> RealmMirror {
    RealmMirror {
        account: db.my_account().iter().next().map(account),
        ticket: db.my_ticket().iter().next().map(ticket),
        party: party_of(db),
        members: {
            let mut members: Vec<MemberInfo> = db.my_party_members().iter().map(member).collect();
            members.sort_by_key(|member| member.slot);
            members
        },
        instance: db.my_instance().iter().next().map(instance),
        players: db
            .my_instance_players()
            .iter()
            .map(|row| (row.account_id.clone(), player(&row)))
            .collect(),
        monsters: db
            .my_instance_monsters()
            .iter()
            .map(|row| (row.key, monster(&row)))
            .collect(),
        combat: db
            .my_player_combat()
            .iter()
            .map(|row| (row.account_id.clone(), player_combat(&row)))
            .collect(),
        events: Vec::new(),
        notices: {
            let mut notices: Vec<Notice> = db.my_notices().iter().map(|row| notice(&row)).collect();
            notices.sort_by_key(|notice| notice.id);
            notices
        },
        population: db.queue_population().iter().map(population).collect(),
        chat: {
            let mut lines: Vec<ChatMessageInfo> = db
                .my_chat()
                .iter()
                .filter_map(|row| chat_message(&row))
                .collect();
            lines.sort_by_key(|line| line.id);
            let excess = lines.len().saturating_sub(crate::model::CHAT_HISTORY);
            lines.drain(..excess);
            lines
        },
        run_summary: latest_run_summary(db),
        revision: 0,
    }
}

fn micros(timestamp: spacetimedb_sdk::Timestamp) -> i64 {
    timestamp.to_micros_since_unix_epoch()
}

fn v3(v: &b::V3) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

fn role(tag: b::RoleTag) -> Role {
    match tag {
        b::RoleTag::Tank => Role::Tank,
        b::RoleTag::Healer => Role::Healer,
        b::RoleTag::Dps => Role::Dps,
    }
}

fn account(row: b::Account) -> AccountInfo {
    AccountInfo {
        account_id: row.account_id,
        display_name: row.display_name,
    }
}

fn ticket(row: b::QueueTicket) -> TicketInfo {
    TicketInfo {
        dungeon_id: row.dungeon_id,
        role: role(row.role),
        character_name: row.character_name,
        gear_score: row.gear_score,
        enqueued_at: micros(row.enqueued_at),
        matched: matches!(row.status, b::TicketStatus::Matched),
    }
}

/// The party, with its leader translated from identity to account id via the member list.
fn party_of(db: &b::RemoteTables) -> Option<PartyInfo> {
    let row = db.my_party().iter().next()?;
    let account_of = |identity| {
        db.my_party_members()
            .iter()
            .find(|member| member.identity == identity)
            .map(|member| member.account_id)
    };
    let leader = row.leader.and_then(account_of);
    let key_owner = row.key_owner.and_then(account_of);
    Some(PartyInfo {
        id: row.id,
        dungeon_id: row.dungeon_id,
        state: match row.state {
            b::PartyStateTag::ReadyCheck => PartyState::ReadyCheck,
            b::PartyStateTag::ChoosingKey => PartyState::ChoosingKey,
            b::PartyStateTag::Ready => PartyState::Ready,
            b::PartyStateTag::Starting => PartyState::Starting,
            b::PartyStateTag::InDungeon => PartyState::InDungeon,
            b::PartyStateTag::Completed => PartyState::Completed,
            b::PartyStateTag::Failed => PartyState::Failed,
            b::PartyStateTag::Disbanded => PartyState::Disbanded,
        },
        leader,
        key_owner,
        key_level: row.key_level,
        ready_deadline: micros(row.ready_deadline),
        instance_id: row.instance_id,
    })
}

fn member(row: b::PartyMember) -> MemberInfo {
    MemberInfo {
        account_id: row.account_id,
        display_name: row.display_name,
        character_name: row.character_name,
        definition_id: row.definition_id,
        role: role(row.role),
        level: row.level,
        gear_score: row.gear_score,
        key_dungeon_id: row.key_dungeon_id,
        key_level: row.key_level,
        ready: match row.ready {
            b::ReadyTag::Pending => ReadyResponse::Pending,
            b::ReadyTag::Accepted => ReadyResponse::Accepted,
            b::ReadyTag::Declined => ReadyResponse::Declined,
        },
        present: matches!(row.presence, b::PresenceTag::Present),
        slot: row.slot,
    }
}

/// The newest run summary in the cache, if any.
fn latest_run_summary(db: &b::RemoteTables) -> Option<RunSummaryInfo> {
    db.my_run_summary()
        .iter()
        .max_by_key(|row| micros(row.created_at))
        .map(|row| run_summary(&row))
}

fn run_summary_update(db: &b::RemoteTables) -> RealmUpdate {
    RealmUpdate::RunSummary(latest_run_summary(db))
}

fn run_summary(row: &b::RunSummary) -> RunSummaryInfo {
    RunSummaryInfo {
        instance_id: row.instance_id,
        dungeon_id: row.dungeon_id.clone(),
        dungeon_name: row.dungeon_name.clone(),
        success: row.success,
        key_level: row.key_level,
        key_owner: row.key_owner,
        key_level_after: row.key_level_after,
        duration_us: row.duration_us,
        experience: row.experience,
        resources: serde_json::from_str(&row.resources).unwrap_or_else(|error| {
            log::warn!("realm: unreadable run resources: {error}");
            Vec::new()
        }),
        loot_item: row.loot_item.clone(),
    }
}

fn instance(row: b::DungeonInstance) -> InstanceInfo {
    InstanceInfo {
        id: row.id,
        dungeon_id: row.dungeon_id,
        state: match row.state {
            b::InstanceStateTag::Created => InstanceState::Created,
            b::InstanceStateTag::Loading => InstanceState::Loading,
            b::InstanceStateTag::Ready => InstanceState::Ready,
            b::InstanceStateTag::Countdown => InstanceState::Countdown,
            b::InstanceStateTag::Active => InstanceState::Active,
            b::InstanceStateTag::Completed => InstanceState::Completed,
            b::InstanceStateTag::Failed => InstanceState::Failed,
            b::InstanceStateTag::CleaningUp => InstanceState::CleaningUp,
            b::InstanceStateTag::Destroyed => InstanceState::Destroyed,
        },
        countdown_ends_at: row.countdown_ends_at.map(micros),
        barrier_open: row.barrier_open,
    }
}

fn player(row: &b::InstancePlayer) -> RemotePlayerState {
    RemotePlayerState {
        account_id: row.account_id.clone(),
        name: row.name.clone(),
        definition_id: row.definition_id.clone(),
        role: role(row.role),
        position: v3(&row.position),
        velocity: v3(&row.velocity),
        ground_normal: v3(&row.ground_normal),
        yaw: row.yaw,
        grounded: row.grounded,
        last_seq: row.last_seq,
        health: row.health,
        max_health: row.max_health,
        present: matches!(row.presence, b::PresenceTag::Present),
    }
}

fn net_entity(reference: &EntityRef) -> b::NetEntity {
    match reference {
        EntityRef::None => b::NetEntity::Nobody,
        EntityRef::Player(account) => b::NetEntity::Player(account.clone()),
        EntityRef::Monster(key) => b::NetEntity::Monster(*key),
    }
}

fn entity_ref(net: &b::NetEntity) -> EntityRef {
    match net {
        b::NetEntity::Nobody => EntityRef::None,
        b::NetEntity::Player(account) => EntityRef::Player(account.clone()),
        b::NetEntity::Monster(key) => EntityRef::Monster(*key),
    }
}

fn cast(net: &b::NetCast) -> CastState {
    CastState {
        ability: match net.ability {
            b::NetAbility::Nothing => AbilityRef::None,
            b::NetAbility::Player(index) => AbilityRef::Player(index),
            b::NetAbility::Monster(index) => AbilityRef::Monster(index),
        },
        elapsed: net.elapsed,
        duration: net.duration,
    }
}

fn statuses(rows: &[b::NetStatus]) -> Vec<StatusState> {
    rows.iter()
        .map(|status| StatusState {
            kind: status.kind,
            source: entity_ref(&status.source),
            remaining: status.remaining,
            duration: status.duration,
            stacks: status.stacks,
        })
        .collect()
}

fn player_combat(row: &b::PlayerCombat) -> PlayerCombatInfo {
    PlayerCombatInfo {
        account_id: row.account_id.clone(),
        alive: row.alive,
        target: entity_ref(&row.target),
        cast: row.cast.as_ref().map(cast),
        resources: Resources {
            heat: row.resources.heat,
            energy: row.resources.energy,
            mana: row.resources.mana,
            max_mana: row.resources.max_mana,
            valor: row.resources.valor,
            astral: row.resources.astral,
            global_cooldown: row.resources.global_cooldown,
            absorb: row.resources.absorb,
        },
        cooldowns: row
            .cooldowns
            .iter()
            .map(|c| CooldownState {
                ability: c.ability,
                charges: c.charges,
                recharge: c.recharge,
            })
            .collect(),
        statuses: statuses(&row.statuses),
        received: std::time::Instant::now(),
    }
}

fn monster(row: &b::InstanceMonster) -> MonsterInfo {
    MonsterInfo {
        key: row.key,
        id: row.id,
        archetype: row.archetype.clone(),
        name: row.name.clone(),
        position: v3(&row.position),
        yaw: row.yaw,
        scale: row.scale,
        level: row.level,
        health: row.health,
        max_health: row.max_health,
        alive: row.alive,
        target: entity_ref(&row.target),
        cast: row.cast.as_ref().map(cast),
        statuses: statuses(&row.statuses),
        received: std::time::Instant::now(),
    }
}

fn notice(row: &b::PlayerNotice) -> Notice {
    Notice {
        id: row.id,
        code: row.code.clone(),
        level: match row.level {
            b::NoticeLevel::Info => NoticeLevel::Info,
            b::NoticeLevel::Warning => NoticeLevel::Warning,
            b::NoticeLevel::Error => NoticeLevel::Error,
        },
        message: row.message.clone(),
    }
}

/// A chat row, or `None` for a channel this client does not know.
fn chat_message(row: &b::ChatLine) -> Option<ChatMessageInfo> {
    Some(ChatMessageInfo {
        id: row.id,
        channel: Channel::from_u8(row.channel)?,
        sender_account: row.sender_account.clone(),
        sender_name: row.sender_name.clone(),
        recipient_account: row.recipient_account.clone(),
        recipient_name: row.recipient_name.clone(),
        text: row.text.clone(),
        sent_at: micros(row.sent_at),
    })
}

fn population(row: b::QueuePopulation) -> (String, Population) {
    (
        row.dungeon_id,
        Population {
            tanks: row.tanks,
            healers: row.healers,
            dps: row.dps,
        },
    )
}
