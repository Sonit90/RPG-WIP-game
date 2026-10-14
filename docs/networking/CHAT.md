# Chat

Server-authoritative text chat with three channels. The client never says who is speaking.

| Channel | Heard by |
| --- | --- |
| Server | every connected player |
| Party | members of the sender's party |
| Whisper | one character, found by name, and the sender |

## Flow

```text
client: chat_send(channel, target, text)       target = a whisper's character name, else ""
module: rate limit (burst 5, 1/s) -> account -> speaker -> sanitise -> channel rule -> chat_message
client: my_chat view -> RealmUpdate::Chat -> RealmMirror.chat -> chat panel
```

* **Identity.** The sender is `ctx.sender()`. The name on the line is the character the realm
  already knows for that connection: the party member's character, else the queue ticket's
  (`engine_realm::chat::speaker_name`). A player with neither cannot chat ("Choose a character
  and join the queue to chat."). There is no name, party or sender argument to spoof.
* **Text.** Control characters are stripped, the ends trimmed, an empty result refused, more
  than 200 characters refused (`engine_realm::chat::sanitize`).
* **Party.** Refused outside a party. The line stores the sender's party id.
* **Whisper.** The target is matched case-insensitively against the character names of
  *connected* accounts. No match, or the sender's own character, leaves a `whisper_failed`
  notice for the sender ("No player named X is online.") and stores nothing.
* **Visibility.** `chat_message` is private. The `my_chat` view returns Server lines, the
  caller's party's lines and whispers the caller sent or received
  (`engine_realm::chat::visible_to`, applied again after the index lookups). Its rows carry
  account ids and names, never identities.
* **History.** Every `chat_send` and every janitor pass deletes lines older than 10 minutes and
  Server lines beyond the newest 200. The client keeps the last 200 it has seen.

## Client

`ui/chat.rs` (engine-presentation) draws the panel bottom-left (lifted above the hub's queue
panel) and parses input. Enter opens the line, Enter sends, Escape cancels. While it is open
`captures_keyboard()` is true and `engine-app` ignores every key as play (movement, abilities,
window toggles, Escape for the pause menu).

| Input | Does |
| --- | --- |
| `text` | default channel: Party while in a party, else Server |
| `/s text` | Server |
| `/p text` | Party |
| `/w Name text` | whisper to `Name` |
| `/r text` | reply to whoever whispered last |

Lines print as `[Server] Name: text`, `[Party] Name: text`, `To Name: text`, `From Name: text`,
coloured white, blue and pink.

## Testing

`cargo test -p engine-realm` (rules), `-p engine-net` (mirror), `-p engine-presentation`
(panel, slash commands). Against a running realm: `realm-bot scenario chat`.
