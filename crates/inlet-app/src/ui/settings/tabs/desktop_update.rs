// SPDX-License-Identifier: AGPL-3.0-or-later
//! The "Updates" section at the end of the Desktop tab: the switch for automatic updates,
//! the installed version, what the updater is doing, a button to check now or to restart
//! into a downloaded update, and the changelog of the update. The logic is in
//! `crate::update`; this file only shows it and calls it.

use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight};

use super::super::SettingsView;
use super::desktop::{heading, note, toggle};
use crate::theme;
use crate::ui::widgets::button;
use crate::update::{self, Release, Status, Updater};

/// The section, or nothing when the app has no updater (demo runs).
pub(super) fn section(cx: &mut Context<SettingsView>) -> AnyElement {
    let Some(u) = cx.try_global::<Updater>() else {
        return div().into_any_element();
    };
    let enabled = u.prefs.enabled;
    let available = u.status != Status::Unavailable;
    let version = u.current.clone();
    let status = u.status.clone();
    let can_check = u.can_check();
    let busy = u.busy();

    let mut col = div()
        .flex()
        .flex_col()
        .child(heading(t!("settings-update-heading")));
    if !available {
        return col
            .child(note(t!("settings-update-unavailable")))
            .into_any_element();
    }

    col = col
        .child(
            toggle(
                "settings-update-auto",
                t!("settings-update-auto"),
                enabled,
                true,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                update::set_enabled(!enabled, cx);
                cx.notify();
            })),
        )
        .child(note(t!("settings-update-auto-note")));
    if let Some(version) = version {
        col = col.child(
            div()
                .pt_2()
                .text_color(theme::text_muted())
                .child(t!("settings-update-version", version = version)),
        );
    }

    let line = match &status {
        Status::Idle | Status::Unavailable => None,
        Status::Checking => Some(t!("settings-update-checking")),
        Status::Downloading(release, percent) => Some(t!(
            "settings-update-downloading",
            version = release.version.clone(),
            percent = u32::from(*percent)
        )),
        Status::UpToDate => Some(t!("settings-update-up-to-date")),
        Status::Ready(release) if enabled => Some(t!(
            "settings-update-ready",
            version = release.version.clone()
        )),
        Status::Ready(release) => Some(t!(
            "settings-update-ready-off",
            version = release.version.clone()
        )),
        Status::Failed(error) => Some(t!("settings-update-failed", error = error.clone())),
    };
    if let Some(line) = line {
        col = col.child(
            div()
                .id("settings-update-status")
                .pt_1()
                .text_color(theme::text_strong())
                .child(line),
        );
    }

    let mut buttons = div().flex().flex_row().gap_2().pt_2();
    let check = button("settings-update-check", t!("settings-update-check"), false);
    buttons = buttons.child(if can_check && !busy {
        check.on_click(cx.listener(|_, _, _, cx| {
            update::check_now(cx);
            cx.notify();
        }))
    } else {
        check.opacity(0.5).cursor_default()
    });
    if matches!(status, Status::Ready(_)) {
        buttons = buttons.child(
            button(
                "settings-update-restart",
                t!("settings-update-restart"),
                true,
            )
            .on_click(cx.listener(|_, _, _, cx| {
                update::restart_now(cx);
            })),
        );
    }
    col = col.child(buttons);

    if let Status::Ready(release) | Status::Downloading(release, _) = &status {
        col = col.child(notes(release));
    }
    col.into_any_element()
}

/// The changelog of a release, as plain lines.
fn notes(release: &Release) -> AnyElement {
    let lines = release.notes.lines().map(|line| {
        div()
            .min_h(px(8.))
            .text_size(px(13.))
            .text_color(theme::text_strong())
            .child(line.to_owned())
    });
    div()
        .id("settings-update-notes")
        .flex()
        .flex_col()
        .mt_3()
        .p_3()
        .rounded(px(8.))
        .bg(theme::bg_textarea())
        .child(
            div()
                .pb_1()
                .font_weight(FontWeight::BOLD)
                .text_color(theme::text_strong())
                .child(t!(
                    "settings-update-notes",
                    version = release.version.clone()
                )),
        )
        .children(lines)
        .into_any_element()
}
