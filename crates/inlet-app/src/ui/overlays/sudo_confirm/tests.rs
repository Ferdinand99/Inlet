// SPDX-License-Identifier: AGPL-3.0-or-later
//! The "Confirm it's you" dialog in a window (gpui's test platform): what it sends, how
//! often, and what it clears.

use gpui::{Entity, TestAppContext, VisualTestContext};
use inlet_protocol as p;
use serde_json::json;
use tokio::sync::mpsc::UnboundedReceiver;

use super::state::{Mode, Problem};
use super::*;
use crate::app::Screen;
use crate::backend::accounts::AccountKey;
use crate::backend::settings::sudo::{Response, Wrong};
use crate::backend::{BackendHandle, Command};
use crate::desktop::Layer;

type Sent = UnboundedReceiver<(Option<AccountKey>, Command)>;

fn rig(cx: &mut TestAppContext) -> (Entity<Inlet>, &mut VisualTestContext, Sent) {
    let (handle, rx) = BackendHandle::recording();
    let (inlet, cx) =
        cx.add_window_view(move |window, cx| Inlet::new_with(window, cx, move |_| handle));
    inlet.update(cx, |this, _| this.screen = Screen::Main);
    (inlet, cx, rx)
}

fn request() -> Request {
    Request::recovery_kit()
}

fn methods(password: bool, totp: bool) -> p::SudoMethods {
    p::SudoMethods {
        password,
        totp,
        backup_codes: totp,
        ..p::SudoMethods::default()
    }
}

/// The next command of the sudo kind, if one was sent. The commands of other kinds (the
/// start-up's request for the stored accounts) are skipped.
fn next(rx: &mut Sent) -> Option<Cmd> {
    loop {
        match rx.try_recv() {
            Ok((_, Command::Settings(crate::backend::settings::Cmd::Sudo(cmd)))) => {
                return Some(cmd)
            }
            Ok(_) => continue,
            Err(_) => return None,
        }
    }
}

fn typed(inlet: &Entity<Inlet>, cx: &mut VisualTestContext, text: &str, code: bool) {
    inlet.update(cx, |this, cx| {
        let dialog = this.overlays.sudo.dialog.as_ref().expect("the dialog");
        let input = if code { &dialog.code } else { &dialog.password };
        input.update(cx, |i, cx| i.set_text(text.to_owned(), cx));
    });
}

fn field_text(inlet: &Entity<Inlet>, cx: &mut VisualTestContext, code: bool) -> String {
    inlet.read_with(cx, |this, cx| {
        let dialog = this.overlays.sudo.dialog.as_ref().expect("the dialog");
        let input = if code { &dialog.code } else { &dialog.password };
        input.read(cx).text().to_owned()
    })
}

/// Opens the dialog the way a feature does: the action goes out, the instance asks.
fn open(
    inlet: &Entity<Inlet>,
    cx: &mut VisualTestContext,
    rx: &mut Sent,
    m: p::SudoMethods,
) -> u64 {
    let ticket = inlet.update(cx, |this, cx| this.ask_sudo(request(), cx));
    match next(rx) {
        Some(Cmd::Run {
            ticket: t, proof, ..
        }) => {
            assert_eq!(t, ticket);
            assert!(proof.is_none(), "the first try carries no proof");
        }
        None => panic!("the action was not sent"),
    }
    inlet.update_in(cx, |this, window, cx| {
        this.on_sudo_event(Ev::Needed { ticket, methods: m }, window, cx)
    });
    ticket
}

#[gpui::test]
fn the_dialog_opens_when_the_instance_asks_and_is_the_topmost_layer(cx: &mut TestAppContext) {
    let (inlet, cx, mut rx) = rig(cx);
    assert!(!inlet.read_with(cx, |this, _| this.sudo_open()));
    open(&inlet, cx, &mut rx, methods(true, false));
    inlet.read_with(cx, |this, _| {
        assert!(this.sudo_open());
        assert!(this.is_top_layer(Layer::SudoConfirm));
        let state = &this.overlays.sudo.dialog.as_ref().unwrap().state;
        assert_eq!(state.mode, Mode::Password);
    });
    // It draws (the demo scene's screenshot checks how it looks).
    cx.run_until_parked();
    assert!(next(&mut rx).is_none(), "opening sends nothing");
}

#[gpui::test]
fn a_proof_is_sent_once_and_the_field_is_cleared_at_once(cx: &mut TestAppContext) {
    let (inlet, cx, mut rx) = rig(cx);
    let ticket = open(&inlet, cx, &mut rx, methods(true, false));
    // Nothing typed: Confirm does nothing.
    inlet.update(cx, |this, cx| this.sudo_submit(cx));
    assert!(next(&mut rx).is_none());

    typed(&inlet, cx, "correct-horse", false);
    inlet.update(cx, |this, cx| this.sudo_submit(cx));
    match next(&mut rx) {
        Some(Cmd::Run {
            ticket: t,
            proof: Some(Proof::Password(secret)),
            request,
        }) => {
            assert_eq!(t, ticket);
            assert_eq!(secret.expose_secret(), "correct-horse");
            assert!(request.path().ends_with("recovery-kit"));
        }
        _ => panic!("expected the password with the action"),
    }
    assert_eq!(field_text(&inlet, cx, false), "", "cleared when sent");
    // Pressing Confirm again (or Enter) while the request is out sends nothing.
    typed(&inlet, cx, "again", false);
    inlet.update(cx, |this, cx| this.sudo_submit(cx));
    assert!(next(&mut rx).is_none(), "one request at a time");
}

#[gpui::test]
fn a_wrong_password_shows_an_error_and_a_second_try_works(cx: &mut TestAppContext) {
    let (inlet, cx, mut rx) = rig(cx);
    let ticket = open(&inlet, cx, &mut rx, methods(true, false));
    typed(&inlet, cx, "wrong-password", false);
    inlet.update(cx, |this, cx| this.sudo_submit(cx));
    let _ = next(&mut rx);
    inlet.update_in(cx, |this, window, cx| {
        this.on_sudo_event(
            Ev::Rejected {
                ticket,
                wrong: Wrong::Password,
            },
            window,
            cx,
        )
    });
    inlet.read_with(cx, |this, _| {
        let state = &this.overlays.sudo.dialog.as_ref().unwrap().state;
        assert_eq!(state.problem, Some(Problem::WrongPassword));
        assert!(!state.busy);
    });
    typed(&inlet, cx, "right-password", false);
    inlet.update(cx, |this, cx| this.sudo_submit(cx));
    assert!(matches!(
        next(&mut rx),
        Some(Cmd::Run {
            proof: Some(Proof::Password(_)),
            ..
        })
    ));
    // The action went through: the dialog closes.
    inlet.update_in(cx, |this, window, cx| {
        this.on_sudo_event(
            Ev::Done {
                ticket,
                response: Response(json!({})),
            },
            window,
            cx,
        )
    });
    assert!(!inlet.read_with(cx, |this, _| this.sudo_open()));
}

#[gpui::test]
fn a_code_goes_as_a_code_and_the_way_can_be_changed(cx: &mut TestAppContext) {
    let (inlet, cx, mut rx) = rig(cx);
    open(&inlet, cx, &mut rx, methods(true, true));
    // Both ways: the password first, a link to the code.
    inlet.update_in(cx, |this, window, cx| {
        this.sudo_use_mode(Mode::Code, window, cx)
    });
    typed(&inlet, cx, "123456", true);
    inlet.update(cx, |this, cx| this.sudo_submit(cx));
    match next(&mut rx) {
        Some(Cmd::Run {
            proof: Some(Proof::Code(c)),
            ..
        }) => assert_eq!(c.expose_secret(), "123456"),
        _ => panic!("expected a code"),
    }
    assert_eq!(field_text(&inlet, cx, true), "");
}

#[gpui::test]
fn the_password_is_dropped_from_the_field_when_the_way_changes(cx: &mut TestAppContext) {
    let (inlet, cx, mut rx) = rig(cx);
    open(&inlet, cx, &mut rx, methods(true, true));
    typed(&inlet, cx, "half-typed-password", false);
    inlet.update_in(cx, |this, window, cx| {
        this.sudo_use_mode(Mode::Code, window, cx)
    });
    assert_eq!(field_text(&inlet, cx, false), "");
}

#[gpui::test]
fn escape_closes_the_dialog_and_sends_nothing(cx: &mut TestAppContext) {
    let (inlet, cx, mut rx) = rig(cx);
    open(&inlet, cx, &mut rx, methods(true, false));
    typed(&inlet, cx, "typed-then-cancelled", false);
    inlet.update(cx, |this, cx| this.close_top_layer(cx));
    assert!(!inlet.read_with(cx, |this, _| this.sudo_open()));
    assert!(next(&mut rx).is_none());
    // An answer that arrives later changes nothing.
    inlet.update_in(cx, |this, window, cx| {
        this.on_sudo_event(
            Ev::Done {
                ticket: 1,
                response: Response(json!({})),
            },
            window,
            cx,
        )
    });
    assert!(!inlet.read_with(cx, |this, _| this.sudo_open()));
}

#[gpui::test]
fn an_account_that_can_only_use_a_passkey_gets_an_explanation_and_no_way_to_send(
    cx: &mut TestAppContext,
) {
    let (inlet, cx, mut rx) = rig(cx);
    let only_passkey = p::SudoMethods {
        webauthn: true,
        ..p::SudoMethods::default()
    };
    open(&inlet, cx, &mut rx, only_passkey);
    typed(&inlet, cx, "anything", false);
    inlet.update(cx, |this, cx| this.sudo_submit(cx));
    assert!(next(&mut rx).is_none());
    cx.run_until_parked();
}

#[gpui::test]
fn an_action_that_goes_through_without_a_proof_opens_nothing(cx: &mut TestAppContext) {
    let (inlet, cx, mut rx) = rig(cx);
    let ticket = inlet.update(cx, |this, cx| this.ask_sudo(request(), cx));
    let _ = next(&mut rx);
    inlet.update_in(cx, |this, window, cx| {
        this.on_sudo_event(
            Ev::Done {
                ticket,
                response: Response(json!({})),
            },
            window,
            cx,
        )
    });
    inlet.read_with(cx, |this, _| {
        assert!(!this.sudo_open());
        assert!(this.overlays.sudo.asked.is_none());
    });
}

#[gpui::test]
fn another_failure_before_the_dialog_shows_in_the_error_line(cx: &mut TestAppContext) {
    let (inlet, cx, mut rx) = rig(cx);
    let ticket = inlet.update(cx, |this, cx| this.ask_sudo(request(), cx));
    let _ = next(&mut rx);
    inlet.update_in(cx, |this, window, cx| {
        this.on_sudo_event(
            Ev::Failed {
                ticket,
                reason: "denied-by-the-instance".into(),
            },
            window,
            cx,
        )
    });
    inlet.read_with(cx, |this, _| {
        assert!(!this.sudo_open());
        assert_eq!(this.chat.error.as_deref(), Some("denied-by-the-instance"));
    });
}

#[gpui::test]
fn an_answer_for_another_action_does_not_open_the_dialog(cx: &mut TestAppContext) {
    let (inlet, cx, mut rx) = rig(cx);
    let ticket = inlet.update(cx, |this, cx| this.ask_sudo(request(), cx));
    let _ = next(&mut rx);
    inlet.update_in(cx, |this, window, cx| {
        this.on_sudo_event(
            Ev::Needed {
                ticket: ticket + 5,
                methods: methods(true, false),
            },
            window,
            cx,
        )
    });
    assert!(!inlet.read_with(cx, |this, _| this.sudo_open()));
}

#[gpui::test]
fn signing_out_closes_the_dialog(cx: &mut TestAppContext) {
    let (inlet, cx, mut rx) = rig(cx);
    open(&inlet, cx, &mut rx, methods(true, false));
    inlet.update(cx, |this, _| this.reset_main());
    assert!(!inlet.read_with(cx, |this, _| this.sudo_open()));
}
