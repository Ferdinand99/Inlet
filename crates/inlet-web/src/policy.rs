// SPDX-License-Identifier: AGPL-3.0-or-later
//! Which URLs the web view may load: the host patterns of `allow` and the prefixes of
//! `intercept`.
//!
//! The rules fail closed. A URL is allowed only when it parses, is `https`, has no user
//! name or password, has a domain name as its host (no IP address, no trailing dot) and
//! that name and port match a pattern. Everything else, `javascript:`, `data:`, `file:`
//! and `blob:` included, is blocked. Both the pattern and the URL go through the `url`
//! crate (the WHATWG parser the web engines use), so a URL is read the way the engine
//! reads it: `https://good.example@evil.example/` has the host `evil.example`, `\` is a
//! `/`, `%`-escapes and Unicode names are decoded and turned into punycode before they
//! are compared.
//!
//! Intercept prefixes are compared as URLs too, never as strings: `https://a.example/cb`
//! matches `/cb`, `/cb/` and `/cb?x=1`, but not `/cbx` or `https://a.example.evil/cb`.

use std::net::IpAddr;

use url::{Host, Url};

use crate::protocol::{OpenRequest, RequestError, MAX_RULES, MAX_URL_LEN};

/// How the rules are applied.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PolicyOptions {
    /// Also accept `http`, and an IP address as the host, when the host is on loopback
    /// (`localhost`, `127.0.0.1`, `::1`). For tests and the demo only: a build gets it
    /// from [`LOOPBACK_HTTP`], which is off outside the `loopback` feature.
    pub loopback_http: bool,
}

/// Whether this build lets pages on loopback load over plain `http` (the `loopback`
/// feature, never in a release build).
pub const LOOPBACK_HTTP: bool = cfg!(feature = "loopback");

impl PolicyOptions {
    /// The options of this build.
    pub const fn of_this_build() -> Self {
        PolicyOptions {
            loopback_http: LOOPBACK_HTTP,
        }
    }
}

/// What to do with a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Load it.
    Allow,
    /// It ends the flow: do not load it, report it.
    Intercept,
    /// Do not load it.
    Block(BlockReason),
}

/// Why a URL is blocked. For the logs and the tests; it never leaves the helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    /// Longer than [`MAX_URL_LEN`].
    TooLong,
    /// Not a URL.
    Unparseable,
    /// Not `https` (`javascript:`, `data:`, `file:`, `blob:`, `about:`, `http:` ...).
    Scheme,
    /// Has a user name or password.
    Userinfo,
    /// Has no host, or one ending in a dot.
    Host,
    /// The host is an IP address.
    IpAddress,
    /// No pattern of `allow` matches.
    NotAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostRule {
    /// `example.com`
    Exact(String),
    /// `*.example.com`: any name below it, not the name itself.
    Below(String),
    /// A loopback address (loopback mode only).
    Ip(IpAddr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortRule {
    /// No port in the pattern: the scheme's own port.
    Default,
    Exact(u16),
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AllowRule {
    host: HostRule,
    port: PortRule,
}

/// The rules of one window.
#[derive(Debug, Clone)]
pub struct Policy {
    allow: Vec<AllowRule>,
    /// Hosts that may be frames of the page, and only frames.
    frames: Vec<AllowRule>,
    intercept: Vec<Url>,
    options: PolicyOptions,
}

impl Policy {
    /// Builds the rules, or says which one is not accepted.
    pub fn new(
        allow: &[String],
        intercept: &[String],
        options: PolicyOptions,
    ) -> Result<Policy, RequestError> {
        if allow.is_empty() {
            return Err(RequestError::NoAllow);
        }
        if allow.len() > MAX_RULES || intercept.len() > MAX_RULES {
            return Err(RequestError::TooManyRules);
        }
        let allow = allow
            .iter()
            .enumerate()
            .map(|(i, p)| parse_pattern(p, options).ok_or(RequestError::BadPattern(i)))
            .collect::<Result<Vec<_>, _>>()?;
        let intercept = intercept
            .iter()
            .enumerate()
            .map(|(i, p)| parse_prefix(p, options).ok_or(RequestError::BadPrefix(i)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Policy {
            allow,
            frames: Vec::new(),
            intercept,
            options,
        })
    }

    /// Adds the hosts that may appear as frames (the `frames` of a request).
    pub fn with_frames(mut self, frames: &[String]) -> Result<Policy, RequestError> {
        if frames.len() > MAX_RULES {
            return Err(RequestError::TooManyRules);
        }
        self.frames = frames
            .iter()
            .enumerate()
            .map(|(i, p)| parse_pattern(p, self.options).ok_or(RequestError::BadFramePattern(i)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self)
    }

    /// The rules of an `open` request. Also checks the title, the size and that the start
    /// URL is one the rules allow.
    pub fn for_request(
        request: &OpenRequest,
        options: PolicyOptions,
    ) -> Result<Policy, RequestError> {
        request.check_window()?;
        let policy = Policy::new(&request.allow, &request.intercept, options)?
            .with_frames(&request.frames)?;
        if policy.check(&request.url) != Verdict::Allow {
            return Err(RequestError::BadUrl);
        }
        Ok(policy)
    }

    /// What to do with a navigation of the window to `raw`: only `allow` counts.
    pub fn check(&self, raw: &str) -> Verdict {
        self.verdict(raw, false)
    }

    /// What to do with a navigation of a frame to `raw`: `allow` and `frames` count. The
    /// same basic rules apply as for the window, and an `intercept` URL still ends the
    /// flow.
    pub fn check_frame(&self, raw: &str) -> Verdict {
        self.verdict(raw, true)
    }

    fn verdict(&self, raw: &str, frame: bool) -> Verdict {
        if raw.len() > MAX_URL_LEN {
            return Verdict::Block(BlockReason::TooLong);
        }
        let Ok(url) = Url::parse(raw) else {
            return Verdict::Block(BlockReason::Unparseable);
        };
        if let Err(reason) = self.basics(&url) {
            return Verdict::Block(reason);
        }
        if self
            .intercept
            .iter()
            .any(|prefix| prefix_matches(prefix, &url))
        {
            return Verdict::Intercept;
        }
        let listed = |rules: &[AllowRule]| rules.iter().any(|rule| rule_matches(rule, &url));
        if listed(&self.allow) || (frame && listed(&self.frames)) {
            Verdict::Allow
        } else {
            Verdict::Block(BlockReason::NotAllowed)
        }
    }

    /// What every URL must be, whichever rule it matches.
    fn basics(&self, url: &Url) -> Result<(), BlockReason> {
        let host_ok = host_is_acceptable(url, self.options)?;
        let https = url.scheme() == "https";
        let loopback_http =
            self.options.loopback_http && url.scheme() == "http" && host_ok.loopback;
        if !https && !loopback_http {
            return Err(BlockReason::Scheme);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(BlockReason::Userinfo);
        }
        Ok(())
    }
}

struct HostFacts {
    loopback: bool,
}

/// The checks on the host that do not depend on the rules.
fn host_is_acceptable(url: &Url, options: PolicyOptions) -> Result<HostFacts, BlockReason> {
    // The scheme comes first in the caller's mind, but a `javascript:` URL has no host and
    // must end up as a scheme block, so a missing host is only an error for https/http.
    if !matches!(url.scheme(), "https" | "http") {
        return Err(BlockReason::Scheme);
    }
    match url.host() {
        None => Err(BlockReason::Host),
        Some(Host::Domain(domain)) => {
            if domain.ends_with('.') || domain.is_empty() {
                return Err(BlockReason::Host);
            }
            Ok(HostFacts {
                loopback: options.loopback_http && domain == "localhost",
            })
        }
        Some(Host::Ipv4(ip)) => ip_host(IpAddr::V4(ip), options),
        Some(Host::Ipv6(ip)) => ip_host(IpAddr::V6(ip), options),
    }
}

fn ip_host(ip: IpAddr, options: PolicyOptions) -> Result<HostFacts, BlockReason> {
    if options.loopback_http && ip.is_loopback() {
        Ok(HostFacts { loopback: true })
    } else {
        Err(BlockReason::IpAddress)
    }
}

fn rule_matches(rule: &AllowRule, url: &Url) -> bool {
    let port_ok = match rule.port {
        PortRule::Default => url.port().is_none(),
        PortRule::Exact(port) => url.port_or_known_default() == Some(port),
        PortRule::Any => true,
    };
    if !port_ok {
        return false;
    }
    match (&rule.host, url.host()) {
        (HostRule::Exact(name), Some(Host::Domain(domain))) => domain == name,
        (HostRule::Below(suffix), Some(Host::Domain(domain))) => {
            domain.len() > suffix.len() + 1
                && domain.ends_with(suffix.as_str())
                && domain[..domain.len() - suffix.len()].ends_with('.')
        }
        (HostRule::Ip(ip), Some(Host::Ipv4(found))) => *ip == IpAddr::V4(found),
        (HostRule::Ip(ip), Some(Host::Ipv6(found))) => *ip == IpAddr::V6(found),
        _ => false,
    }
}

/// Reads one pattern of `allow`; `None` when it is not accepted.
fn parse_pattern(pattern: &str, options: PolicyOptions) -> Option<AllowRule> {
    // Only plain characters: no scheme, path, user name, escape, space or bracket. A
    // pattern is a host and maybe a port, nothing a parser could read two ways.
    if pattern.is_empty()
        || pattern.len() > 270
        || pattern
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/\\@?#%[]".contains(c))
    {
        return None;
    }
    let (host_part, port) = match pattern.rsplit_once(':') {
        None => (pattern, PortRule::Default),
        Some((host, "*")) => (host, PortRule::Any),
        Some((host, digits)) => {
            let port: u16 = digits.parse().ok()?;
            // `:0` and `:0443` are typos, not ports.
            if port == 0 || port.to_string() != digits {
                return None;
            }
            (host, PortRule::Exact(port))
        }
    };
    if host_part.contains(':') {
        return None;
    }
    let (wildcard, name) = match host_part.strip_prefix("*.") {
        Some(rest) => (true, rest),
        None => (false, host_part),
    };
    if name.is_empty() || name.contains('*') || name.starts_with('.') || name.ends_with('.') {
        return None;
    }
    match Host::parse(name).ok()? {
        Host::Domain(domain) => {
            if domain.split('.').any(str::is_empty) {
                return None;
            }
            let loopback_name = options.loopback_http && domain == "localhost" && !wildcard;
            // A name with a dot, and below a wildcard at least two labels: `*.com` would
            // allow a whole top-level domain.
            if !loopback_name && !domain.contains('.') {
                return None;
            }
            let host = if wildcard {
                HostRule::Below(domain)
            } else {
                HostRule::Exact(domain)
            };
            Some(AllowRule { host, port })
        }
        Host::Ipv4(ip) if options.loopback_http && !wildcard && ip.is_loopback() => {
            Some(AllowRule {
                host: HostRule::Ip(IpAddr::V4(ip)),
                port,
            })
        }
        Host::Ipv6(_) => None,
        Host::Ipv4(_) => None,
    }
}

/// Reads one prefix of `intercept`: an absolute URL with no user name, password or
/// fragment, on the same terms as any other URL.
fn parse_prefix(prefix: &str, options: PolicyOptions) -> Option<Url> {
    if prefix.len() > MAX_URL_LEN {
        return None;
    }
    let url = Url::parse(prefix).ok()?;
    let facts = host_is_acceptable(&url, options).ok()?;
    let https = url.scheme() == "https";
    if !https && !(options.loopback_http && url.scheme() == "http" && facts.loopback) {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return None;
    }
    Some(url)
}

/// Whether `url` is at or below `prefix`: the same scheme, host and port, the path equal
/// or continuing after a `/`, and a query that starts with the prefix's when it has one.
fn prefix_matches(prefix: &Url, url: &Url) -> bool {
    if prefix.scheme() != url.scheme()
        || prefix.host() != url.host()
        || prefix.port_or_known_default() != url.port_or_known_default()
    {
        return false;
    }
    let base = prefix.path();
    let path = url.path();
    let path_ok = if base == "/" {
        true
    } else if let Some(trimmed) = base.strip_suffix('/') {
        path == trimmed || path.starts_with(base)
    } else {
        path == base || (path.starts_with(base) && path[base.len()..].starts_with('/'))
    };
    if !path_ok {
        return false;
    }
    match prefix.query() {
        None => true,
        Some(query) => url.query().is_some_and(|q| q.starts_with(query)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NORMAL: PolicyOptions = PolicyOptions {
        loopback_http: false,
    };
    const LOOPBACK: PolicyOptions = PolicyOptions {
        loopback_http: true,
    };

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn policy(allow: &[&str], intercept: &[&str]) -> Policy {
        Policy::new(&strings(allow), &strings(intercept), NORMAL).expect("rules are accepted")
    }

    fn blocked(reason: BlockReason) -> Verdict {
        Verdict::Block(reason)
    }

    #[test]
    fn an_exact_host_matches_only_itself() {
        let p = policy(&["id.example"], &[]);
        assert_eq!(p.check("https://id.example/"), Verdict::Allow);
        assert_eq!(p.check("https://id.example/a/b?c=d#e"), Verdict::Allow);
        assert_eq!(p.check("https://ID.Example/"), Verdict::Allow);
        assert_eq!(
            p.check("https://sub.id.example/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            p.check("https://example/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            p.check("https://other.example/"),
            blocked(BlockReason::NotAllowed)
        );
    }

    #[test]
    fn a_wildcard_matches_names_below_the_domain_but_not_the_domain() {
        let p = policy(&["*.id.example"], &[]);
        assert_eq!(p.check("https://a.id.example/"), Verdict::Allow);
        assert_eq!(p.check("https://a.b.id.example/"), Verdict::Allow);
        assert_eq!(
            p.check("https://id.example/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            p.check("https://evilid.example/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            p.check("https://a.evilid.example/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            p.check("https://id.example.evil.test/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            p.check("https://.id.example/"),
            blocked(BlockReason::NotAllowed)
        );
    }

    #[test]
    fn the_host_is_not_found_in_the_path_query_or_fragment() {
        let p = policy(&["id.example"], &[]);
        for url in [
            "https://evil.test/id.example",
            "https://evil.test/?u=https://id.example/",
            "https://evil.test/#id.example",
            "https://evil.test/@id.example",
            "https://id.example.evil.test/",
            "https://evil.test.id.example.evil.test/",
        ] {
            assert_eq!(p.check(url), blocked(BlockReason::NotAllowed), "{url}");
        }
    }

    #[test]
    fn ports() {
        let default = policy(&["id.example"], &[]);
        assert_eq!(default.check("https://id.example:443/"), Verdict::Allow);
        assert_eq!(
            default.check("https://id.example:8443/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            default.check("https://id.example:80/"),
            blocked(BlockReason::NotAllowed)
        );

        let exact = policy(&["id.example:8443"], &[]);
        assert_eq!(exact.check("https://id.example:8443/"), Verdict::Allow);
        assert_eq!(
            exact.check("https://id.example/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            exact.check("https://id.example:8444/"),
            blocked(BlockReason::NotAllowed)
        );

        let any = policy(&["id.example:*"], &[]);
        assert_eq!(any.check("https://id.example/"), Verdict::Allow);
        assert_eq!(any.check("https://id.example:1234/"), Verdict::Allow);
        assert_eq!(
            any.check("https://other.example:1234/"),
            blocked(BlockReason::NotAllowed)
        );
    }

    #[test]
    fn a_user_name_or_password_is_never_loaded() {
        let p = policy(&["id.example"], &[]);
        // The host is the part after the last `@`; either way the URL is blocked.
        assert_eq!(
            p.check("https://id.example@evil.test/"),
            blocked(BlockReason::Userinfo)
        );
        assert_eq!(
            p.check("https://id.example:443@evil.test/"),
            blocked(BlockReason::Userinfo)
        );
        // The host is allowed, but the user name is a way to hide where a link goes.
        assert_eq!(
            p.check("https://evil.test@id.example/"),
            blocked(BlockReason::Userinfo)
        );
        assert_eq!(
            p.check("https://a:b@id.example/"),
            blocked(BlockReason::Userinfo)
        );
        assert_eq!(
            p.check("https://:b@id.example/"),
            blocked(BlockReason::Userinfo)
        );
        assert_eq!(
            p.check("https://a@id.example/"),
            blocked(BlockReason::Userinfo)
        );
        // `%40` is part of the host, which is not a valid host.
        assert_eq!(
            p.check("https://id.example%40evil.test/"),
            blocked(BlockReason::Unparseable)
        );
        // `\` ends the host like `/` does.
        assert_eq!(
            p.check("https://evil.test\\@id.example/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(p.check("https://id.example\\.evil.test/"), Verdict::Allow);
    }

    #[test]
    fn escapes_in_the_host_are_decoded_before_the_comparison() {
        let p = policy(&["id.example"], &[]);
        assert_eq!(p.check("https://id%2Eexample/"), Verdict::Allow);
        assert_eq!(
            p.check("https://ev%69l.test/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            p.check("https://id.example%2Eevil.test/"),
            blocked(BlockReason::NotAllowed)
        );
    }

    #[test]
    fn a_trailing_dot_is_not_the_same_name() {
        let p = policy(&["id.example"], &[]);
        assert_eq!(p.check("https://id.example./"), blocked(BlockReason::Host));
        assert_eq!(
            p.check("https://id.example.:443/"),
            blocked(BlockReason::Host)
        );
    }

    #[test]
    fn international_names_compare_as_punycode() {
        let unicode = policy(&["bücher.example"], &[]);
        assert_eq!(unicode.check("https://bücher.example/"), Verdict::Allow);
        assert_eq!(
            unicode.check("https://xn--bcher-kva.example/"),
            Verdict::Allow
        );
        assert_eq!(unicode.check("https://BÜCHER.example/"), Verdict::Allow);
        assert_eq!(
            unicode.check("https://bucher.example/"),
            blocked(BlockReason::NotAllowed)
        );

        let puny = policy(&["xn--bcher-kva.example"], &[]);
        assert_eq!(puny.check("https://bücher.example/"), Verdict::Allow);

        // Look-alike letters are other letters: Cyrillic "а" and "е" in "apple" and
        // "example", a full-width "ｉ".
        let plain = policy(&["apple.example"], &[]);
        assert_eq!(
            plain.check("https://\u{430}pple.example/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            plain.check("https://apple.\u{435}xample/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            plain.check("https://xn--pple-43d.example/"),
            blocked(BlockReason::NotAllowed)
        );
        let ascii = policy(&["id.example"], &[]);
        assert_eq!(ascii.check("https://\u{ff49}d.example/"), Verdict::Allow);
        assert_eq!(
            ascii.check("https://\u{131}d.example/"),
            blocked(BlockReason::NotAllowed)
        );

        let wild = policy(&["*.bücher.example"], &[]);
        assert_eq!(
            wild.check("https://shop.xn--bcher-kva.example/"),
            Verdict::Allow
        );
        assert_eq!(
            wild.check("https://shop.bucher.example/"),
            blocked(BlockReason::NotAllowed)
        );
    }

    #[test]
    fn only_https_is_loaded() {
        let p = policy(&["id.example"], &["https://id.example/done"]);
        for url in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            " javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "data:text/html;base64,PGgxPmhpPC9oMT4=",
            "file:///etc/passwd",
            "file://id.example/etc/passwd",
            "blob:https://id.example/6f3f7c52-0000-0000-0000-000000000000",
            "about:blank",
            "about:srcdoc",
            "ftp://id.example/",
            "ws://id.example/",
            "wss://id.example/",
            "http://id.example/",
            "view-source:https://id.example/",
            "inlet://open",
            "chrome://settings",
            "mailto:a@id.example",
            "tel:123",
        ] {
            assert!(
                matches!(
                    p.check(url),
                    Verdict::Block(BlockReason::Scheme | BlockReason::Unparseable)
                ),
                "{url}: {:?}",
                p.check(url)
            );
        }
        // A scheme the engine would read as one of those, written to hide it.
        assert_eq!(
            p.check("java\tscript:alert(1)"),
            blocked(BlockReason::Scheme)
        );
        assert_eq!(
            p.check("\u{1}javascript:alert(1)"),
            blocked(BlockReason::Scheme)
        );
    }

    #[test]
    fn an_ip_address_is_never_the_host() {
        let p = policy(&["id.example"], &[]);
        for url in [
            "https://127.0.0.1/",
            "https://127.0.0.1:443/",
            "https://[::1]/",
            "https://0x7f.1/",
            "https://2130706433/",
            "https://0177.0.0.1/",
            "https://192.0.2.1/",
            "https://[::ffff:127.0.0.1]/",
            "https://169.254.169.254/latest/meta-data",
        ] {
            assert_eq!(p.check(url), blocked(BlockReason::IpAddress), "{url}");
        }
    }

    #[test]
    fn a_url_that_does_not_parse_is_blocked() {
        let p = policy(&["id.example"], &[]);
        for url in [
            "",
            "id.example",
            "//id.example/",
            "/path",
            "https://",
            "https:///x",
            "https://exa mple/",
        ] {
            assert!(
                matches!(p.check(url), Verdict::Block(_)),
                "{url}: {:?}",
                p.check(url)
            );
        }
        assert_eq!(p.check(""), blocked(BlockReason::Unparseable));
    }

    #[test]
    fn a_very_long_url_is_blocked() {
        let p = policy(&["id.example"], &[]);
        let long = format!("https://id.example/{}", "a".repeat(MAX_URL_LEN));
        assert_eq!(p.check(&long), blocked(BlockReason::TooLong));
        let fits = format!("https://id.example/{}", "a".repeat(MAX_URL_LEN - 40));
        assert_eq!(p.check(&fits), Verdict::Allow);
    }

    #[test]
    fn patterns_that_could_be_read_two_ways_are_refused() {
        let bad = [
            "",
            "*",
            "*.",
            "*.com",
            "*.*.id.example",
            "id.*.example",
            "i*d.example",
            "**.id.example",
            "https://id.example",
            "id.example/",
            "id.example/path",
            "id.example?x=1",
            "id.example#x",
            "user@id.example",
            "id.example\\",
            "id%2Eexample",
            "id.example:",
            "id.example:0",
            "id.example:08",
            "id.example:65536",
            "id.example:-1",
            "id.example:80:80",
            "id.example:https",
            ".id.example",
            "id.example.",
            "..id.example",
            "id..example",
            " id.example",
            "id.example ",
            "id .example",
            "id.example\n",
            "localhost",
            "intranet",
            "1.2.3.4",
            "127.0.0.1:8443",
            "[::1]",
            "[::1]:8443",
            "::1",
            "0x7f.1",
        ];
        for pattern in bad {
            assert!(
                Policy::new(&strings(&[pattern]), &[], NORMAL).is_err(),
                "{pattern:?} was accepted"
            );
        }
        // One bad pattern among good ones names itself.
        assert_eq!(
            Policy::new(&strings(&["id.example", "*.com"]), &[], NORMAL).unwrap_err(),
            RequestError::BadPattern(1)
        );
    }

    #[test]
    fn rules_have_limits() {
        assert_eq!(
            Policy::new(&[], &[], NORMAL).unwrap_err(),
            RequestError::NoAllow
        );
        let many: Vec<String> = (0..=MAX_RULES).map(|i| format!("h{i}.example")).collect();
        assert_eq!(
            Policy::new(&many, &[], NORMAL).unwrap_err(),
            RequestError::TooManyRules
        );
        assert_eq!(
            Policy::new(&strings(&["id.example"]), &many, NORMAL).unwrap_err(),
            RequestError::TooManyRules
        );
        assert!(Policy::new(&many[..MAX_RULES], &[], NORMAL).is_ok());
    }

    #[test]
    fn an_intercept_prefix_matches_as_a_url() {
        let p = policy(&["id.example"], &["https://id.example/done"]);
        for url in [
            "https://id.example/done",
            "https://id.example/done/",
            "https://id.example/done/more",
            "https://id.example/done?code=1&state=2",
            "https://id.example/done#fragment",
            "https://ID.EXAMPLE/done",
            "https://id.example:443/done",
            // Dot segments and the like are resolved the way the engine resolves them.
            "https://id.example/x/../done",
            "https://id.example/./done",
            "https://id.example/%2e/done",
        ] {
            assert_eq!(p.check(url), Verdict::Intercept, "{url}");
        }
        for (url, expected) in [
            ("https://id.example/donezo", Verdict::Allow),
            ("https://id.example/done.evil", Verdict::Allow),
            ("https://id.example/done%2Fmore", Verdict::Allow),
            ("https://id.example/other", Verdict::Allow),
            ("https://id.example/", Verdict::Allow),
            ("https://id.example/done/../other", Verdict::Allow),
            (
                "https://id.example:8443/done",
                blocked(BlockReason::NotAllowed),
            ),
            (
                "https://id.example.evil.test/done",
                blocked(BlockReason::NotAllowed),
            ),
            ("https://evil.test/done", blocked(BlockReason::NotAllowed)),
            (
                "https://evil.test/https://id.example/done",
                blocked(BlockReason::NotAllowed),
            ),
            (
                "https://id.example@evil.test/done",
                blocked(BlockReason::Userinfo),
            ),
            (
                "https://evil.test@id.example/done",
                blocked(BlockReason::Userinfo),
            ),
            ("http://id.example/done", blocked(BlockReason::Scheme)),
        ] {
            assert_eq!(p.check(url), expected, "{url}");
        }
    }

    #[test]
    fn an_intercept_prefix_may_name_another_host_than_allow() {
        // The redirect target of a sign-in is often not a page the window may show.
        let p = policy(&["id.example"], &["https://app.example/callback"]);
        assert_eq!(
            p.check("https://app.example/callback?code=1"),
            Verdict::Intercept
        );
        assert_eq!(
            p.check("https://app.example/other"),
            blocked(BlockReason::NotAllowed)
        );
    }

    #[test]
    fn an_intercept_prefix_with_a_query_needs_that_query() {
        let p = policy(&["id.example"], &["https://id.example/cb?state=abc"]);
        assert_eq!(
            p.check("https://id.example/cb?state=abc"),
            Verdict::Intercept
        );
        assert_eq!(
            p.check("https://id.example/cb?state=abc&code=1"),
            Verdict::Intercept
        );
        assert_eq!(p.check("https://id.example/cb?state=xyz"), Verdict::Allow);
        assert_eq!(p.check("https://id.example/cb"), Verdict::Allow);
    }

    #[test]
    fn an_intercept_prefix_with_a_trailing_slash_or_the_root() {
        let dir = policy(&["id.example"], &["https://id.example/flow/"]);
        assert_eq!(dir.check("https://id.example/flow/"), Verdict::Intercept);
        assert_eq!(dir.check("https://id.example/flow/a"), Verdict::Intercept);
        assert_eq!(dir.check("https://id.example/flow"), Verdict::Intercept);
        assert_eq!(dir.check("https://id.example/flowers"), Verdict::Allow);

        let root = policy(&["id.example"], &["https://id.example/"]);
        assert_eq!(
            root.check("https://id.example/anything"),
            Verdict::Intercept
        );
        assert_eq!(root.check("https://id.example"), Verdict::Intercept);
    }

    #[test]
    fn intercept_prefixes_are_held_to_the_same_rules() {
        let bad = [
            "",
            "done",
            "/done",
            "http://id.example/done",
            "javascript:alert(1)",
            "data:text/plain,x",
            "file:///done",
            "https://user@id.example/done",
            "https://id.example/done#frag",
            "https://127.0.0.1/done",
            "https://id.example./done",
            "https://id example/done",
        ];
        for prefix in bad {
            assert!(
                Policy::new(&strings(&["id.example"]), &strings(&[prefix]), NORMAL).is_err(),
                "{prefix:?} was accepted"
            );
        }
        assert_eq!(
            Policy::new(
                &strings(&["id.example"]),
                &strings(&["https://id.example/a", "x"]),
                NORMAL
            )
            .unwrap_err(),
            RequestError::BadPrefix(1)
        );
    }

    #[test]
    fn an_intercept_wins_over_allow_but_not_over_the_basic_rules() {
        let p = policy(&["id.example"], &["https://id.example/"]);
        assert_eq!(p.check("https://id.example/x"), Verdict::Intercept);
        assert_eq!(
            p.check("https://u@id.example/x"),
            blocked(BlockReason::Userinfo)
        );
        assert_eq!(p.check("http://id.example/x"), blocked(BlockReason::Scheme));
    }

    fn request(url: &str) -> OpenRequest {
        OpenRequest {
            url: url.into(),
            allow: strings(&["id.example"]),
            intercept: strings(&["https://id.example/done"]),
            frames: vec![],
            title: "Sign in".into(),
            size: crate::protocol::Size {
                width: 480,
                height: 640,
            },
            parent: None,
        }
    }

    #[test]
    fn a_request_must_start_on_an_allowed_page() {
        assert!(Policy::for_request(&request("https://id.example/login"), NORMAL).is_ok());
        for url in [
            "https://other.example/",
            "http://id.example/",
            "javascript:alert(1)",
            "https://id.example/done",
            "",
        ] {
            assert_eq!(
                Policy::for_request(&request(url), NORMAL).unwrap_err(),
                RequestError::BadUrl,
                "{url}"
            );
        }
        let mut odd = request("https://id.example/");
        odd.title.clear();
        assert_eq!(
            Policy::for_request(&odd, NORMAL).unwrap_err(),
            RequestError::BadWindow
        );
    }

    #[test]
    fn a_frame_host_is_allowed_as_a_frame_and_never_as_the_window() {
        let p = Policy::new(&strings(&["id.example"]), &[], NORMAL)
            .unwrap()
            .with_frames(&strings(&["captcha.example", "*.pay.example"]))
            .unwrap();
        // The window may only go to `allow`.
        assert_eq!(p.check("https://id.example/a"), Verdict::Allow);
        assert_eq!(
            p.check("https://captcha.example/"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            p.check("https://js.pay.example/"),
            blocked(BlockReason::NotAllowed)
        );
        // A frame may go to either list, and to nothing else.
        for url in [
            "https://id.example/a",
            "https://captcha.example/widget",
            "https://js.pay.example/frame",
        ] {
            assert_eq!(p.check_frame(url), Verdict::Allow, "{url}");
        }
        for (url, reason) in [
            ("https://evil.example/", BlockReason::NotAllowed),
            ("https://pay.example/", BlockReason::NotAllowed),
            (
                "https://captcha.example@evil.example/",
                BlockReason::Userinfo,
            ),
            ("http://captcha.example/", BlockReason::Scheme),
            ("data:text/html,x", BlockReason::Scheme),
            ("javascript:alert(1)", BlockReason::Scheme),
            ("https://127.0.0.1/", BlockReason::IpAddress),
        ] {
            assert_eq!(p.check_frame(url), blocked(reason), "{url}");
        }
    }

    #[test]
    fn frame_patterns_are_held_to_the_same_rules() {
        let base = || Policy::new(&strings(&["id.example"]), &[], NORMAL).unwrap();
        for bad in [
            "*",
            "*.com",
            "https://x.example",
            "localhost",
            "1.2.3.4",
            "",
        ] {
            assert!(
                base().with_frames(&strings(&[bad])).is_err(),
                "{bad:?} was accepted"
            );
        }
        assert_eq!(
            base()
                .with_frames(&strings(&["ok.example", "*.com"]))
                .unwrap_err(),
            RequestError::BadFramePattern(1)
        );
        let many: Vec<String> = (0..=MAX_RULES).map(|i| format!("h{i}.example")).collect();
        assert_eq!(
            base().with_frames(&many).unwrap_err(),
            RequestError::TooManyRules
        );
    }

    #[test]
    fn an_intercept_url_in_a_frame_ends_the_flow_too() {
        let p = policy(&["id.example"], &["https://id.example/done"]);
        assert_eq!(
            p.check_frame("https://id.example/done?code=1"),
            Verdict::Intercept
        );
    }

    #[test]
    fn loopback_mode_allows_plain_http_on_loopback_only() {
        let p = Policy::new(
            &strings(&["127.0.0.1:8080", "localhost:8080", "id.example"]),
            &strings(&["http://127.0.0.1:8080/cb"]),
            LOOPBACK,
        )
        .unwrap();
        assert_eq!(p.check("http://127.0.0.1:8080/x"), Verdict::Allow);
        assert_eq!(p.check("https://127.0.0.1:8080/x"), Verdict::Allow);
        assert_eq!(p.check("http://localhost:8080/x"), Verdict::Allow);
        assert_eq!(p.check("http://127.0.0.1:8080/cb?x=1"), Verdict::Intercept);
        assert_eq!(
            p.check("http://127.0.0.1:8081/x"),
            blocked(BlockReason::NotAllowed)
        );
        assert_eq!(
            p.check("http://127.0.0.2:8080/x"),
            blocked(BlockReason::NotAllowed)
        );
        // Not on loopback: still https only, and no other IP address.
        assert_eq!(p.check("http://id.example/"), blocked(BlockReason::Scheme));
        assert_eq!(p.check("https://id.example/"), Verdict::Allow);
        assert_eq!(
            p.check("https://192.0.2.1/"),
            blocked(BlockReason::IpAddress)
        );
        assert_eq!(p.check("javascript:alert(1)"), blocked(BlockReason::Scheme));
        assert_eq!(
            p.check("http://u@127.0.0.1:8080/"),
            blocked(BlockReason::Userinfo)
        );
        // Not a loophole for other names that look like loopback.
        assert_eq!(
            p.check("http://localhost.evil.test:8080/"),
            blocked(BlockReason::Scheme)
        );
        // Only the loopback addresses are accepted as patterns.
        assert!(Policy::new(&strings(&["192.0.2.1"]), &[], LOOPBACK).is_err());
        assert!(Policy::new(&strings(&["*.localhost"]), &[], LOOPBACK).is_err());
    }

    #[test]
    fn this_build_does_not_trust_loopback_unless_it_is_a_test_build() {
        // Loopback is on exactly when the `loopback` feature is: a plain build trusts none.
        assert_eq!(
            PolicyOptions::of_this_build().loopback_http,
            cfg!(feature = "loopback")
        );
        assert_eq!(LOOPBACK_HTTP, cfg!(feature = "loopback"));
    }
}
