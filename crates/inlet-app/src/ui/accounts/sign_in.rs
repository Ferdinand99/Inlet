// SPDX-License-Identifier: AGPL-3.0-or-later
//! The sign-in form: signing in inside the app (email or username and password, an
//! emailed code, an authenticator or backup code, the new-IP email), the browser sign-in,
//! or a pasted token.
//!
//! The password and the codes are typed into inputs, read once when the person submits,
//! wrapped in a [`p::Secret`] at once and sent to the backend; the input is cleared in the
//! same step, so no field keeps one after a submit. Nothing here is logged or kept.

use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, ClickEvent, Context, Div, Focusable, FontWeight, Stateful, Window,
};
use inlet_protocol as p;

use super::flow::{Flow, Step};
use super::{hash_text, safe_handoff_url, SignInMode};
use crate::app::{Inlet, Screen};
use crate::backend::accounts::entry;
use crate::backend::accounts::Cmd;
use crate::theme;
use crate::ui::markdown;
use crate::ui::widgets::{button, field};

/// A text link under a form: quiet, but a real click target.
fn quiet(id: &'static str, label: String) -> Stateful<Div> {
    div()
        .id(id)
        .text_size(px(13.))
        .text_color(theme::text_muted())
        .cursor_pointer()
        .hover(|s| s.text_color(theme::text()))
        .child(label)
}

/// A quiet link that cannot be clicked now (a countdown is running).
fn quiet_off(id: &'static str, label: String) -> Stateful<Div> {
    div()
        .id(id)
        .text_size(px(13.))
        .text_color(theme::text_muted())
        .opacity(0.6)
        .child(label)
}

fn heading(title: String, text: Option<String>) -> Div {
    let mut head = div().flex().flex_col().gap_1().child(
        div()
            .text_size(px(18.))
            .font_weight(FontWeight::BOLD)
            .text_color(theme::text_strong())
            .child(title),
    );
    if let Some(text) = text {
        head = head.child(div().text_color(theme::text_muted()).child(text));
    }
    head
}

impl Inlet {
    /// Enter or a click on the main button of the step on screen.
    pub(crate) fn submit_sign_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sign_in.busy || self.screen != Screen::SignIn || self.sign_in.handoff.is_some() {
            return;
        }
        if self.sign_in.flow.step != Step::Instance {
            return self.submit_entry_step(window, cx);
        }
        let instance = self
            .sign_in
            .instance_input
            .read(cx)
            .text()
            .trim()
            .to_string();
        if instance.is_empty() {
            self.sign_in.error = Some(t!("sign-in-enter-instance"));
            cx.notify();
            return;
        }
        let command = match self.sign_in.mode {
            SignInMode::Password => {
                if !self.sign_in.flow.begin(Instant::now()) {
                    return;
                }
                self.sign_in.error = None;
                self.sign_in.notice = None;
                self.backend
                    .send(Cmd::Entry(entry::Cmd::Start { instance }));
                self.start_sign_in_clock(cx);
                cx.notify();
                return;
            }
            SignInMode::Browser => Cmd::SignInBrowser { instance },
            SignInMode::Token => {
                let token = self.sign_in.token_input.read(cx).text().trim().to_string();
                if token.is_empty() {
                    self.sign_in.error = Some(t!("sign-in-paste-first"));
                    cx.notify();
                    return;
                }
                self.sign_in.pasted_token_hash = Some(hash_text(&token));
                Cmd::SignInToken {
                    instance,
                    token: inlet_protocol::Token::new(token),
                }
            }
        };
        self.sign_in.busy = true;
        self.sign_in.error = None;
        self.sign_in.notice = None;
        self.backend.send(command);
        cx.notify();
    }

    /// Sends what the step on screen asks for. A password or a code is read here, put
    /// into a secret and cleared from its field at once.
    fn submit_entry_step(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let now = Instant::now();
        let text = |input: &gpui::Entity<crate::text_input::TextInput>, cx: &Context<Self>| {
            input.read(cx).text().to_string()
        };
        let command = match self.sign_in.flow.step {
            Step::Instance | Step::IpAuth => return,
            Step::Identifier => {
                let identifier = text(&self.sign_in.identifier_input, cx).trim().to_string();
                if identifier.is_empty() {
                    return;
                }
                if !self.sign_in.flow.begin(now) {
                    return;
                }
                self.sign_in.flow.identifier = identifier.clone();
                entry::Cmd::Identify { identifier }
            }
            Step::Credentials | Step::Password => {
                let classic = self.sign_in.flow.step == Step::Credentials;
                let identifier = if classic {
                    text(&self.sign_in.identifier_input, cx).trim().to_string()
                } else {
                    self.sign_in.flow.identifier.clone()
                };
                let password = p::Secret::new(text(&self.sign_in.password_input, cx));
                if identifier.is_empty() || password.is_empty() {
                    return;
                }
                if !self.sign_in.flow.begin(now) {
                    return;
                }
                self.sign_in.flow.identifier = identifier.clone();
                self.sign_in.password_input.update(cx, |i, cx| i.clear(cx));
                entry::Cmd::Password {
                    identifier,
                    password,
                }
            }
            Step::Code | Step::Mfa => {
                let code = p::Secret::new(text(&self.sign_in.code_input, cx));
                if code.is_empty() {
                    return;
                }
                if !self.sign_in.flow.begin(now) {
                    return;
                }
                self.sign_in.code_input.update(cx, |i, cx| i.clear(cx));
                if self.sign_in.flow.step == Step::Code {
                    entry::Cmd::Code { code }
                } else {
                    entry::Cmd::Mfa { code }
                }
            }
        };
        self.backend.send(Cmd::Entry(command));
        self.start_sign_in_clock(cx);
        cx.notify();
    }

    /// Ticks the screen once a second while something on it counts down or waits (a
    /// resend countdown, a pause the instance asked for, the "still working" hint).
    pub(crate) fn start_sign_in_clock(&mut self, cx: &mut Context<Self>) {
        if self.sign_in.clock.is_some() {
            return;
        }
        self.sign_in.clock = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let keep = this.update(cx, |this, cx| {
                let keep = this.sign_in.flow.needs_clock(Instant::now());
                if !keep {
                    // Detach, so letting go of the handle does not cancel the task that
                    // is running this very closure.
                    if let Some(task) = this.sign_in.clock.take() {
                        task.detach();
                    }
                }
                cx.notify();
                keep
            });
            if !matches!(keep, Ok(true)) {
                break;
            }
        }));
    }

    fn cancel_wait(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.backend.send(Cmd::CancelSignIn);
        self.sign_in.handoff = None;
        self.sign_in.busy = false;
        cx.notify();
    }

    /// Sends the person to the first step of the form, as "Use a different email" does.
    fn entry_restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.send(Cmd::Entry(entry::Cmd::Back));
        self.sign_in.flow.restart();
        self.sign_in.flow.error = None;
        self.sign_in.password_input.update(cx, |i, cx| i.clear(cx));
        self.sign_in.code_input.update(cx, |i, cx| i.clear(cx));
        self.focus_sign_in_step(window, cx);
        cx.notify();
    }

    /// Back to the instance step: forget what was found.
    fn entry_change_instance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backend.send(Cmd::Entry(entry::Cmd::Leave));
        self.sign_in_reset_flow(cx);
        self.sign_in
            .instance_input
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    /// The sign-in in the browser, from any step: the same code flow as the browser tab.
    fn entry_use_browser(&mut self, cx: &mut Context<Self>) {
        let instance = self
            .sign_in
            .instance_input
            .read(cx)
            .text()
            .trim()
            .to_string();
        if instance.is_empty() || self.sign_in.busy {
            return;
        }
        self.sign_in.busy = true;
        self.sign_in.error = None;
        self.backend.send(Cmd::SignInBrowser { instance });
        cx.notify();
    }

    /// Everything the form kept: the step, the typed identifier and the codes.
    pub(crate) fn sign_in_reset_flow(&mut self, cx: &mut Context<Self>) {
        self.sign_in.flow.reset();
        for input in [
            &self.sign_in.identifier_input,
            &self.sign_in.password_input,
            &self.sign_in.code_input,
        ] {
            input.update(cx, |i, cx| i.clear(cx));
        }
    }

    pub(crate) fn render_sign_in(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut form = div().flex().flex_col().gap_3();
        if let Some((url, code)) = self.sign_in.handoff.clone() {
            form = self.handoff_view(form, url, code, cx);
        } else {
            form = match self.sign_in.flow.step {
                Step::Instance => self.instance_step(form, cx),
                Step::Identifier => self.identifier_step(form, cx),
                Step::Credentials => self.credentials_step(form, cx),
                Step::Password => self.password_step(form, cx),
                Step::Code => self.code_step(form, cx),
                Step::Mfa => self.mfa_step(form, cx),
                Step::IpAuth => self.ip_step(form, cx),
            };
        }
        // The steps put their error under the field; the first step and the browser
        // sign-in have it last.
        if self.sign_in.handoff.is_some() || self.sign_in.flow.step == Step::Instance {
            if let Some(line) = self.error_line() {
                form = form.child(line);
            }
        }
        let notice = self
            .sign_in
            .flow
            .notice
            .as_ref()
            .or(self.sign_in.notice.as_ref());
        if let Some(notice) = notice {
            form = form.child(div().text_color(theme::text_muted()).child(notice.clone()));
        }
        if self.sign_in.flow.working_hint(Instant::now()) {
            form = form.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(t!("entry-still-working")),
            );
        }
        let at_first_step = self.sign_in.flow.step == Step::Instance;
        if self.sign_in_has_way_back()
            && self.sign_in.handoff.is_none()
            && !self.sign_in.busy
            && !self.sign_in.flow.busy()
            && at_first_step
        {
            form =
                form.child(button("sign-in-back", t!("sign-in-back"), false).on_click(
                    cx.listener(|this, _, window, cx| this.back_from_sign_in(window, cx)),
                ));
        }
        form = form.child(Self::about_link(t!("about-link"), cx));
        self.render_card(form)
    }

    /// The error to show: the one of the in-app flow, else the one of the other ways.
    fn error_line(&self) -> Option<Div> {
        let message = self
            .sign_in
            .flow
            .error
            .as_ref()
            .map(|e| e.message.clone())
            .or_else(|| self.sign_in.error.clone())?;
        Some(div().text_color(theme::danger()).child(message))
    }

    fn busy_label(&self, idle: String) -> String {
        if self.sign_in.busy || self.sign_in.flow.busy() {
            t!("signing-in")
        } else {
            idle
        }
    }

    /// The identifier label of this instance: email or username.
    fn identifier_label(&self) -> String {
        match self.sign_in.flow.info.as_ref().map(|i| i.identity) {
            Some(p::Identity::Username) => t!("entry-label-username"),
            _ => t!("entry-label-email"),
        }
    }

    fn handoff_view(&self, form: Div, url: String, code: String, cx: &mut Context<Self>) -> Div {
        form.child(div().child(t!("sign-in-handoff-instructions")))
            .child(
                div()
                    .text_size(px(28.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_strong())
                    .font_family(markdown::mono_font())
                    .child(code),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(url.clone()),
            )
            .child(
                div()
                    .text_color(theme::text_muted())
                    .child(t!("sign-in-waiting")),
            )
            .child(
                button("reopen", t!("sign-in-reopen"), false).on_click(move |_, _, cx| {
                    if safe_handoff_url(&url) {
                        cx.open_url(&url)
                    }
                }),
            )
            .child(
                button("cancel-wait", t!("common-cancel"), false)
                    .on_click(cx.listener(Self::cancel_wait)),
            )
    }

    /// Which instance, and how to sign in there.
    fn instance_step(&mut self, form: Div, cx: &mut Context<Self>) -> Div {
        let mode = self.sign_in.mode;
        let tab = |id: &'static str, label: String, active: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_md()
                .cursor_pointer()
                .text_color(if active {
                    theme::text_strong()
                } else {
                    theme::text_muted()
                })
                .when(active, |d| d.bg(theme::selected_bg()))
                .hover(|s| s.bg(theme::hover_bg()))
                .child(label)
        };
        let mut form = form;
        if self.sign_in.invite.is_some() {
            let host = self.sign_in.invite_host().unwrap_or_default();
            form = form.child(
                div()
                    .text_color(theme::text_strong())
                    .child(t!("sign-in-invite-note", host = host)),
            );
        }
        form = form
            .child(field(
                t!("sign-in-instance-label"),
                self.sign_in.instance_input.clone(),
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        tab(
                            "mode-password",
                            t!("sign-in-mode-password"),
                            mode == SignInMode::Password,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sign_in.mode = SignInMode::Password;
                            cx.notify();
                        })),
                    )
                    .child(
                        tab(
                            "mode-browser",
                            t!("sign-in-mode-browser"),
                            mode == SignInMode::Browser,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sign_in.mode = SignInMode::Browser;
                            cx.notify();
                        })),
                    )
                    .child(
                        tab(
                            "mode-token",
                            t!("sign-in-mode-token"),
                            mode == SignInMode::Token,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.sign_in.mode = SignInMode::Token;
                            cx.notify();
                        })),
                    ),
            );
        form = match mode {
            SignInMode::Password => form.child(
                div()
                    .text_size(px(13.))
                    .text_color(theme::text_muted())
                    .child(t!("sign-in-password-hint")),
            ),
            SignInMode::Browser => form.child(
                div()
                    .text_size(px(13.))
                    .text_color(theme::text_muted())
                    .child(t!("sign-in-browser-hint")),
            ),
            SignInMode::Token => form
                .child(field(
                    t!("sign-in-token-label"),
                    self.sign_in.token_input.clone(),
                ))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::text_muted())
                        .child(t!("sign-in-token-hint")),
                ),
        };
        let idle = match mode {
            SignInMode::Password => t!("sign-in-continue"),
            SignInMode::Browser => t!("sign-in-open-page"),
            SignInMode::Token => t!("sign-in-submit"),
        };
        let label = self.busy_label(idle);
        form.child(
            button("sign-in", label, true)
                .on_click(cx.listener(|this, _, window, cx| this.submit_sign_in(window, cx))),
        )
    }

    /// The instance a later step is on, with a way to change it.
    fn instance_line(&self, cx: &mut Context<Self>) -> Div {
        let host = self
            .sign_in
            .flow
            .info
            .as_ref()
            .map(|i| i.host.clone())
            .unwrap_or_default();
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(t!("entry-on-instance", host = host)),
            )
            .child(
                quiet("entry-change-instance", t!("entry-change-instance")).on_click(
                    cx.listener(|this, _, window, cx| this.entry_change_instance(window, cx)),
                ),
            )
    }

    fn browser_link(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        quiet("entry-use-browser", t!("entry-use-browser"))
            .on_click(cx.listener(|this, _, _, cx| this.entry_use_browser(cx)))
    }

    fn submit_button(
        &self,
        id: &'static str,
        idle: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        button(id, self.busy_label(idle), true)
            .on_click(cx.listener(|this, _, window, cx| this.submit_sign_in(window, cx)))
    }

    /// Identifier-first: the email address or username.
    fn identifier_step(&mut self, form: Div, cx: &mut Context<Self>) -> Div {
        form.child(self.instance_line(cx))
            .child(heading(t!("entry-title-sign-in"), None))
            .child(field(
                self.identifier_label(),
                self.sign_in.identifier_input.clone(),
            ))
            .children(self.error_line())
            .child(self.submit_button("entry-continue", t!("sign-in-continue"), cx))
            .child(self.browser_link(cx))
    }

    /// The classic form: identifier and password together.
    fn credentials_step(&mut self, form: Div, cx: &mut Context<Self>) -> Div {
        form.child(self.instance_line(cx))
            .child(heading(t!("entry-title-welcome"), None))
            .child(field(
                self.identifier_label(),
                self.sign_in.identifier_input.clone(),
            ))
            .child(field(
                t!("entry-label-password"),
                self.sign_in.password_input.clone(),
            ))
            .children(self.error_line())
            .child(self.submit_button("entry-sign-in", t!("sign-in-submit"), cx))
            .child(self.browser_link(cx))
    }

    /// Identifier-first: the password of the account that was found.
    fn password_step(&mut self, form: Div, cx: &mut Context<Self>) -> Div {
        let different = match self.sign_in.flow.info.as_ref().map(|i| i.identity) {
            Some(p::Identity::Username) => t!("entry-different-username"),
            _ => t!("entry-different-email"),
        };
        form.child(self.instance_line(cx))
            .child(heading(
                t!("entry-title-password"),
                Some(self.sign_in.flow.identifier.clone()),
            ))
            .child(field(
                t!("entry-label-password"),
                self.sign_in.password_input.clone(),
            ))
            .children(self.error_line())
            .child(self.submit_button("entry-sign-in", t!("sign-in-submit"), cx))
            .child(
                quiet("entry-different", different)
                    .on_click(cx.listener(|this, _, window, cx| this.entry_restart(window, cx))),
            )
            .child(self.browser_link(cx))
    }

    /// The code that was emailed.
    fn code_step(&mut self, form: Div, cx: &mut Context<Self>) -> Div {
        let flow = &self.sign_in.flow;
        let left = Flow::seconds_until(flow.code_resend_at, Instant::now());
        let different = match flow.info.as_ref().map(|i| i.identity) {
            Some(p::Identity::Username) => t!("entry-different-username"),
            _ => t!("entry-different-email"),
        };
        let sent_to = t!("entry-code-sent-to", email = flow.identifier.clone());
        let resend = if left > 0 {
            quiet_off("entry-resend", t!("entry-send-new-code-in", seconds = left))
        } else {
            quiet("entry-resend", t!("entry-send-new-code")).on_click(cx.listener(
                |this, _, _, cx| {
                    if this.sign_in.flow.begin(Instant::now()) {
                        this.backend.send(Cmd::Entry(entry::Cmd::SendCode));
                        this.start_sign_in_clock(cx);
                        cx.notify();
                    }
                },
            ))
        };
        form.child(self.instance_line(cx))
            .child(heading(t!("entry-title-code"), Some(sent_to)))
            .child(field(
                t!("entry-label-code"),
                self.sign_in.code_input.clone(),
            ))
            .children(self.error_line())
            .child(self.submit_button("entry-sign-in", t!("sign-in-submit"), cx))
            .child(resend)
            .child(
                quiet("entry-different", different)
                    .on_click(cx.listener(|this, _, window, cx| this.entry_restart(window, cx))),
            )
    }

    /// The authenticator or backup code.
    fn mfa_step(&mut self, form: Div, cx: &mut Context<Self>) -> Div {
        let mfa = self.sign_in.flow.mfa;
        let back = quiet("entry-back", t!("entry-back-to-sign-in"))
            .on_click(cx.listener(|this, _, window, cx| this.entry_restart(window, cx)));
        let form = form
            .child(self.instance_line(cx))
            .child(heading(t!("entry-title-mfa"), None));
        if !mfa.takes_a_code() {
            // A passkey or security key only: Inlet cannot use it yet.
            return form
                .child(
                    div()
                        .text_color(theme::text_muted())
                        .child(t!("entry-mfa-passkey-only")),
                )
                .child(
                    button("entry-use-browser-button", t!("entry-use-browser"), true)
                        .on_click(cx.listener(|this, _, _, cx| this.entry_use_browser(cx))),
                )
                .child(back);
        }
        let text = if mfa.totp {
            t!("entry-mfa-text")
        } else {
            t!("entry-mfa-backup-only")
        };
        let label = if mfa.totp {
            t!("entry-label-authenticator")
        } else {
            t!("entry-label-backup")
        };
        form.child(div().text_color(theme::text_muted()).child(text))
            .child(field(label, self.sign_in.code_input.clone()))
            .children(self.error_line())
            .child(self.submit_button("entry-sign-in", t!("sign-in-submit"), cx))
            .child(back)
    }

    /// Waiting for the link in the new-IP email.
    fn ip_step(&mut self, form: Div, cx: &mut Context<Self>) -> Div {
        let Some(ip) = self.sign_in.flow.ip.clone() else {
            return form;
        };
        let back = quiet("entry-back", t!("entry-back-to-sign-in"))
            .on_click(cx.listener(|this, _, window, cx| this.entry_restart(window, cx)));
        if ip.lost {
            return form
                .child(heading(
                    t!("entry-title-ip-lost"),
                    Some(t!("entry-ip-lost-text")),
                ))
                .child(
                    button("entry-try-again", t!("entry-try-again"), true).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.sign_in.flow.ip_retrying();
                            this.backend.send(Cmd::Entry(entry::Cmd::IpRetry));
                            cx.notify();
                        },
                    )),
                )
                .child(back);
        }
        let text = match &ip.email {
            Some(email) => t!("entry-ip-text", email = email.clone()),
            None => t!("entry-ip-text-no-address"),
        };
        let left = Flow::seconds_until(Some(ip.resend_at), Instant::now());
        let resend = if ip.resent {
            button("entry-ip-resend", t!("entry-ip-resent"), false)
        } else if left > 0 {
            button(
                "entry-ip-resend",
                t!("entry-ip-resend-in", seconds = left),
                false,
            )
            .opacity(0.6)
        } else {
            button("entry-ip-resend", t!("entry-ip-resend"), false).on_click(cx.listener(
                |this, _, _, cx| {
                    if this.sign_in.flow.begin(Instant::now()) {
                        this.backend.send(Cmd::Entry(entry::Cmd::IpResend));
                        this.start_sign_in_clock(cx);
                        cx.notify();
                    }
                },
            ))
        };
        form.child(heading(t!("entry-title-ip"), Some(text)))
            .children(self.error_line())
            .child(resend)
            .child(back)
    }

    /// Puts the cursor in the input of the step on screen.
    pub(crate) fn focus_sign_in_step(&self, window: &mut Window, cx: &mut Context<Self>) {
        let input = match self.sign_in.flow.step {
            Step::Instance => &self.sign_in.instance_input,
            Step::Identifier | Step::Credentials => &self.sign_in.identifier_input,
            Step::Password => &self.sign_in.password_input,
            Step::Code | Step::Mfa => &self.sign_in.code_input,
            Step::IpAuth => return,
        };
        input.focus_handle(cx).focus(window, cx);
    }
}
