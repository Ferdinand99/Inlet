// SPDX-License-Identifier: AGPL-3.0-or-later
//! Trusted link domains, as Fluxer keeps them (`trusted_domain/state/TrustedDomain.ts`):
//! a link whose host is trusted opens without the "you are leaving" question. Trust comes
//! from three places: a short built-in list, the instance's own hosts and its
//! `trusted_domains` (both from the discovery document), and the account's list
//! (`trusted_domains` in the user settings, which roams between clients). `*` in the
//! account's list trusts everything; the instance's list never can.
//!
//! The question stays for a link that can fool the reader (a non-ASCII host or user
//! information before the host), whatever the lists say.

use gpui::Context;
use inlet_protocol as p;

use crate::app::Inlet;
use crate::backend::composer::Cmd as ComposerCmd;
use crate::ui::markdown;

/// Fluxer's own sites, which its client never warns about: the host, and whether its
/// subdomains count too.
const BUILT_IN: &[(&str, bool)] = &[
    ("fluxer.app", true),
    ("fluxer.com", true),
    ("fluxer.dev", true),
    ("fluxerstatus.com", true),
    ("fluxer.gg", false),
    ("fluxer.gift", false),
];

/// The entry that trusts every domain.
pub const TRUST_ALL: &str = "*";
/// The server accepts at most this many entries of the account's list.
pub const MAX_USER_DOMAINS: usize = 1000;
/// Longest entry the server accepts.
const MAX_DOMAIN_LEN: usize = 253;

/// A host as it is compared: lower case, without a trailing dot.
fn comparable(host: &str) -> String {
    host.trim().to_lowercase().trim_end_matches('.').to_owned()
}

/// Whether `pattern` (`host` or `*.host`) covers `host`. `*.example.org` covers
/// `example.org` and everything below it.
fn covers(pattern: &str, host: &str) -> bool {
    let pattern = comparable(pattern);
    match pattern.strip_prefix("*.") {
        Some(base) => host == base || host.ends_with(&format!(".{base}")),
        None => host == pattern,
    }
}

/// What this window knows about trust, for the shown account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustedDomains {
    /// The instance's hosts and its `trusted_domains`.
    instance: Vec<String>,
    /// The account's own list, as shown (a change waiting for the server's answer included).
    user: Vec<String>,
    /// The list as the server last reported it.
    confirmed: Vec<String>,
}

impl TrustedDomains {
    pub fn set_instance(&mut self, domains: Vec<String>) {
        self.instance = domains;
    }

    /// The server's list.
    pub fn set_user(&mut self, domains: Vec<String>) {
        self.confirmed = domains.clone();
        self.user = domains;
    }

    /// Shows a change at once, before the server has taken it.
    pub fn show_pending(&mut self, domains: Vec<String>) {
        self.user = domains;
    }

    /// The server refused the change: back to its list.
    pub fn revert(&mut self) {
        self.user = self.confirmed.clone();
    }

    /// The account trusts every domain.
    pub fn trust_all(&self) -> bool {
        self.user.iter().any(|d| d == TRUST_ALL)
    }

    /// Whether a link to `host` opens without the question.
    pub fn is_trusted(&self, host: &str) -> bool {
        if self.trust_all() {
            return true;
        }
        let host = comparable(host);
        if host.is_empty() {
            return false;
        }
        let built_in = BUILT_IN.iter().any(|(base, subdomains)| {
            host == *base || (*subdomains && host.ends_with(&format!(".{base}")))
        });
        built_in
            || self
                .instance
                .iter()
                .chain(&self.user)
                .any(|pattern| covers(pattern, &host))
    }

    /// Whether a link to `url` opens without the question: http(s), a host that is
    /// trusted, and nothing in the link that can fool the reader.
    pub fn trusts_url(&self, url: &str) -> bool {
        if markdown::is_deceptive(url) {
            return false;
        }
        markdown::openable(url)
            .and_then(|u| u.host_str().map(|h| self.is_trusted(h)))
            .unwrap_or(false)
    }

    /// The account's list with `host` added, or `None` when there is nothing to save:
    /// everything is trusted already, the host is on the list, it is not a plain host
    /// name, or the list is full.
    pub fn with_added(&self, host: &str) -> Option<Vec<String>> {
        let host = comparable(host);
        let plain = !host.is_empty()
            && host.len() <= MAX_DOMAIN_LEN
            && host != TRUST_ALL
            && !host.contains(char::is_whitespace);
        if !plain
            || self.trust_all()
            || self.user.iter().any(|d| comparable(d) == host)
            || self.user.len() >= MAX_USER_DOMAINS
        {
            return None;
        }
        let mut next = self.user.clone();
        next.push(host);
        Some(next)
    }
}

/// The account's list from the `trusted_domains` of a settings document, cleaned. Only
/// this one field is read: the document can carry large preference blobs.
pub fn user_list(field: Option<&serde_json::Value>) -> Vec<String> {
    field
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty() && s.len() <= MAX_DOMAIN_LEN)
                .take(MAX_USER_DOMAINS)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

impl Inlet {
    /// The account's list of trusted domains follows READY and later settings updates
    /// (an update without the field keeps the list).
    pub(crate) fn links_on_dispatch(&mut self, d: &p::Dispatch) {
        let field = match d {
            // A READY always carries the settings; no field means an empty list.
            p::Dispatch::Ready(ready) => Some(
                ready
                    .extra
                    .get("user_settings")
                    .and_then(|s| s.get("trusted_domains")),
            ),
            p::Dispatch::UserSettingsUpdate(settings) => {
                settings.extra.get("trusted_domains").map(Some)
            }
            _ => None,
        };
        if let Some(field) = field {
            self.overlays.trusted.set_user(user_list(field));
        }
    }

    /// The server did not take the list: the shown one goes back.
    pub(crate) fn trusted_domains_failed(&mut self) {
        self.overlays.trusted.revert();
    }

    /// "Always trust": adds `host` to the account's list at once and saves it. A failed
    /// save goes back to the server's list (see [`Inlet::trusted_domains_failed`]).
    pub(crate) fn trust_domain(&mut self, host: &str, cx: &mut Context<Self>) {
        let Some(next) = self.overlays.trusted.with_added(host) else {
            return;
        };
        self.overlays.trusted.show_pending(next.clone());
        self.backend.send(ComposerCmd::TrustDomains(next));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lists(instance: &[&str], user: &[&str]) -> TrustedDomains {
        let mut t = TrustedDomains::default();
        t.set_instance(instance.iter().map(|s| s.to_string()).collect());
        t.set_user(user.iter().map(|s| s.to_string()).collect());
        t
    }

    #[test]
    fn built_in_instance_and_account_lists_all_count() {
        let t = lists(&["chat.example.org", "*.cdn.example.org"], &["docs.test"]);
        assert!(t.is_trusted("fluxer.app"));
        assert!(t.is_trusted("beta.fluxer.app"));
        assert!(t.is_trusted("CHAT.example.org."));
        assert!(t.is_trusted("cdn.example.org"));
        assert!(t.is_trusted("img.cdn.example.org"));
        assert!(t.is_trusted("docs.test"));
        assert!(!t.is_trusted("evil-fluxer.app"));
        assert!(!t.is_trusted("fluxer.app.evil.test"));
        assert!(!t.is_trusted("sub.docs.test"));
        assert!(!t.is_trusted(""));
    }

    #[test]
    fn trust_all_comes_only_from_the_accounts_own_list() {
        let t = lists(&[], &["*"]);
        assert!(t.trust_all());
        assert!(t.is_trusted("anything.example"));
        assert_eq!(t.with_added("x.example"), None);
        // The instance cannot trust everything for the account.
        let from_instance = lists(&["*"], &[]);
        assert!(!from_instance.is_trusted("anything.example"));
    }

    #[test]
    fn a_link_that_can_fool_the_reader_is_never_trusted() {
        let t = lists(&["example.org"], &[]);
        assert!(t.trusts_url("https://example.org/page"));
        assert!(t.trusts_url("http://sub.fluxer.app/x"));
        assert!(!t.trusts_url("https://example.org@evil.test/"));
        assert!(!t.trusts_url("https://exаmple.org/"), "a Cyrillic а");
        assert!(!t.trusts_url("ftp://example.org/"));
        assert!(!t.trusts_url("not a url"));
    }

    #[test]
    fn adding_a_domain_checks_it_first() {
        let t = lists(&[], &["a.test"]);
        assert_eq!(
            t.with_added("B.Test."),
            Some(vec!["a.test".to_string(), "b.test".to_string()])
        );
        assert_eq!(t.with_added("a.test"), None, "already on the list");
        assert_eq!(t.with_added("*"), None);
        assert_eq!(t.with_added(""), None);
        assert_eq!(t.with_added("two words"), None);
        let full = lists(
            &[],
            &(0..MAX_USER_DOMAINS)
                .map(|i| format!("h{i}.test"))
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            full.with_added("new.test"),
            None,
            "the server caps the list"
        );
    }

    #[test]
    fn a_refused_save_puts_the_servers_list_back() {
        let mut t = lists(&[], &["a.test"]);
        let next = t.with_added("b.test").unwrap();
        t.show_pending(next);
        assert!(t.is_trusted("b.test"), "shown at once");
        t.revert();
        assert!(!t.is_trusted("b.test"));
        assert!(t.is_trusted("a.test"));
    }

    #[test]
    fn the_accounts_list_is_read_from_the_settings_document() {
        let field = json!(["a.test", " b.test ", "", 3, "x".repeat(300)]);
        assert_eq!(user_list(Some(&field)), ["a.test", "b.test"]);
        assert!(user_list(None).is_empty());
        assert!(user_list(Some(&json!("a.test"))).is_empty());
    }
}
