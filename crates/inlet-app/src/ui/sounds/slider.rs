// SPDX-License-Identifier: AGPL-3.0-or-later
//! A horizontal slider: a track, a filled part, a thumb and optional marks, dragged with
//! the mouse or set with a click. Used for the volumes of the sound effects.
//!
//! The track reports its size while it is painted; a click is turned into a value with
//! it, and a drag follows the pointer through gpui's drag events (the pointer may leave
//! the track while dragging).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    canvas, div, prelude::*, px, App, Bounds, Context, ElementId, MouseButton, Pixels, Render,
    SharedString, Window,
};

use crate::theme;

/// What a drag of a slider carries: which slider it is, so another slider on the page
/// ignores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliderDrag(pub SharedString);

/// The picture that follows the pointer during a drag: none.
struct NoGhost;

impl Render for NoGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

const TRACK_HEIGHT: f32 = 6.;
const THUMB: f32 = 16.;

/// The value (0 to `max`) the pointer at `x` stands for on a track that starts at
/// `left` and is `width` wide.
pub fn value_at(x: f32, left: f32, width: f32, max: u16) -> u16 {
    if width.is_nan() || width <= 0.0 || !x.is_finite() {
        return 0;
    }
    let fraction = ((x - left) / width).clamp(0.0, 1.0);
    (fraction * f32::from(max)).round() as u16
}

/// A slider from 0 to `max`, at `value`. `marks` are values that get a label under the
/// track (`label` turns a value into its text). `on_change` gets every new value
/// while the person drags or clicks. A slider that is not `enabled` is dimmed and
/// ignores the mouse.
pub fn slider(
    id: impl Into<SharedString>,
    value: u16,
    max: u16,
    marks: &[u16],
    label: impl Fn(u16) -> String,
    enabled: bool,
    on_change: impl Fn(u16, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let id: SharedString = id.into();
    let value = value.min(max);
    let fraction = if max == 0 {
        0.0
    } else {
        f32::from(value) / f32::from(max)
    };
    let on_change = Rc::new(on_change);
    let seen: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));

    let mut track = div()
        .id(ElementId::from(id.clone()))
        .relative()
        .w_full()
        .h(px(THUMB + 4.))
        .flex()
        .items_center()
        .cursor_pointer();
    if enabled {
        let bounds = Rc::clone(&seen);
        let change = Rc::clone(&on_change);
        track = track
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                let b = bounds.get();
                change(
                    value_at(
                        event.position.x.as_f32(),
                        b.origin.x.as_f32(),
                        b.size.width.as_f32(),
                        max,
                    ),
                    window,
                    cx,
                );
            })
            .on_drag(SliderDrag(id.clone()), |_, _, _, cx| cx.new(|_| NoGhost));
        let mine = id.clone();
        let change = Rc::clone(&on_change);
        track = track.on_drag_move::<SliderDrag>(move |event, window, cx| {
            if event.drag(cx).0 != mine {
                return;
            }
            change(
                value_at(
                    event.event.position.x.as_f32(),
                    event.bounds.origin.x.as_f32(),
                    event.bounds.size.width.as_f32(),
                    max,
                ),
                window,
                cx,
            );
        });
    } else {
        track = track.opacity(0.5).cursor_default();
    }

    let bar = div()
        .absolute()
        .top(px((THUMB + 4. - TRACK_HEIGHT) / 2.))
        .left_0()
        .right_0()
        .h(px(TRACK_HEIGHT))
        .rounded_full()
        .bg(theme::bg_textarea())
        .child(
            div()
                .h_full()
                .rounded_full()
                .bg(theme::brand())
                .w(gpui::relative(fraction)),
        );
    // The thumb sits on the track; its left edge is placed by the share of the width
    // that is left after the thumb's own, so it never leaves the track.
    let thumb = div()
        .absolute()
        .top(px(2.))
        .left(gpui::relative(fraction))
        .ml(px(-THUMB * fraction))
        .size(px(THUMB))
        .rounded_full()
        .bg(theme::text_strong())
        .border_2()
        .border_color(theme::brand());
    let size_probe = canvas(
        {
            let seen = Rc::clone(&seen);
            move |bounds, _, _| seen.set(bounds)
        },
        |_, (), _, _| {},
    )
    .absolute()
    .size_full();
    // The marks under the track need no interaction.
    let marks_row = (!marks.is_empty()).then(|| {
        let mut row = div().relative().w_full().h(px(16.));
        for mark in marks {
            let f = if max == 0 {
                0.0
            } else {
                f32::from(*mark) / f32::from(max)
            };
            row = row.child(
                div()
                    .absolute()
                    .left(gpui::relative(f))
                    .ml(px(-18. * f))
                    .w(px(36.))
                    .text_size(px(11.))
                    .text_color(theme::text_muted())
                    .child(label(*mark)),
            );
        }
        row
    });
    div()
        .w_full()
        .flex()
        .flex_col()
        .child(track.child(size_probe).child(bar).child(thumb))
        .children(marks_row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pointer_position_becomes_a_value_and_stops_at_the_ends() {
        assert_eq!(value_at(100.0, 100.0, 200.0, 200), 0);
        assert_eq!(value_at(200.0, 100.0, 200.0, 200), 100);
        assert_eq!(value_at(300.0, 100.0, 200.0, 200), 200);
        // Left of the track and right of it.
        assert_eq!(value_at(-50.0, 100.0, 200.0, 200), 0);
        assert_eq!(value_at(900.0, 100.0, 200.0, 200), 200);
        // Rounds to the nearest whole value.
        assert_eq!(value_at(100.9, 100.0, 200.0, 200), 1);
        // A track with no width (not painted yet) or a pointer that is not a number.
        assert_eq!(value_at(150.0, 100.0, 0.0, 200), 0);
        assert_eq!(value_at(f32::NAN, 100.0, 200.0, 200), 0);
    }
}
