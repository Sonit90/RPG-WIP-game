//! The dungeon timer: the time left on the run, turning to `+mm:ss` once it is over.
//!
//! ```text
//!        ┌──────────┐
//!        │  18:42   │      counting down          (+01:05 and red once the timer is out)
//!        └──────────┘
//! ```
//!
//! The application computes the text from the instance row it already has (`countdown_ends_at`
//! is when the run's clock started) and the dungeon's timer from the catalog, with the same
//! functions the server classifies the run by (`engine_realm::outcome`); no network traffic is
//! involved and nothing here decides anything.

use engine_render_data::ui::{TextAlign, UiColor, UiDrawList, UiRect, UiScale};

use super::kit::{self, Kit, tone};
use super::map_style::{color, type_scale};

/// What the timer shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTimerView {
    /// `mm:ss` or `+mm:ss`.
    pub text: String,
    /// The timer has run out: the run can still be completed, but not in time.
    pub overtime: bool,
    /// Less than a minute is left.
    pub urgent: bool,
}

/// Where the timer sits: top centre of the window.
#[must_use]
pub fn timer_rect(scale: UiScale, viewport: (u32, u32)) -> UiRect {
    let kit = Kit::new(scale);
    let (width, height) = (kit.px(120.0), kit.px(36.0));
    UiRect::new(
        ((viewport.0 as f32 - width) * 0.5).round(),
        kit.px(10.0),
        width,
        height,
    )
}

/// Draws the timer.
pub fn push_run_timer(
    list: &mut UiDrawList,
    scale: UiScale,
    viewport: (u32, u32),
    view: &RunTimerView,
) {
    let kit = Kit::new(scale);
    let rect = timer_rect(scale, viewport);
    let tint: UiColor = if view.overtime {
        tone::DANGER
    } else if view.urgent {
        tone::WARN
    } else {
        color::BORDER_STRONG
    };
    kit::panel(list, &kit, rect, tint);
    kit.text(
        list,
        rect.x + rect.width * 0.5,
        rect.y + rect.height * 0.5,
        &view.text,
        type_scale::HEADING,
        if view.overtime {
            tone::DANGER
        } else {
            color::TEXT_TITLE
        },
        TextAlign::Center,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::layout_audit;

    #[test]
    fn the_timer_is_centred_and_on_screen_at_every_size() {
        for (viewport, scale) in layout_audit::sizes() {
            let rect = timer_rect(scale, viewport);
            assert!(layout_audit::on_screen(rect, viewport), "{viewport:?}");
            let centre = rect.x + rect.width * 0.5;
            assert!((centre - viewport.0 as f32 * 0.5).abs() <= 1.0);
            for (overtime, urgent) in [(false, false), (false, true), (true, false)] {
                let mut list = UiDrawList::new();
                push_run_timer(
                    &mut list,
                    scale,
                    viewport,
                    &RunTimerView {
                        text: "+12:34".into(),
                        overtime,
                        urgent,
                    },
                );
                layout_audit::assert_sound(&list, viewport, "run timer");
                assert!(!list.is_empty());
            }
        }
    }
}
