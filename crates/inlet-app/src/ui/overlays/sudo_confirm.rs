// SPDX-License-Identifier: AGPL-3.0-or-later
//! "Confirm it's you": the dialog that asks for the password or a code before an action
//! Fluxer protects (sudo mode, see `inlet_protocol::sudo`). It is reusable: a feature
//! sends the action through [`Inlet::ask_sudo`], and when the instance wants a proof
//! this dialog opens over everything, takes the proof and sends the same action again.
//! A passkey is not offered yet (that comes with the passkey work); an account that can
//! only confirm that way is told so.
//!
//! The dialog is a layer (`desktop::Layer::SudoConfirm`): it has the keyboard, Escape
//! closes it, and nothing behind it reacts. What the person typed goes into a
//! [`inlet_protocol::SudoSecret`] when they press Confirm, and the field is cleared in the
//! same step. The proof is never kept, logged or shown again; a wrong one only shows an
//! error. The state of the dialog is a plain value ([`state::State`]) with its own tests.
//!
//! Nothing opens it yet except the demo scene `sudo-confirm`; the first feature that
//! needs sudo mode (deleting the account, the guild menu's "Delete my messages") calls
//! [`Inlet::ask_sudo`] with its action.

pub mod state;

use std::sync::Arc;

use gpui::{
    div, prelude::*, px, AnyElement, Context, Entity, Focusable, FontWeight, Subscription, Window,
};
use inlet_protocol as p;

use super::modal::modal;
use crate::app::Inlet;
use crate::backend::settings::sudo::{Cmd, Ev, Proof, Request};
use crate::desktop::Layer;
use crate::text_input::{InputEvent, TextInput};
use crate::theme;
use crate::ui::widgets::{button, field};
use state::{Mode, Outcome, Problem, State};

/// Width of the dialog.
const WIDTH: f32 = 400.;
/// Longest password Fluxer accepts, and longest code.
const PASSWORD_CHARS: usize = 256;
const CODE_CHARS: usize = 32;

/// The dialog while it is open, and the action that has been sent and not answered yet.
#[derive(Default)]
pub struct Sudo {
    /// Counts up with every action, so an answer for an old one is ignored.
    seq: u64,
    /// An action sent without a proof, waiting to hear whether one is needed.
    asked: Option<(u64, Arc<Request>)>,
    dialog: Option<Dialog>,
}

struct Dialog {
    state: State,
    password: Entity<TextInput>,
    code: Entity<TextInput>,
    _subs: Vec<Subscription>,
}

impl Dialog {
    fn input(&self) -> &Entity<TextInput> {
        match self.state.mode {
            Mode::Password => &self.password,
            Mode::Code => &self.code,
        }
    }

    fn clear(&self, cx: &mut gpui::App) {
        for input in [&self.password, &self.code] {
            input.update(cx, |i, cx| i.clear(cx));
        }
    }
}

impl Inlet {
    /// Whether the dialog is open (it is a layer over every screen).
    pub(crate) fn sudo_open(&self) -> bool {
        self.overlays.sudo.dialog.is_some()
    }

    /// Whether the dialog's field has the keyboard now.
    pub(crate) fn sudo_field_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.overlays
            .sudo
            .dialog
            .as_ref()
            .is_some_and(|d| d.input().focus_handle(cx).is_focused(window))
    }

    /// Where the keyboard goes while the dialog is the topmost layer.
    pub(crate) fn sudo_focus(&self, cx: &gpui::App) -> Option<gpui::FocusHandle> {
        self.overlays
            .sudo
            .dialog
            .as_ref()
            .map(|d| d.input().focus_handle(cx))
    }

    /// Sends `request` (an action that may need sudo mode). If the account is confirmed
    /// already it simply goes through; if the instance wants a proof, the dialog opens
    /// and sends the action again with it. The answer of the action itself is not shown
    /// here: the feature that asked takes it from [`Ev::Done`] (see
    /// [`Inlet::on_sudo_event`]).
    // No feature asks yet; the first one that needs sudo mode calls this.
    #[allow(dead_code)]
    pub(crate) fn ask_sudo(&mut self, request: Request, cx: &mut Context<Self>) -> u64 {
        let sudo = &mut self.overlays.sudo;
        sudo.seq += 1;
        let ticket = sudo.seq;
        let request = Arc::new(request);
        sudo.asked = Some((ticket, request.clone()));
        self.backend.send(Cmd::Run {
            ticket,
            request,
            proof: None,
        });
        cx.notify();
        ticket
    }

    /// An answer of the backend's sudo part.
    pub(crate) fn on_sudo_event(&mut self, ev: Ev, window: &mut Window, cx: &mut Context<Self>) {
        // The dialog is open: it takes the answers for its action.
        if let Some(dialog) = self.overlays.sudo.dialog.as_mut() {
            let was = dialog.state.mode;
            if dialog.state.apply(&ev) == Outcome::Done {
                self.overlays.sudo.dialog = None;
                cx.notify();
                return;
            }
            if dialog.state.mode != was {
                dialog.clear(cx);
            }
            // The field is free again after an answer: the keyboard goes back to it.
            let focus = dialog.input().focus_handle(cx);
            focus.focus(window, cx);
            cx.notify();
            return;
        }
        // No dialog yet: the answer to the action sent without a proof.
        let Some((ticket, request)) = self.overlays.sudo.asked.clone() else {
            return;
        };
        match ev {
            Ev::Needed {
                ticket: t, methods, ..
            } if t == ticket => {
                self.overlays.sudo.asked = None;
                self.open_sudo_dialog(ticket, request, methods, window, cx);
            }
            Ev::Done { ticket: t, .. } if t == ticket => {
                // Confirmed already (a token from a moment ago): the action went through.
                self.overlays.sudo.asked = None;
            }
            Ev::Failed {
                ticket: t, reason, ..
            } if t == ticket => {
                self.overlays.sudo.asked = None;
                self.chat.error = Some(reason);
            }
            _ => {}
        }
        cx.notify();
    }

    fn open_sudo_dialog(
        &mut self,
        ticket: u64,
        request: Arc<Request>,
        methods: p::SudoMethods,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let password = cx.new(|cx| {
            TextInput::new(t!("sudo-password-placeholder"), cx)
                .masked()
                .max_chars(PASSWORD_CHARS)
        });
        let code =
            cx.new(|cx| TextInput::new(t!("sudo-code-placeholder"), cx).max_chars(CODE_CHARS));
        let subs = vec![
            cx.subscribe(&password, |this, _, _: &InputEvent, cx| {
                this.sudo_submit(cx)
            }),
            cx.subscribe(&code, |this, _, _: &InputEvent, cx| this.sudo_submit(cx)),
        ];
        let dialog = Dialog {
            state: State::new(ticket, request, methods),
            password,
            code,
            _subs: subs,
        };
        dialog.input().focus_handle(cx).focus(window, cx);
        self.overlays.sudo.dialog = Some(dialog);
        cx.notify();
    }

    /// Confirm: the typed proof goes to the backend with the action, and the field is
    /// cleared at once.
    fn sudo_submit(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.overlays.sudo.dialog.as_mut() else {
            return;
        };
        let typed = dialog.input().read(cx).text().trim().to_owned();
        if !dialog.state.can_submit(!typed.is_empty()) || !dialog.state.submit() {
            return;
        }
        let secret = p::SudoSecret::new(typed);
        let proof = match dialog.state.mode {
            Mode::Password => Proof::Password(secret),
            Mode::Code => Proof::Code(secret),
        };
        let (ticket, request) = (dialog.state.ticket, dialog.state.request.clone());
        dialog.clear(cx);
        self.backend.send(Cmd::Run {
            ticket,
            request,
            proof: Some(proof),
        });
        cx.notify();
    }

    fn sudo_use_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.overlays.sudo.dialog.as_mut() {
            dialog.state.use_mode(mode);
            dialog.clear(cx);
            let focus = dialog.input().focus_handle(cx);
            focus.focus(window, cx);
            cx.notify();
        }
    }

    /// Cancel, Escape: the dialog closes and the action is not sent again.
    pub(crate) fn sudo_cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.overlays.sudo.dialog.take() {
            dialog.clear(cx);
        }
        self.overlays.sudo.asked = None;
        cx.notify();
    }

    /// Closes the dialog without a trace (the session ended, another account took over).
    pub(crate) fn sudo_reset(&mut self) {
        self.overlays.sudo = Sudo {
            seq: self.overlays.sudo.seq,
            ..Sudo::default()
        };
    }

    /// The dialog.
    pub(crate) fn render_sudo_confirm(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let top = self.is_top_layer(Layer::SudoConfirm);
        let Some(dialog) = self.overlays.sudo.dialog.as_ref() else {
            return div().into_any_element();
        };
        let state = &dialog.state;
        let typed = !dialog.input().read(cx).text().is_empty();
        let mut children: Vec<AnyElement> = vec![
            div()
                .text_size(px(20.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme::text_strong())
                .child(t!("sudo-title"))
                .into_any_element(),
            div()
                .text_color(theme::text_muted())
                .child(t!("sudo-body"))
                .into_any_element(),
        ];
        if state.can_confirm() {
            let (label, input) = match state.mode {
                Mode::Password => (t!("sudo-password-label"), dialog.password.clone()),
                Mode::Code => (t!("sudo-code-label"), dialog.code.clone()),
            };
            children.push(field(label, input).into_any_element());
            if let Some(problem) = &state.problem {
                children.push(problem_line(problem).into_any_element());
            }
            if let Some(other) = state.other_mode() {
                let text = match other {
                    Mode::Password => t!("sudo-use-password"),
                    Mode::Code => t!("sudo-use-code"),
                };
                children.push(
                    div()
                        .id("sudo-other-mode")
                        .cursor_pointer()
                        .text_size(px(13.))
                        .text_color(theme::link())
                        .hover(|s| s.underline())
                        .child(text)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.sudo_use_mode(other, window, cx)
                        }))
                        .into_any_element(),
                );
            }
        } else {
            children.push(
                div()
                    .text_color(theme::text())
                    .child(t!("sudo-unsupported"))
                    .into_any_element(),
            );
        }
        let ready = state.can_submit(typed);
        let busy = state.busy;
        let mut confirm = button(
            "sudo-confirm",
            if busy {
                t!("sudo-working")
            } else {
                t!("sudo-confirm")
            },
            true,
        );
        if state.can_confirm() {
            confirm = if ready {
                confirm.on_click(cx.listener(|this, _, _, cx| this.sudo_submit(cx)))
            } else {
                confirm.opacity(0.5)
            };
        }
        let mut buttons = div().flex().flex_row().gap_2();
        if state.can_confirm() {
            buttons = buttons.child(confirm);
        }
        buttons = buttons.child(
            button("sudo-cancel", t!("common-cancel"), false)
                .on_click(cx.listener(|this, _, _, cx| this.sudo_cancel(cx))),
        );
        children.push(buttons.into_any_element());
        let card = modal("sudo-confirm-dialog", WIDTH, children);
        if top {
            self.layer_keys(card, cx).into_any_element()
        } else {
            card.into_any_element()
        }
    }
}

/// The line under the field for a problem.
fn problem_line(problem: &Problem) -> impl IntoElement {
    let text = match problem {
        Problem::WrongPassword => t!("sudo-wrong-password"),
        Problem::NoPassword => t!("sudo-no-password"),
        Problem::WrongCode => t!("sudo-wrong-code"),
        Problem::NotAccepted => t!("sudo-not-accepted"),
        Problem::Locked { minutes } => t!("sudo-locked", minutes = *minutes),
        Problem::Failed(reason) => t!("sudo-failed", reason = reason.clone()),
    };
    div()
        .text_size(px(13.))
        .text_color(theme::danger())
        .child(text)
}

#[cfg(test)]
mod tests;

/// Demo scenes `sudo-confirm` and `sudo-confirm-code`: the dialog is open over the chat
/// with an error under the field. The scene files of the visual checks use them.
#[cfg(feature = "demo")]
impl Inlet {
    pub(crate) fn demo_sudo_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use std::sync::atomic::{AtomicBool, Ordering};

        use crate::app::Screen;
        use crate::demo::scenes::{current, Scene};
        static DONE: AtomicBool = AtomicBool::new(false);
        let scene = current();
        if !matches!(scene, Scene::SudoConfirm | Scene::SudoConfirmCode)
            || self.screen != Screen::Main
            || DONE.swap(true, Ordering::Relaxed)
        {
            return;
        }
        let request = Request::recovery_kit();
        let (methods, problem) = if scene == Scene::SudoConfirm {
            (
                p::SudoMethods {
                    password: true,
                    ..p::SudoMethods::default()
                },
                Problem::WrongPassword,
            )
        } else {
            (
                p::SudoMethods {
                    totp: true,
                    backup_codes: true,
                    ..p::SudoMethods::default()
                },
                Problem::WrongCode,
            )
        };
        cx.defer_in(window, move |this, window, cx| {
            this.open_sudo_dialog(1, Arc::new(request), methods, window, cx);
            if let Some(dialog) = this.overlays.sudo.dialog.as_mut() {
                dialog.state.problem = Some(problem);
            }
            cx.notify();
        });
    }
}
