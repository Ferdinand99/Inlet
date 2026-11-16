//! Typed views of the parts of the discovery document a client acts on: the
//! feature flags, the community policy, the optional services, the branding,
//! the administrator's notice and the domain move. The document itself stays
//! raw in [`Discovery`]; these read it tolerantly and with Fluxer's own
//! defaults for a field that is missing (`InstanceSchemas.ts` says what
//! "missing means" for each). Text from the instance is cut to a sensible
//! length here and cleaned for display by the app.
//!
//! The views never fail: a field of the wrong type counts as missing.

use serde_json::Value;

use crate::instance::Discovery;
use crate::models::parse_timestamp_ms;

/// Longest notice text kept, in characters.
pub const MAX_NOTICE_CHARS: usize = 2_000;
/// Longest name, ID or label kept, in characters.
pub const MAX_LABEL_CHARS: usize = 200;
/// Longest URL kept, in characters.
pub const MAX_URL_CHARS: usize = 2_048;

fn text(v: Option<&Value>, max: usize) -> Option<String> {
    let s = v?.as_str()?;
    if s.is_empty() {
        return None;
    }
    Some(s.chars().take(max).collect())
}

fn flag(v: Option<&Value>) -> Option<bool> {
    v?.as_bool()
}

/// The notice from the instance's administrator (`notice`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceNotice {
    /// Changes whenever the text changes, so a dismissal is remembered per text.
    pub id: String,
    /// The text.
    pub message: String,
    /// A link shown with it.
    pub url: Option<String>,
    /// When to start showing it (Unix milliseconds), `None` for now.
    pub starts_at_ms: Option<i64>,
    /// When to stop showing it (Unix milliseconds), `None` for never.
    pub ends_at_ms: Option<i64>,
}

impl InstanceNotice {
    /// Whether the notice is to be shown at `now_ms` (Fluxer's `isInstanceNoticeLive`).
    pub fn is_live(&self, now_ms: i64) -> bool {
        self.starts_at_ms.is_none_or(|s| s <= now_ms) && self.ends_at_ms.is_none_or(|e| now_ms < e)
    }
}

/// The instance's own look and links (`app_public.branding`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Branding {
    /// The name the instance gives the product.
    pub product_name: Option<String>,
    /// Image URLs. Draw one only from the instance's media proxy.
    pub icon_url: Option<String>,
    /// The compact symbol.
    pub symbol_url: Option<String>,
    /// The logo.
    pub logo_url: Option<String>,
    /// The wordmark.
    pub wordmark_url: Option<String>,
    /// The accent colour, `#rrggbb`.
    pub theme_color: Option<String>,
    /// The public status page.
    pub status_page_url: Option<String>,
    /// The server found that the status page is an Instatus page whose incidents
    /// and maintenance a client can read.
    pub status_page_instatus: bool,
    /// The status page's incident history.
    pub status_page_incident_history_url: Option<String>,
    /// The name of the premium tier.
    pub premium_product_name: Option<String>,
    /// The instance applies its own stylesheet in its web and desktop app. Inlet never
    /// applies it.
    pub custom_css: bool,
    /// Extra domains whose links open without a warning.
    pub trusted_domains: Vec<String>,
}

/// The instance's legal documents (`app_public.legal`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LegalLinks {
    /// The terms of service.
    pub terms_url: Option<String>,
    /// The privacy policy.
    pub privacy_url: Option<String>,
    /// The community guidelines.
    pub guidelines_url: Option<String>,
}

/// `features`: what the instance offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceFeatures {
    /// Voice and video.
    pub voice_enabled: bool,
    /// A self-hosted instance (not Fluxer's own).
    pub self_hosted: bool,
    /// The instance has a premium tier.
    pub premium_enabled: bool,
    /// Purchases through Stripe.
    pub stripe_enabled: bool,
    /// The instance sends email (verification, password reset).
    pub emails_enabled: bool,
    /// The Explore directory. Missing means yes.
    pub discovery_enabled: bool,
    /// Verification mails carry a code to type in, not only a link.
    pub email_codes: bool,
    /// Limits for accounts that have not finished setting up (`legacy`, `enforced` …).
    /// Missing means `legacy`.
    pub interaction_tier: String,
    /// When joining a community needs a finished account. Missing means `always`.
    pub guild_join_gate: String,
    /// What an account needs to finish setting up, when the instance says.
    pub verification_basis: Option<String>,
}

/// `community`: the instance's policy for conversations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceCommunity {
    /// The instance is one community every account joins (Explore is then off).
    pub single_community: bool,
    /// Direct messages are off instance-wide.
    pub direct_messages_disabled: bool,
    /// Friend requests are off. Missing means `direct_messages_disabled` decides.
    pub friends_disabled: bool,
    /// People can use personal notes. Missing means yes.
    pub personal_notes_enabled: bool,
    /// Every account can create communities. Informational: Fluxer's client never reads it,
    /// "create" stays visible and the server decides.
    pub guild_create_access: bool,
}

/// `services`: optional third-party services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InstanceServices {
    /// The GIF picker.
    pub gif_enabled: bool,
    /// YouTube link previews.
    pub youtube_enabled: bool,
    /// Bluesky connections.
    pub bluesky_enabled: bool,
}

/// `domain_migration`: the move of the web app to a new domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DomainMigrationSwitch {
    /// The move is switched on.
    pub enabled: bool,
    /// Installed desktop web apps forward to the new domain once moved.
    pub standalone_forwarding: bool,
}

impl Discovery {
    /// `notice`, when there is one with a text.
    pub fn notice(&self) -> Option<InstanceNotice> {
        let n = self.extra.get("notice")?;
        let message = text(n.get("message"), MAX_NOTICE_CHARS)?;
        Some(InstanceNotice {
            id: text(n.get("id"), MAX_LABEL_CHARS)?,
            message,
            url: text(n.get("url"), MAX_URL_CHARS),
            starts_at_ms: n
                .get("starts_at")
                .and_then(Value::as_str)
                .and_then(parse_timestamp_ms),
            ends_at_ms: n
                .get("ends_at")
                .and_then(Value::as_str)
                .and_then(parse_timestamp_ms),
        })
    }

    /// `app_public.branding`.
    pub fn branding(&self) -> Branding {
        let Some(b) = self.app_public.get("branding") else {
            return Branding::default();
        };
        Branding {
            product_name: text(b.get("product_name"), MAX_LABEL_CHARS),
            icon_url: text(b.get("icon_url"), MAX_URL_CHARS),
            symbol_url: text(b.get("symbol_url"), MAX_URL_CHARS),
            logo_url: text(b.get("logo_url"), MAX_URL_CHARS),
            wordmark_url: text(b.get("wordmark_url"), MAX_URL_CHARS),
            theme_color: text(b.get("theme_color"), 16),
            status_page_url: text(b.get("status_page_url"), MAX_URL_CHARS),
            status_page_instatus: flag(b.get("status_page_instatus")).unwrap_or(false),
            status_page_incident_history_url: text(
                b.get("status_page_incident_history_url"),
                MAX_URL_CHARS,
            ),
            premium_product_name: text(b.get("premium_product_name"), MAX_LABEL_CHARS),
            custom_css: text(b.get("custom_css"), 1).is_some(),
            trusted_domains: b
                .get("trusted_domains")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|d| text(Some(d), MAX_LABEL_CHARS))
                        .take(100)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// `app_public.legal`.
    pub fn legal(&self) -> LegalLinks {
        let l = self.app_public.get("legal");
        let url = |k: &str| text(l.and_then(|l| l.get(k)), MAX_URL_CHARS);
        LegalLinks {
            terms_url: url("terms_url"),
            privacy_url: url("privacy_url"),
            guidelines_url: url("guidelines_url"),
        }
    }

    /// `features`, with Fluxer's defaults.
    pub fn instance_features(&self) -> InstanceFeatures {
        let f = |k: &str| flag(self.features.get(k));
        let s = |k: &str| text(self.features.get(k), MAX_LABEL_CHARS);
        InstanceFeatures {
            voice_enabled: f("voice_enabled").unwrap_or(false),
            self_hosted: f("self_hosted").unwrap_or(false),
            premium_enabled: f("premium_enabled").unwrap_or(false),
            stripe_enabled: f("stripe_enabled").unwrap_or(false),
            emails_enabled: f("emails_enabled").unwrap_or(false),
            discovery_enabled: f("discovery_enabled").unwrap_or(true),
            email_codes: f("email_codes").unwrap_or(false),
            interaction_tier: s("interaction_tier").unwrap_or_else(|| "legacy".into()),
            guild_join_gate: s("guild_join_gate").unwrap_or_else(|| "always".into()),
            verification_basis: s("verification_basis"),
        }
    }

    /// `community`, with Fluxer's defaults.
    pub fn community(&self) -> InstanceCommunity {
        let c = self.extra.get("community");
        let f = |k: &str| flag(c.and_then(|c| c.get(k)));
        let direct_messages_disabled = f("direct_messages_disabled").unwrap_or(false);
        InstanceCommunity {
            single_community: f("single_community").unwrap_or(false),
            direct_messages_disabled,
            friends_disabled: f("friends_disabled").unwrap_or(direct_messages_disabled),
            personal_notes_enabled: f("personal_notes_enabled").unwrap_or(true),
            guild_create_access: f("guild_create_access").unwrap_or(true),
        }
    }

    /// `services`.
    pub fn services(&self) -> InstanceServices {
        let s = self.extra.get("services");
        let f = |k: &str| flag(s.and_then(|s| s.get(k)));
        InstanceServices {
            gif_enabled: f("gif_enabled").unwrap_or(false),
            youtube_enabled: f("youtube_enabled").unwrap_or(false),
            bluesky_enabled: f("bluesky_enabled").unwrap_or(false),
        }
    }

    /// `domain_migration`.
    pub fn domain_migration(&self) -> DomainMigrationSwitch {
        let d = self.extra.get("domain_migration");
        let f = |k: &str| flag(d.and_then(|d| d.get(k)));
        DomainMigrationSwitch {
            enabled: f("enabled").unwrap_or(false),
            standalone_forwarding: f("standalone_forwarding").unwrap_or(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn discovery(extra: Value) -> Discovery {
        let mut doc = json!({
            "api_code_version": 1,
            "endpoints": {"api_public": "https://x.example/api", "gateway": "wss://x.example/gw"},
            "features": {}
        });
        for (k, v) in extra.as_object().unwrap() {
            doc[k] = v.clone();
        }
        serde_json::from_value(doc).unwrap()
    }

    #[test]
    fn a_document_without_the_fields_gets_fluxers_defaults() {
        let d = discovery(json!({}));
        let f = d.instance_features();
        assert!(f.discovery_enabled && !f.voice_enabled && !f.self_hosted && !f.emails_enabled);
        assert_eq!(f.interaction_tier, "legacy");
        assert_eq!(f.guild_join_gate, "always");
        let c = d.community();
        assert!(c.personal_notes_enabled && c.guild_create_access);
        assert!(!c.direct_messages_disabled && !c.friends_disabled);
        assert_eq!(d.services(), InstanceServices::default());
        assert_eq!(d.domain_migration(), DomainMigrationSwitch::default());
        assert!(d.notice().is_none());
        assert_eq!(d.branding(), Branding::default());
        assert_eq!(d.legal(), LegalLinks::default());
    }

    #[test]
    fn the_legal_links_are_read() {
        let d = discovery(json!({"app_public": {"legal": {
            "terms_url": "https://x.example/terms", "privacy_url": null,
            "guidelines_url": "https://x.example/guidelines"}}}));
        let l = d.legal();
        assert_eq!(l.terms_url.as_deref(), Some("https://x.example/terms"));
        assert_eq!(l.privacy_url, None);
        assert_eq!(
            l.guidelines_url.as_deref(),
            Some("https://x.example/guidelines")
        );
    }

    #[test]
    fn friends_follow_direct_messages_unless_the_instance_says_otherwise() {
        let off = discovery(json!({"community": {"direct_messages_disabled": true}}));
        assert!(off.community().friends_disabled);
        let split = discovery(
            json!({"community": {"direct_messages_disabled": true, "friends_disabled": false}}),
        );
        assert!(!split.community().friends_disabled);
        assert!(split.community().direct_messages_disabled);
    }

    #[test]
    fn the_notice_is_live_between_its_times() {
        let d = discovery(json!({"notice": {
            "id": "n1", "message": "Maintenance on Friday", "url": "https://status.example/",
            "starts_at": "2026-11-20T10:00:00.000Z", "ends_at": "2026-11-20T12:00:00.000Z"
        }}));
        let n = d.notice().unwrap();
        assert_eq!(n.id, "n1");
        let start = n.starts_at_ms.unwrap();
        let end = n.ends_at_ms.unwrap();
        assert_eq!(end - start, 2 * 3_600_000);
        assert!(!n.is_live(start - 1));
        assert!(n.is_live(start));
        assert!(n.is_live(end - 1));
        assert!(!n.is_live(end));
        let open = discovery(
            json!({"notice": {"id": "n2", "message": "Hi", "url": null, "starts_at": null, "ends_at": null}}),
        )
        .notice()
        .unwrap();
        assert!(open.is_live(0) && open.is_live(i64::MAX - 1));
        // No text or no ID: no notice.
        assert!(discovery(json!({"notice": {"id": "n3", "message": ""}}))
            .notice()
            .is_none());
        assert!(discovery(json!({"notice": null})).notice().is_none());
    }

    #[test]
    fn long_text_from_the_instance_is_cut() {
        let long = "x".repeat(10_000);
        let d = discovery(json!({
            "notice": {"id": "n", "message": long},
            "app_public": {"branding": {"product_name": "p".repeat(500), "custom_css": "a{}",
                                          "status_page_instatus": true,
                                          "trusted_domains": ["a.example", 7, "*.b.example"]}}
        }));
        assert_eq!(
            d.notice().unwrap().message.chars().count(),
            MAX_NOTICE_CHARS
        );
        let b = d.branding();
        assert_eq!(b.product_name.unwrap().chars().count(), MAX_LABEL_CHARS);
        assert!(b.custom_css && b.status_page_instatus);
        assert_eq!(b.trusted_domains, ["a.example", "*.b.example"]);
    }
}
