# Reconnection

A lost connection costs the player nothing they had: not their queue place within the grace,
not their party, not their instance, not their position. Nobody replaces them — not another
player, not an NPC — and the dungeon does not fail because one player dropped.

## States

| Client connection (`ConnectionStatus`) | Realm presence (`PresenceTag`) |
| --- | --- |
| Offline · Connecting · Connected · Reconnecting { attempt, retry_in } | Present · TemporarilyDisconnected |

## Server side

`client_disconnected` runs when the account's **last** connection closes (an account may have
several; the realm counts them). It never removes anyone:

| Where the player is | What happens |
| --- | --- |
| Queue | ticket kept but ineligible; removed after `queue_disconnect_grace` (60 s) |
| Ready check | cancelled at once — a disconnected player cannot confirm, and is never counted as accepting |
| Party (Ready) / instance | presence → TemporarilyDisconnected; the run continues; the party waits; after `reconnect_grace` (300 s) the janitor applies a normal departure |

While a player is away their character stays in the fight: monsters can still attack it, and
it can die and respawn. Coming back, the client re-subscribes and receives the current
`player_combat`, monster rows and health; event batches that already happened are not
replayed.

Graces are tunable live: `admin_set_timeouts <ready_check_s> <reconnect_grace_s> <queue_grace_s>`.

`client_connected` with the same Identity (same account, any machine) marks the account
connected, the party member and instance player Present, resets the movement budget (time away
does not bank frames) and leaves a "Reconnected" notice.

## Client side (`engine-net::session`)

```text
link closed ─► Reconnecting{attempt} ─► wait 0.5, 1, 2, 4, 8, 15, 15… s
           ─► refresh the access token if within two minutes of expiry
           ─► new link (new generation) ─► subscribe ─► Reset(mirror) ─► Connected
```

* **Link generations:** every connection attempt gets a number; signals from an older link (a
  late "disconnected" from the socket being replaced) are ignored, so a reconnect cannot be
  torn down by its predecessor.
* **State restore is a subscription:** the views describe where the player is, so the
  front-end lands on the right screen by construction — queue, ready check, party or the
  dungeon — and the instance is re-entered at the server's position for the player with
  prediction restarted from the server's last acknowledged input.
* The player sees "Connection lost — Reconnecting…" over the game; the party sees their
  frame faded and "Disconnected".
* If the refresh token is refused (expired, revoked, reused), the session ends with "Your
  session has expired. Please sign in again." A network failure keeps the session and retries.

## The party's side

While a party is `Starting` or `InDungeon`, every connected member sees, for each other member
who is `TemporarilyDisconnected`, a panel over the world:

```text
┌ Mirelle has disconnected.                 4:12 ┐
│ Their place is held until the timer ends.      │
│ Wait for them, or leave and return to the hub. │
│        [ Wait ]        [ Leave Party ]         │
└────────────────────────────────────────────────┘
```

* The countdown is the server's: `party_member.disconnected_at + reconnect_grace_us`, both
  written when the member dropped (the grace is the realm's value at that moment, so
  `admin_set_timeouts` is honoured). Nothing counts on the client's own clock. The rules are
  `engine_realm::reconnect` (`prompt_for`, `grace_seconds_left`).
* **Wait** hides the panel for that drop. The drop is keyed by account and `disconnected_at`, so
  the same member dropping again, or another member dropping, shows it again. The panel also
  disappears by itself when the grace ends.
* **Leave Party** is the ordinary `party_leave`. Mid-run it is the "continue without" departure:
  the leaver's party membership and instance player are deleted, they are back at the hub, and
  the others carry on. What it touches: currency they spent in the run (a key reroll) is settled
  as a result with no rewards, so it is debited exactly once; their key is not changed (a key
  moves only with a finished run's result); chest items they never opened and loot they picked up
  are mailed when the instance is torn down, as for any abandoned run. It never fails the run for
  the others.
* When the member returns, `client_connected` marks them present, so the panel closes for
  everyone, and the other members get a "<Name> reconnected." notice (a toast).
* A player who drops and would rather not go back simply leaves on reconnecting: the in-world
  *Leave Party* button, or the one on this panel, sends the same `party_leave`. If they never
  return, the janitor applies the same departure after the grace.

The server's character body of a member who left stays in the fight (there is no removal in the
simulation yet); it is no longer controlled by anyone.

## Locked run configuration

The run's talents and equipment are the snapshot taken at queue time (`party_member.loadout`,
`.talents`), and the `PlayerSpec` of the fight is built from it; a reconnecting client is given
the live rows, never a local rebuild. The API refuses equipment moves, taking mail, salvage,
sharpening, key downgrade, key cleanse and talent changes while `svc_engagement` says the
account is queued, in a party or in an instance (exchange and marking mail read touch only
resources and flags, and stay open).

On the client the same lock shows before asking:

* the character sheet is read-only with a **Locked for this run** banner (no drag, no
  right-click move);
* the talent window shows its lock line and emits nothing;
* salvage, sharpen, key downgrade and cleanse actions are dropped with the API's sentence as a
  toast, and the hub's key buttons are already hidden while engaged.

## Tests

`realm-bot scenario reconnect_party` (see above) covers the party prompt's data, Leave, the untouched key and currency, the locked configuration and the API refusals. `realm-bot scenario reconnect`: a player drops mid-run; the others see them disconnected, the
party keeps three members, the run stays Active; a new client for the same account resumes in
the same instance, with the same account, at the same authoritative position, and the party
sees them present again. `decline` covers the ready-check case; unit tests cover the back-off,
the mirror reset and the janitor rules.

Not scripted: a reconnect while the boss encounter is engaged. The realm's admin surface has no
way to walk a party to the boss (`admin_finish_instance` ends a run, it does not engage it).
The path is the same as in combat generally - presence is a column, the fight keeps ticking -
and it is checked by hand.
