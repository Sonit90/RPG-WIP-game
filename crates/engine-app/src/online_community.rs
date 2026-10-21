//! The Community maps page's state: what was fetched, what is selected and the card the
//! front-end draws from it. Like [`crate::online_records`], pages are fetched when the page opens
//! or a control changes, never per frame; everything shown was decided by the server (the
//! validator's report, the plays counter, the status) and the client only formats it.

use engine_audio::SoundId;
use engine_net::session::NetCommand;
use engine_presentation::ui::frontend::{
    CommunityAction, CommunityCard, IssueLine, MAP_ROWS, MapDetailCard, MapRowCard,
};
use engine_realm::community::{
    self, MapDetail, MapPage, MapQuery, MapSort, MapStatus, MapSummary, SaveMapResponse,
};

use crate::online_records::format_duration_us;

/// The sound for a map whose status changed from `before` to `after`: becoming published is
/// the party-formed chord; anything else is silent.
#[must_use]
pub fn published_sound(before: MapStatus, after: MapStatus) -> Option<SoundId> {
    (before != MapStatus::Published && after == MapStatus::Published)
        .then_some(SoundId::PartyFormed)
}

/// What a press on the page asks the session to do next.
#[derive(Debug)]
pub enum Step {
    /// Nothing more.
    None,
    /// Send this to the network layer.
    Net(Vec<NetCommand>),
    /// Continue to role selection for this community dungeon at this difficulty.
    Play {
        /// `cm:<id>:<version>`.
        dungeon_id: String,
        /// 1 to 10.
        difficulty: u32,
    },
    /// Read the level file at `path` and upload it (as a new version of `map_id`, if given).
    Upload {
        /// The file.
        path: String,
        /// The map to add a version to.
        map_id: Option<String>,
    },
}

/// The Community maps page's state.
#[derive(Debug)]
pub struct CommunityState {
    mine: bool,
    search: String,
    sort: usize,
    difficulty: Option<u8>,
    page: u32,
    list: Option<MapPage>,
    selected: Option<usize>,
    detail: Option<MapDetail>,
    loading: bool,
    play_difficulty: u32,
    message: Option<String>,
}

impl Default for CommunityState {
    fn default() -> Self {
        Self {
            mine: false,
            search: String::new(),
            sort: 0,
            difficulty: None,
            page: 0,
            list: None,
            selected: None,
            detail: None,
            loading: false,
            play_difficulty: 1,
            message: None,
        }
    }
}

impl CommunityState {
    fn query(&self) -> MapQuery {
        MapQuery {
            search: self.search.clone(),
            author: None,
            mine: self.mine,
            sort: MapSort::ALL.get(self.sort).copied(),
            difficulty: self.difficulty,
            page: self.page,
        }
    }

    fn reload(&mut self) -> NetCommand {
        self.loading = true;
        self.selected = None;
        self.detail = None;
        NetCommand::CommunityList(self.query())
    }

    /// Opens the page: the first page of the list as it was last asked.
    pub fn open(&mut self) -> NetCommand {
        self.message = None;
        self.page = 0;
        self.reload()
    }

    /// The selected map's summary.
    fn selected_summary(&self) -> Option<&MapSummary> {
        let index = self.selected?;
        self.list.as_ref()?.maps.get(index)
    }

    /// Handles a press. `my_account` is the signed-in account id.
    pub fn act(&mut self, action: CommunityAction) -> Step {
        self.message = None;
        match action {
            CommunityAction::Tab(mine) => {
                if mine != self.mine {
                    self.mine = mine;
                    self.page = 0;
                    return Step::Net(vec![self.reload()]);
                }
                Step::None
            }
            CommunityAction::Select(index) => {
                let Some(summary) = self
                    .list
                    .as_ref()
                    .and_then(|page| page.maps.get(index))
                    .cloned()
                else {
                    return Step::None;
                };
                self.selected = Some(index);
                self.play_difficulty = self.play_difficulty.clamp(1, community::MAX_DIFFICULTY);
                // The list row first; the detail (with the report) follows from the server.
                self.detail = None;
                Step::Net(vec![NetCommand::CommunityOpen(summary.id)])
            }
            CommunityAction::Sort => {
                self.sort = (self.sort + 1) % MapSort::ALL.len();
                self.page = 0;
                Step::Net(vec![self.reload()])
            }
            CommunityAction::Filter => {
                self.difficulty = match self.difficulty {
                    None => Some(1),
                    Some(5) => None,
                    Some(d) => Some(d + 1),
                };
                self.page = 0;
                Step::Net(vec![self.reload()])
            }
            CommunityAction::Page(step) => {
                let next = i64::from(self.page) + i64::from(step);
                if next < 0 || self.loading {
                    return Step::None;
                }
                self.page = u32::try_from(next).unwrap_or(0);
                Step::Net(vec![self.reload()])
            }
            CommunityAction::Search(text) => {
                self.search = text;
                self.page = 0;
                Step::Net(vec![self.reload()])
            }
            CommunityAction::Refresh => Step::Net(vec![self.reload()]),
            CommunityAction::Difficulty(step) => {
                let next = i64::from(self.play_difficulty) + i64::from(step);
                self.play_difficulty = u32::try_from(next.clamp(
                    1,
                    i64::from(community::MAX_DIFFICULTY),
                ))
                .unwrap_or(1);
                Step::None
            }
            CommunityAction::Play => match &self.detail {
                Some(detail) if detail.summary.status == MapStatus::Published => Step::Play {
                    dungeon_id: detail.dungeon_id.clone(),
                    difficulty: self.play_difficulty,
                },
                _ => Step::None,
            },
            CommunityAction::Publish | CommunityAction::Unpublish => {
                let publish = matches!(action, CommunityAction::Publish);
                match self.selected_summary().map(|summary| summary.id.clone()) {
                    Some(map_id) => {
                        self.loading = true;
                        Step::Net(vec![NetCommand::CommunityPublish { map_id, publish }])
                    }
                    None => Step::None,
                }
            }
            CommunityAction::Upload(path) => {
                if path.is_empty() {
                    self.message = Some("Type the path of a level .json file first.".into());
                    return Step::None;
                }
                Step::Upload {
                    path,
                    map_id: None,
                }
            }
            CommunityAction::UploadVersion(path) => {
                if path.is_empty() {
                    self.message = Some("Type the path of a level .json file first.".into());
                    return Step::None;
                }
                match self.selected_summary() {
                    Some(summary) => Step::Upload {
                        path,
                        map_id: Some(summary.id.clone()),
                    },
                    None => Step::None,
                }
            }
        }
    }

    /// The title of the map behind a community dungeon id, if the page has seen it.
    #[must_use]
    pub fn title_of(&self, dungeon_id: &str) -> Option<String> {
        let (map_id, _) = community::parse_dungeon_id(dungeon_id)?;
        self.detail
            .iter()
            .map(|detail| &detail.summary)
            .chain(self.list.iter().flat_map(|page| page.maps.iter()))
            .find(|map| map.id == map_id)
            .map(|map| map.title.clone())
    }

    /// A list page arrived.
    pub fn list_arrived(&mut self, page: MapPage) {
        self.loading = false;
        self.page = page.page;
        self.selected = None;
        self.detail = None;
        self.list = Some(page);
    }

    /// A map arrived (opened, published or unpublished): shown, and its row refreshed. A map
    /// that has just become published is a success worth a sound.
    pub fn detail_arrived(&mut self, detail: MapDetail) -> Option<SoundId> {
        self.loading = false;
        let mut cue = None;
        if let Some(page) = self.list.as_mut()
            && let Some(row) = page.maps.iter_mut().find(|m| m.id == detail.summary.id)
        {
            cue = published_sound(row.status, detail.summary.status);
            *row = detail.summary.clone();
        }
        let selected_id = self.selected_summary().map(|m| m.id.clone());
        if selected_id.as_deref() == Some(detail.summary.id.as_str()) {
            self.detail = Some(detail);
        }
        cue
    }

    /// An upload was answered: the list reloads, and the answer is heard (a refusal or a draft
    /// with errors is the error tone, a clean upload the confirm chime).
    pub fn saved(&mut self, response: &SaveMapResponse) -> (NetCommand, SoundId) {
        let errors = response.report.errors.len();
        let warnings = response.report.warnings.len();
        self.message = Some(match &response.map {
            None => format!(
                "Not uploaded: {}",
                response
                    .report
                    .errors
                    .first()
                    .map_or("the file is not a level", |issue| issue.message.as_str())
            ),
            Some(map) if errors == 0 => format!(
                "Uploaded \"{}\" as version {} ({warnings} warning(s)). Publish it from My maps.",
                map.summary.title, map.summary.version
            ),
            Some(map) => format!(
                "Saved \"{}\" as a draft with {errors} error(s); fix them before publishing.",
                map.summary.title
            ),
        });
        self.mine = true;
        self.page = 0;
        let cue = if response.map.is_some() && errors == 0 {
            SoundId::ReadyAccept
        } else {
            SoundId::UiError
        };
        (self.reload(), cue)
    }

    /// A request failed.
    pub fn failed(&mut self, message: &str) {
        self.loading = false;
        self.message = Some(message.to_owned());
    }

    /// Tells the page a file could not be read.
    pub fn note(&mut self, message: String) {
        self.message = Some(message);
    }

    /// The page as the front-end draws it. `can_queue`: a character is chosen.
    #[must_use]
    pub fn card(&self, my_account: &str, can_queue: bool) -> CommunityCard {
        let rows: Vec<MapRowCard> = self
            .list
            .as_ref()
            .map(|page| {
                page.maps
                    .iter()
                    .take(MAP_ROWS)
                    .map(|map| MapRowCard {
                        title: map.title.clone(),
                        author: map.author.clone(),
                        difficulty: map.difficulty,
                        plays: map.plays,
                        status: status_label(map.status).to_owned(),
                        valid: map.valid,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let detail = self.detail.as_ref().map(|detail| detail_card(detail, my_account));
        CommunityCard {
            mine: self.mine,
            sort: MapSort::ALL
                .get(self.sort)
                .map_or_else(String::new, |s| s.label().to_owned()),
            filter: self
                .difficulty
                .map_or_else(|| "Any difficulty".to_owned(), |d| format!("Difficulty {d}")),
            rows,
            selected: self.selected,
            detail,
            play_difficulty: self.play_difficulty,
            loading: self.loading,
            page: self.page + 1,
            can_prev: self.page > 0,
            can_next: self.list.as_ref().is_some_and(|p| p.has_more),
            message: self.message.clone(),
            can_queue,
        }
    }
}

fn status_label(status: MapStatus) -> &'static str {
    match status {
        MapStatus::Draft => "Draft",
        MapStatus::Published => "Published",
        MapStatus::Hidden => "Hidden",
        MapStatus::Rejected => "Rejected",
    }
}

fn detail_card(detail: &MapDetail, my_account: &str) -> MapDetailCard {
    let map = &detail.summary;
    let mine = map.author_id == my_account;
    let issues = detail
        .report
        .errors
        .iter()
        .map(|issue| (issue, true))
        .chain(detail.report.warnings.iter().map(|issue| (issue, false)))
        .map(|(issue, error)| IssueLine {
            code: issue.code.clone(),
            text: format!(
                "{}: {} - {}",
                community::code_title(&issue.code),
                issue.path,
                issue.message
            ),
            error,
        })
        .collect();
    let fastest = map
        .fastest_us
        .map_or_else(|| "no clears yet".to_owned(), |us| format!("fastest {}", format_duration_us(us)));
    MapDetailCard {
        title: map.title.clone(),
        author: map.author.clone(),
        description: map.description.clone(),
        facts: format!(
            "Difficulty {} of 5 | version {} | {}",
            map.difficulty,
            map.version,
            status_label(map.status)
        ),
        stats: format!("{} plays | {} clears | {fastest}", map.plays, map.completions),
        tags: map.tags.clone(),
        issues,
        best_times: map
            .fastest_us
            .map(|us| format!("Fastest clear: {}", format_duration_us(us)))
            .into_iter()
            .collect(),
        can_play: map.status == MapStatus::Published,
        can_publish: mine && map.status == MapStatus::Draft && detail.report.is_valid(),
        can_unpublish: mine && map.status == MapStatus::Published,
        mine,
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test code asserts on known-good values"
)]
mod tests {
    use super::*;
    use engine_realm::community::{IssueInfo, ReportInfo};

    fn summary(id: &str, status: MapStatus, author: &str) -> MapSummary {
        MapSummary {
            id: id.into(),
            title: format!("Map {id}"),
            author_id: author.into(),
            author: "Ash".into(),
            description: "A crypt.".into(),
            difficulty: 3,
            tags: vec![],
            status,
            version: 2,
            plays: 7,
            fastest_us: Some(95 * engine_realm::SECOND),
            completions: 4,
            valid: true,
            created_at_us: 0,
            updated_at_us: 0,
        }
    }

    fn page(maps: Vec<MapSummary>) -> MapPage {
        MapPage {
            maps,
            page: 0,
            page_size: 12,
            has_more: true,
        }
    }

    fn detail(status: MapStatus, author: &str, errors: Vec<IssueInfo>) -> MapDetail {
        MapDetail {
            summary: summary("a", status, author),
            report: ReportInfo {
                errors,
                warnings: vec![],
            },
            versions: vec![],
            dungeon_id: if status == MapStatus::Published {
                "cm:a:2".into()
            } else {
                String::new()
            },
            max_difficulty: 10,
        }
    }

    #[test]
    fn the_page_fetches_on_open_and_on_control_changes_only() {
        let mut state = CommunityState::default();
        let NetCommand::CommunityList(query) = state.open() else {
            panic!("a list fetch");
        };
        assert!(!query.mine && query.page == 0);
        let Step::Net(commands) = state.act(CommunityAction::Tab(true)) else {
            panic!("a fetch");
        };
        assert!(matches!(&commands[0], NetCommand::CommunityList(q) if q.mine));
        // The same tab again asks for nothing.
        assert!(matches!(state.act(CommunityAction::Tab(true)), Step::None));
        let Step::Net(commands) = state.act(CommunityAction::Search("crypt".into())) else {
            panic!("a fetch");
        };
        assert!(matches!(&commands[0], NetCommand::CommunityList(q) if q.search == "crypt"));
    }

    #[test]
    fn publishing_chimes_and_a_refused_upload_sounds_the_error() {
        let mut state = CommunityState::default();
        state.list_arrived(page(vec![summary("a", MapStatus::Draft, "me")]));
        state.act(CommunityAction::Select(0));
        // Opening a draft is silent; becoming published is the chord; a repeat is silent.
        assert_eq!(
            state.detail_arrived(detail(MapStatus::Draft, "me", vec![])),
            None
        );
        assert_eq!(
            state.detail_arrived(detail(MapStatus::Published, "me", vec![])),
            Some(SoundId::PartyFormed)
        );
        assert_eq!(
            state.detail_arrived(detail(MapStatus::Published, "me", vec![])),
            None
        );
        assert_eq!(
            published_sound(MapStatus::Published, MapStatus::Draft),
            None
        );
        // A clean upload confirms; a draft saved with errors, or nothing saved, is an error.
        let bad = IssueInfo {
            code: "unknown_monster".into(),
            path: "scene.monsters[0].monster".into(),
            message: "no".into(),
        };
        let clean = SaveMapResponse {
            report: ReportInfo::default(),
            map: Some(detail(MapStatus::Draft, "me", vec![])),
        };
        let with_errors = SaveMapResponse {
            report: ReportInfo {
                errors: vec![bad.clone()],
                warnings: vec![],
            },
            map: Some(detail(MapStatus::Draft, "me", vec![bad.clone()])),
        };
        let refused = SaveMapResponse {
            report: ReportInfo {
                errors: vec![bad],
                warnings: vec![],
            },
            map: None,
        };
        assert_eq!(state.saved(&clean).1, SoundId::ReadyAccept);
        assert_eq!(state.saved(&with_errors).1, SoundId::UiError);
        assert_eq!(state.saved(&refused).1, SoundId::UiError);
    }

    #[test]
    fn selecting_a_map_opens_it_and_play_needs_a_published_one() {
        let mut state = CommunityState::default();
        state.list_arrived(page(vec![summary("a", MapStatus::Published, "x")]));
        assert!(matches!(state.act(CommunityAction::Play), Step::None));
        let Step::Net(commands) = state.act(CommunityAction::Select(0)) else {
            panic!("an open");
        };
        assert!(matches!(&commands[0], NetCommand::CommunityOpen(id) if id == "a"));
        state.detail_arrived(detail(MapStatus::Published, "x", vec![]));
        for _ in 0..20 {
            state.act(CommunityAction::Difficulty(1));
        }
        let Step::Play {
            dungeon_id,
            difficulty,
        } = state.act(CommunityAction::Play)
        else {
            panic!("play");
        };
        assert_eq!((dungeon_id.as_str(), difficulty), ("cm:a:2", 10));
        state.act(CommunityAction::Difficulty(-50));
        assert_eq!(state.card("me", true).play_difficulty, 1);
    }

    #[test]
    fn my_draft_shows_issues_and_can_publish_only_without_errors() {
        let mut state = CommunityState::default();
        state.list_arrived(page(vec![summary("a", MapStatus::Draft, "me")]));
        state.act(CommunityAction::Select(0));
        let bad = IssueInfo {
            code: "unknown_monster".into(),
            path: "scene.monsters[3].monster".into(),
            message: "'x' is not an enemy".into(),
        };
        state.detail_arrived(detail(MapStatus::Draft, "me", vec![bad]));
        let card = state.card("me", true);
        let detail = card.detail.expect("detail");
        assert!(!detail.can_publish && !detail.can_play && detail.mine);
        assert_eq!(detail.issues[0].code, "unknown_monster");
        assert!(detail.issues[0].text.contains("Unknown monster"));
        state.detail_arrived(detail_of_valid_draft());
        let card = state.card("me", true);
        assert!(card.detail.expect("detail").can_publish);
        // Someone else's draft never offers publishing.
        let other = state.card("someone-else", true).detail.expect("detail");
        assert!(!other.can_publish && !other.mine);
    }

    fn detail_of_valid_draft() -> MapDetail {
        detail(MapStatus::Draft, "me", vec![])
    }

    #[test]
    fn an_upload_needs_a_path_and_a_version_needs_a_selection() {
        let mut state = CommunityState::default();
        assert!(matches!(
            state.act(CommunityAction::Upload(String::new())),
            Step::None
        ));
        assert!(state.card("me", true).message.is_some());
        assert!(matches!(
            state.act(CommunityAction::UploadVersion("a.json".into())),
            Step::None
        ));
        assert!(matches!(
            state.act(CommunityAction::Upload("a.json".into())),
            Step::Upload { map_id: None, .. }
        ));
    }
}
