-- Boss chest loot, delivered once per player per dungeon run.
--
-- The realm reports a delivery under an idempotency key ("<instance>:<account>:loot"); a
-- delivery seen twice (the bridge crashed between persisting and acknowledging) is applied
-- once, because the key is recorded in the same transaction that changes the character.
-- `outcome` says where the item went: 'bag', 'mail', 'mail_bag_full' (wanted the bag, had no
-- room), or, when nothing could be given, 'no_character' / 'unknown_item' so support can find
-- and re-grant what a player earned. Mail itself lives in game.characters.loadout ("mail").
CREATE TABLE game.loot_deliveries (
    loot_id      TEXT PRIMARY KEY,
    account_id   UUID NOT NULL,
    character_id UUID,
    item         TEXT NOT NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('bag', 'mail')),
    reason       TEXT NOT NULL DEFAULT '',
    outcome      TEXT NOT NULL,
    applied_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX loot_deliveries_character_idx ON game.loot_deliveries (character_id);
