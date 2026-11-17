// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Desktop tab, as in Fluxer's: the window (launch at login, start minimized, remember
//! the window, the tray icon and how it looks, the badges, minimizing and closing to the
//! tray), the colour of the tray icon, the macOS permissions, and, from Inlet's own
//! needs, the title bar, Neko, the zoom and the reset of the app data.
//! The values live in `desktop::Desktop`; this tab only shows and changes them.

use gpui::{div, prelude::*, px, AnyElement, Context, Div, FontWeight, SharedString, Stateful};
use inlet_platform::mac_permissions::{Permission, Status};

use super::super::controls::{button_with_id, radio};
use super::super::{SettingsTab, SettingsView};
use crate::assets::Icon;
use crate::backend::platform::prefs::TrayTone;
use crate::desktop::{self, Desktop};
use crate::theme;
use crate::ui::widgets::button;

pub struct DesktopTab;

const MAC: bool = cfg!(target_os = "macos");
const WINDOWS: bool = cfg!(windows);
const LINUX: bool = cfg!(any(target_os = "linux", target_os = "freebsd"));

/// A row with a check box. A row that cannot be changed right now is dimmed and does
/// not react.
pub(super) fn toggle(id: &'static str, label: String, on: bool, enabled: bool) -> Stateful<Div> {
    let mark = if on {
        Icon::CheckSquare
            .el(px(20.), theme::brand())
            .into_any_element()
    } else {
        div()
            .size(px(20.))
            .flex_none()
            .rounded(px(4.))
            .border_2()
            .border_color(theme::text_muted())
            .into_any_element()
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .py_2()
        .when(enabled, |d| d.cursor_pointer())
        .when(!enabled, |d| d.opacity(0.5))
        .child(mark)
        .child(div().text_color(theme::text_strong()).child(label))
}

pub(super) fn note(text: String) -> Div {
    div()
        .pl(px(32.))
        .text_size(px(13.))
        .text_color(theme::text_muted())
        .child(text)
}

pub(super) fn heading(text: String) -> Div {
    div()
        .pt_4()
        .pb_1()
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .text_color(theme::text_muted())
        .child(text)
}

/// What the tab shows of the desktop side, copied out of the global so the rows can
/// close over plain values.
#[derive(Clone, Debug)]
struct Shown {
    autostart: Option<bool>,
    autostart_error: Option<String>,
    start_minimized: bool,
    remember_window: bool,
    show_tray: bool,
    tray_present: bool,
    restart_pending: bool,
    colored_tray: bool,
    tone: TrayTone,
    launcher_badge: bool,
    dark_badge: bool,
    minimize_to_tray: bool,
    close_to_tray: bool,
    own_title_bar: bool,
    system_title_bar: bool,
    neko: bool,
    neko_still: bool,
    zoom: f32,
    reset_asked: bool,
}

impl Shown {
    fn of(d: &Desktop) -> Self {
        Shown {
            autostart: d.autostart,
            autostart_error: d.autostart_error.clone(),
            start_minimized: d.prefs.start_minimized,
            remember_window: d.prefs.remember_window_state,
            show_tray: d.prefs.show_tray_icon,
            tray_present: d.tray_shown(),
            restart_pending: d.tray_change_waits_for_restart(),
            colored_tray: d.prefs.colored_tray_icon,
            tone: d.prefs.tray_tone,
            launcher_badge: d.prefs.unread_launcher_badge,
            dark_badge: d.prefs.dark_taskbar_badge,
            minimize_to_tray: d.prefs.minimize_to_tray,
            close_to_tray: d.prefs.close_to_tray,
            own_title_bar: d.custom_titlebar,
            system_title_bar: d.prefs.use_system_title_bar,
            neko: d.prefs.show_neko,
            neko_still: d.prefs.keep_neko_still,
            zoom: d.prefs.zoom,
            reset_asked: d.reset_asked,
        }
    }
}

/// The name of a macOS permission in the list.
fn permission_name(which: Permission) -> String {
    match which {
        Permission::Microphone => t!("settings-desktop-permission-microphone"),
        Permission::Camera => t!("settings-desktop-permission-camera"),
        Permission::ScreenRecording => t!("settings-desktop-permission-screen"),
        Permission::InputMonitoring => t!("settings-desktop-permission-input"),
    }
}

fn permission_id(which: Permission) -> &'static str {
    match which {
        Permission::Microphone => "settings-permission-microphone",
        Permission::Camera => "settings-permission-camera",
        Permission::ScreenRecording => "settings-permission-screen",
        Permission::InputMonitoring => "settings-permission-input",
    }
}

impl SettingsTab for DesktopTab {
    fn id(&self) -> &'static str {
        "desktop"
    }

    fn title(&self) -> SharedString {
        t!("settings-desktop-title").into()
    }

    fn render(&mut self, cx: &mut Context<SettingsView>) -> AnyElement {
        if cx.try_global::<Desktop>().is_none() {
            return div().into_any_element();
        }
        if MAC {
            desktop::look_at_permissions(cx);
        }
        let s = Shown::of(cx.global::<Desktop>());
        let tray_on = s.show_tray;

        let mut col = div()
            .flex()
            .flex_col()
            .max_w(px(640.))
            .pb_8()
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_strong())
                    .child(t!("settings-desktop-title")),
            )
            .child(heading(t!("settings-desktop-window")));

        // Launch at login, and starting minimized.
        let autostart = s.autostart;
        let hidden = s.start_minimized;
        col = col.child(
            toggle(
                "settings-autostart",
                t!("settings-desktop-autostart"),
                autostart == Some(true),
                autostart.is_some(),
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(on) = autostart {
                    desktop::set_autostart(!on, hidden, cx);
                    cx.notify();
                }
            })),
        );
        if autostart.is_none() {
            col = col.child(note(t!("settings-desktop-autostart-unknown")));
        }
        if let Some(error) = s.autostart_error.clone() {
            col = col.child(note(t!("settings-desktop-autostart-failed", error = error)));
        }
        let can_hide = autostart == Some(true) && tray_on;
        col = col
            .child(
                toggle(
                    "settings-start-hidden",
                    t!("settings-desktop-start-minimized"),
                    hidden,
                    can_hide,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    if can_hide {
                        desktop::set_autostart(true, !hidden, cx);
                        cx.notify();
                    }
                })),
            )
            .child(note(if MAC {
                t!("settings-desktop-start-minimized-menubar")
            } else {
                t!("settings-desktop-start-minimized-tray")
            }));

        // Remember the window.
        let remember = s.remember_window;
        col = col.child(
            toggle(
                "settings-remember-window",
                t!("settings-desktop-remember-window"),
                remember,
                true,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                desktop::change_prefs(cx, |p| p.remember_window_state = !remember);
                cx.notify();
            })),
        );

        // The tray icon.
        col = col.child(
            toggle(
                "settings-show-tray",
                if MAC {
                    t!("settings-desktop-show-menubar")
                } else {
                    t!("settings-desktop-show-tray")
                },
                tray_on,
                true,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                desktop::change_prefs(cx, |p| p.show_tray_icon = !tray_on);
                cx.notify();
            })),
        );
        if s.restart_pending {
            col = col.child(note(t!("settings-desktop-tray-restart"))).child(
                div().pl(px(32.)).pt_1().flex().flex_row().child(
                    button(
                        "settings-restart-now",
                        t!("settings-desktop-restart-now"),
                        false,
                    )
                    .on_click(cx.listener(|_, _, _, cx| desktop::relaunch(cx))),
                ),
            );
        } else if tray_on && !s.tray_present {
            col = col.child(note(t!("settings-desktop-no-tray")));
        }
        if !MAC {
            let colored = s.colored_tray;
            col = col
                .child(
                    toggle(
                        "settings-colored-tray",
                        t!("settings-desktop-colored-tray"),
                        colored,
                        tray_on,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if tray_on {
                            desktop::change_prefs(cx, |p| p.colored_tray_icon = !colored);
                            cx.notify();
                        }
                    })),
                )
                .child(note(t!("settings-desktop-colored-tray-note")));
        }
        if LINUX {
            let on = s.launcher_badge;
            col = col
                .child(
                    toggle(
                        "settings-launcher-badge",
                        t!("settings-desktop-launcher-badge"),
                        on,
                        true,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        desktop::change_prefs(cx, |p| p.unread_launcher_badge = !on);
                        cx.notify();
                    })),
                )
                .child(note(t!("settings-desktop-launcher-badge-note")));
        }
        if WINDOWS {
            let on = s.dark_badge;
            col = col
                .child(
                    toggle(
                        "settings-dark-badge",
                        t!("settings-desktop-dark-badge"),
                        on,
                        true,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        desktop::change_prefs(cx, |p| p.dark_taskbar_badge = !on);
                        cx.notify();
                    })),
                )
                .child(note(t!("settings-desktop-dark-badge-note")));
        }

        // Minimizing and closing to the tray.
        let minimize = s.minimize_to_tray;
        let close = s.close_to_tray;
        col = col
            .child(
                toggle(
                    "settings-minimize-to-tray",
                    if MAC {
                        t!("settings-desktop-minimize-menubar")
                    } else {
                        t!("settings-desktop-minimize-tray")
                    },
                    minimize,
                    tray_on,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    if tray_on {
                        desktop::change_prefs(cx, |p| p.minimize_to_tray = !minimize);
                        cx.notify();
                    }
                })),
            )
            .child(
                toggle(
                    "settings-close-to-tray",
                    if MAC {
                        t!("settings-desktop-close-menubar")
                    } else {
                        t!("settings-desktop-close-tray")
                    },
                    close,
                    tray_on,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    if tray_on {
                        desktop::set_close_to_tray(!close, cx);
                        cx.notify();
                    }
                })),
            );

        // The title bar and the cat.
        if !MAC {
            let system = s.system_title_bar;
            col = col
                .child(
                    toggle(
                        "settings-system-title-bar",
                        t!("settings-desktop-system-title-bar"),
                        system,
                        true,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        desktop::change_prefs(cx, |p| p.use_system_title_bar = !system);
                        cx.notify();
                    })),
                )
                .children((system == s.own_title_bar).then(|| {
                    // The bar that is shown is not the one that was asked for: it takes
                    // a restart.
                    div()
                        .flex()
                        .flex_col()
                        .child(note(t!("settings-desktop-system-title-bar-note")))
                        .child(
                            div().pl(px(32.)).pt_1().flex().flex_row().child(
                                button(
                                    "settings-title-bar-restart",
                                    t!("settings-desktop-restart-now"),
                                    false,
                                )
                                .on_click(cx.listener(|_, _, _, cx| desktop::relaunch(cx))),
                            ),
                        )
                }));
        }
        let neko = s.neko;
        let still = s.neko_still;
        col = col
            .child(
                toggle("settings-neko", t!("settings-desktop-neko"), neko, true).on_click(
                    cx.listener(move |_, _, _, cx| {
                        desktop::change_prefs(cx, |p| p.show_neko = !neko);
                        cx.notify();
                    }),
                ),
            )
            .child(
                toggle(
                    "settings-neko-still",
                    t!("settings-desktop-neko-still"),
                    still,
                    true,
                )
                .on_click(cx.listener(move |_, _, _, cx| {
                    desktop::change_prefs(cx, |p| p.keep_neko_still = !still);
                    cx.notify();
                })),
            )
            .child(note(t!("settings-desktop-neko-still-note")));

        // The colour of the tray icon (not on macOS, where the menu bar decides).
        if !MAC {
            let colored = s.colored_tray;
            let can_pick = tray_on && !colored;
            col = col.child(heading(t!("settings-desktop-tray-color")));
            if colored {
                col = col.child(note(t!("settings-desktop-tray-color-colored")).pl_0());
            }
            for (id, tone, label, hint) in [
                (
                    "settings-tone-auto",
                    TrayTone::Auto,
                    t!("settings-desktop-tone-auto"),
                    Some(t!("settings-desktop-tone-auto-note")),
                ),
                (
                    "settings-tone-white",
                    TrayTone::White,
                    t!("settings-desktop-tone-white"),
                    None,
                ),
                (
                    "settings-tone-black",
                    TrayTone::Black,
                    t!("settings-desktop-tone-black"),
                    None,
                ),
            ] {
                col = col.child(
                    radio(id, label, hint, s.tone == tone)
                        .when(!can_pick, |d| d.opacity(0.5).cursor_default())
                        .on_click(cx.listener(move |_, _, _, cx| {
                            if can_pick {
                                desktop::change_prefs(cx, |p| p.tray_tone = tone);
                                cx.notify();
                            }
                        })),
                );
            }
        }

        // The macOS permissions.
        if MAC {
            col = col
                .child(heading(t!("settings-desktop-mac-permissions")))
                .child(note(t!("settings-desktop-mac-permissions-note")).pl_0());
            for which in Permission::ALL {
                let status = cx.global::<Desktop>().permission(which);
                col = col.child(permission_row(which, status, cx));
            }
            col = col.child(note(t!("settings-desktop-permission-advice")).pl_0());
        }

        // The zoom.
        let percent = (s.zoom * 100.0).round() as u32;
        col = col.child(heading(t!("settings-desktop-zoom"))).child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .py_2()
                .child(
                    button("settings-zoom-out", "\u{2212}", false).on_click(cx.listener(
                        |_, _, _, cx| {
                            desktop::zoom_by(-1, cx);
                            cx.notify();
                        },
                    )),
                )
                .child(
                    div()
                        .w(px(64.))
                        .flex()
                        .justify_center()
                        .text_color(theme::text_strong())
                        .child(t!("settings-desktop-zoom-value", percent = percent)),
                )
                .child(button("settings-zoom-in", "+", false).on_click(cx.listener(
                    |_, _, _, cx| {
                        desktop::zoom_by(1, cx);
                        cx.notify();
                    },
                )))
                .child(
                    button(
                        "settings-zoom-reset",
                        t!("settings-desktop-zoom-reset"),
                        false,
                    )
                    .on_click(cx.listener(|_, _, _, cx| {
                        desktop::set_zoom(1.0, cx);
                        cx.notify();
                    })),
                ),
        );
        col = col.child(note(t!("settings-desktop-zoom-hint")));

        // Troubleshooting: reset the app data.
        col = col.child(heading(t!("settings-desktop-troubleshooting")));
        if s.reset_asked {
            col = col.child(reset_panel(cx));
        } else {
            col = col.child(div().flex().flex_row().py_2().child(
                button_with_id("settings-reset-app-data", t!("settings-desktop-reset")).on_click(
                    cx.listener(|_, _, _, cx| {
                        cx.global_mut::<Desktop>().reset_asked = true;
                        cx.notify();
                    }),
                ),
            ));
        }
        col.into_any_element()
    }
}

/// One line of the macOS permissions: a dot, the name, and what to do about it.
fn permission_row(which: Permission, status: Status, cx: &mut Context<SettingsView>) -> AnyElement {
    let dot = match status {
        Status::Granted => theme::brand(),
        Status::Denied => theme::danger(),
        Status::NotDetermined | Status::Unsupported => theme::text_muted(),
    };
    let id = permission_id(which);
    let action = match status {
        Status::Granted => div()
            .text_size(px(13.))
            .text_color(theme::text_muted())
            .child(t!("settings-desktop-permission-allowed"))
            .into_any_element(),
        Status::NotDetermined => button_with_id(
            SharedString::from(format!("{id}-allow")),
            t!("settings-desktop-permission-allow"),
        )
        .on_click(cx.listener(move |_, _, _, cx| desktop::ask_permission(which, cx)))
        .into_any_element(),
        Status::Denied | Status::Unsupported => button_with_id(
            SharedString::from(format!("{id}-open")),
            t!("settings-desktop-permission-open"),
        )
        .on_click(cx.listener(move |_, _, _, _| {
            inlet_platform::mac_permissions::open_settings(which);
        }))
        .into_any_element(),
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .py_1()
        .child(div().size(px(10.)).flex_none().rounded_full().bg(dot))
        .child(
            div()
                .flex_1()
                .text_color(theme::text_strong())
                .child(permission_name(which)),
        )
        .child(action)
        .into_any_element()
}

/// The question before the app data is reset: Fluxer asks it in a dialog of the system.
fn reset_panel(cx: &mut Context<SettingsView>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .p_4()
        .rounded(px(8.))
        .bg(theme::bg_textarea())
        .child(
            div()
                .font_weight(FontWeight::BOLD)
                .text_color(theme::text_strong())
                .child(t!("settings-desktop-reset-title")),
        )
        .child(
            div()
                .text_color(theme::text_strong())
                .child(t!("settings-desktop-reset-message")),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(theme::text_muted())
                .child(t!("settings-desktop-reset-detail")),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    button_with_id(
                        "settings-reset-confirm",
                        t!("settings-desktop-reset-confirm"),
                    )
                    .bg(theme::danger())
                    .on_click(cx.listener(|_, _, _, cx| desktop::reset_app_data(cx))),
                )
                .child(
                    button_with_id("settings-reset-cancel", t!("settings-desktop-reset-cancel"))
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.global_mut::<Desktop>().reset_asked = false;
                            cx.notify();
                        })),
                ),
        )
        .into_any_element()
}
