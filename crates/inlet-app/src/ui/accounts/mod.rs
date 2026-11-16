// SPDX-License-Identifier: AGPL-3.0-or-later
//! Accounts in the UI: the sign-in form and the session screens, and what the accounts
//! domain of the backend reports (sign-in progress, session state, connection status).

#[cfg(feature = "demo")]
mod demo_steps;
mod entry_events;
mod flow;
mod invite;
mod screens;
mod sign_in;

use std::hash::{DefaultHasher, Hash, Hasher};

use gpui::{
    div, prelude::*, px, ClickEvent, ClipboardItem, Context, Entity, Focusable, FontWeight,
    Subscription, Window,
};

use crate::app::{Inlet, Screen};
use crate::backend::accounts::{Cmd, Ev, DEFAULT_INSTANCE};
use crate::backend::platform::links::Incoming;
use crate::text_input::{InputEvent, TextInput};
use crate::theme;

#[derive(Clone, Copy, PartialEq)]
pub enum SignInMode {
    /// Email or username and password, in the app.
    Password,
    Browser,
    Token,
}

/// The sign-in form.
pub struct SignInState {
    pub instance_input: Entity<TextInput>,
    pub token_input: Entity<TextInput>,
    /// The email address or username.
    pub identifier_input: Entity<TextInput>,
    pub password_input: Entity<TextInput>,
    /// The emailed code, or the authenticator or backup code.
    pub code_input: Entity<TextInput>,
    /// Where the in-app sign-in is (step, instance options, countdowns, its error).
    pub flow: flow::Flow,
    /// Ticks the screen while something on it counts down.
    pub clock: Option<gpui::Task<()>>,
    /// An invite link to another instance that the person is signing in for: its origin
    /// and code. Offered again once the sign-in worked.
    pub invite: Option<(String, String)>,
    pub mode: SignInMode,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    /// Browser sign-in in progress: page URL and the code to enter there.
    pub handoff: Option<(String, String)>,
    /// Hash of a pasted token, to clear the clipboard after sign-in if it still holds it.
    pub pasted_token_hash: Option<u64>,
}

impl SignInState {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Inlet>,
        subscriptions: &mut Vec<Subscription>,
    ) -> Self {
        let instance_input = cx.new(|cx| {
            TextInput::new(t!("sign-in-instance-placeholder"), cx)
                .with_text(DEFAULT_INSTANCE)
                .max_chars(253)
        });
        let token_input = cx.new(|cx| {
            TextInput::new(t!("sign-in-token-placeholder"), cx)
                .masked()
                .max_chars(512)
        });
        let identifier_input = cx.new(|cx| TextInput::new("", cx).max_chars(320));
        let password_input = cx.new(|cx| TextInput::new("", cx).masked().max_chars(256));
        let code_input =
            cx.new(|cx| TextInput::new(t!("entry-code-placeholder"), cx).max_chars(64));
        for input in [
            &instance_input,
            &token_input,
            &identifier_input,
            &password_input,
            &code_input,
        ] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Submit => this.submit_sign_in(window, cx),
                },
            ));
        }
        Self {
            instance_input,
            token_input,
            identifier_input,
            password_input,
            code_input,
            flow: flow::Flow::default(),
            clock: None,
            invite: None,
            mode: SignInMode::Password,
            busy: false,
            error: None,
            notice: None,
            handoff: None,
            pasted_token_hash: None,
        }
    }
}

impl SignInState {
    /// The host of the invite the person is signing in for.
    pub fn invite_host(&self) -> Option<String> {
        let (origin, _) = self.invite.as_ref()?;
        host_of(origin)
    }
}

/// The host (with a port, if any) of an origin such as `https://chat.example`.
pub(crate) fn host_of(origin: &str) -> Option<String> {
    let url = url::Url::parse(origin).ok()?;
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

pub(crate) fn hash_text(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// The handoff page may only be opened over https (http only for a loopback test instance).
pub(crate) fn safe_handoff_url(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    match u.scheme() {
        "https" => true,
        "http" => matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
        _ => false,
    }
}

impl Inlet {
    pub(crate) fn on_accounts_event(
        &mut self,
        event: Ev,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            Ev::Starting { instance } => {
                self.screen = Screen::Starting {
                    instance: Some(instance),
                };
            }
            Ev::Offline {
                instance,
                message,
                retry_in_secs,
            } => {
                self.screen = Screen::Offline {
                    instance,
                    message,
                    retry_in_secs,
                };
            }
            Ev::NeedsSignIn { instance } => {
                if let Some(instance) = instance {
                    self.sign_in
                        .instance_input
                        .update(cx, |i, cx| i.set_text(instance, cx));
                }
                self.screen = Screen::SignIn;
                self.sign_in.busy = false;
                // A fresh start: the form forgets the step it was on and what was typed.
                self.sign_in_reset_flow(cx);
                self.sign_in
                    .instance_input
                    .focus_handle(cx)
                    .focus(window, cx);
            }
            Ev::HandoffStarted { url, code } => {
                self.sign_in.busy = false;
                self.sign_in.error = None;
                if safe_handoff_url(&url) {
                    cx.open_url(&url);
                    self.sign_in.handoff = Some((url, code));
                } else {
                    self.backend.send(Cmd::CancelSignIn);
                    self.sign_in.error = Some(t!("sign-in-handoff-not-https"));
                }
            }
            Ev::Entry(event) => self.on_entry_event(event, window, cx),
            Ev::SignInFailed(message) => {
                self.sign_in.busy = false;
                self.sign_in.flow.busy_since = None;
                self.sign_in.handoff = None;
                self.sign_in.pasted_token_hash = None;
                self.sign_in.error = Some(message);
                if matches!(self.screen, Screen::Starting { .. }) {
                    self.screen = Screen::SignIn;
                }
            }
            Ev::Notice(message) => {
                if self.screen == Screen::Main {
                    self.chat.notice = Some(message);
                } else {
                    self.sign_in.notice = Some(message);
                }
            }
            Ev::ConfirmAccount {
                display_name,
                account,
                instance,
                pasted,
            } => {
                self.sign_in.busy = false;
                self.sign_in.handoff = None;
                self.screen = Screen::Confirm {
                    display_name,
                    account,
                    instance,
                    pasted,
                };
            }
            Ev::GatewayDown(message) => self.chat.gateway_down = Some(message),
            Ev::SignedIn {
                key,
                username,
                instance,
            } => {
                self.sign_in.busy = false;
                self.sign_in.handoff = None;
                self.sign_in.error = None;
                self.sign_in.notice = None;
                self.sign_in.token_input.update(cx, |i, cx| i.clear(cx));
                self.sign_in_reset_flow(cx);
                let invite = self.sign_in.invite.take();
                if let Some(hash) = self.sign_in.pasted_token_hash.take() {
                    let still_there = cx
                        .read_from_clipboard()
                        .and_then(|item| item.text())
                        .is_some_and(|text| hash_text(text.trim()) == hash);
                    if still_there {
                        cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
                    }
                }
                // Back from a sign-in or a start-up screen to the account already shown
                // (signed in again, or connected after a switch): load its place again.
                let reopen = self.switcher.is_active(&key) && self.screen != Screen::Main;
                self.switch_ui_to(key, window, cx);
                self.switcher.adding = false;
                self.username = username;
                self.instance = instance;
                self.screen = Screen::Main;
                if reopen {
                    self.reopen_place(cx);
                }
                // Signed in on the instance an invite link pointed at: the link is asked
                // about again, now as an invite on the signed-in instance.
                if let Some((origin, code)) = invite {
                    if host_of(&origin) == host_of(&self.instance) {
                        self.ask_about_link(Incoming::WebInvite { origin, code }, cx);
                    }
                }
                // Demo screenshots (CI): the account switcher scenes.
                #[cfg(feature = "demo")]
                if std::env::var_os("INLET_DEMO").is_some() {
                    match crate::demo::scenes::current() {
                        crate::demo::scenes::Scene::Accounts => self.switcher.open = true,
                        crate::demo::scenes::Scene::AccountsAdd => {
                            return self.begin_add_account(window, cx);
                        }
                        crate::demo::scenes::Scene::SignIn(step) => {
                            return self.demo_sign_in(step, window, cx);
                        }
                        _ => {}
                    }
                }
                // Demo screenshots (CI): open a guild and channel straight away.
                #[cfg(feature = "demo")]
                if let Ok(guild) = std::env::var("INLET_DEMO_GUILD") {
                    self.select_guild(Some(crate::demo::snowflake(&guild)), cx);
                    self.demo_channel = std::env::var("INLET_DEMO_CHANNEL")
                        .ok()
                        .map(|c| crate::demo::snowflake(&c));
                }
            }
            Ev::SessionEnded(message) => {
                // The account is gone: its drafts go too.
                self.composer.sign_out();
                self.reset_main();
                self.sign_in.error = Some(message);
            }
            Ev::SignedOut => {
                self.composer.sign_out();
                self.reset_main();
            }
            Ev::Accounts(list) => self.on_account_list(list, cx),
            Ev::AccountRemoved { key, message } => self.on_account_removed(key, message, cx),
            // The messages domain loads the open channel again on every READY.
            Ev::Reconnected => {}
            Ev::Status(status) => {
                if status == t!("status-connected") {
                    self.chat.gateway_down = None;
                }
                self.chat.status = status;
            }
            Ev::Error(message) => self.chat.error = Some(message),
        }
    }

    /// From the fatal screen: start a fresh backend that ends the stored session on the
    /// server (best effort, with timeouts) and removes it, all off the UI thread.
    pub(crate) fn fatal_sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.sign_out();
        self.restart_backend(false, window, cx);
        self.backend.send(Cmd::ForgetStored);
    }

    pub(crate) fn sign_out(
        &mut self,
        _: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The account's drafts go now, before another account may take its place.
        self.composer.sign_out();
        self.backend
            .send(Cmd::SignOut(self.switcher.active.clone()));
        cx.notify();
    }

    // ---------- views ----------

    pub(crate) fn render_card(&self, body: impl IntoElement) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::bg_app())
            .child(
                div()
                    .w(px(480.))
                    .p_8()
                    .rounded_lg()
                    .bg(theme::bg_chat())
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(24.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_strong())
                                    .child(t!("app-name")),
                            )
                            .child(
                                div()
                                    .text_color(theme::text_muted())
                                    .child(t!("app-tagline")),
                            ),
                    )
                    .child(body),
            )
    }

    pub(crate) fn about_link(label: String, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("about-link")
            .text_size(px(12.))
            .text_color(theme::text_muted())
            .cursor_pointer()
            .hover(|s| s.text_color(theme::text()))
            .child(label)
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_about = true;
                cx.notify();
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_page_must_be_https() {
        assert!(safe_handoff_url("https://web.fluxer.app/login?handoff=1"));
        assert!(!safe_handoff_url("http://fluxer.example/login"));
        assert!(safe_handoff_url("http://localhost:8080/login"));
        assert!(!safe_handoff_url("fluxer://handoff"));
        assert!(!safe_handoff_url("not a url"));
    }

    use gpui::{Render, TestAppContext, Window};
    use inlet_protocol::Identity;
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::flow::Step;
    use crate::backend::accounts::entry::{self, EntryError, EntryInfo, Field};
    use crate::backend::accounts::AccountKey;
    use crate::backend::Command;

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn info(identifier_first: bool) -> EntryInfo {
        EntryInfo {
            origin: "https://chat.example".into(),
            host: "chat.example".into(),
            identity: Identity::Email,
            identifier_first,
            sso_enforced: false,
        }
    }

    /// The in-app sign-in commands the UI has sent so far.
    fn sent(rx: &mut UnboundedReceiver<(Option<AccountKey>, Command)>) -> Vec<entry::Cmd> {
        let mut out = Vec::new();
        while let Ok((_, command)) = rx.try_recv() {
            if let Command::Accounts(Cmd::Entry(cmd)) = command {
                out.push(cmd);
            }
        }
        out
    }

    /// An app on the sign-in screen with a backend that records what it is sent. The
    /// closure drives the app; it gets the window too.
    fn on_the_sign_in_screen(
        cx: &mut TestAppContext,
        drive: impl FnOnce(&mut Inlet, &mut Window, &mut Context<Inlet>),
    ) -> Vec<entry::Cmd> {
        let (handle, mut rx) = crate::backend::BackendHandle::recording();
        let window = cx.add_window(|_, _| Probe);
        let inlet = window
            .update(cx, |_, window, cx| {
                // No backend thread: its start-up events would reach the test's scheduler
                // from another thread.
                cx.new(|cx| Inlet::new_with(window, cx, move |_| handle))
            })
            .unwrap();
        window
            .update(cx, |_, window, cx| {
                inlet.update(cx, |this, cx| {
                    this.screen = Screen::SignIn;
                    drive(this, window, cx);
                })
            })
            .unwrap();
        sent(&mut rx)
    }

    #[gpui::test]
    fn the_password_is_cleared_from_its_field_when_it_is_sent(cx: &mut TestAppContext) {
        let commands = on_the_sign_in_screen(cx, |this, window, cx| {
            this.sign_in.flow.started(info(false));
            this.sign_in
                .identifier_input
                .update(cx, |i, cx| i.set_text("ana@example.org", cx));
            this.sign_in
                .password_input
                .update(cx, |i, cx| i.set_text("correct horse", cx));
            this.submit_sign_in(window, cx);
            assert_eq!(this.sign_in.password_input.read(cx).text(), "");
            assert!(this.sign_in.flow.busy());
        });
        match commands.as_slice() {
            [entry::Cmd::Password {
                identifier,
                password,
            }] => {
                assert_eq!(identifier, "ana@example.org");
                assert_eq!(password.expose_secret(), "correct horse");
            }
            _ => panic!("expected one password command, got {}", commands.len()),
        }
    }

    #[gpui::test]
    fn a_code_is_cleared_from_its_field_when_it_is_sent(cx: &mut TestAppContext) {
        let commands = on_the_sign_in_screen(cx, |this, window, cx| {
            this.sign_in.flow.started(info(true));
            this.sign_in.flow.step = Step::Mfa;
            this.sign_in
                .code_input
                .update(cx, |i, cx| i.set_text("123456", cx));
            this.submit_sign_in(window, cx);
            assert_eq!(this.sign_in.code_input.read(cx).text(), "");
        });
        assert!(
            matches!(commands.as_slice(), [entry::Cmd::Mfa { code }] if code.expose_secret() == "123456")
        );
    }

    #[gpui::test]
    fn a_second_submit_while_a_request_is_out_sends_nothing(cx: &mut TestAppContext) {
        let commands = on_the_sign_in_screen(cx, |this, window, cx| {
            this.sign_in.flow.started(info(false));
            this.sign_in
                .identifier_input
                .update(cx, |i, cx| i.set_text("ana@example.org", cx));
            this.sign_in
                .password_input
                .update(cx, |i, cx| i.set_text("one", cx));
            this.submit_sign_in(window, cx);
            this.sign_in
                .password_input
                .update(cx, |i, cx| i.set_text("two", cx));
            this.submit_sign_in(window, cx);
        });
        assert_eq!(commands.len(), 1);
    }

    #[gpui::test]
    fn a_pause_the_instance_asked_for_holds_back_the_next_attempt(cx: &mut TestAppContext) {
        let commands = on_the_sign_in_screen(cx, |this, window, cx| {
            this.sign_in.flow.started(info(false));
            this.sign_in.flow.failed(
                EntryError {
                    message: String::new(),
                    field: Field::Password,
                    wait_secs: Some(60),
                    restart: false,
                },
                std::time::Instant::now(),
            );
            this.sign_in
                .identifier_input
                .update(cx, |i, cx| i.set_text("ana@example.org", cx));
            this.sign_in
                .password_input
                .update(cx, |i, cx| i.set_text("correct horse", cx));
            this.submit_sign_in(window, cx);
        });
        assert!(commands.is_empty());
    }

    #[gpui::test]
    fn an_empty_password_is_not_sent(cx: &mut TestAppContext) {
        let commands = on_the_sign_in_screen(cx, |this, window, cx| {
            this.sign_in.flow.started(info(false));
            this.sign_in
                .identifier_input
                .update(cx, |i, cx| i.set_text("ana@example.org", cx));
            this.submit_sign_in(window, cx);
            assert!(!this.sign_in.flow.busy());
        });
        assert!(commands.is_empty());
    }

    #[gpui::test]
    fn the_instance_step_asks_the_backend_to_look_the_instance_up(cx: &mut TestAppContext) {
        let commands = on_the_sign_in_screen(cx, |this, window, cx| {
            this.sign_in
                .instance_input
                .update(cx, |i, cx| i.set_text("chat.example", cx));
            this.submit_sign_in(window, cx);
        });
        assert!(
            matches!(commands.as_slice(), [entry::Cmd::Start { instance }] if instance == "chat.example")
        );
    }

    #[gpui::test]
    fn leaving_the_sign_in_forgets_the_step_the_codes_and_the_invite(cx: &mut TestAppContext) {
        let _ = on_the_sign_in_screen(cx, |this, _, cx| {
            this.sign_in.flow.started(info(true));
            this.sign_in.flow.step = Step::Code;
            this.sign_in.invite = Some(("https://chat.example".into(), "abc".into()));
            this.sign_in
                .code_input
                .update(cx, |i, cx| i.set_text("123", cx));
            this.sign_in_left(cx);
            assert_eq!(this.sign_in.flow.step, Step::Instance);
            assert!(this.sign_in.flow.info.is_none());
            assert!(this.sign_in.invite.is_none());
            assert_eq!(this.sign_in.code_input.read(cx).text(), "");
        });
    }

    #[gpui::test]
    fn an_error_event_keeps_the_field_empty_and_shows_the_message(cx: &mut TestAppContext) {
        let _ = on_the_sign_in_screen(cx, |this, window, cx| {
            this.sign_in.flow.started(info(false));
            this.on_entry_event(
                entry::Ev::Failed(EntryError {
                    message: "The email address or the password is wrong.".into(),
                    field: Field::Password,
                    wait_secs: None,
                    restart: false,
                }),
                window,
                cx,
            );
            assert_eq!(this.sign_in.password_input.read(cx).text(), "");
            assert_eq!(
                this.sign_in.flow.error.as_ref().map(|e| e.message.as_str()),
                Some("The email address or the password is wrong.")
            );
        });
    }

    #[gpui::test]
    fn a_sign_in_the_backend_lost_returns_to_the_instance_step_with_a_message(
        cx: &mut TestAppContext,
    ) {
        let _ = on_the_sign_in_screen(cx, |this, window, cx| {
            this.sign_in.flow.started(info(true));
            this.sign_in.flow.step = Step::Password;
            this.on_entry_event(entry::Ev::Lost, window, cx);
            assert_eq!(this.sign_in.flow.step, Step::Instance);
            assert!(this.sign_in.flow.info.is_none());
            assert!(this.sign_in.flow.error.is_some());
            assert!(!this.sign_in.flow.busy());
        });
    }

    /// Every in-app sign-in command sent, in order, with `Leave` and `Back` told apart.
    fn names(commands: &[entry::Cmd]) -> Vec<&'static str> {
        commands
            .iter()
            .map(|c| match c {
                entry::Cmd::Start { .. } => "start",
                entry::Cmd::Identify { .. } => "identify",
                entry::Cmd::Password { .. } => "password",
                entry::Cmd::SendCode => "send-code",
                entry::Cmd::Code { .. } => "code",
                entry::Cmd::Mfa { .. } => "mfa",
                entry::Cmd::IpResend => "ip-resend",
                entry::Cmd::IpRetry => "ip-retry",
                entry::Cmd::Back => "back",
                entry::Cmd::Leave => "leave",
            })
            .collect()
    }

    #[gpui::test]
    fn resetting_the_form_tells_the_backend_to_forget_the_sign_in(cx: &mut TestAppContext) {
        let commands = on_the_sign_in_screen(cx, |this, _, cx| {
            this.sign_in.flow.started(info(true));
            this.sign_in_reset_flow(cx);
        });
        assert_eq!(names(&commands), ["leave"]);
    }

    #[gpui::test]
    fn the_links_between_steps_do_nothing_while_a_request_is_out(cx: &mut TestAppContext) {
        let commands = on_the_sign_in_screen(cx, |this, window, cx| {
            this.sign_in.flow.started(info(true));
            this.sign_in.flow.step = Step::Password;
            assert!(this.sign_in.flow.begin(std::time::Instant::now()));
            this.entry_restart(window, cx);
            this.entry_change_instance(window, cx);
            assert_eq!(this.sign_in.flow.step, Step::Password);
            assert!(this.sign_in.flow.info.is_some());
        });
        assert!(commands.is_empty());
    }

    #[gpui::test]
    fn the_browser_link_clears_a_password_that_was_typed_and_not_sent(cx: &mut TestAppContext) {
        let _ = on_the_sign_in_screen(cx, |this, _, cx| {
            this.sign_in.flow.started(info(false));
            this.sign_in
                .instance_input
                .update(cx, |i, cx| i.set_text("chat.example", cx));
            this.sign_in
                .password_input
                .update(cx, |i, cx| i.set_text("typed but not sent", cx));
            this.entry_use_browser(cx);
            assert_eq!(this.sign_in.password_input.read(cx).text(), "");
            assert!(this.sign_in.busy);
        });
    }
}
