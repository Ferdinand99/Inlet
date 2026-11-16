//! The tray icon (Windows notification area, macOS menu bar, Linux
//! StatusNotifierItem).
//!
//! gpui has no tray. Windows and macOS use the `tray-icon` crate; Linux has its
//! own small StatusNotifierItem with a dbusmenu over zbus (see `sni`), so neither
//! GTK nor libappindicator is needed.
//!
//! Create the tray on the UI thread. On Windows the tray's hidden window gets its
//! messages from that thread's message loop (gpui's main loop pumps them); on
//! macOS AppKit only allows it on the main thread.
//!
//! Tray calls must not run while gpui's app is borrowed: on Windows the shell can
//! dispatch window messages during them, and gpui's window procedure then cannot
//! borrow the app ("RefCell already borrowed", a skipped frame). So the methods
//! here are not public; apps use `gpui_glue::create_tray`, `set_tray_state` and
//! `set_tray_tooltip`, which run them in a task outside any borrow.

use std::cell::Cell;

use crate::{Icon, Result};

#[cfg(target_os = "linux")]
mod sni;

/// An entry in the tray menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayItem {
    /// A clickable entry; choosing it sends [`crate::PlatformEvent::TrayMenu`] with `id`.
    Action {
        id: String,
        label: String,
        enabled: bool,
    },
    /// A separator line.
    Separator,
}

impl TrayItem {
    pub fn action(id: impl Into<String>, label: impl Into<String>) -> Self {
        TrayItem::Action {
            id: id.into(),
            label: label.into(),
            enabled: true,
        }
    }

    pub fn separator() -> Self {
        TrayItem::Separator
    }
}

/// What the tray icon shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrayState {
    #[default]
    Normal,
    /// Unread messages: a grey dot.
    Unread,
    /// Mentions: a red dot.
    Mention,
}

/// Grey for unread messages without mentions.
// Deviation: Fluxer draws one red dot for any unread badge; Inlet tells unread and
// mentions apart, as TrayState asks.
const UNREAD_GREY: [u8; 3] = [0xb5, 0xba, 0xc1];

/// The indicator on a template icon (the menu bar tints the icon; the dot stays black).
const TEMPLATE_DOT: [u8; 3] = [0x00, 0x00, 0x00];

/// The colour of the monochrome tray icon, when the person picks one. `Auto` follows
/// the taskbar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrayTone {
    #[default]
    Auto,
    White,
    Black,
}

/// How the person wants the tray icon to look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrayLook {
    /// The full-colour icon instead of a monochrome one (not on macOS).
    pub colored: bool,
    pub tone: TrayTone,
}

impl Default for TrayLook {
    /// The icon as it is, in colour.
    fn default() -> Self {
        TrayLook {
            colored: true,
            tone: TrayTone::Auto,
        }
    }
}

/// The systems the tray icon is drawn for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    pub(crate) fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Linux
        }
    }
}

/// What the icon is painted as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Appearance {
    Colored,
    /// The menu bar tints it (macOS).
    Template,
    White,
    Black,
}

/// Fluxer's rule: macOS always uses a template icon; with the colour switch on the icon
/// is coloured; else the tone the person picked, and for "match the taskbar" white on a
/// dark taskbar (and on Linux, whose panel colour Inlet cannot read) and black on a light
/// one. `dark_taskbar` is `None` when the system does not say.
// Deviation: Fluxer also reads the panel colour on KDE; Inlet shows white on Linux.
pub(crate) fn appearance(os: Os, look: TrayLook, dark_taskbar: Option<bool>) -> Appearance {
    if os == Os::Mac {
        return Appearance::Template;
    }
    if look.colored {
        return Appearance::Colored;
    }
    match look.tone {
        TrayTone::White => Appearance::White,
        TrayTone::Black => Appearance::Black,
        TrayTone::Auto => match (os, dark_taskbar) {
            (Os::Windows, Some(false)) => Appearance::Black,
            _ => Appearance::White,
        },
    }
}

/// The pictures a tray icon is made from: the coloured icon and Inlet's own one-colour
/// versions (white for a dark taskbar, black for a light one, and the template the macOS
/// menu bar tints).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayIcons {
    pub colored: Icon,
    pub white: Icon,
    pub black: Icon,
    pub template: Icon,
}

impl TrayIcons {
    /// One picture for every look.
    pub fn single(icon: Icon) -> Self {
        TrayIcons {
            colored: icon.clone(),
            white: icon.clone(),
            black: icon.clone(),
            template: icon,
        }
    }
}

/// The icon for a state: the picture for `appearance`, with the indicator dot.
pub(crate) fn icon_for(icons: &TrayIcons, state: TrayState, appearance: Appearance) -> Icon {
    let base = match appearance {
        Appearance::Colored => &icons.colored,
        Appearance::Template => &icons.template,
        Appearance::White => &icons.white,
        Appearance::Black => &icons.black,
    };
    match state {
        TrayState::Normal => base.clone(),
        _ if appearance == Appearance::Template => base.with_dot(TEMPLATE_DOT),
        TrayState::Unread => base.with_dot(UNREAD_GREY),
        TrayState::Mention => base.with_dot(crate::badge::BADGE_RED),
    }
}

/// A tray icon with a menu. Removed from the tray when dropped. Created and changed
/// through `gpui_glue`.
pub struct Tray {
    icons: TrayIcons,
    state: Cell<TrayState>,
    look: Cell<TrayLook>,
    dark_taskbar: Cell<Option<bool>>,
    inner: imp::Tray,
}

impl Tray {
    /// Shows the icon in the tray. On macOS pass a monochrome icon: it is used as a
    /// template image, as Fluxer does, so the menu bar can tint it. On Linux this
    /// fails with [`crate::Error::Unsupported`] when the desktop has no tray host
    /// (GNOME without the AppIndicator extension); the app should then not hide its
    /// window to the tray.
    #[cfg_attr(not(feature = "gpui"), allow(dead_code))]
    pub(crate) fn new(icon: Icon, menu: Vec<TrayItem>) -> Result<Self> {
        Self::with_look(TrayIcons::single(icon), menu, TrayLook::default(), None)
    }

    /// As [`Tray::new`], with the look the person picked. `dark_taskbar`: whether the
    /// taskbar is dark, when the system says (see [`crate::taskbar_is_dark`]).
    #[cfg_attr(not(feature = "gpui"), allow(dead_code))]
    pub(crate) fn with_look(
        icons: TrayIcons,
        menu: Vec<TrayItem>,
        look: TrayLook,
        dark_taskbar: Option<bool>,
    ) -> Result<Self> {
        let first = icon_for(
            &icons,
            TrayState::Normal,
            appearance(Os::current(), look, dark_taskbar),
        );
        let inner = imp::Tray::new(&first, &menu)?;
        Ok(Self {
            icons,
            state: Cell::new(TrayState::Normal),
            look: Cell::new(look),
            dark_taskbar: Cell::new(dark_taskbar),
            inner,
        })
    }

    /// Draws the icon again from the state, the look and the taskbar colour.
    fn redraw(&self) {
        let shown = icon_for(
            &self.icons,
            self.state.get(),
            appearance(Os::current(), self.look.get(), self.dark_taskbar.get()),
        );
        if let Err(err) = self.inner.set_icon(&shown) {
            log::warn!("could not update the tray icon: {err}");
        }
    }

    /// Switches the icon between normal, unread and mention.
    #[cfg_attr(not(feature = "gpui"), allow(dead_code))]
    pub(crate) fn set_state(&self, state: TrayState) {
        self.state.set(state);
        self.redraw();
    }

    /// Changes how the icon looks (colour or monochrome, and the tone), and whether the
    /// taskbar is dark. Nothing is drawn when nothing changed.
    #[cfg_attr(not(feature = "gpui"), allow(dead_code))]
    pub(crate) fn set_look(&self, look: TrayLook, dark_taskbar: Option<bool>) {
        let os = Os::current();
        let changed = appearance(os, look, dark_taskbar)
            != appearance(os, self.look.get(), self.dark_taskbar.get());
        self.look.set(look);
        self.dark_taskbar.set(dark_taskbar);
        if changed {
            self.redraw();
        }
    }

    /// Whether a tray host shows the icon right now. Always true on Windows and
    /// macOS; on Linux false while the tray host (plasmashell, the GNOME extension)
    /// is gone, and true again once the icon has registered with its next start.
    /// Never hide the window to the tray while this is false.
    pub fn has_host(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.inner.has_host()
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
    }

    /// Sets the hover text, for example "Inlet – 3 mentions".
    #[cfg_attr(not(feature = "gpui"), allow(dead_code))]
    pub(crate) fn set_tooltip(&self, text: &str) {
        let text = crate::clean_text(text, 64);
        if let Err(err) = self.inner.set_tooltip(&text) {
            log::warn!("could not update the tray tooltip: {err}");
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod imp {
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::TrayItem;
    use crate::events::emit;
    use crate::{Error, Icon, PlatformEvent, Result};

    fn os<E: std::fmt::Display>(err: E) -> Error {
        Error::Os(err.to_string())
    }

    fn to_tray_icon(icon: &Icon) -> Result<tray_icon::Icon> {
        tray_icon::Icon::from_rgba(icon.rgba().to_vec(), icon.width(), icon.height()).map_err(os)
    }

    pub(super) struct Tray {
        icon: TrayIcon,
    }

    impl Tray {
        pub(super) fn new(icon: &Icon, items: &[TrayItem]) -> Result<Self> {
            let menu = Menu::new();
            for item in items {
                match item {
                    TrayItem::Action { id, label, enabled } => {
                        let entry = MenuItem::with_id(MenuId::new(id), label, *enabled, None);
                        menu.append(&entry).map_err(os)?;
                    }
                    TrayItem::Separator => {
                        menu.append(&PredefinedMenuItem::separator()).map_err(os)?
                    }
                }
            }
            TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    emit(PlatformEvent::TrayClicked);
                }
            }));
            MenuEvent::set_event_handler(Some(|event: MenuEvent| {
                emit(PlatformEvent::TrayMenu(event.id.0));
            }));
            let builder = TrayIconBuilder::new()
                .with_tooltip(crate::APP_NAME)
                .with_menu(Box::new(menu));
            // As in Fluxer: a left click toggles the window on Windows and opens
            // the menu on macOS, where the icon is a template image.
            #[cfg(windows)]
            let builder = builder
                .with_icon(to_tray_icon(icon)?)
                .with_menu_on_left_click(false);
            #[cfg(target_os = "macos")]
            let builder = builder.with_icon_templated(to_tray_icon(icon)?);
            let icon = builder.build().map_err(os)?;
            Ok(Self { icon })
        }

        pub(super) fn set_icon(&self, icon: &Icon) -> Result<()> {
            #[cfg(windows)]
            self.icon.set_icon(Some(to_tray_icon(icon)?)).map_err(os)?;
            #[cfg(target_os = "macos")]
            self.icon
                .set_icon_templated(Some(to_tray_icon(icon)?))
                .map_err(os)?;
            Ok(())
        }

        pub(super) fn set_tooltip(&self, text: &str) -> Result<()> {
            self.icon.set_tooltip(Some(text)).map_err(os)
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    pub(super) use super::sni::Tray;
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    use super::TrayItem;
    use crate::{Error, Icon, Result};

    pub(super) struct Tray;

    impl Tray {
        pub(super) fn new(_icon: &Icon, _items: &[TrayItem]) -> Result<Self> {
            Err(Error::Unsupported("no tray on this OS"))
        }

        pub(super) fn set_icon(&self, _icon: &Icon) -> Result<()> {
            Ok(())
        }

        pub(super) fn set_tooltip(&self, _text: &str) -> Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_draw_different_icons() {
        let base = Icon::from_rgba([40, 40, 40, 255].repeat(32 * 32), 32, 32).unwrap();
        let icons = TrayIcons::single(base.clone());
        let normal = icon_for(&icons, TrayState::Normal, Appearance::Colored);
        let unread = icon_for(&icons, TrayState::Unread, Appearance::Colored);
        let mention = icon_for(&icons, TrayState::Mention, Appearance::Colored);
        assert_eq!(normal, base);
        assert_ne!(unread, base);
        assert_ne!(mention, unread);
    }

    #[test]
    fn the_icon_look_follows_fluxers_rules() {
        let colored = TrayLook {
            colored: true,
            tone: TrayTone::Black,
        };
        let mono = |tone| TrayLook {
            colored: false,
            tone,
        };
        // macOS: always a template, whatever is asked.
        assert_eq!(appearance(Os::Mac, colored, None), Appearance::Template);
        assert_eq!(
            appearance(Os::Mac, mono(TrayTone::White), Some(true)),
            Appearance::Template
        );
        // Colour on: coloured, the tone does not matter.
        assert_eq!(
            appearance(Os::Windows, colored, Some(true)),
            Appearance::Colored
        );
        assert_eq!(appearance(Os::Linux, colored, None), Appearance::Colored);
        // A chosen tone is used as it is.
        assert_eq!(
            appearance(Os::Windows, mono(TrayTone::White), Some(false)),
            Appearance::White
        );
        assert_eq!(
            appearance(Os::Linux, mono(TrayTone::Black), None),
            Appearance::Black
        );
        // "Match the taskbar": white on a dark one, black on a light one; white where the
        // system does not say.
        assert_eq!(
            appearance(Os::Windows, mono(TrayTone::Auto), Some(true)),
            Appearance::White
        );
        assert_eq!(
            appearance(Os::Windows, mono(TrayTone::Auto), Some(false)),
            Appearance::Black
        );
        assert_eq!(
            appearance(Os::Windows, mono(TrayTone::Auto), None),
            Appearance::White
        );
        assert_eq!(
            appearance(Os::Linux, mono(TrayTone::Auto), None),
            Appearance::White
        );
    }

    #[test]
    fn each_look_draws_its_own_picture_and_the_dot_goes_on_top() {
        let solid = |v: u8| Icon::from_rgba([v, v, v, 255].repeat(32 * 32), 32, 32).unwrap();
        let icons = TrayIcons {
            colored: solid(10),
            white: solid(250),
            black: solid(20),
            template: solid(0),
        };
        let at = |icon: &Icon, x: usize, y: usize| {
            let i = (y * icon.width() as usize + x) * 4;
            icon.rgba()[i..i + 3].to_vec()
        };
        let plain = |a| icon_for(&icons, TrayState::Normal, a);
        assert_eq!(at(&plain(Appearance::White), 5, 28), vec![250; 3]);
        assert_eq!(at(&plain(Appearance::Black), 5, 28), vec![20; 3]);
        assert_eq!(at(&plain(Appearance::Colored), 5, 28), vec![10; 3]);
        // The dot sits top right: red on a coloured or white icon, black on a template.
        let dot_at = |icon: &Icon| {
            let side = icon.width() as f32;
            at(icon, (side - side * 0.22) as usize, (side * 0.22) as usize)
        };
        assert_eq!(
            dot_at(&icon_for(&icons, TrayState::Mention, Appearance::White)),
            vec![0xdd, 0, 0]
        );
        assert_eq!(
            dot_at(&icon_for(&icons, TrayState::Unread, Appearance::Template)),
            vec![0, 0, 0]
        );
    }

    /// Creates a real tray icon. Windows only: macOS allows it only on the main
    /// thread, which the test harness does not use, so macOS is covered by the
    /// tray_demo example in CI.
    #[cfg(windows)]
    #[test]
    #[ignore = "puts an icon in the real notification area; run in CI with --include-ignored"]
    fn windows_tray_icon_can_be_created_and_updated() {
        let base = Icon::from_rgba([40, 90, 200, 255].repeat(32 * 32), 32, 32).unwrap();
        let tray = Tray::new(
            base,
            vec![TrayItem::action("open", "Open"), TrayItem::separator()],
        )
        .unwrap();
        tray.set_state(TrayState::Mention);
        tray.set_tooltip("Inlet \u{2013} 2 mentions");
        drop(tray);
    }
}
