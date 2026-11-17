// SPDX-License-Identifier: AGPL-3.0-or-later
//! The floating unread indicators of the channel list: when a channel with something
//! unread has scrolled out of sight above or below, a pill at that edge says so and a
//! click scrolls to it.
//!
//! Fluxer's `ScrollIndicatorOverlay` and `FloatingUnreadIndicators`: the pill at the top
//! points at the first unread channel above the view (the first one with a mention, when
//! one above has a mention), the pill at the bottom at the last unread channel below it
//! (the last one with a mention, likewise). A pill is red when it points at a mention.
//! The scroll leaves 12 px between the channel and the edge.

use gpui::{div, point, prelude::*, px, AnyElement, Context, FontWeight};

use crate::app::Inlet;
use crate::theme;

/// What a channel row has that the pill is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Unread,
    Mention,
}

/// A channel with something unread: its row's place in the list (pixels from the top of
/// the view; negative above it) and what it has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub top: f32,
    pub bottom: f32,
    pub severity: Severity,
}

/// The targets the two pills point at (indexes into the targets given).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub above: Option<usize>,
    pub below: Option<usize>,
}

/// How much of a row may be outside the view before it counts as out of sight.
const EPSILON: f32 = 0.5;
/// The room left between the channel and the edge after a click.
const SCROLL_PADDING: f32 = 12.;

/// Which targets the pills point at, for a view `height` high. `targets` are in list
/// order.
pub fn select_edges(targets: &[Target], height: f32) -> Edges {
    let above: Vec<usize> = (0..targets.len())
        .filter(|&i| targets[i].bottom <= EPSILON)
        .collect();
    let below: Vec<usize> = (0..targets.len())
        .filter(|&i| targets[i].top >= height - EPSILON)
        .collect();
    let mention = |i: &&usize| targets[**i].severity == Severity::Mention;
    Edges {
        // The first of those above, or the first with a mention.
        above: above
            .iter()
            .find(mention)
            .or_else(|| above.first())
            .copied(),
        // The last of those below, or the last with a mention.
        below: below
            .iter()
            .rev()
            .find(mention)
            .or_else(|| below.last())
            .copied(),
    }
}

/// The scroll position (pixels from the top of the content) that shows `target` at the
/// top edge (`above`) or the bottom edge of a view `height` high, kept inside `0..=max`.
/// `scroll` is the position now.
pub fn scroll_for(target: Target, above: bool, scroll: f32, height: f32, max: f32) -> f32 {
    let delta = if above {
        target.top - SCROLL_PADDING
    } else {
        target.bottom + SCROLL_PADDING - height
    };
    (scroll + delta).clamp(0., max.max(0.))
}

impl Inlet {
    /// The channel rows with something unread, measured against the view as the last
    /// frame drew them. Nothing until the view has a size.
    fn unread_targets(
        &self,
        rows: &[(usize, Severity)],
        scrolled: f32,
    ) -> Option<(Vec<Target>, f32)> {
        let view = self.sidebar.scroll.bounds();
        let height = f32::from(view.size.height);
        if height <= 0. || rows.is_empty() {
            return None;
        }
        let targets: Vec<Target> = rows
            .iter()
            .filter_map(|(ix, severity)| {
                let b = self.sidebar.scroll.bounds_for_item(*ix)?;
                // The rows were drawn at the scroll position of the frame before; the
                // list may have scrolled since.
                Some(Target {
                    top: f32::from(b.top() - view.top()) + scrolled,
                    bottom: f32::from(b.bottom() - view.top()) + scrolled,
                    severity: *severity,
                })
            })
            .collect();
        Some((targets, height))
    }

    /// The pills over the channel list for the rows with something unread (`rows`: the
    /// row's index in the list and what it has).
    pub(super) fn floating_unread(
        &mut self,
        rows: &[(usize, Severity)],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // The bounds are where the last frame laid the rows out (the scroll is added
        // below); a frame later is asked for when the rows or the view changed, so the
        // pills do not wait for the next input.
        let offset = f32::from(self.sidebar.scroll.offset().y);
        let height = f32::from(self.sidebar.scroll.bounds().size.height);
        let shown = (rows.to_vec(), height.round() as i32);
        if self.sidebar.shown != shown {
            self.sidebar.shown = shown;
            let weak = cx.entity().downgrade();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |_, cx| cx.notify());
            });
        }
        let Some((targets, height)) = self.unread_targets(rows, offset) else {
            return Vec::new();
        };
        let edges = select_edges(&targets, height);
        let mut pills = Vec::new();
        for (above, found) in [(true, edges.above), (false, edges.below)] {
            let Some(i) = found else { continue };
            let target = targets[i];
            let pill = div()
                .id(if above {
                    "unread-above"
                } else {
                    "unread-below"
                })
                .debug_selector(move || {
                    if above {
                        "unread-above".into()
                    } else {
                        "unread-below".into()
                    }
                })
                .px(px(12.))
                .py(px(4.))
                .rounded_full()
                .bg(match target.severity {
                    Severity::Mention => theme::danger(),
                    Severity::Unread => gpui::hsla(220. / 360., 0.06, 0.30, 1.),
                })
                .text_color(theme::on_danger())
                .text_size(px(11.))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .shadow_md()
                .child(t!("sidebar-new-messages").to_uppercase())
                .on_click(
                    cx.listener(move |this, _, _, cx| this.scroll_to_unread(target, above, cx)),
                );
            let slot = div()
                .absolute()
                .left_0()
                .w_full()
                .flex()
                .justify_center()
                .when(above, |d| d.top(px(8.)))
                .when(!above, |d| d.bottom(px(8.)))
                .child(pill);
            pills.push(slot.into_any_element());
        }
        pills
    }

    /// A click on a pill: the list scrolls so that the channel is shown at that edge.
    fn scroll_to_unread(&mut self, target: Target, above: bool, cx: &mut Context<Self>) {
        let handle = &self.sidebar.scroll;
        let view = handle.bounds();
        let height = f32::from(view.size.height);
        let offset = handle.offset();
        let scroll = -f32::from(offset.y);
        // The content is as long as the first row's top to the last row's bottom, with
        // the list's padding (8 px above and below).
        let ends = (0..).map_while(|ix| handle.bounds_for_item(ix)).fold(
            None,
            |acc: Option<(f32, f32)>, b| {
                let (top, bottom) = (f32::from(b.top()), f32::from(b.bottom()));
                Some(acc.map_or((top, bottom), |(t, bt)| (t.min(top), bt.max(bottom))))
            },
        );
        let max = ends.map_or(0., |(top, bottom)| bottom - top + 16. - height);
        let next = scroll_for(target, above, scroll, height, max);
        handle.set_offset(point(offset.x, px(-next)));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(top: f32, severity: Severity) -> Target {
        Target {
            top,
            bottom: top + 32.,
            severity,
        }
    }

    #[test]
    fn nothing_out_of_sight_shows_no_pill() {
        let targets = [t(0., Severity::Unread), t(100., Severity::Mention)];
        assert_eq!(select_edges(&targets, 300.), Edges::default());
        assert_eq!(select_edges(&[], 300.), Edges::default());
    }

    #[test]
    fn a_pill_points_at_the_far_channel_and_prefers_a_mention() {
        use Severity::*;
        // Three above the view (the second has a mention), one in sight, two below.
        let targets = [
            t(-300., Unread),
            t(-200., Mention),
            t(-100., Unread),
            t(50., Unread),
            t(400., Unread),
            t(500., Unread),
        ];
        let edges = select_edges(&targets, 300.);
        assert_eq!(edges.above, Some(1), "the mention above");
        assert_eq!(edges.below, Some(5), "the last one below");
        // With a mention below as well, that one wins.
        let mut targets = targets;
        targets[4].severity = Mention;
        assert_eq!(select_edges(&targets, 300.).below, Some(4));
        // Without mentions: the first above and the last below.
        targets[1].severity = Unread;
        targets[4].severity = Unread;
        let edges = select_edges(&targets, 300.);
        assert_eq!((edges.above, edges.below), (Some(0), Some(5)));
    }

    #[test]
    fn a_channel_cut_by_the_edge_is_not_out_of_sight() {
        // Half of it is in the view: no pill.
        let targets = [t(-16., Severity::Unread), t(290., Severity::Unread)];
        assert_eq!(select_edges(&targets, 300.), Edges::default());
        // Just out: a pill.
        let targets = [t(-32., Severity::Unread)];
        assert_eq!(select_edges(&targets, 300.).above, Some(0));
    }

    #[test]
    fn a_click_scrolls_the_channel_to_the_edge_with_room_to_spare() {
        let above = t(-200., Severity::Unread);
        // 500 px down, the channel 200 above the top: it ends 12 px under the top edge.
        assert_eq!(
            scroll_for(above, true, 500., 300., 1_000.),
            500. - 200. - 12.
        );
        let below = t(400., Severity::Unread);
        // Its bottom (432) comes 12 px above the bottom edge of a 300 px view.
        assert_eq!(
            scroll_for(below, false, 100., 300., 1_000.),
            100. + 432. + 12. - 300.
        );
        // Never past the ends of the list.
        assert_eq!(scroll_for(above, true, 100., 300., 1_000.), 0.);
        assert_eq!(scroll_for(below, false, 900., 300., 1_000.), 1_000.);
    }
}
