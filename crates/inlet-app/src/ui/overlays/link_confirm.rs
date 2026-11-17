// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opening links from messages: http(s) only, and a confirmation when the link text
//! does not show where it goes.

use gpui::{div, prelude::*, px, Context, Div, FontWeight, Stateful};

use super::modal::modal;
use crate::app::Inlet;
use crate::theme;
use crate::ui::markdown;
use crate::ui::widgets::button;

impl Inlet {
    /// Opens a link from a message. As in Fluxer, a link whose host is not trusted (see
    /// `trusted`) gets the question first, plain or masked alike; the instance's own media
    /// host and the trusted lists open at once. `_masked` is kept for the callers: the
    /// question no longer depends on it.
    pub(crate) fn open_link(&mut self, url: &str, _masked: bool, cx: &mut Context<Self>) {
        if let Some(channel_id) = url.strip_prefix(markdown::CHANNEL_LINK) {
            self.chat.pending_channel_jump = Some(channel_id.to_string());
            cx.notify();
            return;
        }
        // Invite links open the invite prompt first (see `desktop`).
        if let Some(invite) = crate::backend::platform::links::web_invite(url) {
            self.ask_about_link(invite, cx);
            return;
        }
        if markdown::openable(url).is_none() {
            self.chat.error = Some(t!("link-only-http"));
        } else if !self.link_is_trusted(url) {
            self.overlays.pending_link = Some(url.to_string());
        } else {
            cx.open_url(url);
        }
        cx.notify();
    }

    /// Whether a link opens without the question: a trusted host, or a file on the
    /// instance's media proxy.
    pub(crate) fn link_is_trusted(&self, url: &str) -> bool {
        let on_media = self
            .media
            .setup
            .as_ref()
            .and_then(|s| url::Url::parse(s.proxy.base()).ok())
            .zip(markdown::openable(url))
            .is_some_and(|(base, u)| crate::backend::media::on_media_base(&base, &u));
        !self.overlays.trusted.asks_first(url, on_media)
    }

    /// Opens an address Inlet built itself (a search the person asked for): straight to
    /// the browser, as Fluxer does, after the http(s) check.
    pub(crate) fn open_built_link(&mut self, url: &str, cx: &mut Context<Self>) {
        if markdown::openable(url).is_some() {
            cx.open_url(url);
        }
    }

    pub(super) fn render_link_confirm(&self, url: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let host = markdown::link_target_label(url);
        // Open exactly what was parsed and shown, never the raw text.
        let parsed = markdown::openable(url);
        let shown = parsed
            .as_ref()
            .map(|u| u.as_str().to_string())
            .unwrap_or_else(|| url.to_string());
        let lines = wrap_url(&shown, 64, 6);
        // "Always trust" for a plain host that is not trusted yet, and never for a link that
        // can fool the reader.
        let trust_host = parsed
            .as_ref()
            .filter(|_| !markdown::is_deceptive(url))
            .and_then(|u| u.host_str().map(str::to_owned))
            .filter(|h| self.overlays.trusted.with_added(h).is_some());
        let trust = trust_host.map(|host| {
            let target = parsed.clone();
            button(
                "link-trust",
                t!("link-trust-open", host = host.as_str()),
                false,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.overlays.pending_link = None;
                this.trust_domain(&host, cx);
                if let Some(u) = &target {
                    cx.open_url(u.as_str());
                }
                cx.notify();
            }))
        });
        modal(
            "link-confirm",
            520.,
            [
                div()
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::text_strong())
                    .child(t!("link-confirm-title"))
                    .into_any_element(),
                div()
                    .text_color(theme::text_muted())
                    .child(t!("link-confirm-body"))
                    .into_any_element(),
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::text_strong())
                    .child(host)
                    .into_any_element(),
                div()
                    .flex()
                    .flex_col()
                    .text_size(px(12.))
                    .font_family(markdown::mono_font())
                    .text_color(theme::text())
                    .children(lines)
                    .into_any_element(),
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(
                        button("link-open", t!("link-open"), true).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.overlays.pending_link = None;
                                if let Some(u) = &parsed {
                                    cx.open_url(u.as_str());
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .children(trust)
                    .child(
                        button("link-cancel", t!("common-cancel"), false).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.overlays.pending_link = None;
                                cx.notify();
                            },
                        )),
                    )
                    .into_any_element(),
            ],
        )
    }
}

/// Splits a URL into lines of `width` characters, at most `max_lines`, ending with "…"
/// when cut. URLs have no spaces, so normal text wrapping would let them run out of the box.
fn wrap_url(url: &str, width: usize, max_lines: usize) -> Vec<String> {
    let chars: Vec<char> = url.chars().collect();
    let mut lines: Vec<String> = chars.chunks(width).map(|c| c.iter().collect()).collect();
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            last.push('\u{2026}');
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_urls_are_wrapped_and_capped() {
        let url = format!("https://example.com/{}", "a".repeat(1000));
        let lines = wrap_url(&url, 64, 6);
        assert_eq!(lines.len(), 6);
        assert!(lines.iter().all(|l| l.chars().count() <= 65));
        assert!(lines.last().unwrap().ends_with('\u{2026}'));
        assert_eq!(
            wrap_url("https://a.b", 64, 6),
            vec!["https://a.b".to_string()]
        );
    }
}
