//! The pages of the inspector that author a community map.
//!
//! ```text
//!  Selection │ Dungeon │ Listing │ Validate
//!  ──────────┴─────────┴─────────┴─────────
//!  Dungeon:   objective (placed vs needed), boss pack, gate, start points, adds, patrol
//!  Listing:   title, description, difficulty, tags
//!  Validate:  [Validate] [Export...]  and one row per issue; click a row to go to it
//! ```
//!
//! Built from the editor and the level by a pure function, like every other panel here: the
//! painter and the pointer code read the same list of elements, so a button that is drawn is a
//! button that is clicked.

use engine_environment::EnvironmentScene;
use engine_render_data::ui::{UiColor, UiRect};

use crate::commands::Command;
use crate::community::{self, LevelField, Tab, TextTarget};
use crate::layout::Layout;
use crate::panels::{Column, Element};
use crate::theme::{self, metrics};
use crate::{Editor, Selected};
use engine_realm::mapcheck::{DESCRIPTION_MAX, TAG_MAX, TAGS_MAX, TITLE_MAX};

impl Column<'_> {
    /// A heading over a group of rows.
    fn heading(&mut self, text: &'static str) {
        self.y += self.layout.px(4.0);
        let rect = self.row(self.layout.px(metrics::ROW_HEIGHT));
        self.out.push(Element::Heading { rect, text });
    }

    /// A row of buttons, left to right, each as wide as it says (in logical pixels).
    fn buttons(&mut self, buttons: &[(&'static str, f32, Command)]) {
        let layout = self.layout;
        let rect = self.row(layout.px(metrics::ROW_HEIGHT + 2.0));
        let mut x = rect.x;
        for (label, width, command) in buttons {
            let width = layout.px(*width).min((rect.right() - x).max(0.0));
            self.out.push(Element::Button {
                rect: UiRect::new(
                    x,
                    rect.y + layout.px(2.0),
                    width,
                    rect.height - layout.px(4.0),
                ),
                label,
                command: *command,
            });
            x += width + layout.px(4.0);
        }
    }

    /// A row of up to three typed numbers, each behind a coloured tag.
    fn fields(&mut self, label: &'static str, cells: &[(LevelField, &'static str, UiColor)]) {
        let layout = self.layout;
        let rect = self.row(layout.px(metrics::ROW_HEIGHT + 2.0));
        let label_width = layout.px(58.0);
        self.out.push(Element::Label {
            rect: UiRect::new(rect.x, rect.y, label_width, rect.height),
            text: label,
        });
        let cell = ((rect.width - label_width) / 3.0).floor();
        let tag = layout.px(12.0);
        let inset = layout.px(2.0);
        for (column, (field, letter, color)) in cells.iter().enumerate() {
            let x = rect.x + label_width + column as f32 * cell;
            // The tag is only a label here: these numbers are typed, not scrubbed.
            self.out.push(Element::Letter {
                rect: UiRect::new(x, rect.y + inset, tag, rect.height - inset * 2.0),
                letter,
                color: *color,
            });
            self.out.push(Element::Field {
                rect: UiRect::new(
                    x + tag,
                    rect.y + inset,
                    cell - tag - layout.px(3.0),
                    rect.height - inset * 2.0,
                ),
                field: *field,
            });
        }
    }

    /// A line of text typed into, behind a label.
    fn text_line(&mut self, label: &'static str, target: TextTarget) {
        let layout = self.layout;
        let rect = self.row(layout.px(metrics::ROW_HEIGHT + 2.0));
        let label_width = layout.px(58.0);
        self.out.push(Element::Label {
            rect: UiRect::new(rect.x, rect.y, label_width, rect.height),
            text: label,
        });
        self.out.push(Element::Text {
            rect: UiRect::new(
                rect.x + label_width,
                rect.y + layout.px(2.0),
                rect.width - label_width,
                rect.height - layout.px(4.0),
            ),
            target,
        });
    }

    /// A line in the colour of its news.
    fn notice(&mut self, text: impl Into<String>, color: UiColor) {
        let rect = self.row(self.layout.px(metrics::ROW_HEIGHT - 2.0));
        self.out.push(Element::Notice {
            rect,
            text: text.into(),
            color,
        });
    }

    /// A line of guidance.
    fn note(&mut self, text: impl Into<String>) {
        let rect = self.row(self.layout.px(metrics::ROW_HEIGHT - 2.0));
        self.out.push(Element::Note {
            rect,
            text: text.into(),
        });
    }
}

/// The page of the inspector `editor.tab()` names, other than the selection's. The tab strip
/// is added by the caller.
pub(crate) fn page(editor: &Editor, scene: &EnvironmentScene, layout: &Layout) -> Vec<Element> {
    let body = layout.inspector_body;
    let mut column = Column {
        layout,
        body,
        y: body.y + layout.px(2.0) - editor.inspector_scroll(),
        out: Vec::with_capacity(48),
    };
    match editor.tab() {
        Tab::Inspector => {}
        Tab::Dungeon => dungeon(editor, scene, &mut column),
        Tab::Community => listing(editor, &mut column),
        Tab::Validate => validation(editor, &mut column),
    }
    let body = layout.inspector_body;
    column.out.retain(|element| {
        let rect = element.rect();
        rect.y >= body.y - 1.0 && rect.bottom() <= body.bottom() + 1.0
    });
    column.out
}

/// The dungeon block.
fn dungeon(editor: &Editor, scene: &EnvironmentScene, column: &mut Column<'_>) {
    let dim = theme::TEXT_DIM;
    let completion = community::completion(scene);

    column.heading("Objective");
    column.info("Placed", format!("{:.1} completion", completion.placed));
    match completion.required {
        Some(required) if completion.is_enough() => {
            column.info("Needed", format!("{required:.1}"));
            column.notice("Enough monsters to finish", theme::SUCCESS);
        }
        Some(required) => {
            column.info("Needed", format!("{required:.1}"));
            column.notice(
                format!(
                    "Short by {:.1}: the objective cannot be met",
                    required - completion.placed
                ),
                theme::DANGER,
            );
        }
        None => column.notice("No dungeon block yet", theme::WARNING),
    }
    let Some(dungeon) = scene.dungeon.as_ref() else {
        column.buttons(&[("Add Dungeon Block", 140.0, Command::AddDungeon)]);
        return;
    };
    column.fields("Required", &[(LevelField::RequiredCompletion, "%", dim)]);
    column.fields("Timer (s, 0 = none)", &[(LevelField::TimerSeconds, "s", dim)]);
    column.text_line("Name", TextTarget::DungeonName);

    column.heading("Boss");
    let packs = community::pack_names(scene);
    column.info(
        "Boss pack",
        dungeon
            .boss_pack
            .clone()
            .unwrap_or_else(|| "not set".to_owned()),
    );
    column.info(
        "Packs",
        if packs.is_empty() {
            "none placed".to_owned()
        } else {
            packs.join(", ")
        },
    );
    column.buttons(&[
        ("< Pack", 52.0, Command::CycleBossPack(false)),
        ("Pack >", 52.0, Command::CycleBossPack(true)),
        ("Mark Selected Pack", 130.0, Command::MarkBossPack),
    ]);
    column.buttons(&[(
        "Move Selected Into Boss Pack",
        190.0,
        Command::AssignBossPack,
    )]);

    column.heading("Gate");
    if dungeon.boss_gate.is_some() {
        column.fields(
            "Centre",
            &[
                (LevelField::GateX, "X", theme::AXIS_X),
                (LevelField::GateY, "Y", theme::AXIS_Y),
                (LevelField::GateZ, "Z", theme::AXIS_Z),
            ],
        );
        column.fields(
            "Size",
            &[
                (LevelField::GateWidth, "W", dim),
                (LevelField::GateHeight, "H", dim),
                (LevelField::GateDepth, "D", dim),
            ],
        );
        column.fields("Turn", &[(LevelField::GateYaw, "Y", theme::AXIS_Y)]);
        column.buttons(&[
            ("Move Gate Here", 110.0, Command::AddGate),
            ("Remove Gate", 90.0, Command::RemoveGate),
        ]);
    } else {
        column.info("Gate", "none".to_owned());
        column.buttons(&[("Place Gate At Centre", 150.0, Command::AddGate)]);
    }

    column.heading("Start Points");
    column.info(
        "Spawns",
        format!(
            "{} placed; the first is where players start",
            scene.spawn_points.len()
        ),
    );
    match editor.selection() {
        Some(Selected::Spawn(index)) => column.info(
            "Selected",
            format!(
                "spawn {}: drag it, or type its position under Selection",
                index + 1
            ),
        ),
        _ => column.note("Select a spawn point to move it, or press P in the world"),
    }
    column.buttons(&[("Add Spawn At Centre", 150.0, Command::AddSpawnHere)]);

    column.heading("Boss Adds");
    column.fields("Count", &[(LevelField::AddCount, "N", dim)]);
    column.check(
        "Allow duplicates",
        dungeon.allow_duplicate_adds,
        Command::ToggleDuplicateAdds,
    );
    column.info(
        "Pool",
        if dungeon.add_pool.is_empty() {
            "empty".to_owned()
        } else {
            dungeon
                .add_pool
                .iter()
                .map(|id| theme::display_name(id))
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    column.buttons(&[
        ("Add Selected Type", 130.0, Command::AddPoolType),
        ("Clear Pool", 80.0, Command::ClearPool),
    ]);

    column.heading("Patrol");
    match editor.selection() {
        Some(Selected::Monster(index)) => {
            let waypoints = scene
                .monsters
                .get(index)
                .map_or(0, |monster| monster.patrol.len());
            column.info("Waypoints", waypoints.to_string());
            column.buttons(&[
                ("Add Waypoint At Centre", 160.0, Command::AddPatrolPoint),
                ("Clear", 56.0, Command::ClearPatrol),
            ]);
        }
        _ => column.note("Select a monster to give it a patrol"),
    }
}

/// The listing: what the map is called and described as.
fn listing(editor: &Editor, column: &mut Column<'_>) {
    let Some(meta) = editor.community_meta() else {
        column.notice("This level is not a community map", theme::WARNING);
        column.note("A community map has a title and is checked");
        column.note("against the approved library.");
        column.buttons(&[("Make Community Map", 150.0, Command::MakeCommunity)]);
        column.buttons(&[("New From Template", 150.0, Command::NewCommunityMap)]);
        return;
    };
    column.heading("Listing");
    column.text_line("Title", TextTarget::Title);
    column.text_line("About", TextTarget::Description);
    column.fields(
        "Difficulty",
        &[(LevelField::Difficulty, "D", theme::TEXT_DIM)],
    );
    column.text_line("Tags", TextTarget::Tags);
    column.note(format!(
        "Title {} of {} characters, about {} of {}",
        meta.title.chars().count(),
        TITLE_MAX,
        meta.description.chars().count(),
        DESCRIPTION_MAX,
    ));
    column.note(format!(
        "Up to {} tags of {} letters, separated by spaces",
        TAGS_MAX, TAG_MAX,
    ));
    column.heading("Library");
    match editor.approval_warning() {
        Some(warning) => column.notice(warning, theme::WARNING),
        None => column.note("Only approved assets and monsters can be used"),
    }
    column.buttons(&[("New From Template", 150.0, Command::NewCommunityMap)]);
}

/// The validator's issues.
fn validation(editor: &Editor, column: &mut Column<'_>) {
    column.heading("Check");
    column.buttons(&[
        ("Validate", 80.0, Command::Validate),
        ("Export...", 80.0, Command::ExportCommunity),
    ]);
    if !editor.is_community() {
        column.notice("Not a community map: see the Listing tab", theme::WARNING);
    }
    if let Some(warning) = editor.approval_warning() {
        column.notice(warning, theme::WARNING);
    }
    if let Some(path) = editor.last_export() {
        column.info(
            "Exported",
            path.file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
        );
    }
    let Some(report) = editor.validation() else {
        column.note("Not checked yet");
        return;
    };
    let stale = if editor.validation_is_stale() {
        " (edited since)"
    } else {
        ""
    };
    column.notice(
        format!(
            "{} error(s), {} warning(s){stale}",
            report.errors.len(),
            report.warnings.len()
        ),
        if !report.is_valid() {
            theme::DANGER
        } else if report.warnings.is_empty() {
            theme::SUCCESS
        } else {
            theme::WARNING
        },
    );
    column.y += column.layout.px(2.0);
    let errors = report.errors.len();
    for (index, issue) in report.errors.iter().chain(&report.warnings).enumerate() {
        let rect = column.row(column.layout.px(metrics::ROW_HEIGHT));
        column.out.push(Element::Issue {
            rect,
            index,
            error: index < errors,
            text: format!("{}: {}", issue.code, issue.message),
        });
    }
}
