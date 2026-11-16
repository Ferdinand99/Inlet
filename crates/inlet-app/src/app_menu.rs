// SPDX-License-Identifier: AGPL-3.0-or-later
//! The macOS application menu, as Fluxer's desktop app has it: the app menu (About, Check
//! for updates, Preferences, Services, Hide, Quit), File, Edit, View (zoom, full screen),
//! Window, and Help (the source, "report an issue", and Troubleshooting with "Reset app
//! data and restart"). Windows and Linux have no menu bar in Inlet; the same things are
//! in the Desktop settings and behind the usual keys.
//!
//! Every entry is an action: the ones for the text fields (cut, copy, paste, select all)
//! are the text input's own, Preferences is the keymap's `OpenSettings` (so the menu
//! shows the key the person may have changed), and the zoom entries are the zoom actions
//! of `desktop`. The rest is handled here.
//!
//! Left out on purpose: Undo and Redo (the composer has no undo action to point at yet),
//! Reload and the developer tools (there is no web view), hardware acceleration (gpui
//! decides that).
// The menu itself is only installed on macOS; the rest is compiled everywhere so the
// other systems check it too.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use gpui::{actions, App, Menu, MenuItem, OsAction, SystemMenuType};

use crate::app::keymap::OpenSettings;
use crate::desktop::{self, ZoomIn, ZoomOut, ZoomReset};
use crate::text_input::{Copy, Cut, Paste, SelectAll};

actions!(
    app_menu,
    [
        About,
        CheckForUpdates,
        HideInlet,
        HideOthers,
        ShowAll,
        QuitInlet,
        CloseWindow,
        MinimizeWindow,
        ZoomWindow,
        ToggleFullScreen,
        ResetAppData,
        OpenSource,
        ReportIssue
    ]
);

/// The page of Inlet's releases: where an update comes from.
const RELEASES: &str = "https://github.com/Ferdinand99/Inlet/releases";
const SOURCE: &str = "https://github.com/Ferdinand99/Inlet";
const ISSUES: &str = "https://github.com/Ferdinand99/Inlet/issues";

/// The menus, in order.
pub fn menus() -> Vec<Menu> {
    vec![
        Menu::new(t!("menu-app-name")).items([
            MenuItem::action(t!("menu-about"), About),
            MenuItem::action(t!("menu-check-for-updates"), CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action(t!("menu-preferences"), OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu(t!("menu-services"), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(t!("menu-hide"), HideInlet),
            MenuItem::action(t!("menu-hide-others"), HideOthers),
            MenuItem::action(t!("menu-show-all"), ShowAll),
            MenuItem::separator(),
            MenuItem::action(t!("menu-quit"), QuitInlet),
        ]),
        Menu::new(t!("menu-file")).items([MenuItem::action(t!("menu-close"), CloseWindow)]),
        Menu::new(t!("menu-edit")).items([
            MenuItem::os_action(t!("menu-cut"), Cut, OsAction::Cut),
            MenuItem::os_action(t!("menu-copy"), Copy, OsAction::Copy),
            MenuItem::os_action(t!("menu-paste"), Paste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action(t!("menu-select-all"), SelectAll, OsAction::SelectAll),
        ]),
        Menu::new(t!("menu-view")).items([
            MenuItem::action(t!("menu-actual-size"), ZoomReset),
            MenuItem::action(t!("menu-zoom-in"), ZoomIn),
            MenuItem::action(t!("menu-zoom-out"), ZoomOut),
            MenuItem::separator(),
            MenuItem::action(t!("menu-full-screen"), ToggleFullScreen),
        ]),
        Menu::new(t!("menu-window")).items([
            MenuItem::action(t!("menu-minimize"), MinimizeWindow),
            MenuItem::action(t!("menu-zoom"), ZoomWindow),
        ]),
        Menu::new(t!("menu-help")).items([
            MenuItem::action(t!("menu-source"), OpenSource),
            MenuItem::separator(),
            MenuItem::action(t!("menu-report-issue"), ReportIssue),
            MenuItem::separator(),
            MenuItem::submenu(
                Menu::new(t!("menu-troubleshooting"))
                    .items([MenuItem::action(t!("menu-reset-app-data"), ResetAppData)]),
            ),
        ]),
    ]
}

/// Sets the handlers of the menu's own actions, and on macOS puts the menu in the menu bar.
pub fn install(cx: &mut App) {
    cx.on_action(|_: &About, cx| {
        desktop::with_app_view(cx, "show About", |this, _, cx| {
            this.show_about = true;
            cx.notify();
        });
        desktop::show_main_window(cx);
    });
    // Inlet has no updater yet: the page of the releases is where a new version is.
    // Deviation: Fluxer's menu checks for an update in the app and offers to install it.
    cx.on_action(|_: &CheckForUpdates, cx| cx.open_url(RELEASES));
    cx.on_action(|_: &HideInlet, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &QuitInlet, cx| desktop::quit(cx));
    cx.on_action(|_: &CloseWindow, cx| {
        desktop::with_main_window(cx, "close the window", desktop::close_requested);
    });
    cx.on_action(|_: &MinimizeWindow, cx| {
        desktop::with_main_window(cx, "minimize the window", desktop::minimize_requested);
    });
    cx.on_action(|_: &ZoomWindow, cx| {
        desktop::with_main_window(cx, "zoom the window", |window, _| window.zoom_window());
    });
    cx.on_action(|_: &ToggleFullScreen, cx| {
        desktop::with_main_window(cx, "toggle full screen", |window, _| {
            window.toggle_fullscreen();
        });
    });
    cx.on_action(|_: &ResetAppData, cx| {
        // The Desktop settings ask before anything is reset.
        desktop::with_app_view(cx, "open the reset", |this, _, cx| {
            this.open_settings(Some("desktop"), cx);
        });
        desktop::ask_to_reset(cx);
        desktop::show_main_window(cx);
    });
    cx.on_action(|_: &OpenSource, cx| cx.open_url(SOURCE));
    cx.on_action(|_: &ReportIssue, cx| cx.open_url(ISSUES));
    #[cfg(target_os = "macos")]
    cx.set_menus(menus());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_has_fluxers_menus_in_fluxers_order() {
        let names: Vec<String> = menus().iter().map(|m| m.name.to_string()).collect();
        assert_eq!(names, ["Inlet", "File", "Edit", "View", "Window", "Help"]);
    }

    #[test]
    fn the_app_menu_has_about_updates_preferences_services_hide_and_quit() {
        let all = menus();
        let app = &all[0];
        assert_eq!(app.items.len(), 12);
        let labels: Vec<String> = app
            .items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Action { name, .. } => Some(name.to_string()),
                MenuItem::Submenu(m) => Some(m.name.to_string()),
                MenuItem::SystemMenu(m) => Some(m.name.to_string()),
                MenuItem::Separator => None,
            })
            .collect();
        assert_eq!(
            labels,
            [
                "About Inlet",
                "Check for updates…",
                "Preferences…",
                "Services",
                "Hide Inlet",
                "Hide others",
                "Show all",
                "Quit Inlet"
            ]
        );
    }

    #[test]
    fn help_has_a_troubleshooting_submenu_with_the_reset() {
        let all = menus();
        let help = all.last().unwrap();
        let trouble = help
            .items
            .iter()
            .find_map(|i| match i {
                MenuItem::Submenu(m) => Some(m),
                _ => None,
            })
            .expect("a Troubleshooting submenu");
        assert_eq!(trouble.name.to_string(), "Troubleshooting");
        assert_eq!(trouble.items.len(), 1);
    }
}
