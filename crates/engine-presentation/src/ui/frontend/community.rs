//! The **Community maps** page: browse published maps, see one in detail and play it, and manage
//! your own uploads (validation report, publish, unpublish, upload a level file).
//!
//! Like the record pages, this is plain data built by the application from what the API
//! answered; the page keeps no network state. The two text fields (title search, file path)
//! belong to [`super::FrontEndUi`]; their text leaves as a [`CommunityAction`].

use engine_render_data::font::UiFont;
use engine_render_data::ui::{TextAlign, UiRect, UiScale};

use super::{
    BAD, ButtonKind, DIM, FAINT, FIELD, GOLD, GOOD, Hit, Layout, Painter, SELECTED, TEXT, size,
};

/// Maps shown per page of the list.
pub const MAP_ROWS: usize = 5;
/// Issue lines the detail lists before "and N more".
const ISSUE_LINES: usize = 5;

/// One map in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapRowCard {
    /// Title.
    pub title: String,
    /// Author's name.
    pub author: String,
    /// The author's claimed difficulty, 1 to 5.
    pub difficulty: u8,
    /// Runs played.
    pub plays: i64,
    /// "Draft", "Published", ... (only meaningful on My maps).
    pub status: String,
    /// Whether the latest version validates.
    pub valid: bool,
}

/// One validator finding, with a readable message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueLine {
    /// The stable code, such as `unknown_monster`.
    pub code: String,
    /// "Unknown monster: scene.monsters[3].monster - 'x' is not an enemy."
    pub text: String,
    /// An error (refuses the map) rather than a warning.
    pub error: bool,
}

/// The selected map in full.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MapDetailCard {
    /// Title.
    pub title: String,
    /// Author's name.
    pub author: String,
    /// Description.
    pub description: String,
    /// "Difficulty 3 of 5 | v2 | Published".
    pub facts: String,
    /// "1,240 plays | 310 clears | fastest 04:12".
    pub stats: String,
    /// Tags.
    pub tags: Vec<String>,
    /// The latest report's findings.
    pub issues: Vec<IssueLine>,
    /// Best completed times, one line each.
    pub best_times: Vec<String>,
    /// It can be played now (published).
    pub can_play: bool,
    /// The author may publish it (a draft whose latest version has no errors).
    pub can_publish: bool,
    /// The author may unpublish it.
    pub can_unpublish: bool,
    /// The caller is the author.
    pub mine: bool,
}

/// The whole page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommunityCard {
    /// "My maps" is up (otherwise Browse).
    pub mine: bool,
    /// The sort's label.
    pub sort: String,
    /// The difficulty filter's label ("Any difficulty", "Difficulty 3").
    pub filter: String,
    /// The page's maps.
    pub rows: Vec<MapRowCard>,
    /// Which is selected.
    pub selected: Option<usize>,
    /// The selected map in full.
    pub detail: Option<MapDetailCard>,
    /// The difficulty a run would be played at, 1 to 10.
    pub play_difficulty: u32,
    /// A request is in flight.
    pub loading: bool,
    /// The page number, from 1.
    pub page: u32,
    /// An earlier page exists.
    pub can_prev: bool,
    /// A later page exists.
    pub can_next: bool,
    /// The last upload's outcome, or another note for the player.
    pub message: Option<String>,
    /// The player can queue (a character is chosen and the realm is connected).
    pub can_queue: bool,
}

/// What the player asked for on this page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommunityAction {
    /// Switch between Browse (`false`) and My maps (`true`).
    Tab(bool),
    /// Select the list's row.
    Select(usize),
    /// Cycle the sort.
    Sort,
    /// Cycle the difficulty filter.
    Filter,
    /// Step the page.
    Page(i32),
    /// Search titles for this text (Enter in the search field).
    Search(String),
    /// Reload the list.
    Refresh,
    /// Step the run difficulty.
    Difficulty(i32),
    /// Play the selected map (continues to role selection).
    Play,
    /// Publish the selected own map.
    Publish,
    /// Unpublish the selected own map.
    Unpublish,
    /// Upload the level file at this path as a new draft.
    Upload(String),
    /// Upload the level file at this path as a new version of the selected map.
    UploadVersion(String),
}

/// A region of this page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CommunityHit {
    Tab(bool),
    Row(usize),
    Sort,
    Filter,
    Page(i32),
    SearchField,
    PathField,
    Refresh,
    Difficulty(i32),
    Play,
    Publish,
    Unpublish,
    Upload,
    UploadVersion,
}

/// Which text field has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommunityField {
    Search,
    Path,
}

// --- layout -------------------------------------------------------------------------------

pub(super) struct Panels {
    pub list: UiRect,
    pub detail: UiRect,
}

pub(super) fn panels(w: f32, h: f32, scale: UiScale) -> Panels {
    let px = |v: f32| scale.px(v);
    let top = px(170.0);
    let height = h - top - px(130.0);
    let total = (w - px(80.0)).min(px(1240.0));
    let left = (w - total) * 0.5;
    Panels {
        list: UiRect::new(left, top, total * 0.50, height),
        detail: UiRect::new(
            left + total * 0.50 + px(20.0),
            top,
            total * 0.50 - px(20.0),
            height,
        ),
    }
}

/// The list panel's controls.
pub(super) struct ListRects {
    pub browse: UiRect,
    pub mine: UiRect,
    pub field: UiRect,
    pub side: UiRect,
    pub left_small: UiRect,
    pub right_small: UiRect,
    pub prev: UiRect,
    pub next: UiRect,
}

pub(super) fn list_rects(list: UiRect, scale: UiScale) -> ListRects {
    let px = |v: f32| scale.px(v);
    let inner = list.width - px(32.0);
    let x = list.x + px(16.0);
    let half = (inner - px(8.0)) * 0.5;
    let row = |y: f32| list.y + px(y);
    let small = px(size::COMPACT_HEIGHT);
    ListRects {
        browse: UiRect::new(x, row(14.0), half, small),
        mine: UiRect::new(x + half + px(8.0), row(14.0), half, small),
        field: UiRect::new(x, row(56.0), inner * 0.62, small),
        side: UiRect::new(
            x + inner * 0.62 + px(8.0),
            row(56.0),
            inner * 0.38 - px(8.0),
            small,
        ),
        left_small: UiRect::new(x, row(96.0), half, small),
        right_small: UiRect::new(x + half + px(8.0), row(96.0), half, small),
        prev: UiRect::new(x, list.bottom() - px(44.0), half, small),
        next: UiRect::new(x + half + px(8.0), list.bottom() - px(44.0), half, small),
    }
}

/// The list's `index`th map row.
pub(super) fn map_row(list: UiRect, index: usize, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(
        list.x + px(16.0),
        list.y + px(138.0) + px(44.0) * index as f32,
        list.width - px(32.0),
        px(40.0),
    )
}

/// The detail panel's action buttons (play stepper or publish).
pub(super) struct DetailRects {
    pub minus: UiRect,
    pub plus: UiRect,
    pub primary: UiRect,
}

pub(super) fn detail_rects(detail: UiRect, scale: UiScale) -> DetailRects {
    let px = |v: f32| scale.px(v);
    let y = detail.bottom() - px(54.0);
    let button = px(size::COMPACT_HEIGHT);
    DetailRects {
        minus: UiRect::new(detail.x + px(16.0), y + px(4.0), button, button),
        plus: UiRect::new(detail.x + px(16.0) + px(96.0), y + px(4.0), button, button),
        primary: UiRect::new(
            detail.right() - px(196.0),
            y,
            px(180.0),
            px(size::SECONDARY_HEIGHT + 6.0),
        ),
    }
}

/// The footer's Back button, centred.
pub(super) fn back_button(w: f32, h: f32, scale: UiScale) -> UiRect {
    let px = |v: f32| scale.px(v);
    UiRect::new(w * 0.5 - px(110.0), h - px(96.0), px(220.0), px(50.0))
}

// --- hit areas ----------------------------------------------------------------------------

fn add(layout: &mut Layout, hit: CommunityHit, rect: UiRect) {
    layout.add(Hit::Community(hit), rect);
}

pub(super) fn hits(layout: &mut Layout, card: &CommunityCard, w: f32, h: f32, s: UiScale) {
    let p = panels(w, h, s);
    layout.add(Hit::Panel, p.detail);
    let l = list_rects(p.list, s);
    add(layout, CommunityHit::Tab(false), l.browse);
    add(layout, CommunityHit::Tab(true), l.mine);
    if card.mine {
        add(layout, CommunityHit::PathField, l.field);
        add(layout, CommunityHit::Upload, l.side);
        add(layout, CommunityHit::Refresh, l.left_small);
        if card.selected.is_some() {
            add(layout, CommunityHit::UploadVersion, l.right_small);
        }
    } else {
        add(layout, CommunityHit::SearchField, l.field);
        add(layout, CommunityHit::Sort, l.side);
        add(layout, CommunityHit::Filter, l.left_small);
        add(layout, CommunityHit::Refresh, l.right_small);
    }
    for index in 0..card.rows.len().min(MAP_ROWS) {
        add(layout, CommunityHit::Row(index), map_row(p.list, index, s));
    }
    if card.can_prev && !card.loading {
        add(layout, CommunityHit::Page(-1), l.prev);
    }
    if card.can_next && !card.loading {
        add(layout, CommunityHit::Page(1), l.next);
    }
    if let Some(detail) = &card.detail {
        let d = detail_rects(p.detail, s);
        if detail.can_play {
            add(layout, CommunityHit::Difficulty(-1), d.minus);
            add(layout, CommunityHit::Difficulty(1), d.plus);
            if card.can_queue {
                add(layout, CommunityHit::Play, d.primary);
            }
        }
        if detail.can_publish {
            add(layout, CommunityHit::Publish, d.primary);
        } else if detail.can_unpublish {
            add(layout, CommunityHit::Unpublish, d.primary);
        }
    }
    layout.add(Hit::Back, back_button(w, h, s));
}

// --- drawing ------------------------------------------------------------------------------

fn fit(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('\u{2026}');
    cut
}

/// `text` broken into lines of at most `width` characters, at spaces.
pub(super) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if !line.is_empty() && line.chars().count() + 1 + word_len > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// A text field's box, with its text (a hint while empty and unfocused).
fn field(
    ctx: &mut Painter<'_>,
    rect: UiRect,
    text: &str,
    hint: &str,
    focused: bool,
    clock: f32,
    before_caret: &str,
) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    ctx.list.panel(rect, px(6.0), FIELD, FIELD);
    ctx.list.panel_stroke(
        rect,
        px(6.0),
        px(if focused { 2.0 } else { 1.0 }),
        if focused {
            GOLD
        } else {
            [0.30, 0.32, 0.38, 1.0]
        },
    );
    let size = 15.0;
    let top = rect.y + (rect.height - UiFont::Regular.line_height(px(size))) * 0.5;
    let (shown, color) = if text.is_empty() && !focused {
        (hint.to_owned(), FAINT)
    } else {
        (text.to_owned(), TEXT)
    };
    ctx.list.label(
        (rect.x + px(10.0), top),
        &fit(&shown, 34),
        color,
        UiFont::Regular,
        px(size),
        TextAlign::Left,
    );
    if focused && (clock * 2.0).fract() < 0.6 {
        let caret_x = rect.x + px(10.0) + UiFont::Regular.width(before_caret, px(size));
        ctx.list.rect(
            UiRect::new(caret_x, rect.y + px(7.0), px(2.0), rect.height - px(14.0)),
            GOLD,
        );
    }
}

/// What the page needs of the two fields to draw them.
pub(super) struct FieldView<'a> {
    pub focus: Option<CommunityField>,
    pub search: (&'a str, &'a str),
    pub path: (&'a str, &'a str),
    pub clock: f32,
}

pub(super) fn draw(
    ctx: &mut Painter<'_>,
    card: &CommunityCard,
    fields: &FieldView<'_>,
    w: f32,
    h: f32,
) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    ctx.heading(
        "COMMUNITY MAPS",
        "Levels made by players. They grant no loot, currency or key progress; runs count for the boards.",
        w,
    );
    let p = panels(w, h, scale);
    ctx.panel(p.list);
    ctx.panel(p.detail);
    let l = list_rects(p.list, scale);
    let tab = |mine: bool| {
        if card.mine == mine {
            ButtonKind::Primary
        } else {
            ButtonKind::Secondary
        }
    };
    ctx.button(
        l.browse,
        "Browse",
        tab(false),
        &Hit::Community(CommunityHit::Tab(false)),
        true,
    );
    ctx.button(
        l.mine,
        "My maps",
        tab(true),
        &Hit::Community(CommunityHit::Tab(true)),
        true,
    );
    if card.mine {
        field(
            ctx,
            l.field,
            fields.path.0,
            "Path to a level .json",
            fields.focus == Some(CommunityField::Path),
            fields.clock,
            fields.path.1,
        );
        ctx.button(
            l.side,
            "Upload",
            ButtonKind::Secondary,
            &Hit::Community(CommunityHit::Upload),
            true,
        );
        ctx.button(
            l.left_small,
            "Refresh",
            ButtonKind::Ghost,
            &Hit::Community(CommunityHit::Refresh),
            true,
        );
        ctx.button(
            l.right_small,
            "Save as new version",
            ButtonKind::Secondary,
            &Hit::Community(CommunityHit::UploadVersion),
            card.selected.is_some(),
        );
    } else {
        field(
            ctx,
            l.field,
            fields.search.0,
            "Search titles (Enter)",
            fields.focus == Some(CommunityField::Search),
            fields.clock,
            fields.search.1,
        );
        ctx.button(
            l.side,
            &fit(&card.sort, 14),
            ButtonKind::Secondary,
            &Hit::Community(CommunityHit::Sort),
            true,
        );
        ctx.button(
            l.left_small,
            &card.filter,
            ButtonKind::Secondary,
            &Hit::Community(CommunityHit::Filter),
            true,
        );
        ctx.button(
            l.right_small,
            "Refresh",
            ButtonKind::Ghost,
            &Hit::Community(CommunityHit::Refresh),
            true,
        );
    }
    if card.rows.is_empty() {
        let message = if card.loading {
            "Loading..."
        } else if card.mine {
            "You have no maps. Upload a level file above."
        } else {
            "No maps match."
        };
        ctx.text(
            (p.list.x + px(20.0), p.list.y + px(150.0)),
            message,
            FAINT,
            UiFont::Medium,
            14.0,
            TextAlign::Left,
        );
    }
    for (index, row) in card.rows.iter().take(MAP_ROWS).enumerate() {
        let rect = map_row(p.list, index, scale);
        let selected = card.selected == Some(index);
        let hovered = ctx.is_hovered(&Hit::Community(CommunityHit::Row(index)));
        let fill = if selected {
            ([0.20, 0.17, 0.10, 1.0], [0.13, 0.11, 0.07, 1.0])
        } else if hovered {
            ([0.16, 0.17, 0.22, 1.0], [0.11, 0.12, 0.16, 1.0])
        } else {
            ([0.09, 0.10, 0.13, 1.0], [0.07, 0.075, 0.10, 1.0])
        };
        ctx.list
            .panel(rect, px(size::BUTTON_CORNER), fill.0, fill.1);
        if selected {
            ctx.list
                .panel_stroke(rect, px(size::BUTTON_CORNER), px(1.5), SELECTED);
        }
        ctx.text(
            (rect.x + px(10.0), rect.y + px(4.0)),
            &fit(&row.title, 28),
            TEXT,
            UiFont::SemiBold,
            14.0,
            TextAlign::Left,
        );
        ctx.text(
            (rect.x + px(10.0), rect.y + px(22.0)),
            &fit(&format!("by {}", row.author), 30),
            FAINT,
            UiFont::Medium,
            12.0,
            TextAlign::Left,
        );
        let (right, color) = if card.mine {
            (
                format!("{}{}", row.status, if row.valid { "" } else { " (errors)" }),
                if row.valid { GOOD } else { BAD },
            )
        } else {
            (format!("{} plays", row.plays), DIM)
        };
        ctx.text(
            (rect.right() - px(10.0), rect.y + px(4.0)),
            &right,
            color,
            UiFont::SemiBold,
            12.0,
            TextAlign::Right,
        );
        ctx.text(
            (rect.right() - px(10.0), rect.y + px(22.0)),
            &format!("difficulty {}/5", row.difficulty),
            DIM,
            UiFont::Medium,
            12.0,
            TextAlign::Right,
        );
    }
    ctx.text(
        (p.list.x + p.list.width * 0.5, p.list.bottom() - px(76.0)),
        &format!("Page {}", card.page.max(1)),
        FAINT,
        UiFont::Medium,
        12.0,
        TextAlign::Center,
    );
    ctx.button(
        l.prev,
        "Previous",
        ButtonKind::Secondary,
        &Hit::Community(CommunityHit::Page(-1)),
        card.can_prev && !card.loading,
    );
    ctx.button(
        l.next,
        "Next",
        ButtonKind::Secondary,
        &Hit::Community(CommunityHit::Page(1)),
        card.can_next && !card.loading,
    );
    draw_detail(ctx, card, p.detail);
    if let Some(message) = &card.message {
        ctx.text(
            (w * 0.5, h - px(122.0)),
            &fit(message, 110),
            GOLD,
            UiFont::Medium,
            14.0,
            TextAlign::Center,
        );
    }
    ctx.button(
        back_button(w, h, scale),
        "Back",
        ButtonKind::Secondary,
        &Hit::Back,
        true,
    );
}

fn draw_detail(ctx: &mut Painter<'_>, card: &CommunityCard, detail: UiRect) {
    let scale = ctx.scale;
    let px = move |v: f32| scale.px(v);
    let Some(map) = &card.detail else {
        ctx.text(
            (detail.x + detail.width * 0.5, detail.y + px(24.0)),
            "Select a map",
            FAINT,
            UiFont::Medium,
            14.0,
            TextAlign::Center,
        );
        return;
    };
    let x = detail.x + px(20.0);
    let mut y = detail.y + px(14.0);
    ctx.text(
        (x, y),
        &fit(&map.title, 34),
        GOLD,
        UiFont::SemiBold,
        19.0,
        TextAlign::Left,
    );
    y += px(28.0);
    ctx.text(
        (x, y),
        &format!("by {}", map.author),
        DIM,
        UiFont::Medium,
        13.0,
        TextAlign::Left,
    );
    y += px(20.0);
    ctx.text((x, y), &map.facts, TEXT, UiFont::Medium, 13.0, TextAlign::Left);
    y += px(18.0);
    ctx.text((x, y), &map.stats, DIM, UiFont::Medium, 13.0, TextAlign::Left);
    y += px(22.0);
    for line in wrap(&map.description, 58).into_iter().take(3) {
        ctx.text((x, y), &line, TEXT, UiFont::Regular, 13.0, TextAlign::Left);
        y += px(17.0);
    }
    if !map.tags.is_empty() {
        ctx.text(
            (x, y),
            &fit(&map.tags.join(", "), 60),
            FAINT,
            UiFont::Medium,
            12.0,
            TextAlign::Left,
        );
        y += px(18.0);
    }
    if !map.best_times.is_empty() {
        y += px(4.0);
        ctx.text((x, y), "BEST TIMES", GOLD, UiFont::SemiBold, 12.0, TextAlign::Left);
        y += px(16.0);
        for line in map.best_times.iter().take(3) {
            ctx.text((x + px(6.0), y), line, TEXT, UiFont::Medium, 12.0, TextAlign::Left);
            y += px(15.0);
        }
    }
    if map.mine && !map.issues.is_empty() {
        y += px(4.0);
        ctx.text(
            (x, y),
            "VALIDATION",
            GOLD,
            UiFont::SemiBold,
            12.0,
            TextAlign::Left,
        );
        y += px(16.0);
        for issue in map.issues.iter().take(ISSUE_LINES) {
            let color = if issue.error { BAD } else { DIM };
            ctx.text(
                (x + px(6.0), y),
                &fit(&format!("[{}] {}", issue.code, issue.text), 66),
                color,
                UiFont::Medium,
                11.0,
                TextAlign::Left,
            );
            y += px(14.0);
        }
        if map.issues.len() > ISSUE_LINES {
            ctx.text(
                (x + px(6.0), y),
                &format!("and {} more", map.issues.len() - ISSUE_LINES),
                FAINT,
                UiFont::Medium,
                11.0,
                TextAlign::Left,
            );
        }
    } else if map.mine {
        ctx.text(
            (x, y + px(4.0)),
            "No problems found.",
            GOOD,
            UiFont::Medium,
            12.0,
            TextAlign::Left,
        );
    }
    let d = detail_rects(detail, scale);
    if map.can_play {
        ctx.button(
            d.minus,
            "-",
            ButtonKind::Secondary,
            &Hit::Community(CommunityHit::Difficulty(-1)),
            true,
        );
        ctx.button(
            d.plus,
            "+",
            ButtonKind::Secondary,
            &Hit::Community(CommunityHit::Difficulty(1)),
            true,
        );
        ctx.text(
            (d.minus.right() + px(48.0), d.minus.y + px(7.0)),
            &format!("Level {}", card.play_difficulty),
            TEXT,
            UiFont::SemiBold,
            14.0,
            TextAlign::Center,
        );
        ctx.button(
            d.primary,
            "Play",
            ButtonKind::Primary,
            &Hit::Community(CommunityHit::Play),
            card.can_queue,
        );
        if !card.can_queue {
            // Why Play is greyed out, not a silent dead button.
            ctx.text(
                (d.primary.right(), d.primary.y - px(18.0)),
                "Finish your queue or party first.",
                DIM,
                UiFont::Medium,
                11.0,
                TextAlign::Right,
            );
        }
    }
    if map.can_publish {
        ctx.button(
            d.primary,
            "Publish",
            ButtonKind::Primary,
            &Hit::Community(CommunityHit::Publish),
            true,
        );
    } else if map.can_unpublish {
        ctx.button(
            d.primary,
            "Unpublish",
            ButtonKind::Secondary,
            &Hit::Community(CommunityHit::Unpublish),
            true,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_breaks_at_spaces_and_keeps_every_word() {
        let lines = wrap("a short crypt with a long description of rooms", 14);
        assert!(lines.iter().all(|l| l.chars().count() <= 14));
        assert_eq!(
            lines.join(" "),
            "a short crypt with a long description of rooms"
        );
        assert!(wrap("", 10).is_empty());
    }

    fn has(layout: &Layout, hit: CommunityHit) -> bool {
        layout.hits.iter().any(|(h, _)| *h == Hit::Community(hit.clone()))
    }

    fn card(can_play: bool, can_publish: bool, can_queue: bool, mine: bool) -> CommunityCard {
        CommunityCard {
            mine,
            rows: vec![MapRowCard {
                title: "Crypt".into(),
                author: "Ash".into(),
                difficulty: 2,
                plays: 3,
                status: "Draft".into(),
                valid: true,
            }],
            selected: Some(0),
            detail: Some(MapDetailCard {
                can_play,
                can_publish,
                mine,
                ..MapDetailCard::default()
            }),
            can_queue,
            page: 1,
            ..CommunityCard::default()
        }
    }

    #[test]
    fn the_page_offers_play_publish_and_upload_only_when_they_apply() {
        let scale = UiScale::default();
        let layout_of = |card: &CommunityCard| {
            let mut layout = Layout::new();
            hits(&mut layout, card, 1000.0, 720.0, scale);
            layout
        };
        let browse = layout_of(&card(true, false, true, false));
        assert!(has(&browse, CommunityHit::Play) && has(&browse, CommunityHit::Row(0)));
        assert!(has(&browse, CommunityHit::SearchField) && !has(&browse, CommunityHit::PathField));
        assert!(!has(&browse, CommunityHit::Publish) && !has(&browse, CommunityHit::Upload));
        // Without a character to queue, Play is not offered.
        assert!(!has(&layout_of(&card(true, false, false, false)), CommunityHit::Play));
        let mine = layout_of(&card(false, true, true, true));
        assert!(has(&mine, CommunityHit::Publish) && has(&mine, CommunityHit::Upload));
        assert!(has(&mine, CommunityHit::PathField) && has(&mine, CommunityHit::UploadVersion));
        assert!(!has(&mine, CommunityHit::Play) && !has(&mine, CommunityHit::SearchField));
    }

    #[test]
    fn long_text_is_cut_with_an_ellipsis() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("abcdefghij", 5).chars().count(), 5);
    }
}
