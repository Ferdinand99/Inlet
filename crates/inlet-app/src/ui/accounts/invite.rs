// SPDX-License-Identifier: AGPL-3.0-or-later
//! An invite link to another instance: the prompt offers to sign in there, in the app.
//! Once the sign-in worked the link is asked about again, now as an invite on the
//! signed-in instance.

use std::time::Instant;

use gpui::{prelude::*, Context, Div, Stateful, Window};

use super::{host_of, SignInMode};
use crate::app::Inlet;
use crate::backend::accounts::{entry, Cmd};
use crate::backend::platform::links::{web_invite, Incoming};
use crate::ui::widgets::button;

impl Inlet {
    /// The "Sign in on <host>" button of the invite prompt, for an invite on an instance
    /// the app is not signed in to. `None` for an invite on the signed-in instance or a
    /// link that is not an invite.
    pub(crate) fn invite_sign_in_button(
        &self,
        url: &str,
        known_instance: bool,
        cx: &Context<Self>,
    ) -> Option<Stateful<Div>> {
        if known_instance {
            return None;
        }
        let Some(Incoming::WebInvite { origin, code }) = web_invite(url) else {
            return None;
        };
        let host = host_of(&origin)?;
        Some(
            button(
                "link-prompt-invite-sign-in",
                t!("link-prompt-invite-sign-in", host = host),
                false,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.sign_in_for_invite(origin.clone(), code.clone(), window, cx)
            })),
        )
    }

    /// Opens the sign-in screen for the instance an invite is on, and looks it up.
    pub(super) fn sign_in_for_invite(
        &mut self,
        origin: String,
        code: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = host_of(&origin) else {
            return;
        };
        self.overlays.link_prompt = None;
        self.begin_add_account(window, cx);
        // The sign-in requests carry the invite, so the instance joins the server.
        self.backend
            .send(Cmd::Entry(entry::Cmd::Invite(Some(code.clone()))));
        self.sign_in.invite = Some((origin, code));
        self.sign_in.mode = SignInMode::Password;
        self.sign_in
            .instance_input
            .update(cx, |input, cx| input.set_text(host.clone(), cx));
        if self.sign_in.flow.begin(Instant::now()) {
            self.backend
                .send(Cmd::Entry(entry::Cmd::Start { instance: host }));
            self.start_sign_in_clock(cx);
        }
        cx.notify();
    }

    /// The person left the sign-in screen: what it kept goes, and so does the invite.
    pub(crate) fn sign_in_left(&mut self, cx: &mut Context<Self>) {
        self.sign_in_reset_flow(cx);
        self.sign_in.invite = None;
    }
}
