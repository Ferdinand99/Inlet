// SPDX-License-Identifier: AGPL-3.0-or-later
//! The settings view: a column of tabs on the left, the open tab on the right.
//!
//! Every tab is its own file under `tabs/` and implements [`SettingsTab`]; `tabs::all`
//! lists them. The gear in the user panel opens the view over the whole window; it
//! hides the chat, so nothing is read behind it (see `chat_is_visible`).

pub mod controls;
pub mod profile;
pub mod store;
pub mod sync;
pub mod tabs;

use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, FontWeight, Global, SharedString, WeakEntity,
    Window,
};
use inlet_state::settings::Display;

use crate::app::Inlet;
use crate::backend::settings::Ev;
use crate::desktop::Layer;
use crate::theme;
use crate::ui::widgets::button;
use sync::Values;

/// One tab of the settings view.
pub trait SettingsTab {
    /// Stable ID, also used for element IDs (`settings-tab-<id>`).
    fn id(&self) -> &'static str;
    /// The tab's name in the list, from the catalogue.
    fn title(&self) -> SharedString;
    /// The tab's content.
    fn render(&mut self, cx: &mut Context<SettingsView>) -> AnyElement;
}

/// The settings view with its tabs.
pub struct SettingsView {
    tabs: Vec<Box<dyn SettingsTab>>,
    selected: usize,
}

impl SettingsView {
    pub fn new() -> Self {
        Self {
            tabs: tabs::all(),
            selected: 0,
        }
    }

    /// Opens the tab with this ID, if there is one.
    pub fn select(&mut self, id: &str) {
        if let Some(ix) = self.tabs.iter().position(|t| t.id() == id) {
            self.selected = ix;
        }
    }
}

impl Default for SettingsView {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected;
        let list = self.tabs.iter().enumerate().map(|(ix, tab)| {
            let active = ix == selected;
            div()
                .id(SharedString::from(format!("settings-tab-{}", tab.id())))
                .px_2()
                .py_1()
                .rounded(px(6.))
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .text_color(if active {
                    theme::text_strong()
                } else {
                    theme::text_channel()
                })
                .when(active, |d| d.bg(theme::selected_bg()))
                .hover(|s| s.bg(theme::hover_bg()))
                .child(tab.title())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = ix;
                    cx.notify();
                }))
        });
        let list: Vec<_> = list.collect();
        let content = self
            .tabs
            .get_mut(selected)
            .map(|tab| tab.render(cx))
            .unwrap_or_else(|| div().into_any_element());
        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(theme::bg_chat())
            .child(
                div()
                    .w(px(232.))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p_2()
                    .bg(theme::bg_app())
                    .children(list),
            )
            .child(div().flex_1().h_full().p_8().child(content))
    }
}

/// The app view, for the tabs: they only get their own view's context, and some of what
/// they show and do belongs to the app (the signed-in accounts, the profile).
struct AppHandle(WeakEntity<Inlet>);

impl Global for AppHandle {}

/// Runs `f` on the app view, if it is there. For a tab's render and its click handlers;
/// never while the app view itself is being updated.
pub(crate) fn with_app<R>(
    cx: &mut Context<SettingsView>,
    f: impl FnOnce(&mut Inlet, &mut Context<Inlet>) -> R,
) -> Option<R> {
    let app = cx.try_global::<AppHandle>()?.0.upgrade()?;
    Some(app.update(cx, f))
}

/// Closes the settings view once the click that asked for it has been handled: the view
/// is the one being updated, so it cannot be dropped from inside.
pub(crate) fn close_later(cx: &mut Context<SettingsView>) {
    let Some(app) = cx.try_global::<AppHandle>().map(|h| h.0.clone()) else {
        return;
    };
    cx.spawn(async move |_, cx| {
        let _ = app.update(cx, |app, cx| app.close_settings(cx));
    })
    .detach();
}

/// The settings domain's view state.
#[derive(Default)]
pub struct State {
    /// The open settings view, if any.
    pub view: Option<Entity<SettingsView>>,
    /// The account's own profile, as the My profile tab edits it.
    pub profile: profile::ProfileState,
    /// The store's generation the view state last followed.
    seen: u64,
    /// The settings the view state last followed.
    applied: Option<Values>,
}

impl Inlet {
    /// Events from the backend's settings domain.
    pub(crate) fn on_settings_event(
        &mut self,
        event: Ev,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Ev::AccountSaved { through, settings } => {
                let display = Display::of(&settings);
                store::server_settings(cx, &settings, display, Some(through));
            }
            Ev::PrefsSaved { through, prefs } => {
                let a = prefs.get().accessibility.as_ref();
                let display = Display {
                    font_size: a.and_then(|a| a.font_size),
                    message_group_spacing: a.and_then(|a| a.message_group_spacing),
                };
                store::preferences(cx, display, through);
            }
            Ev::SaveFailed {
                through,
                preferences,
            } => store::failed(cx, through, preferences),
            Ev::Profile(own) => self.on_own_profile(*own, false, cx),
            Ev::ProfileSaved(own) => self.on_own_profile(*own, true, cx),
            Ev::ProfileFailed(reason) => self.on_profile_failed(reason, false, cx),
            Ev::ProfileSaveFailed(reason) => self.on_profile_failed(reason, true, cx),
        }
        cx.notify();
    }

    /// The account's settings as its state holds them (READY, a settings update, or the
    /// account becoming the active one): the store takes them in the order of their
    /// versions.
    pub(crate) fn follow_account_settings(&mut self, cx: &mut Context<Self>) {
        let Some(account) = self.account.clone() else {
            return;
        };
        let state = account.read(cx).settings().clone();
        // Only a full snapshot says what every setting is: a partial update (or the
        // demo's READY) has no version, and its missing switches would read as off.
        if let Some(user) = state.user().filter(|u| u.version().is_some()) {
            store::server_settings(cx, user, Some(state.display()), None);
        }
    }

    /// Another account is shown: its settings, not the last one's, decide.
    pub(crate) fn settings_account_changed(&mut self, cx: &mut Context<Self>) {
        store::reset_account(cx);
        self.settings.profile.reset();
        self.follow_account_settings(cx);
    }

    /// Before the view draws: follows the store when what it shows moved. The message
    /// view keeps its own state (the display mode, the spoilers, the animation budget,
    /// the heights of the rows), so it is told here.
    pub(crate) fn follow_settings_store(&mut self, cx: &mut Context<Self>) {
        let generation = store::generation(cx);
        if generation == self.settings.seen {
            return;
        }
        self.settings.seen = generation;
        let now = store::values(cx);
        let before = self.settings.applied.replace(now.clone());
        // The first look takes the stored display mode only when it is compact: a
        // demo scene may start compact itself.
        let compact_moved = match &before {
            Some(b) => b.compact != now.compact,
            None => now.compact,
        };
        if compact_moved {
            self.chat.view.compact = now.compact;
        }
        let show_spoilers = now.render_spoilers == 0;
        let spoilers_moved = self.chat.view.show_spoilers != show_spoilers;
        self.chat.view.show_spoilers = show_spoilers;
        self.media.images.animations().borrow_mut().prefs =
            crate::ui::media::animation_budget::MotionPrefs::of_settings(&now);
        let sizes_moved = before.as_ref().is_none_or(|b| {
            b.font_size != now.font_size
                || b.group_spacing != now.group_spacing
                || b.render_embeds != now.render_embeds
                || b.render_reactions != now.render_reactions
        });
        if compact_moved || sizes_moved || spoilers_moved {
            // The rows are built again from the new settings and measured again.
            self.refresh_rows_from_scratch(cx);
        }
    }

    /// Opens the settings view over the window, at the tab `tab` if given.
    pub(crate) fn open_settings(&mut self, tab: Option<&str>, cx: &mut Context<Self>) {
        cx.set_global(AppHandle(cx.weak_entity()));
        let view = cx.new(|_| {
            let mut view = SettingsView::new();
            if let Some(tab) = tab {
                view.select(tab);
            }
            view
        });
        self.settings.view = Some(view);
        self.profile_ensure_inputs(cx);
        self.profile_load(cx);
        cx.notify();
    }

    pub(crate) fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.settings.view = None;
        cx.notify();
    }

    /// The settings view over the whole window, with a button to close it.
    pub(crate) fn render_settings_overlay(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.settings.view.clone() else {
            return div().into_any_element();
        };
        let overlay = div().id("settings-overlay");
        // Under an open link prompt or link confirmation, that layer has the keyboard.
        let overlay = if self.is_top_layer(Layer::Settings) {
            self.layer_keys(overlay, cx)
        } else {
            overlay
        };
        overlay
            .occlude()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::bg_chat())
            .child(div().flex_1().min_h_0().child(view))
            .child(
                div().absolute().top(px(16.)).right(px(16.)).child(
                    button("settings-close", t!("settings-close"), false)
                        .on_click(cx.listener(|this, _, _, cx| this.close_settings(cx))),
                ),
            )
            .into_any_element()
    }
}
