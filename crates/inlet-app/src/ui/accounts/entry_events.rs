// SPDX-License-Identifier: AGPL-3.0-or-later
//! What the in-app sign-in's backend tells the form: the instance that was found, the
//! next step, an error, a countdown that restarted.

use std::time::Instant;

use gpui::{Context, Window};

use super::flow::Step;
use super::SignInMode;
use crate::app::Inlet;
use crate::backend::accounts::entry::{Ev, Field};

impl Inlet {
    pub(crate) fn on_entry_event(
        &mut self,
        event: Ev,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        match event {
            Ev::Started(info) => {
                let sso_only = info.sso_enforced;
                self.sign_in.flow.started(info);
                // Single sign-on is not done in the app: the browser is the way in.
                if sso_only {
                    self.sign_in.mode = SignInMode::Browser;
                }
                // The identifier of an earlier try does not carry over to this instance.
                self.sign_in
                    .identifier_input
                    .update(cx, |i, cx| i.clear(cx));
                self.focus_sign_in_step(window, cx);
            }
            Ev::Step(step) => {
                self.sign_in.flow.step_to(step, now);
                self.sign_in.code_input.update(cx, |i, cx| i.clear(cx));
                self.focus_sign_in_step(window, cx);
                self.start_sign_in_clock(cx);
            }
            Ev::CodeResent { resend_in_secs } => {
                self.sign_in.flow.code_resent(resend_in_secs, now);
                self.start_sign_in_clock(cx);
            }
            Ev::IpResent { resend_in_secs } => {
                self.sign_in.flow.ip_resent(resend_in_secs, now);
                self.start_sign_in_clock(cx);
            }
            Ev::IpLost => self.sign_in.flow.ip_lost(),
            Ev::NoAccount {
                registration_closed,
            } => {
                self.sign_in.flow.no_account(registration_closed);
                self.focus_sign_in_step(window, cx);
            }
            Ev::Lost => {
                self.sign_in_reset_flow(cx);
                self.sign_in.flow.error = Some(super::flow::FormError {
                    message: t!("entry-error-timed-out"),
                    field: Field::Instance,
                });
                self.focus_sign_in_step(window, cx);
            }
            Ev::Failed(err) => {
                let field = err.field;
                self.sign_in.flow.failed(err, now);
                // The cursor goes where the error is, unless it is about the instance.
                if self.sign_in.flow.step != Step::Instance || field == Field::Instance {
                    self.focus_sign_in_step(window, cx);
                }
                self.start_sign_in_clock(cx);
            }
        }
        cx.notify();
    }
}
