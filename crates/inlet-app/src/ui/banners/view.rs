// SPDX-License-Identifier: AGPL-3.0-or-later
//! Drawing the banners and the two windows they open (the updated terms, and the steps
//! after a domain move).
//!
//! The look follows Fluxer's `Nagbar`: a bar across the top of the app, at least 36 px
//! high, bold 14 px text centred, an inverted button (the tone's text colour on white)
//! and a close button on the right of one that can be dismissed. The tones are the fixed
//! colours of Fluxer's `NagbarTones` (they do not follow the theme); the brand tone is
//! the instance's accent.

use gpui::{
    div, prelude::*, px, rgb, AnyElement, ClickEvent, ClipboardItem, Context, Div, FontWeight,
    Hsla, SharedString, Stateful, Window,
};
use inlet_state::TermsUpdate;

use super::{duration_text, moved_target, now_ms, tidy, Banner};
use crate::app::Inlet;
use crate::backend::instance::status_page::MaintenanceState;
use crate::theme;
use crate::ui::overlays::modal::modal;
use crate::ui::threads::ThreadIcon;
use crate::ui::widgets::button;

/// Height of a banner at least, as Fluxer's (`min-height: 2.25rem`).
pub const BANNER_MIN_HEIGHT: f32 = 36.;

/// A banner's colours.
#[derive(Clone, Copy)]
struct Tone {
    bg: Hsla,
    fg: Hsla,
}

fn tone(bg: u32) -> Tone {
    Tone {
        bg: rgb(bg).into(),
        fg: rgb(0xffffff).into(),
    }
}

/// Fluxer's `NagbarTones`.
fn neutral() -> Tone {
    tone(0x4e5058)
}
fn maintenance() -> Tone {
    tone(0x4285f4)
}
fn maintenance_scheduled() -> Tone {
    tone(0x1d4ed8)
}
fn maintenance_active() -> Tone {
    tone(0x9a3412)
}
fn legal() -> Tone {
    tone(0x4338ca)
}
fn brand() -> Tone {
    Tone {
        bg: theme::brand(),
        fg: theme::on_brand(),
    }
}

/// One action on a banner: an inverted button.
fn action(id: &'static str, label: impl Into<SharedString>, tone: Tone) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(12.))
        .py(px(4.))
        .rounded(px(6.))
        .bg(Hsla::white())
        .text_color(tone.bg)
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .child(label.into())
}

/// The bar: `message`, the `actions` next to it and, when it can be dismissed, a close
/// button on the right.
fn bar(
    id: &'static str,
    tone: Tone,
    message: String,
    actions: Vec<AnyElement>,
    dismiss: Option<AnyElement>,
) -> Stateful<Div> {
    div()
        .id(id)
        .relative()
        .w_full()
        .flex_none()
        .min_h(px(BANNER_MIN_HEIGHT))
        .pl(px(8.))
        .pr(px(if dismiss.is_some() { 40. } else { 8. }))
        .py(px(4.))
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .justify_center()
        .gap_x(px(12.))
        .gap_y(px(4.))
        .bg(tone.bg)
        .text_color(tone.fg)
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(div().text_center().child(message))
        .when(!actions.is_empty(), |d| {
            d.child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .children(actions),
            )
        })
        .children(dismiss.map(|button| {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right(px(8.))
                .flex()
                .items_center()
                .child(button)
        }))
}

impl Inlet {
    /// The close button of a banner that can be dismissed.
    fn dismiss_button(
        &self,
        id: &'static str,
        banner: Banner,
        tone: Tone,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .p(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|s| s.opacity(0.7))
            .child(ThreadIcon::Close.el(px(20.), tone.fg))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.dismiss_banner(banner, now_ms(), cx);
            }))
            .into_any_element()
    }

    /// The name of the instance's product, for the texts of banners and windows.
    fn product_name(&self, cx: &gpui::App) -> String {
        let named = self
            .account
            .as_ref()
            .and_then(|a| a.read(cx).capabilities().branding().product_name);
        match named {
            Some(name) => tidy(&name, 60),
            None => self
                .switcher
                .active
                .as_ref()
                .map(|k| tidy(&k.origin.to_string(), 60))
                .unwrap_or_default(),
        }
    }

    /// The banners across the top of the main screen, drawn above the rail.
    pub(crate) fn render_banners(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let visible = self.visible_banners(cx);
        visible
            .into_iter()
            .filter_map(|b| self.render_banner(b, cx))
            .collect()
    }

    fn render_banner(&mut self, banner: Banner, cx: &mut Context<Self>) -> Option<AnyElement> {
        let account = self.account.clone()?;
        let (notices, caps, name) = {
            let state = account.read(cx);
            (
                state.notices(),
                state.capabilities().clone(),
                tidy(state.me().display_name(), 60),
            )
        };
        let status = self
            .banners
            .status
            .as_ref()
            .filter(|(k, _)| self.switcher.is_active(k))
            .map(|(_, s)| s.clone());
        let el = match banner {
            Banner::Connection => {
                let t = neutral();
                // Maintenance or an incident on the status page explains the outage.
                let (message, link) = match &status {
                    Some(s) if s.maintenance.is_some() => {
                        let m = s.maintenance.as_ref()?;
                        (
                            m.name.clone(),
                            m.url.clone().map(|u| (t!("banner-view-maintenance"), u)),
                        )
                    }
                    Some(s) if s.incident.is_some() => {
                        let i = s.incident.as_ref()?;
                        (
                            i.name.clone(),
                            i.url.clone().map(|u| (t!("banner-view-incident"), u)),
                        )
                    }
                    _ => match self.chat.gateway_down.clone() {
                        Some(why) => (t!("chat-disconnected", message = tidy(&why, 200)), None),
                        None => (t!("banner-connection-lost"), None),
                    },
                };
                let mut actions = Vec::new();
                if let Some((label, url)) = link {
                    actions.push(
                        action("banner-connection-details", label, t)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_link(&url, true, cx)),
                            )
                            .into_any_element(),
                    );
                }
                if self.chat.gateway_down.is_some() {
                    actions.push(
                        action("banner-try-again", t!("banner-try-again"), t)
                            .on_click(cx.listener(|this, _, _, cx| this.retry_connection(cx)))
                            .into_any_element(),
                    );
                }
                if self.switcher.accounts.len() > 1 && self.chat.gateway_down.is_some() {
                    actions.push(
                        action("banner-switch-account", t!("banner-switch-account"), t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.switcher.open = true;
                                cx.notify();
                            }))
                            .into_any_element(),
                    );
                }
                bar("banner-connection", t, message, actions, None)
            }
            Banner::Terms => {
                let t = legal();
                bar(
                    "banner-terms",
                    t,
                    t!("banner-terms-message"),
                    vec![action("banner-terms-review", t!("banner-review"), t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.banners.terms_open = true;
                            this.banners.terms_error = None;
                            cx.notify();
                        }))
                        .into_any_element()],
                    None,
                )
            }
            Banner::Email => {
                let t = neutral();
                if notices.email_bounced {
                    bar(
                        "banner-email",
                        t,
                        t!("banner-verify-bounced", name = name),
                        Vec::new(),
                        None,
                    )
                } else {
                    let busy = self.banners.verify_busy;
                    bar(
                        "banner-email",
                        t,
                        t!("banner-verify-message", name = name),
                        vec![action("banner-resend-email", t!("banner-resend-email"), t)
                            .when(busy, |d| d.opacity(0.6))
                            .on_click(cx.listener(|this, _, _, cx| this.resend_verification(cx)))
                            .into_any_element()],
                        None,
                    )
                }
            }
            Banner::AccountLimited => bar(
                "banner-limited",
                neutral(),
                t!("banner-limited-message"),
                Vec::new(),
                None,
            ),
            Banner::InstanceNotice => {
                let t = self::maintenance();
                let notice = caps.notice()?;
                let mut actions = Vec::new();
                if let Some(url) = notice.url.clone() {
                    actions.push(
                        action("banner-notice-learn-more", t!("banner-learn-more"), t)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_link(&url, true, cx)),
                            )
                            .into_any_element(),
                    );
                }
                bar(
                    "banner-notice",
                    t,
                    tidy(&notice.message, 600),
                    actions,
                    Some(self.dismiss_button(
                        "banner-notice-dismiss",
                        Banner::InstanceNotice,
                        t,
                        cx,
                    )),
                )
            }
            Banner::Maintenance => {
                let m = status.as_ref()?.maintenance.clone()?;
                let running = m.state == MaintenanceState::InProgress;
                let t = if running {
                    maintenance_active()
                } else {
                    maintenance_scheduled()
                };
                let duration = duration_text(m.duration_minutes);
                let message = if running {
                    t!("banner-maintenance-active", duration = duration)
                } else {
                    t!(
                        "banner-maintenance-scheduled",
                        time = m
                            .start_ms
                            .map(crate::ui::chat::time::full)
                            .unwrap_or_default(),
                        duration = duration
                    )
                };
                let mut actions = Vec::new();
                if let Some(url) = m.url.clone() {
                    actions.push(
                        action("banner-maintenance-learn-more", t!("banner-learn-more"), t)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_link(&url, true, cx)),
                            )
                            .into_any_element(),
                    );
                }
                bar(
                    "banner-maintenance",
                    t,
                    message,
                    actions,
                    Some(self.dismiss_button(
                        "banner-maintenance-dismiss",
                        Banner::Maintenance,
                        t,
                        cx,
                    )),
                )
            }
            Banner::DomainMoved => {
                let t = brand();
                let host = self.moved_host()?;
                bar(
                    "banner-moved",
                    t,
                    t!(
                        "banner-domain-moved",
                        product = self.product_name(cx),
                        host = host
                    ),
                    vec![action("banner-moved-steps", t!("banner-show-me-how"), t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.banners.steps_open = true;
                            cx.notify();
                        }))
                        .into_any_element()],
                    Some(self.dismiss_button("banner-moved-dismiss", Banner::DomainMoved, t, cx)),
                )
            }
        };
        Some(el.into_any_element())
    }

    /// The new address of the instance the account is on, when it has moved.
    fn moved_host(&self) -> Option<&'static str> {
        let origin = self.switcher.active.as_ref()?.origin.to_string();
        let host = url::Url::parse(&origin).ok()?.host_str()?.to_owned();
        moved_target(&host)
    }

    /// The windows the banners open, drawn over the app.
    pub(crate) fn render_banner_windows(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if self.screen != crate::app::Screen::Main {
            return out;
        }
        if self.banners.terms_open {
            if let Some(w) = self.render_terms_window(cx) {
                out.push(w);
            }
        }
        if self.banners.steps_open {
            if let Some(w) = self.render_moved_steps_window(cx) {
                out.push(w);
            }
        }
        out
    }

    fn render_terms_window(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let account = self.account.clone()?;
        let (update, legal_links) = {
            let state = account.read(cx);
            (state.notices().terms, state.capabilities().legal())
        };
        let product = self.product_name(cx);
        let body = match update {
            TermsUpdate::Terms => t!("terms-changed-terms", product = product.clone()),
            TermsUpdate::Privacy => t!("terms-changed-privacy", product = product.clone()),
            TermsUpdate::Both => t!("terms-changed-both", product = product.clone()),
            // Accepted from another device while the window was open: nothing to take in.
            TermsUpdate::None => return None,
        };
        let agrees = update != TermsUpdate::Privacy;
        let busy = self.banners.terms_busy;
        let mut links = Vec::new();
        for (id, label, url) in [
            (
                "terms-open-terms",
                t!("terms-link-terms"),
                legal_links.terms_url,
            ),
            (
                "terms-open-privacy",
                t!("terms-link-privacy"),
                legal_links.privacy_url,
            ),
        ] {
            if let Some(url) = url {
                links.push(
                    button(id, label, false)
                        .on_click(cx.listener(move |this, _, _, cx| this.open_link(&url, true, cx)))
                        .into_any_element(),
                );
            }
        }
        let mut children = vec![
            div()
                .font_weight(FontWeight::BOLD)
                .text_size(px(18.))
                .text_color(theme::text_strong())
                .child(t!("terms-title"))
                .into_any_element(),
            div()
                .text_color(theme::text())
                .child(body)
                .into_any_element(),
        ];
        if !links.is_empty() {
            children.push(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .children(links)
                    .into_any_element(),
            );
        }
        if agrees {
            children.push(
                div()
                    .text_size(px(12.))
                    .text_color(theme::text_muted())
                    .child(t!("terms-agreement", product = product))
                    .into_any_element(),
            );
        }
        if let Some(why) = self.banners.terms_error.clone() {
            children.push(
                div()
                    .text_color(theme::danger())
                    .child(tidy(&why, 300))
                    .into_any_element(),
            );
        }
        children.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(
                    button("terms-close", t!("banner-close"), false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.banners.terms_open = false;
                            cx.notify();
                        },
                    )),
                )
                .child(
                    button(
                        "terms-accept",
                        if agrees {
                            t!("terms-agree")
                        } else {
                            t!("terms-got-it")
                        },
                        true,
                    )
                    .when(busy, |d| d.opacity(0.6))
                    .on_click(cx.listener(|this, _, _, cx| this.accept_terms(cx))),
                )
                .into_any_element(),
        );
        Some(modal("terms-window", 520., children).into_any_element())
    }

    fn render_moved_steps_window(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let host = self.moved_host()?;
        let product = self.product_name(cx);
        let step = |n: u32, text: String| {
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .text_color(theme::text_muted())
                        .child(format!("{n}.")),
                )
                .child(div().text_color(theme::text()).child(text))
        };
        let children = vec![
            div()
                .font_weight(FontWeight::BOLD)
                .text_size(px(18.))
                .text_color(theme::text_strong())
                .child(t!("moved-title", product = product.clone()))
                .into_any_element(),
            div()
                .text_color(theme::text())
                .child(t!("moved-intro", product = product, host = host))
                .into_any_element(),
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(step(1, t!("moved-step-add")))
                .child(step(2, t!("moved-step-address", host = host)))
                .child(step(3, t!("moved-step-sign-in")))
                .into_any_element(),
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(
                    button("moved-copy", t!("moved-copy-address"), false).on_click(cx.listener(
                        move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(host.to_string()));
                        },
                    )),
                )
                .child(
                    button("moved-close", t!("banner-close"), true).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.banners.steps_open = false;
                            cx.notify();
                        },
                    )),
                )
                .into_any_element(),
        ];
        Some(modal("moved-window", 520., children).into_any_element())
    }
}
