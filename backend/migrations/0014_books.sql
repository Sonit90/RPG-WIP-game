-- Books (docs/books.md): what a character has learned from book items.
--
-- { "unlocked": [book ids], "active": { enhancement book id: option id } }
--
-- Talent books add one talent point each (counted by `engine_realm::books::BookState::point_bonus`);
-- an enhancement book's active option is a hidden talent node the run snapshot carries. The default is
-- the empty object, which every reader treats as "nothing learned", so existing characters need no
-- back-fill.
ALTER TABLE game.characters
    ADD COLUMN books JSONB NOT NULL DEFAULT '{}'::jsonb;
