// SPDX-License-Identifier: AGPL-3.0-or-later
//! Demo scenes of the sign-in steps (demo builds only): the screen is opened to add an
//! account and put on the step the scene names, with a fixed instance, so the screenshots
//! show every step without an instance behind them.

use std::time::Instant;

use gpui::{Context, Window};
use inlet_protocol::Identity;

use crate::app::Inlet;
use crate::backend::accounts::entry::{EntryError, EntryInfo, EntryStep, Field};
use crate::demo::signin::{Step, HOST, IDENTIFIER, ORIGIN};

impl Inlet {
    pub(crate) fn demo_sign_in(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        self.begin_add_account(window, cx);
        self.sign_in
            .instance_input
            .update(cx, |input, cx| input.set_text(HOST, cx));
        let now = Instant::now();
        let info = EntryInfo {
            origin: ORIGIN.into(),
            host: HOST.into(),
            identity: Identity::Email,
            identifier_first: step != Step::Classic,
            sso_enforced: false,
        };
        let flow = &mut self.sign_in.flow;
        flow.started(info);
        match step {
            Step::Identifier | Step::Classic => {}
            Step::Password => {
                flow.identifier = IDENTIFIER.into();
                flow.step_to(EntryStep::Password, now);
                flow.failed(
                    EntryError {
                        message: t!("entry-error-credentials-email"),
                        field: Field::Password,
                        wait_secs: None,
                        restart: false,
                    },
                    now,
                );
            }
            Step::Code => {
                flow.identifier = IDENTIFIER.into();
                // Ready to send again: no countdown in the picture.
                flow.step_to(EntryStep::Code { resend_in_secs: 0 }, now);
            }
            Step::Mfa => {
                flow.identifier = IDENTIFIER.into();
                flow.step_to(
                    EntryStep::Mfa {
                        totp: true,
                        backup_codes: true,
                        passkey: false,
                    },
                    now,
                );
            }
            Step::Ip => {
                flow.identifier = IDENTIFIER.into();
                flow.step_to(
                    EntryStep::IpAuthorization {
                        email: Some(IDENTIFIER.into()),
                        resend_in_secs: 0,
                    },
                    now,
                );
            }
        }
        self.focus_sign_in_step(window, cx);
        cx.notify();
    }
}
