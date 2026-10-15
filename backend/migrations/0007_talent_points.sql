-- Talent points (docs/networking/PERSISTENCE.md): a character starts with the points the
-- talent catalog says and earns more from successful runs. Only the *earned* part is stored;
-- the starting points are catalog data, so changing them reaches every character.
--
-- Numbered 0007 to leave room for a parallel branch's 0006; renumber at merge if needed.

ALTER TABLE game.characters
    ADD COLUMN talent_points_earned INT NOT NULL DEFAULT 0 CHECK (talent_points_earned >= 0);

-- What each applied result granted, next to the experience it paid.
ALTER TABLE game.dungeon_results
    ADD COLUMN talent_points INT NOT NULL DEFAULT 0 CHECK (talent_points >= 0);
