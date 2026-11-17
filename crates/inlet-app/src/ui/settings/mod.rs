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

use std::cell::Cell;
use std::rc::Rc;

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
    /// "Copy diagnostics" was pressed in a search result.
    diagnostics_copied: Rc<Cell<bool>>,
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
            diagnostics_copied: Rc::default(),
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

    /// Opens a tab, which ends a search.
    pub(crate) fn open_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        self.select(id);
        self.search.update(cx, |s, cx| s.clear(cx));
        cx.notify();
    }

    /// The settings that match `typed`, best first. A setting of a tab this view does not
    /// have is left out, as Fluxer does.
    fn matches(&self, typed: &str) -> Vec<(u32, &'static registry::Item)> {
        registry::search_scored(typed, false)
            .into_iter()
            .filter(|(_, item)| self.tabs.iter().any(|t| t.id() == item.tab))
            .collect()
    }

    /// The tabs that hold a match, best tab first (a tab counts by its best setting).
    fn matched_tabs(hits: &[(u32, &'static registry::Item)]) -> Vec<&'static str> {
        let mut order: Vec<&'static str> = Vec::new();
        for (_, item) in hits {
            if !order.contains(&item.tab) {
                order.push(item.tab);
            }
        }
        order
    }

    /// The results of the search: the tabs that match, each with its matching settings.
    /// A setting that has a control shows it, so it is changed where it is found; the
    /// others open their tab.
    fn search_results(
        &mut self,
        hits: &[(u32, &'static registry::Item)],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let values = store::values(cx);
        let me = tabs::account_id(cx);
        let copied = Rc::clone(&self.diagnostics_copied);
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
        for tab_id in Self::matched_tabs(hits) {
            let title = self
                .tabs
                .iter()
                .find(|t| t.id() == tab_id)
                .map(|t| t.title())
                .unwrap_or_default();
            col = col.child(
                div()
                    .id(SharedString::from(format!("settings-found-tab-{tab_id}")))
                    .pt_4()
                    .pb_1()
                    .text_size(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_muted())
                    .cursor_pointer()
                    .child(title.to_uppercase())
                    .on_click(cx.listener(move |this, _, _, cx| this.open_tab(tab_id, cx))),
            );
            for (_, item) in hits.iter().filter(|(_, i)| i.tab == tab_id) {
                let item: &'static registry::Item = item;
                if matches!(item.control, registry::Control::Open) {
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
                            .on_click(cx.listener(move |this, _, _, cx| this.open_item(id, cx))),
                    );
                } else {
                    col = col.child(tabs::item_row(item, &values, me.as_deref(), &copied, cx));
                }
            }
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
        // A search narrows the tab list to the tabs that hold a match, best tab first.
        let hits = if searching {
            self.matches(&typed)
        } else {
            Vec::new()
        };
        let shown: Vec<usize> = if searching {
            Self::matched_tabs(&hits)
                .iter()
                .filter_map(|id| self.tabs.iter().position(|t| t.id() == *id))
                .collect()
        } else {
            (0..self.tabs.len()).collect()
        };
        let list = shown.iter().map(|&ix| {
            let tab = &self.tabs[ix];
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
            self.search_results(&hits, cx)
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
        let event = match event {
            SettingsEv::Account(event) => event,
            SettingsEv::Guild(event) => {
                self.on_guild_event(event, cx);
                return;
            }
        };
        match event {
            Ev::AccountSaved { through, settings } => {
                store::answer_settings(cx, &settings, through);
                // A server order that is on its way: the answer is one more update of
                // the account's settings, taken the way the gateway's are.
                self.rail_answer(settings, window, cx);
            }
            Ev::PrefsSaved { through, prefs } => {
                let display = Display::of_preferences(prefs.get());
                store::answer_preferences(cx, display, through);
            }
            Ev::SaveFailed {
                through,
                preferences,
            } => {
                store::failed(cx, through, preferences);
                // What the server holds shows again where the server order was shown.
                if !preferences {
                    self.rail_save_failed(cx);
                }
            }
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

    /// What the settings decided for the message view survives a switch of account (the
    /// next account may hold the same values, so nothing would tell the view again).
    #[gpui::test]
    fn the_message_view_keeps_the_settings_across_an_account_switch(cx: &mut TestAppContext) {
        use crate::app::Inlet;
        let (inlet, cx) = cx.add_window_view(Inlet::detached);
        inlet.update_in(cx, |this, _, _| {
            this.chat.view.hide_blocked = true;
            this.chat.view.spoilers = 0;
            this.chat.view.compact = true;
            this.reset_messages();
            assert!(this.chat.view.hide_blocked, "hide blocked messages stays");
            assert_eq!(this.chat.view.spoilers, 0, "spoilers keep showing");
            assert!(this.chat.view.compact);
        });
    }

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
mod hidden_message_tests {
    use inlet_protocol::Ready;
    use inlet_state::{AccountState, ChannelId, MessageId, MessagesPage};
    use serde_json::json;

    use crate::ui::chat::messages::{MessageView, Row};

    fn msg_at(id: u64, author: &str, timestamp: &str) -> inlet_protocol::Message {
        serde_json::from_value(json!({
            "id": (id << 22).to_string(), "channel_id": "100", "type": 0,
            "content": format!("m{id}"), "timestamp": timestamp,
            "author": {"id": author, "username": format!("u{author}")}
        }))
        .unwrap()
    }

    /// A direct message with person 3 blocked, holding these messages (oldest first).
    fn state_with(messages: &[(u64, &str, &str)]) -> (AccountState, ChannelId) {
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
        let page: Vec<_> = messages
            .iter()
            .rev()
            .map(|(i, a, t)| msg_at(*i, a, t))
            .collect();
        let _ = state.apply_messages(ch, MessagesPage::latest(page, 50));
        (state, ch)
    }

    /// The days and messages of the rows: "day" for a divider, "mN" for a message and
    /// "mN+" for one that is grouped under the message before it.
    fn shape(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Day { .. } => Some("day".to_owned()),
                Row::Message(m) => Some(format!(
                    "m{}{}",
                    m.id.0 >> 22,
                    if m.grouped { "+" } else { "" }
                )),
                Row::Blocked { count, .. } => Some(format!("blocked {count}")),
                _ => None,
            })
            .collect()
    }

    /// Fluxer leaves a hidden message out before any divider or grouping is worked out
    /// (`MessageGroupingUtils.ts`): a day that holds only hidden messages has no divider,
    /// and A, [B hidden], A groups the two A messages.
    #[test]
    fn a_hidden_message_leaves_no_divider_and_no_extra_header() {
        let (state, ch) = state_with(&[
            (1, "2", "2026-01-01T10:00:00+00:00"),
            (2, "3", "2026-01-02T10:00:00+00:00"),
            (3, "3", "2026-01-02T10:01:00+00:00"),
            (4, "2", "2026-01-03T10:00:00+00:00"),
            (5, "2", "2026-01-03T10:00:30+00:00"),
            (6, "3", "2026-01-03T10:00:40+00:00"),
            (7, "2", "2026-01-03T10:01:00+00:00"),
        ]);
        let mut view = MessageView::default();
        assert_eq!(
            shape(&view.build_rows(&state, ch)),
            [
                "day",
                "m1",
                "day",
                "blocked 2",
                "day",
                "m4",
                "m5+",
                "blocked 1",
                "m7"
            ],
            "folded: the blocked day keeps its divider"
        );
        view.hide_blocked = true;
        assert_eq!(
            shape(&view.build_rows(&state, ch)),
            ["day", "m1", "day", "m4", "m5+", "m7+"],
            "hidden: no divider for the blocked-only day, and m7 groups under m5"
        );
    }

    /// Fluxer never hides the message the person jumped to.
    #[test]
    fn the_message_jumped_to_is_shown_although_its_author_is_hidden() {
        let (state, ch) = state_with(&[
            (1, "2", "2026-01-01T10:00:00+00:00"),
            (2, "3", "2026-01-01T10:00:10+00:00"),
            (3, "2", "2026-01-01T10:00:20+00:00"),
        ]);
        let mut view = MessageView::default();
        view.hide_blocked = true;
        assert_eq!(shape(&view.build_rows(&state, ch)), ["day", "m1", "m3+"]);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        view.highlight(MessageId(2 << 22), now);
        let rows = shape(&view.build_rows(&state, ch));
        assert!(
            rows.iter().any(|r| r.starts_with("blocked")),
            "the jump target is in the list ({rows:?})"
        );
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

    /// The tab list narrows to the tabs that hold a match, best tab first, and a setting
    /// of a tab the view does not have is left out.
    #[gpui::test]
    fn a_search_narrows_the_tabs_to_those_with_a_match(cx: &mut TestAppContext) {
        let view = cx.new(SettingsView::new);
        cx.update(|cx| {
            view.update(cx, |v, _| {
                let tabs = SettingsView::matched_tabs(&v.matches("theme"));
                assert_eq!(tabs.first(), Some(&"appearance"));
                assert!(!tabs.contains(&"desktop"));
                assert!(v.matches("qqqqq").is_empty());
                v.tabs.retain(|t| t.id() != "appearance");
                let tabs = SettingsView::matched_tabs(&v.matches("theme"));
                assert!(!tabs.contains(&"appearance"));
            });
        });
    }
}
