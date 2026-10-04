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

## Tests

`realm-bot scenario reconnect`: a player drops mid-run; the others see them disconnected, the
party keeps three members, the run stays Active; a new client for the same account resumes in
the same instance, with the same account, at the same authoritative position, and the party
sees them present again. `decline` covers the ready-check case; unit tests cover the back-off,
the mirror reset and the janitor rules.
