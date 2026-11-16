// SPDX-License-Identifier: AGPL-3.0-or-later
//! Joining a server through an invite: the window shows the server first (its name,
//! icon and size, and who invited), and joins only when the person clicks "Join".
//!
//! The invite code is untrusted: it comes from a link in a message, a deep link or a
//! field. It is checked (`invite_code`) before a request, the preview is one
//! `GET /invites/{code}` through the limiter (no more than one invite request a second,
//! see `backend::settings::guilds`), names are cleaned, and nothing is joined without
//! the button. An invite on another instance is never sent to this one.
//!
//! "Add a server" (the plus at the end of the rail) opens a window with a field for an
//! invite code or link, and the entry for creating a server, which Inlet cannot do yet.

use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, Focusable, FontWeight, Subscription, Window,
};
use inlet_protocol::api::invites::invite_code;

use super::modal::modal;
use super::nav_dialog::{title_el, NavDialog, Soon};
use crate::app::Inlet;
use crate::backend::platform::links::{web_invite, Incoming};
use crate::backend::settings::guilds::{Cmd, Ev, Kind, Preview};
use crate::backend::settings::Cmd as SettingsCmd;
use crate::text_input::{InputEvent, TextInput};
use crate::theme;
use crate::ui::media::avatar::initials;
use crate::ui::widgets::button;

/// How far an invite is.
pub enum Stage {
    /// The preview is on its way.
    Loading,
    /// The server is shown; the button joins.
    Loaded(Box<Preview>),
    /// Joining is on its way.
    Joining(Box<Preview>),
    /// The preview failed.
    Failed(String),
    /// Joining failed; the preview stays so the person can try again.
    JoinFailed(Box<Preview>, String),
}

impl Stage {
    /// The click on "Join": the stage moves to joining and the preview it was shown with
    /// comes back, once. Nothing joins before the server was shown, while a join is on
    /// its way, or after a preview failed.
    pub fn join(&mut self) -> Option<Box<Preview>> {
        let shown = match self {
            Stage::Loaded(p) | Stage::JoinFailed(p, _) => p.clone(),
            _ => return None,
        };
        *self = Stage::Joining(shown.clone());
        Some(shown)
    }

    /// The preview's answer, when it is the one this window waits for.
    pub fn previewed(&mut self, result: Result<Box<Preview>, String>) {
        if matches!(self, Stage::Loading) {
            *self = match result {
                Ok(preview) => Stage::Loaded(preview),
                Err(why) => Stage::Failed(why),
            };
        }
    }
}

/// An invite window.
pub struct InviteView {
    pub code: String,
    pub stage: Stage,
}

/// The "add a server" window.
pub struct AddServer {
    pub input: Entity<TextInput>,
    pub error: Option<String>,
    _submit: Subscription,
}

/// What a person typed or pasted, read as an invite of this instance. A bare code or a
/// `…/invite/<code>` link; a link to another host is not this instance's invite.
pub fn read_invite(text: &str, instance: &str) -> Result<String, String> {
    let text = text.trim();
    if invite_code(text).is_ok() {
        return Ok(text.to_owned());
    }
    match web_invite(text) {
        Some(Incoming::WebInvite { origin, code }) => {
            // Host and port, as the link prompt compares them.
            let host = |o: &str| {
                url::Url::parse(o).ok().and_then(|u| {
                    u.host_str().map(|h| match u.port() {
                        Some(p) => format!("{}:{p}", h.to_ascii_lowercase()),
                        None => h.to_ascii_lowercase(),
                    })
                })
            };
            if host(&origin).is_some() && host(&origin) == host(instance) {
                Ok(code)
            } else {
                Err(t!(
                    "invite-other-instance",
                    host = host(&origin).unwrap_or_default()
                ))
            }
        }
        _ => Err(t!("invite-not-valid")),
    }
}

impl Inlet {
    /// Opens the window for an invite and asks for its preview. The code is checked
    /// first; a bad one is told so without a request.
    pub(crate) fn open_invite(&mut self, code: String, cx: &mut Context<Self>) {
        if invite_code(&code).is_err() {
            self.nav_show(
                NavDialog::Invite(InviteView {
                    code,
                    stage: Stage::Failed(t!("invite-not-valid")),
                }),
                cx,
            );
            return;
        }
        self.backend.send(SettingsCmd::Guild(Cmd::PreviewInvite {
            code: code.clone(),
        }));
        self.nav_show(
            NavDialog::Invite(InviteView {
                code,
                stage: Stage::Loading,
            }),
            cx,
        );
    }

    /// Opens the "add a server" window with the field focused.
    pub(crate) fn open_add_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| TextInput::new(t!("nav-add-placeholder"), cx).max_chars(300));
        let submit = cx.subscribe(&input, |this, _, _: &InputEvent, cx| {
            this.add_server_submit(cx)
        });
        let field = input.focus_handle(cx);
        self.focus_layer_field(&field, window, cx);
        self.nav_show(
            NavDialog::AddServer(AddServer {
                input,
                error: None,
                _submit: submit,
            }),
            cx,
        );
    }

    fn add_server_submit(&mut self, cx: &mut Context<Self>) {
        let text = match self.nav.dialog.as_ref() {
            Some(NavDialog::AddServer(a)) => a.input.read(cx).text().to_owned(),
            _ => return,
        };
        match read_invite(&text, &self.instance) {
            Ok(code) => self.open_invite(code, cx),
            Err(why) => {
                if let Some(NavDialog::AddServer(a)) = self.nav.dialog.as_mut() {
                    a.error = Some(why);
                }
                cx.notify();
            }
        }
    }

    /// The answers to the invite and leave commands.
    pub(crate) fn on_guild_event(&mut self, event: Ev, cx: &mut Context<Self>) {
        match event {
            Ev::Preview { code, result } => {
                if let Some(NavDialog::Invite(view)) = self.nav.dialog.as_mut() {
                    if view.code == code {
                        view.stage.previewed(result);
                    }
                }
            }
            Ev::Accepted { code, result } => {
                let preview = match self.nav.dialog.as_ref() {
                    Some(NavDialog::Invite(view)) if view.code == code => match &view.stage {
                        Stage::Joining(p) | Stage::JoinFailed(p, _) => Some(p.clone()),
                        _ => None,
                    },
                    _ => return,
                };
                match result {
                    Ok(joined) => {
                        self.nav.dialog = None;
                        if let Some(id) = joined.guild_id {
                            self.nav.join_target = Some(id);
                            self.nav_on_guilds(cx);
                        } else if let Some(channel) = joined.channel_id {
                            // A group DM: its conversation opens.
                            if self.selected_guild.is_none() {
                                self.chat.pending_channel_jump = Some(channel);
                            } else {
                                self.overlays.link_channel = Some(channel);
                                self.select_guild(None, cx);
                            }
                        }
                    }
                    Err(why) => {
                        if let (Some(NavDialog::Invite(view)), Some(p)) =
                            (self.nav.dialog.as_mut(), preview)
                        {
                            view.stage = Stage::JoinFailed(p, why);
                        }
                    }
                }
            }
            Ev::Instance(nav) => self.nav.instance = *nav,
            Ev::Left { guild_id, result } => {
                let asked = matches!(
                    self.nav.dialog.as_ref(),
                    Some(NavDialog::Leave { guild, .. }) if *guild == guild_id
                );
                if !asked {
                    return;
                }
                match result {
                    Ok(()) => self.nav.dialog = None,
                    Err(why) => {
                        if let Some(NavDialog::Leave { busy, error, .. }) = self.nav.dialog.as_mut()
                        {
                            *busy = false;
                            *error = Some(why);
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    /// "Join" in the invite window.
    fn join_invite(&mut self, cx: &mut Context<Self>) {
        let Some(NavDialog::Invite(view)) = self.nav.dialog.as_mut() else {
            return;
        };
        if view.stage.join().is_none() {
            return;
        }
        let code = view.code.clone();
        self.backend
            .send(SettingsCmd::Guild(Cmd::AcceptInvite { code }));
        cx.notify();
    }

    /// "Leave" in the question: sent once; the answer closes the window.
    pub(crate) fn leave_guild_confirmed(&mut self, guild: String, cx: &mut Context<Self>) {
        if let Some(NavDialog::Leave { busy, error, .. }) = self.nav.dialog.as_mut() {
            if *busy {
                return;
            }
            *busy = true;
            *error = None;
        } else {
            return;
        }
        self.backend
            .send(SettingsCmd::Guild(Cmd::Leave { guild_id: guild }));
        cx.notify();
    }

    pub(super) fn render_invite(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let Some(NavDialog::Invite(view)) = self.nav.dialog.as_ref() else {
            return modal("invite", 440., []);
        };
        let (preview, joining, line) = match &view.stage {
            Stage::Loading => (None, false, Some((t!("invite-loading"), false))),
            Stage::Failed(why) => (None, false, Some((why.clone(), true))),
            Stage::Loaded(p) => (Some(p.clone()), false, None),
            Stage::Joining(p) => (Some(p.clone()), true, None),
            Stage::JoinFailed(p, why) => (Some(p.clone()), false, Some((why.clone(), true))),
        };
        let card = match &preview {
            Some(p) => self.invite_card(p, cx),
            None => div().into_any_element(),
        };
        let already = preview
            .as_ref()
            .and_then(|p| p.guild_id.as_ref())
            .is_some_and(|id| self.guilds.iter().any(|g| &g.id == id));
        let mut children = vec![title_el(t!("invite-title"))];
        children.push(card);
        if let Some((text, bad)) = line {
            children.push(
                div()
                    .text_size(px(14.))
                    .text_color(if bad {
                        theme::danger()
                    } else {
                        theme::text_muted()
                    })
                    .child(text)
                    .into_any_element(),
            );
        }
        if already {
            children.push(
                div()
                    .text_size(px(14.))
                    .text_color(theme::text_muted())
                    .child(t!("invite-already"))
                    .into_any_element(),
            );
        }
        let mut buttons = div().flex().flex_row().justify_end().gap_2().child(
            button("invite-cancel", t!("common-cancel"), false)
                .on_click(cx.listener(|this, _, _, cx| this.nav_close(cx))),
        );
        if preview.is_some() {
            buttons = buttons.child(
                button(
                    "invite-join",
                    if joining {
                        t!("invite-joining")
                    } else {
                        t!("invite-join")
                    },
                    true,
                )
                .when(joining, |b| b.opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| this.join_invite(cx))),
            );
        }
        children.push(buttons.into_any_element());
        modal("invite", 440., children)
    }

    /// The server an invite leads to: icon, name, size and who invited.
    fn invite_card(&mut self, p: &Preview, cx: &mut Context<Self>) -> AnyElement {
        let icon = match (&p.guild_id, &p.icon) {
            (Some(id), Some(hash)) => self
                .guild_icon_for(id, hash, 64., cx)
                .map(|i| i.rounded(px(16.))),
            _ => None,
        };
        let size = match (p.members, p.online) {
            (Some(m), Some(o)) => Some(t!("invite-size", members = m, online = o)),
            (Some(m), None) => Some(t!("invite-members", members = m)),
            _ => None,
        };
        let kind_line = match (p.kind, &p.channel_name) {
            (Kind::GroupDm, _) => Some(t!("invite-group-dm")),
            (Kind::Guild, Some(c)) => Some(t!("invite-channel", channel = c.as_str())),
            _ => None,
        };
        div()
            .id("invite-card")
            .debug_selector(|| "invite-card".into())
            .flex()
            .flex_row()
            .gap_3()
            .items_center()
            .p_3()
            .rounded(px(8.))
            .bg(theme::bg_textarea())
            .child(match icon {
                Some(i) => i.into_any_element(),
                None => initials(&p.guild_name, 64.)
                    .rounded(px(16.))
                    .into_any_element(),
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::text_strong())
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(p.guild_name.clone()),
                    )
                    .children(size.map(|s| {
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child(s)
                    }))
                    .children(kind_line.map(|s| {
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child(s)
                    }))
                    .children(p.inviter.clone().map(|i| {
                        div()
                            .text_size(px(13.))
                            .text_color(theme::text_muted())
                            .child(t!("invite-by", name = i.as_str()))
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_add_server(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let Some(NavDialog::AddServer(add)) = self.nav.dialog.as_ref() else {
            return modal("add-server", 480., []);
        };
        let (input, error) = (add.input.clone(), add.error.clone());
        modal(
            "add-server",
            480.,
            [
                title_el(t!("nav-add-title")),
                button("add-server-create", t!("nav-create-server"), false)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.nav_show(NavDialog::Soon(Soon::CreateServer), cx)
                    }))
                    .into_any_element(),
                div()
                    .text_color(theme::text_muted())
                    .child(t!("nav-add-body"))
                    .into_any_element(),
                input.into_any_element(),
                div()
                    .children(error.map(|e| {
                        div()
                            .text_size(px(14.))
                            .text_color(theme::danger())
                            .child(e)
                    }))
                    .into_any_element(),
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap_2()
                    .child(
                        button("add-server-cancel", t!("common-cancel"), false)
                            .on_click(cx.listener(|this, _, _, cx| this.nav_close(cx))),
                    )
                    .child(
                        button("add-server-join", t!("nav-add-join"), true)
                            .on_click(cx.listener(|this, _, _, cx| this.add_server_submit(cx))),
                    )
                    .into_any_element(),
            ],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_accept_shows_the_guild_before_joining() {
        // A pasted code or link is read as an invite of this instance only.
        let at = "https://chat.example";
        assert_eq!(read_invite("  aB3dEf7h ", at), Ok("aB3dEf7h".into()));
        assert_eq!(
            read_invite("https://chat.example/invite/my-guild", at),
            Ok("my-guild".into())
        );
        for bad in [
            "https://other.example/invite/abc",
            "../users/@me",
            "abc?x=1",
            "http://chat.example/invite/abc",
            "",
            "a b",
        ] {
            assert!(read_invite(bad, at).is_err(), "{bad:?}");
        }
        // The stages: the preview comes first; only a click on a shown server joins.
        let preview = || {
            Box::new(Preview {
                code: "abc".into(),
                kind: Kind::Guild,
                guild_id: Some("55".into()),
                guild_name: "Hedgerow".into(),
                icon: None,
                channel_name: Some("general".into()),
                channel_id: Some("7".into()),
                inviter: Some("Ada".into()),
                members: Some(12),
                online: Some(3),
            })
        };
        let mut stage = Stage::Loading;
        assert!(stage.join().is_none(), "nothing joins before the preview");
        stage.previewed(Err("expired".into()));
        assert!(matches!(&stage, Stage::Failed(_)));
        assert!(
            stage.join().is_none(),
            "nothing joins after a failed preview"
        );
        let mut stage = Stage::Loading;
        stage.previewed(Ok(preview()));
        match &stage {
            Stage::Loaded(p) => assert_eq!(p.guild_name, "Hedgerow"),
            _ => panic!("the server is shown"),
        }
        // A late second answer does not replace what is shown.
        stage.previewed(Err("late".into()));
        assert!(matches!(&stage, Stage::Loaded(_)));
        // One click joins; a second while it is on its way does nothing.
        assert_eq!(stage.join().map(|p| p.code), Some("abc".into()));
        assert!(matches!(&stage, Stage::Joining(_)));
        assert!(stage.join().is_none());
        // A failed join keeps the server so the person can try again.
        stage = Stage::JoinFailed(preview(), "full".into());
        assert!(stage.join().is_some());
    }
}
