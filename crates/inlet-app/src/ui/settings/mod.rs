// SPDX-License-Identifier: AGPL-3.0-or-later
//! The settings view: a column of tabs on the left, the open tab on the right.
//!
//! Every tab is its own file under `tabs/` and implements [`SettingsTab`]; `tabs::all`
//! lists them. The gear in the user panel opens the view over the whole window; it
//! hides the chat, so nothing is read behind it (see `chat_is_visible`).

pub mod controls;
pub mod profile;
pub mod registry;
pub mod store;
pub mod sync;
pub mod tabs;

use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, FontWeight, Global, SharedString, WeakEntity,
    Window,
};
use inlet_state::flags::Flag;
use inlet_state::settings::Display;

use crate::app::Inlet;
use crate::backend::settings::account::Ev;
use crate::backend::settings::Ev as SettingsEv;
use crate::desktop::Layer;
use crate::text_input::TextInput;
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
    /// The search opened this tab for the setting `id` (of [`registry`]): the tab may
    /// bring it into view.
    fn focus_item(&mut self, _id: &str, _cx: &mut Context<SettingsView>) {}
}

/// The settings view with its tabs.
pub struct SettingsView {
    tabs: Vec<Box<dyn SettingsTab>>,
    selected: usize,
    /// The search across all tabs.
    search: Entity<TextInput>,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new(t!("settings-search-placeholder"), cx));
        // The results follow what is typed.
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        Self {
            tabs: tabs::all(),
            selected: 0,
            search,
        }
    }

    /// Opens the tab of the setting `id`, and has the tab bring the setting into view.
    fn open_item(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let Some(item) = registry::ITEMS.iter().find(|i| i.id == id) else {
            return;
        };
        self.select(item.tab);
        let selected = self.selected;
        if let Some(tab) = self.tabs.get_mut(selected) {
            tab.focus_item(id, cx);
        }
        self.search.update(cx, |s, cx| s.clear(cx));
        cx.notify();
    }

    /// The results of the search, as rows to click; none when nothing is typed.
    fn search_results(&mut self, typed: &str, cx: &mut Context<Self>) -> AnyElement {
        let hits = registry::search(typed, false);
        let mut col = div()
            .flex()
            .flex_col()
            .max_w(px(controls::PAGE_WIDTH))
            .gap_1()
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_strong())
                    .child(t!("settings-search-placeholder")),
            );
        if hits.is_empty() {
            col = col.child(
                div()
                    .py_2()
                    .text_color(theme::text_muted())
                    .child(t!("settings-search-none")),
            );
        }
        for item in hits {
            let tab = self
                .tabs
                .iter()
                .find(|t| t.id() == item.tab)
                .map(|t| t.title())
                .unwrap_or_default();
            let id = item.id;
            col = col.child(
                div()
                    .id(SharedString::from(format!("settings-found-{id}")))
                    .flex()
                    .flex_col()
                    .px_3()
                    .py_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(theme::hover_bg()))
                    .child(
                        div()
                            .text_color(theme::text_strong())
                            .font_weight(FontWeight::MEDIUM)
                            .child((item.label)()),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child((item.description)()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::text_tertiary())
                            .child(t!("settings-search-in-tab", tab = tab.to_string())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.open_item(id, cx))),
            );
        }
        col.into_any_element()
    }

    /// Opens the tab with this ID, if there is one.
    pub fn select(&mut self, id: &str) {
        if let Some(ix) = self.tabs.iter().position(|t| t.id() == id) {
            self.selected = ix;
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected;
        let typed = self.search.read(cx).text().trim().to_owned();
        let searching = !typed.is_empty();
        let list = self.tabs.iter().enumerate().map(|(ix, tab)| {
            // While the search shows its results, no tab is the open one.
            let active = ix == selected && !searching;
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
                    // Picking a tab ends a search.
                    this.search.update(cx, |s, cx| s.clear(cx));
                    cx.notify();
                }))
        });
        let list: Vec<_> = list.collect();
        let content = if !searching {
            self.tabs
                .get_mut(selected)
                .map(|tab| tab.render(cx))
                .unwrap_or_else(|| div().into_any_element())
        } else {
            self.search_results(&typed, cx)
        };
        let search = self.search.clone();
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
                    .child(div().pb_2().child(search))
                    .children(list),
            )
            .child(
                // A long tab scrolls instead of running off the window.
                div()
                    .id("settings-content")
                    .flex_1()
                    .h_full()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_8()
                    .child(content),
            )
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

/// A question that waits for an answer: a change of who may send DMs, and whether it
/// applies to the communities already joined too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmQuestion {
    /// About bots (else members).
    pub bots: bool,
    /// The change allows messages (else blocks them).
    pub allow: bool,
}

/// The settings domain's view state.
#[derive(Default)]
pub struct State {
    /// The Privacy tab asks whether a change of who may send DMs also applies to the
    /// communities already joined.
    pub dm_question: Option<DmQuestion>,
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
        event: SettingsEv,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SettingsEv::Account(event) = event;
        match event {
            Ev::AccountSaved { through, settings } => {
                store::answer_settings(cx, &settings, through);
            }
            Ev::PrefsSaved { through, prefs } => {
                let display = Display::of_preferences(prefs.get());
                store::answer_preferences(cx, display, through);
            }
            Ev::SaveFailed {
                through,
                preferences,
            } => store::failed(cx, through, preferences),
            Ev::Profile(own) => self.on_own_profile(*own, false, window, cx),
            Ev::ProfileSaved(own) => self.on_own_profile(*own, true, window, cx),
            Ev::ProfileFailed(reason) => self.on_profile_failed(reason, false, cx),
            Ev::ProfileSaveFailed(reason) => self.on_profile_failed(reason, true, cx),
        }
        cx.notify();
    }

    /// The account's settings as its state holds them (READY, a settings update, or the
    /// account becoming the active one): the store takes them in the order they arrive.
    pub(crate) fn follow_account_settings(&mut self, cx: &mut Context<Self>) {
        let Some(account) = self.account.clone() else {
            return;
        };
        let state = account.read(cx).settings().clone();
        // The state keeps only whole snapshots (a partial update, or the demo's READY,
        // would read as every switch off).
        if let Some(user) = state.user() {
            store::gateway_settings(cx, user, Some(state.display()));
        }
    }

    /// Another account is shown: its settings, not the last one's, decide.
    pub(crate) fn settings_account_changed(&mut self, cx: &mut Context<Self>) {
        store::reset_account(cx);
        self.settings.profile.reset();
        self.follow_account_settings(cx);
    }

    /// The settings as the view last followed them. Rows and menus read this instead of
    /// the store, which works the values out again on every call.
    pub(crate) fn settings_values(&self) -> &Values {
        static DEFAULTS: std::sync::OnceLock<Values> = std::sync::OnceLock::new();
        self.settings
            .applied
            .as_ref()
            .unwrap_or_else(|| DEFAULTS.get_or_init(Values::default))
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
        let spoilers_moved = self.chat.view.spoilers != now.render_spoilers;
        self.chat.view.spoilers = now.render_spoilers;
        let hide_blocked = now.flag(Flag::HideBlockedMessages);
        let blocked_moved = self.chat.view.hide_blocked != hide_blocked;
        self.chat.view.hide_blocked = hide_blocked;
        self.media.images.animations().borrow_mut().prefs =
            crate::ui::media::animation_budget::MotionPrefs::of_settings(&now);
        let sizes_moved = before.as_ref().is_none_or(|b| {
            b.font_size != now.font_size
                || b.group_spacing != now.group_spacing
                || b.compact_group_spacing != now.compact_group_spacing
                || b.compact_avatars != now.compact_avatars
                || b.inline_embed_media != now.inline_embed_media
                || b.inline_attachment_media != now.inline_attachment_media
                || b.render_embeds != now.render_embeds
                || b.render_reactions != now.render_reactions
        });
        if compact_moved || sizes_moved || spoilers_moved || blocked_moved {
            // The rows are built again from the new settings and measured again.
            self.refresh_rows_from_scratch(cx);
        }
    }

    /// Opens the settings view over the window, at the tab `tab` if given.
    pub(crate) fn open_settings(&mut self, tab: Option<&str>, cx: &mut Context<Self>) {
        cx.set_global(AppHandle(cx.weak_entity()));
        let view = cx.new(|cx| {
            let mut view = SettingsView::new(cx);
            if let Some(tab) = tab {
                // A demo scene may open the search with some words: `search:words`.
                #[cfg(feature = "demo")]
                if let Some(words) = tab.strip_prefix("search:") {
                    view.search
                        .update(cx, |s, cx| s.set_text(words.to_owned(), cx));
                } else {
                    view.select(tab);
                }
                #[cfg(not(feature = "demo"))]
                view.select(tab);
            }
            view
        });
        self.settings.view = Some(view);
        self.profile_ensure_inputs(cx);
        // Read again each time the settings open: Fluxer may have changed the profile.
        self.settings.profile.load = profile::Load::NotAsked;
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

#[cfg(test)]
#[cfg(feature = "demo")]
mod tests {
    use gpui::{px, size, AppContext, TestAppContext};

    use super::store;
    use super::sync::Values;
    use crate::backend::settings::write::Change;
    use crate::layout_probe::{selector, Probe};

    /// The settings reach the message view that keeps state of its own: the display
    /// mode, the spoilers, and the height of the rows that a new font size changes.
    #[gpui::test]
    fn the_message_view_follows_the_settings(cx: &mut TestAppContext) {
        let (probe, cx) = cx.add_window_view(Probe::new);
        cx.update(|_, cx| cx.set_global(store::Store::new(Values::default(), None)));
        cx.simulate_resize(size(px(1280.), px(860.)));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let inlet = cx.read_entity(&probe, |p, _| p.inlet().clone());
        let height = |cx: &mut gpui::VisualTestContext| {
            cx.debug_bounds(selector::MESSAGE_ROW)
                .expect("a cozy message row")
                .size
                .height
                .as_f32()
        };
        let before = height(cx);
        cx.read_entity(&inlet, |i, _| {
            assert!(!i.chat.view.compact && i.chat.view.spoilers == 1);
        });

        cx.update(|_, cx| store::change(cx, Change::FontSize(24)));
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        assert!(
            height(cx) > before,
            "bigger text makes a taller row: {before} then {}",
            height(cx)
        );

        cx.update(|_, cx| {
            store::change(cx, Change::RenderSpoilers(0));
            store::change(cx, Change::Compact(true));
        });
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.read_entity(&inlet, |i, _| {
            assert!(i.chat.view.compact, "the display mode follows");
            assert_eq!(i.chat.view.spoilers, 0, "spoilers show without a click");
            assert_eq!(i.settings_values().font_size, 24);
        });
        cx.update(|_, cx| {
            store::change(cx, Change::FontSize(16));
        });
    }
}

#[cfg(test)]
mod flag_tests {
    use inlet_protocol::Ready;
    use inlet_state::{AccountState, ChannelId, MessagesPage};
    use serde_json::json;

    use crate::ui::chat::messages::{MessageView, Row};

    fn msg(id: u64, author: &str) -> inlet_protocol::Message {
        serde_json::from_value(json!({
            "id": (id << 22).to_string(), "channel_id": "100", "type": 0,
            "content": format!("m{id}"), "timestamp": "2026-01-01T10:00:00+00:00",
            "author": {"id": author, "username": format!("u{author}")}
        }))
        .unwrap()
    }

    /// With "hide messages from blocked users" the blocked person's messages are left out
    /// of the list, not folded into a line; with it off they fold, as before.
    #[test]
    fn hiding_blocked_messages_leaves_them_out_instead_of_folding_them() {
        let ready: Ready = serde_json::from_value(json!({
            "session_id": "s", "user": {"id": "1", "username": "me"},
            "guilds": [], "users": [],
            "private_channels": [{"id": "100", "type": 3,
                "recipients": [{"id": "2", "username": "u2"}, {"id": "3", "username": "u3"}]}],
            "relationships": [{"id": "3", "type": 2}],
            "read_states": [], "user_guild_settings": []
        }))
        .unwrap();
        let mut state = AccountState::from_ready_at(&ready, 1);
        let ch = ChannelId(100);
        let _ = state.begin_load(ch);
        let page: Vec<_> = [(1, "2"), (2, "3"), (3, "3"), (4, "2"), (5, "3")]
            .into_iter()
            .map(|(i, a)| msg(i, a))
            .rev()
            .collect();
        let _ = state.apply_messages(ch, MessagesPage::latest(page, 50));
        let shape = |rows: &[Row]| -> Vec<String> {
            rows.iter()
                .filter_map(|r| match r {
                    Row::Message(m) => Some(format!("m{}", m.id.0 >> 22)),
                    Row::Blocked { count, .. } => Some(format!("blocked {count}")),
                    _ => None,
                })
                .collect()
        };
        let mut view = MessageView::default();
        assert_eq!(
            shape(&view.build_rows(&state, ch)),
            ["m1", "blocked 2", "m4", "blocked 1"],
            "folded by default"
        );
        view.hide_blocked = true;
        assert_eq!(shape(&view.build_rows(&state, ch)), ["m1", "m4"]);
    }
}

#[cfg(test)]
mod search_tests {
    use gpui::{AppContext, TestAppContext};

    use super::SettingsView;

    /// Opening a result of the search goes to the setting's tab and ends the search; the
    /// Advanced tab is then narrowed to that setting.
    #[gpui::test]
    fn a_search_result_opens_the_tab_of_the_setting_and_clears_the_search(cx: &mut TestAppContext) {
        let view = cx.new(SettingsView::new);
        cx.update(|cx| {
            view.update(cx, |v, cx| {
                v.search.update(cx, |s, cx| s.set_text("action bar", cx));
                assert_eq!(v.tabs[v.selected].id(), "my-profile");
                v.open_item("advanced-action-bar", cx);
                assert_eq!(v.tabs[v.selected].id(), "advanced");
                assert_eq!(v.search.read(cx).text(), "", "the search ended");
            });
            view.update(cx, |v, cx| {
                v.open_item("appearance-theme", cx);
                assert_eq!(v.tabs[v.selected].id(), "appearance");
            });
        });
    }
}
