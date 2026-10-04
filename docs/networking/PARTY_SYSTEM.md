# Parties and the ready check

## State machine (`engine_realm::party::PartyState`)

```text
            all accept                 leader starts            countdown ends
ReadyCheck ──────────▶ Ready ─────────────────▶ Starting ────────────────▶ InDungeon
    │                    │                         │                         │
    │ decline/timeout/   │ member leaves/kicked    │                         │ success / failure
    │ disconnect         │ before the start        │                         ▼
    ▼                    ▼                         │                Completed / Failed
Disbanded ◀──────────────┴─────────────────────────┴──────── everyone gone ──┘
```

One enum; every transition goes through `PartyState::can_become`, which the module checks
before writing. There is no flag soup.

## Ready check

* Formed by the matchmaker with every member `Pending` and a deadline
  (`ready_check_timeout`, 30 s default). Every client gets a "Dungeon found" notice and the
  ready-check screen.
* `ready_check_respond(accept)`: answering twice with the same answer is harmless; changing
  an answer is refused.
* **All accept** → `Ready`, leader picked with the realm's own RNG (`ctx.random()`), the
  members' queue tickets removed.
* **Any decline or disconnect** → cancelled at once. Members who had accepted or not yet
  answered go back to searching with their **original** queue time; the decliner and the
  disconnected leave the queue. A disconnected member is never counted as accepting.
* **Deadline** (`ready_check_expire`, a scheduled reducer) → non-responders leave the queue,
  the rest are re-queued.

## Who may do what (`engine_realm::party::authorize`)

| Action | Who | When |
| --- | --- | --- |
| Answer the ready check | any member | `ReadyCheck` |
| Start | leader | `Ready`, full 1/1/1 roster, everyone accepted and connected |
| Kick | leader, not themselves | `Ready`, `InDungeon` |
| Transfer leadership | leader, to a member | `Ready`, `Starting`, `InDungeon` |
| Leave | any member | any state but `ReadyCheck` (decline instead) |

Every reducer builds a plain `PartyView` and asks `authorize`; the answer is the whole
permission model. Non-members get `Authorization` errors; leader-only actions from others get
`Authorization` errors that the client shows as a sentence.

## Departures (`departure_effect`)

| When | Effect |
| --- | --- |
| Before the run (`ReadyCheck`, `Ready`) | Party disbands; the others are re-queued in place. No partial party can start, and roles are never replaced. |
| During the run | The member is removed; the run continues with the rest. If they led, a **random remaining member** leads (connected members preferred). |
| Last member | Party and instance are torn down. |

A member who disconnects is not a departure: see [RECONNECTION.md](RECONNECTION.md). After the
reconnect grace expires, the janitor applies the departure above.

## Client

The party screen lists role, character, player, level, gear score, connection and leader. The
leader sees **Make Leader**, **Kick** and **Start Dungeon**; everyone sees **Leave Party**. None
of those buttons is drawn for someone who may not use it, and the server refuses them anyway.

## Tests

`cargo test -p engine-realm party`: transitions, non-members refused, non-leaders refused
(start, kick, transfer), leader allowed, invalid kick/transfer (self, stranger, wrong state),
start validation (disconnected, incomplete, double start), all-confirm, wait-then-timeout,
decline cancels, disconnect cancels and never accepts, duplicate confirmations, random leader
reaches every member, successor prefers the connected, departures. End to end:
`realm-bot scenario decline`, `security` (non-leader start/kick/transfer, kicking a stranger,
answering a closed check).
