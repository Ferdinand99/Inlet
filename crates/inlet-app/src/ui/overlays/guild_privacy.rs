// SPDX-License-Identifier: AGPL-3.0-or-later
//! The privacy window of one server (from its menu): whether its members, and whether its
//! bots, may send the account direct messages. Fluxer keeps this in the account settings
//! as two lists, `restricted_guilds` and `bot_restricted_guilds`, of the servers where
//! they may not; the default for servers joined later is the privacy tab's own switch.
//! The whole list is written, with the same writer as the other account settings.

use gpui::{div, prelude::*, px, Context, SharedString};
use serde_json::Value;

use super::modal::modal;
use super::nav_dialog::{title_el, NavDialog};
use crate::app::Inlet;
use crate::assets::Icon;
use crate::theme;
use crate::ui::settings::store;
use crate::ui::widgets::button;

/// Servers kept in one list when it is written back (Fluxer's guild limit is far lower).
const MAX_LISTED: usize = 1_000;

/// The IDs a list of the account settings holds.
pub fn listed(extra: &serde_json::Map<String, Value>, key: &str) -> Vec<String> {
    extra
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) if n.is_u64() => Some(n.to_string()),
                    _ => None,
                })
                .filter(|s| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()))
                .take(MAX_LISTED)
                .collect()
        })
        .unwrap_or_default()
}

/// The list with `guild` in it (`restricted`) or out of it, once.
pub fn with_guild(mut list: Vec<String>, guild: &str, restricted: bool) -> Vec<String> {
    list.retain(|g| g != guild);
    if restricted && list.len() < MAX_LISTED {
        list.push(guild.to_owned());
    }
    list
}

/// What the window shows: whether DMs from the server's members and bots are allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Allowed {
    pub members: bool,
    pub bots: bool,
}

impl Inlet {
    /// What the account settings say about `guild`.
    fn guild_dms_allowed(&self, guild: &str, cx: &gpui::App) -> Allowed {
        let user = self
            .account
            .as_ref()
            .and_then(|a| a.read(cx).settings().user().cloned());
        let extra = user.map(|u| u.extra).unwrap_or_default();
        Allowed {
            members: !listed(&extra, "restricted_guilds")
                .iter()
                .any(|g| g == guild),
            bots: !listed(&extra, "bot_restricted_guilds")
                .iter()
                .any(|g| g == guild),
        }
    }

    /// Opens the privacy window of a server.
    pub(crate) fn open_guild_privacy(&mut self, guild: String, cx: &mut Context<Self>) {
        let name = self
            .guilds
            .iter()
            .find(|g| g.id == guild)
            .map(|g| g.name.clone())
            .unwrap_or_default();
        let allowed = self.guild_dms_allowed(&guild, cx);
        self.nav_show(
            NavDialog::Privacy {
                guild,
                name,
                allowed,
            },
            cx,
        );
    }

    /// A switch of the window: shown at once, and the whole list written.
    fn set_guild_dms(&mut self, bots: bool, allowed: bool, cx: &mut Context<Self>) {
        let Some(NavDialog::Privacy {
            guild,
            allowed: shown,
            ..
        }) = self.nav.dialog.as_mut()
        else {
            return;
        };
        if bots {
            shown.bots = allowed;
        } else {
            shown.members = allowed;
        }
        let guild = guild.clone();
        let key = if bots {
            "bot_restricted_guilds"
        } else {
            "restricted_guilds"
        };
        let extra = self
            .account
            .as_ref()
            .and_then(|a| a.read(cx).settings().user().cloned())
            .map(|u| u.extra)
            .unwrap_or_default();
        let list = with_guild(listed(&extra, key), &guild, !allowed);
        store::restrict_guilds(cx, 0, bots, list);
        cx.notify();
    }

    pub(super) fn render_guild_privacy(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let Some(NavDialog::Privacy { name, allowed, .. }) = self.nav.dialog.as_ref() else {
            return modal("guild-privacy", 480., []);
        };
        let (name, allowed) = (name.clone(), *allowed);
        let toggle =
            |id: &'static str, label: String, on: bool, bots: bool, cx: &mut Context<Self>| {
                div()
                    .id(SharedString::from(id))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .text_color(theme::text())
                    .child(
                        div()
                            .size(px(20.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(if on {
                                theme::brand()
                            } else {
                                theme::bg_textarea()
                            })
                            .when(on, |d| {
                                d.child(Icon::Check.el(px(14.), theme::text_strong()))
                            }),
                    )
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_guild_dms(bots, !on, cx)))
                    .into_any_element()
            };
        let members = toggle(
            "guild-privacy-members",
            t!("nav-privacy-members"),
            allowed.members,
            false,
            cx,
        );
        let bots = toggle(
            "guild-privacy-bots",
            t!("nav-privacy-bots"),
            allowed.bots,
            true,
            cx,
        );
        modal(
            "guild-privacy",
            480.,
            [
                title_el(t!("nav-privacy-title", name = name.as_str())),
                div()
                    .text_color(theme::text_muted())
                    .child(t!("nav-privacy-body"))
                    .into_any_element(),
                members,
                bots,
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .child(
                        button("guild-privacy-done", t!("nav-privacy-done"), true)
                            .on_click(cx.listener(|this, _, _, cx| this.nav_close(cx))),
                    )
                    .into_any_element(),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_server_enters_and_leaves_the_restricted_list_once() {
        let list = vec!["1".to_owned(), "2".to_owned()];
        // Restricting adds it once, allowing takes it out, others stay.
        assert_eq!(with_guild(list.clone(), "3", true), ["1", "2", "3"]);
        assert_eq!(with_guild(list.clone(), "2", true), ["1", "2"]);
        assert_eq!(with_guild(list.clone(), "1", false), ["2"]);
        assert_eq!(with_guild(list, "9", false), ["1", "2"]);
    }

    #[test]
    fn the_lists_are_read_from_the_account_settings_and_bounded() {
        let extra = json!({
            "restricted_guilds": ["10", 11, "x", "../1", null],
            "bot_restricted_guilds": "not a list"
        });
        let extra = extra.as_object().unwrap();
        assert_eq!(listed(extra, "restricted_guilds"), ["10", "11"]);
        assert!(listed(extra, "bot_restricted_guilds").is_empty());
        assert!(listed(extra, "missing").is_empty());
        let big: Vec<String> = (1..2_000).map(|i| i.to_string()).collect();
        let extra = json!({ "restricted_guilds": big });
        assert_eq!(
            listed(extra.as_object().unwrap(), "restricted_guilds").len(),
            MAX_LISTED
        );
    }
}
