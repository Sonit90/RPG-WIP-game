//! The chat panel: a message log in the bottom-left corner and an input line behind Enter.
//!
//! ```text
//! ┌──────────────────────────────────────┐
//! │ [Server] Aldric: anyone for a run?   │  white
//! │ [Party] Brienne: ready when you are  │  blue
//! │ From Cade: got a key to spare?       │  pink
//! │ [Party] ▏                            │  Enter opens, Enter sends, Escape cancels
//! └──────────────────────────────────────┘
//! ```
//!
//! Plain data in, plain data out: the application hands over the lines and a few facts (am I in
//! a party, who whispered me last) and gets back a [`ChatEvent`] when the player sends
//! something. Nothing here knows the network. While the input line is open the panel
//! [captures the keyboard](ChatPanel::captures_keyboard), so a letter typed to a friend never
//! casts a spell or walks the character.
//!
//! Slash commands: `/s text` (Server), `/p text` (Party), `/w Name text` (whisper) and
//! `/r text` (reply to the last whisperer). Text with no command goes to the default channel:
//! Party while in a party, Server otherwise.

use engine_realm::chat::{Channel, MAX_MESSAGE_CHARS};
use engine_render_data::font::UiFont;
use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiRect, UiScale};

use super::frontend::{FieldKeys, TextField};
use super::kit::{self, Kit, tokens};
use super::map_style::{TextStyle, color, radius};

/// The most lines the log keeps to draw.
pub const LOG_LINES: usize = 50;
/// How many text rows the log shows at once.
const VISIBLE_ROWS: usize = 8;

/// Server chat: near white.
pub const SERVER_COLOR: UiColor = [0.88, 0.89, 0.92, 1.0];
/// Party chat: blue.
pub const PARTY_COLOR: UiColor = [0.45, 0.68, 1.0, 1.0];
/// Whispers: pink.
pub const WHISPER_COLOR: UiColor = [1.0, 0.55, 0.80, 1.0];

const LINE_STYLE: TextStyle = TextStyle {
    font: UiFont::Regular,
    size: 14.0,
    line: 1.3,
};
const HINT_STYLE: TextStyle = TextStyle {
    font: UiFont::Medium,
    size: 12.5,
    line: 1.3,
};

/// A channel's colour.
#[must_use]
pub fn channel_color(channel: Channel) -> UiColor {
    match channel {
        Channel::Server => SERVER_COLOR,
        Channel::Party => PARTY_COLOR,
        Channel::Whisper => WHISPER_COLOR,
    }
}

/// One line of the log, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLineView {
    /// Decides the colour.
    pub channel: Channel,
    /// What is printed.
    pub text: String,
}

impl ChatLineView {
    /// Formats a stored message: `[Server] Name: text`, `[Party] Name: text`, and for a
    /// whisper `To Name: text` when the player sent it or `From Name: text` when they got it.
    #[must_use]
    pub fn format(
        channel: Channel,
        sender: &str,
        recipient: Option<&str>,
        sent_by_me: bool,
        text: &str,
    ) -> Self {
        let text = match channel {
            Channel::Server => format!("[Server] {sender}: {text}"),
            Channel::Party => format!("[Party] {sender}: {text}"),
            Channel::Whisper if sent_by_me => {
                format!("To {}: {text}", recipient.unwrap_or("?"))
            }
            Channel::Whisper => format!("From {sender}: {text}"),
        };
        Self { channel, text }
    }
}

/// What the panel needs to know about the world each frame.
#[derive(Debug, Clone, Copy)]
pub struct ChatView<'a> {
    /// The log, oldest first.
    pub lines: &'a [ChatLineView],
    /// Whether the player is in a party: decides the default channel and whether `/p` works.
    pub in_party: bool,
    /// The character who whispered the player last, for `/r`.
    pub reply_to: Option<&'a str>,
    /// Logical pixels to lift the panel, to clear something else in the corner.
    pub lift: f32,
}

/// The frame's input, as chat reads it.
pub struct ChatInput<'a> {
    /// Characters typed this frame.
    pub typed: &'a [char],
    /// Navigation keys pressed this frame.
    pub keys: FieldKeys,
    /// Enter went down this frame (opens the input line when it is closed).
    pub enter: bool,
    /// Escape went down this frame.
    pub escape: bool,
    /// Reads the clipboard, when the field asks to paste.
    pub clipboard: &'a dyn Fn() -> Option<String>,
}

/// A message the player sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatSend {
    /// Where.
    pub channel: Channel,
    /// A whisper's character name; empty otherwise.
    pub target: String,
    /// What was said.
    pub text: String,
}

/// What a frame of chat did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatEvent {
    /// Nothing the application needs to act on.
    None,
    /// Send this.
    Send(ChatSend),
    /// The input could not be sent; tell the player why.
    Refused(String),
}

/// The channel plain text goes to: Party while in a party, Server otherwise.
#[must_use]
pub fn default_channel(in_party: bool) -> Channel {
    if in_party {
        Channel::Party
    } else {
        Channel::Server
    }
}

/// Reads what the player typed.
///
/// `Ok(None)` is an empty line, which sends nothing.
///
/// # Errors
///
/// A sentence for the player when the line is a command used wrongly or not a command at all.
pub fn parse_input(
    input: &str,
    in_party: bool,
    reply_to: Option<&str>,
) -> Result<Option<ChatSend>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    let send = |channel, target: &str, text: &str| {
        let text = text.trim();
        if text.chars().count() > MAX_MESSAGE_CHARS {
            return Err(format!(
                "That message is too long (at most {MAX_MESSAGE_CHARS} characters)."
            ));
        }
        Ok(Some(ChatSend {
            channel,
            target: target.to_owned(),
            text: text.to_owned(),
        }))
    };
    let Some(command) = input.strip_prefix('/') else {
        return send(default_channel(in_party), "", input);
    };
    let (name, rest) = command
        .split_once(char::is_whitespace)
        .map_or((command, ""), |(name, rest)| (name, rest.trim()));
    match name.to_lowercase().as_str() {
        "s" | "server" | "say" => {
            if rest.is_empty() {
                return Err("Usage: /s message".to_owned());
            }
            send(Channel::Server, "", rest)
        }
        "p" | "party" => {
            if rest.is_empty() {
                return Err("Usage: /p message".to_owned());
            }
            if !in_party {
                return Err("You are not in a party.".to_owned());
            }
            send(Channel::Party, "", rest)
        }
        "w" | "whisper" | "tell" | "t" => {
            let (target, text) = rest
                .split_once(char::is_whitespace)
                .map_or((rest, ""), |(target, text)| (target, text.trim()));
            if target.is_empty() || text.is_empty() {
                return Err("Usage: /w Name message".to_owned());
            }
            send(Channel::Whisper, target, text)
        }
        "r" | "reply" => {
            let Some(target) = reply_to else {
                return Err("Nobody has whispered you yet.".to_owned());
            };
            if rest.is_empty() {
                return Err("Usage: /r message".to_owned());
            }
            send(Channel::Whisper, target, rest)
        }
        "" => Err("Type a message, or a command like /s, /p, /w or /r.".to_owned()),
        other => Err(format!("Unknown command /{other}. Try /s, /p, /w or /r.")),
    }
}

/// The panel's rectangles, at one viewport and scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChatLayout {
    /// The whole panel.
    pub panel: UiRect,
    /// Where the log's rows go.
    pub log: UiRect,
    /// The input line.
    pub input: UiRect,
}

/// Where the panel sits: bottom-left, `lift` logical pixels above the corner margin.
#[must_use]
pub fn layout(viewport: (u32, u32), scale: UiScale, lift: f32) -> ChatLayout {
    let kit = Kit::new(scale);
    let line = kit.style.line(LINE_STYLE);
    let pad = kit.px(10.0);
    let width = kit.px(380.0);
    #[allow(clippy::cast_precision_loss, reason = "a row count")]
    let log_height = line * VISIBLE_ROWS as f32;
    let input_height = kit.px(28.0);
    let height = pad * 3.0 + log_height + input_height;
    #[allow(clippy::cast_precision_loss, reason = "client pixel size")]
    let bottom = viewport.1 as f32 - kit.px(20.0 + lift);
    let panel = UiRect::new(kit.px(20.0), bottom - height, width, height);
    ChatLayout {
        panel,
        log: UiRect::new(panel.x + pad, panel.y + pad, width - pad * 2.0, log_height),
        input: UiRect::new(
            panel.x + pad,
            panel.bottom() - pad - input_height,
            width - pad * 2.0,
            input_height,
        ),
    }
}

/// The chat panel's own state: whether the input line is open and what is typed in it.
#[derive(Debug, Clone)]
pub struct ChatPanel {
    field: TextField,
    open: bool,
    captured: bool,
    clock: f32,
}

impl Default for ChatPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatPanel {
    /// A closed panel.
    #[must_use]
    pub fn new() -> Self {
        Self {
            field: TextField::new(MAX_MESSAGE_CHARS + 40),
            open: false,
            captured: false,
            clock: 0.0,
        }
    }

    /// Whether the input line is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether this frame's keys are chat's: the line was open at some point in the frame. The
    /// application must not read them as play, and an Escape that closed the line must not
    /// also open the pause menu.
    #[must_use]
    pub fn captures_keyboard(&self) -> bool {
        self.captured
    }

    /// Whether the pointer is over the panel (and so not over the world).
    #[must_use]
    pub fn contains(
        &self,
        pointer: (f32, f32),
        viewport: (u32, u32),
        scale: UiScale,
        lift: f32,
    ) -> bool {
        layout(viewport, scale, lift)
            .panel
            .contains(pointer.0, pointer.1)
    }

    /// Closes the input line and drops what was typed.
    pub fn close(&mut self) {
        self.open = false;
        self.field.clear();
    }

    /// Runs one frame. `available`: chat may be used right now (signed in with a realm
    /// connection, no window of the game over the interface); when it is not, an open input
    /// line closes.
    pub fn update(
        &mut self,
        input: &ChatInput<'_>,
        view: &ChatView<'_>,
        available: bool,
        delta_seconds: f32,
    ) -> ChatEvent {
        self.clock += delta_seconds;
        let was_open = self.open;
        let event = self.step(input, view, available);
        self.captured = was_open || self.open;
        event
    }

    fn step(&mut self, input: &ChatInput<'_>, view: &ChatView<'_>, available: bool) -> ChatEvent {
        if !available {
            self.close();
            return ChatEvent::None;
        }
        if !self.open {
            if input.enter {
                // The characters typed with this Enter are not for the line.
                self.open = true;
                self.field.clear();
                self.clock = 0.0;
            }
            return ChatEvent::None;
        }
        if input.escape {
            self.close();
            return ChatEvent::None;
        }
        let outcome = self.field.apply(input.typed, input.keys);
        if outcome.paste_requested
            && let Some(text) = (input.clipboard)()
        {
            self.field.paste(&text);
        }
        if !outcome.submitted {
            return ChatEvent::None;
        }
        let line = self.field.text().to_owned();
        self.close();
        match parse_input(&line, view.in_party, view.reply_to) {
            Ok(Some(send)) => ChatEvent::Send(send),
            Ok(None) => ChatEvent::None,
            Err(reason) => ChatEvent::Refused(reason),
        }
    }

    /// Draws the panel.
    pub fn draw(
        &self,
        list: &mut UiDrawList,
        view: &ChatView<'_>,
        viewport: (u32, u32),
        scale: UiScale,
    ) {
        let kit = Kit::new(scale);
        let rects = layout(viewport, scale, view.lift);
        if self.open {
            kit::panel(
                list,
                &kit,
                rects.panel,
                color::alpha(tokens::ACCENT_DIM, 0.8),
            );
        } else {
            // Resting: a quiet backdrop so the log reads over the world, no frame.
            list.panel(
                rects.panel,
                kit.px(radius::CARD),
                [0.05, 0.055, 0.07, 0.42],
                [0.03, 0.035, 0.045, 0.42],
            );
        }

        let line = kit.style.line(LINE_STYLE);
        self.draw_rows(list, &kit, view, &rects, line);
        self.draw_input(list, &kit, view, &rects);
    }

    /// The newest rows that fit, bottom-aligned.
    fn draw_rows(
        &self,
        list: &mut UiDrawList,
        kit: &Kit,
        view: &ChatView<'_>,
        rects: &ChatLayout,
        line: f32,
    ) {
        let mut rows: Vec<(String, UiColor)> = Vec::new();
        let start = view.lines.len().saturating_sub(LOG_LINES);
        'collect: for entry in view.lines[start..].iter().rev() {
            let wrapped = kit.style.wrap(LINE_STYLE, &entry.text, rects.log.width);
            for text in wrapped.into_iter().rev() {
                if rows.len() >= VISIBLE_ROWS {
                    break 'collect;
                }
                rows.push((text, channel_color(entry.channel)));
            }
        }
        let mut y = rects.log.bottom() - line * 0.5;
        for (text, colour) in rows {
            kit.text(
                list,
                rects.log.x,
                y,
                &text,
                LINE_STYLE,
                colour,
                TextAlign::Left,
            );
            y -= line;
        }
    }

    fn draw_input(
        &self,
        list: &mut UiDrawList,
        kit: &Kit,
        view: &ChatView<'_>,
        rects: &ChatLayout,
    ) {
        let input = rects.input;
        let centre = input.y + input.height * 0.5;
        if !self.open {
            kit.text(
                list,
                input.x + kit.px(4.0),
                centre,
                "Press Enter to chat",
                HINT_STYLE,
                color::alpha(color::TEXT_MUTED, 0.9),
                TextAlign::Left,
            );
            return;
        }
        list.panel(input, kit.px(radius::CONTROL), tokens::WELL, tokens::WELL);
        list.panel_stroke(
            input,
            kit.px(radius::CONTROL),
            kit.px(1.0).max(1.0),
            tokens::ACCENT_DIM,
        );
        let mut x = input.x + kit.px(10.0);
        let text = self.field.display();
        if !text.starts_with('/') {
            let channel = default_channel(view.in_party);
            let tag = format!("[{}]", channel.label());
            kit.text(
                list,
                x,
                centre,
                &tag,
                LINE_STYLE,
                channel_color(channel),
                TextAlign::Left,
            );
            x += kit.width(LINE_STYLE, &tag) + kit.px(6.0);
        }
        if text.is_empty() {
            kit.text(
                list,
                x,
                centre,
                "/s  /p  /w Name  /r",
                HINT_STYLE,
                color::alpha(color::TEXT_MUTED, 0.7),
                TextAlign::Left,
            );
        } else {
            kit.text(
                list,
                x,
                centre,
                &text,
                LINE_STYLE,
                color::TEXT_PRIMARY,
                TextAlign::Left,
            );
        }
        if (self.clock * 2.0).fract() < 0.6 {
            let caret = x + kit.width(LINE_STYLE, &self.field.display_before_caret());
            list.rect(
                UiRect::new(
                    caret,
                    input.y + kit.px(6.0),
                    kit.px(1.5),
                    input.height - kit.px(12.0),
                ),
                tokens::ACCENT,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: (u32, u32) = (1920, 1080);

    fn none() -> Option<String> {
        None
    }

    fn input<'a>(typed: &'a [char], enter: bool, escape: bool) -> ChatInput<'a> {
        ChatInput {
            typed,
            keys: FieldKeys::default(),
            enter,
            escape,
            clipboard: &none,
        }
    }

    fn view(lines: &[ChatLineView]) -> ChatView<'_> {
        ChatView {
            lines,
            in_party: false,
            reply_to: None,
            lift: 0.0,
        }
    }

    fn typed(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    fn sent(text: &str, in_party: bool, reply_to: Option<&str>) -> ChatSend {
        match parse_input(text, in_party, reply_to) {
            Ok(Some(send)) => send,
            other => panic!("{text:?} parsed to {other:?}"),
        }
    }

    #[test]
    fn plain_text_goes_to_party_in_a_party_and_server_otherwise() {
        assert_eq!(sent("hello", true, None).channel, Channel::Party);
        assert_eq!(sent("hello", false, None).channel, Channel::Server);
        assert_eq!(sent("  hello there  ", false, None).text, "hello there");
    }

    #[test]
    fn slash_commands_pick_their_channel() {
        let server = sent("/s anyone?", true, None);
        assert_eq!(
            (server.channel, server.text.as_str()),
            (Channel::Server, "anyone?")
        );
        let party = sent("/P  ready ", true, None);
        assert_eq!(
            (party.channel, party.text.as_str()),
            (Channel::Party, "ready")
        );
        let whisper = sent("/w Brienne got a key?", false, None);
        assert_eq!(whisper.channel, Channel::Whisper);
        assert_eq!(whisper.target, "Brienne");
        assert_eq!(whisper.text, "got a key?");
        let reply = sent("/r sure", false, Some("Cade"));
        assert_eq!(
            (reply.channel, reply.target.as_str(), reply.text.as_str()),
            (Channel::Whisper, "Cade", "sure")
        );
    }

    #[test]
    fn misused_commands_are_explained_not_sent() {
        assert_eq!(parse_input("", false, None), Ok(None));
        assert_eq!(parse_input("   ", false, None), Ok(None));
        for bad in ["/s", "/w", "/w Brienne", "/r hi", "/p hi", "/dance", "/"] {
            assert!(parse_input(bad, false, None).is_err(), "{bad}");
        }
        assert!(parse_input("/r", false, Some("Cade")).is_err());
        assert!(parse_input(&"x".repeat(MAX_MESSAGE_CHARS + 1), false, None).is_err());
    }

    #[test]
    fn a_line_is_formatted_by_channel_and_direction() {
        let line = |channel, sent_by_me| {
            ChatLineView::format(channel, "Aldric", Some("Brienne"), sent_by_me, "hi").text
        };
        assert_eq!(line(Channel::Server, false), "[Server] Aldric: hi");
        assert_eq!(line(Channel::Party, true), "[Party] Aldric: hi");
        assert_eq!(line(Channel::Whisper, true), "To Brienne: hi");
        assert_eq!(line(Channel::Whisper, false), "From Aldric: hi");
    }

    #[test]
    fn enter_opens_types_and_sends_and_keys_are_captured_throughout() {
        let mut panel = ChatPanel::new();
        let lines = Vec::new();
        let view = view(&lines);

        // Typing while closed is play, not chat.
        panel.update(&input(&typed("w"), false, false), &view, true, 0.016);
        assert!(!panel.is_open());
        assert!(!panel.captures_keyboard());

        // Enter opens; the characters that arrive with it are not typed into the line.
        panel.update(&input(&typed("\r"), true, false), &view, true, 0.016);
        assert!(panel.is_open());
        assert!(panel.captures_keyboard());

        panel.update(&input(&typed("hi"), false, false), &view, true, 0.016);
        assert!(panel.captures_keyboard());
        let event = panel.update(&input(&typed("\r"), true, false), &view, true, 0.016);
        assert_eq!(
            event,
            ChatEvent::Send(ChatSend {
                channel: Channel::Server,
                target: String::new(),
                text: "hi".into()
            })
        );
        // The frame that closed it still captures; the next does not.
        assert!(!panel.is_open());
        assert!(panel.captures_keyboard());
        panel.update(&input(&[], false, false), &view, true, 0.016);
        assert!(!panel.captures_keyboard());
    }

    #[test]
    fn escape_cancels_and_an_empty_line_sends_nothing() {
        let mut panel = ChatPanel::new();
        let lines = Vec::new();
        let view = view(&lines);
        panel.update(&input(&[], true, false), &view, true, 0.016);
        panel.update(&input(&typed("draft"), false, false), &view, true, 0.016);
        let event = panel.update(&input(&[], false, true), &view, true, 0.016);
        assert_eq!(event, ChatEvent::None);
        assert!(!panel.is_open());
        assert!(panel.captures_keyboard(), "the Escape was chat's");

        panel.update(&input(&[], true, false), &view, true, 0.016);
        let event = panel.update(&input(&typed("\r"), false, false), &view, true, 0.016);
        assert_eq!(event, ChatEvent::None);
        assert!(!panel.is_open());
    }

    #[test]
    fn a_bad_command_comes_back_as_a_refusal_and_unavailability_closes_the_line() {
        let mut panel = ChatPanel::new();
        let lines = Vec::new();
        let view = view(&lines);
        panel.update(&input(&[], true, false), &view, true, 0.016);
        let event = panel.update(&input(&typed("/w Bob\r"), false, false), &view, true, 0.016);
        assert!(matches!(event, ChatEvent::Refused(_)));

        panel.update(&input(&[], true, false), &view, true, 0.016);
        assert!(panel.is_open());
        panel.update(&input(&[], false, false), &view, false, 0.016);
        assert!(!panel.is_open());
        // Not available: Enter does not open it either.
        panel.update(&input(&[], true, false), &view, false, 0.016);
        assert!(!panel.is_open());
    }

    #[test]
    fn the_panel_draws_its_log_in_channel_colours_inside_the_screen() {
        let lines = vec![
            ChatLineView::format(Channel::Server, "Aldric", None, false, "hello"),
            ChatLineView::format(Channel::Party, "Brienne", None, false, "ready"),
            ChatLineView::format(Channel::Whisper, "Cade", Some("Me"), false, "psst"),
        ];
        let view = view(&lines);
        let scale = UiScale::default();
        let mut panel = ChatPanel::new();
        let mut list = UiDrawList::new();
        panel.draw(&mut list, &view, VIEWPORT, scale);
        assert!(!list.is_empty());
        let text = list.text().to_owned();
        for expected in [
            "[Server] Aldric: hello",
            "[Party] Brienne: ready",
            "From Cade: psst",
        ] {
            assert!(text.contains(expected), "{expected} missing from {text:?}");
        }
        assert!(text.contains("Press Enter to chat"));

        let rects = layout(VIEWPORT, scale, 0.0);
        assert!(rects.panel.x >= 0.0 && rects.panel.y >= 0.0);
        assert!(rects.panel.right() <= 1920.0 && rects.panel.bottom() <= 1080.0);
        assert!(rects.input.bottom() <= rects.panel.bottom());
        assert!(rects.log.bottom() <= rects.input.y);

        // Open: the input line shows the channel tag.
        panel.update(&input(&[], true, false), &view, true, 0.016);
        let mut open = UiDrawList::new();
        panel.draw(&mut open, &view, VIEWPORT, scale);
        assert!(open.text().contains("[Server]"));
        assert!(!open.text().contains("Press Enter to chat"));
    }

    #[test]
    fn a_long_log_shows_only_the_newest_rows_and_a_lift_clears_the_corner() {
        let lines: Vec<ChatLineView> = (0..80)
            .map(|i| ChatLineView::format(Channel::Server, "A", None, false, &format!("line {i}")))
            .collect();
        let view = view(&lines);
        let mut list = UiDrawList::new();
        ChatPanel::new().draw(&mut list, &view, VIEWPORT, UiScale::default());
        assert!(list.text().contains("line 79"));
        assert!(!list.text().contains("line 60"));

        let low = layout(VIEWPORT, UiScale::default(), 0.0);
        let high = layout(VIEWPORT, UiScale::default(), 120.0);
        assert!(high.panel.bottom() < low.panel.bottom());
    }
}
