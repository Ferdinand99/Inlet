//! [`Capabilities`]: what one account's instance lets it do, in one place.
//!
//! Fluxer's own client asks its runtime config, the experiment assignments and
//! the account's flags in many places and hides or locks what is off. Inlet asks
//! one value instead: `caps.has(Cap::ChannelThreads)`. A capability is built from
//!
//! - the instance's discovery document (`/.well-known/fluxer`): its features,
//!   community policy and services ([`Capabilities::set_discovery`]),
//! - the account's experiment assignments (`GET /experiments`,
//!   [`Capabilities::set_experiments`]),
//! - the account itself, from READY and `USER_UPDATE` ([`Capabilities::notices`]),
//! - and the server's own `features` ([`Capabilities::guild_has`]).
//!
//! # Defaults
//!
//! Until a discovery document is known, an instance flag has the value Fluxer's
//! schema gives a flag that is missing (voice off, direct messages, friends and
//! personal notes on). Until the first answer of `/experiments` arrives, only
//! threads are assumed on, as before this module; an instance that never answers
//! the route (404 three times) has none of them. A flag the document has with a
//! value of the wrong type counts as missing.
//!
//! The value is plain data and has no clock, so every flag can be switched on
//! and off in a test.

use inlet_protocol::{
    Branding, CurrentUser, Discovery, DomainMigrationSwitch, Experiments, InstanceCommunity,
    InstanceFeatures, InstanceNotice, InstanceServices, LegalLinks,
};

/// A thing an instance can switch on or off for an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Cap {
    /// Threads and forums (`assignments.channel_threads`). Without it the routes answer 404.
    ChannelThreads,
    /// Visiting a community without joining (`assignments.guild_visits`).
    GuildVisits,
    /// Peer-to-peer voice calls (`assignments.voice_p2p`).
    VoiceP2p,
    /// The new premium page (`assignments.plutonium_page`).
    PlutoniumPage,
    /// The domain move reaches this account (`assignments.domain_migration`).
    DomainMigration,
    /// Voice and video (`features.voice_enabled`).
    Voice,
    /// A premium tier (`features.premium_enabled`).
    Premium,
    /// Purchases through Stripe (`features.stripe_enabled`).
    Stripe,
    /// The Explore directory (`features.discovery_enabled`).
    Explore,
    /// The instance sends email (`features.emails_enabled`).
    Email,
    /// Verification emails carry a code (`features.email_codes`).
    EmailCodes,
    /// A self-hosted instance (`features.self_hosted`): Fluxer's own terms, premium and
    /// help texts do not apply.
    SelfHosted,
    /// Direct messages and group DMs (`community.direct_messages_disabled` is false).
    DirectMessages,
    /// Friends and friend requests (`community.friends_disabled` is false).
    Friends,
    /// Personal notes (`community.personal_notes_enabled`).
    PersonalNotes,
    /// Every account can create communities (`community.guild_create_access`).
    GuildCreate,
    /// The GIF picker (`services.gif_enabled`).
    Gif,
    /// YouTube link previews (`services.youtube_enabled`).
    YouTube,
    /// Bluesky connections (`services.bluesky_enabled`).
    Bluesky,
}

impl Cap {
    /// Every capability, in declaration order.
    pub const ALL: [Cap; 19] = [
        Cap::ChannelThreads,
        Cap::GuildVisits,
        Cap::VoiceP2p,
        Cap::PlutoniumPage,
        Cap::DomainMigration,
        Cap::Voice,
        Cap::Premium,
        Cap::Stripe,
        Cap::Explore,
        Cap::Email,
        Cap::EmailCodes,
        Cap::SelfHosted,
        Cap::DirectMessages,
        Cap::Friends,
        Cap::PersonalNotes,
        Cap::GuildCreate,
        Cap::Gif,
        Cap::YouTube,
        Cap::Bluesky,
    ];

    /// Whether this capability comes from an experiment assignment.
    pub fn is_experiment(self) -> bool {
        matches!(
            self,
            Cap::ChannelThreads
                | Cap::GuildVisits
                | Cap::VoiceP2p
                | Cap::PlutoniumPage
                | Cap::DomainMigration
        )
    }
}

/// Where the account's experiment assignments stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExperimentState {
    /// Not asked yet, or the last asks failed: only threads are assumed on.
    #[default]
    Pending,
    /// The instance answered.
    Loaded(Experiments),
    /// The instance has no `/experiments` route: no experiment is on.
    Unavailable,
}

/// What is kept of the discovery document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceSnapshot {
    /// `features`.
    pub features: InstanceFeatures,
    /// `community`.
    pub community: InstanceCommunity,
    /// `services`.
    pub services: InstanceServices,
    /// `domain_migration`.
    pub domain_migration: DomainMigrationSwitch,
    /// `app_public.branding`.
    pub branding: Branding,
    /// `app_public.legal`.
    pub legal: LegalLinks,
    /// `notice`.
    pub notice: Option<InstanceNotice>,
}

impl InstanceSnapshot {
    /// Reads the parts of a discovery document.
    pub fn of(d: &Discovery) -> Self {
        InstanceSnapshot {
            features: d.instance_features(),
            community: d.community(),
            services: d.services(),
            domain_migration: d.domain_migration(),
            branding: d.branding(),
            legal: d.legal(),
            notice: d.notice(),
        }
    }
}

impl Default for InstanceSnapshot {
    /// What an empty document gives: Fluxer's defaults for every missing flag.
    fn default() -> Self {
        let empty: Discovery = serde_json::from_value(serde_json::json!({
            "api_code_version": 1,
            "endpoints": {"api_public": "https://invalid.example", "gateway": "wss://invalid.example"},
        }))
        .expect("an empty discovery document parses");
        InstanceSnapshot::of(&empty)
    }
}

/// When the instance's policies last changed. Fluxer's official client has these as
/// constants in its build (`packages/constants/src/PolicyConstants.ts`, `null` at the
/// time of writing), and only checks them on an instance that is not self-hosted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PolicyDates {
    /// The terms of service last changed (Unix milliseconds).
    pub terms_updated_ms: Option<i64>,
    /// The privacy policy last changed (Unix milliseconds).
    pub privacy_updated_ms: Option<i64>,
}

/// The dates Fluxer's current client has. Update them from `PolicyConstants.ts` when
/// Fluxer sets them.
pub const POLICY_DATES: PolicyDates = PolicyDates {
    terms_updated_ms: None,
    privacy_updated_ms: None,
};

/// Which updated policies the account still has to take in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TermsUpdate {
    /// Nothing to take in.
    #[default]
    None,
    /// The terms changed: the account must agree.
    Terms,
    /// The privacy policy changed: the account only reads it.
    Privacy,
    /// Both changed.
    Both,
}

/// What the account's own flags ask the person to do. Each one is a banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccountNotices {
    /// The email address still needs to be confirmed.
    pub confirm_email: bool,
    /// Emails to the address bounce, so it cannot be confirmed.
    pub email_bounced: bool,
    /// Messaging is paused on the account until it does a step the instance emailed.
    pub limited: bool,
    /// Updated policies to take in.
    pub terms: TermsUpdate,
}

/// A feature of a server (`features` of a guild).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GuildFeature {
    /// New invites are off.
    InvitesDisabled,
    /// Anyone may visit without joining.
    VisitsEnabled,
    /// Members may not be direct messaged through the server.
    MemberDmsDisabled,
    /// Announcement channels are off.
    AnnouncementChannelsDisabled,
    /// The server has a vanity invite link.
    VanityUrl,
    /// The server is listed in Explore.
    Discoverable,
    /// More emoji slots.
    MoreEmoji,
    /// More sticker slots.
    MoreStickers,
    /// The owner's crown is hidden.
    HideOwnerCrown,
    /// An animated icon.
    AnimatedIcon,
    /// A banner.
    Banner,
    /// An animated banner.
    AnimatedBanner,
    /// An invite splash.
    InviteSplash,
    /// A banner that sits apart from the header.
    DetachedBanner,
    /// Channel names are free text.
    TextChannelFlexibleNames,
    /// A raid was detected.
    RaidDetected,
    /// A very large server (the member list works differently).
    VeryLargeGuild,
    /// Treated as large whatever the member count.
    LargeGuildOverride,
    /// A verified server.
    Verified,
    /// A partnered server.
    Partnered,
    /// A visionary server.
    Visionary,
}

impl GuildFeature {
    /// The name on the wire.
    pub fn wire(self) -> &'static str {
        match self {
            GuildFeature::InvitesDisabled => "INVITES_DISABLED",
            GuildFeature::VisitsEnabled => "VISITS_ENABLED",
            GuildFeature::MemberDmsDisabled => "MEMBER_DMS_DISABLED",
            GuildFeature::AnnouncementChannelsDisabled => "ANNOUNCEMENT_CHANNELS_DISABLED",
            GuildFeature::VanityUrl => "VANITY_URL",
            GuildFeature::Discoverable => "DISCOVERABLE",
            GuildFeature::MoreEmoji => "MORE_EMOJI",
            GuildFeature::MoreStickers => "MORE_STICKERS",
            GuildFeature::HideOwnerCrown => "HIDE_OWNER_CROWN",
            GuildFeature::AnimatedIcon => "ANIMATED_ICON",
            GuildFeature::Banner => "BANNER",
            GuildFeature::AnimatedBanner => "ANIMATED_BANNER",
            GuildFeature::InviteSplash => "INVITE_SPLASH",
            GuildFeature::DetachedBanner => "DETACHED_BANNER",
            GuildFeature::TextChannelFlexibleNames => "TEXT_CHANNEL_FLEXIBLE_NAMES",
            GuildFeature::RaidDetected => "RAID_DETECTED",
            GuildFeature::VeryLargeGuild => "VERY_LARGE_GUILD",
            GuildFeature::LargeGuildOverride => "LARGE_GUILD_OVERRIDE",
            GuildFeature::Verified => "VERIFIED",
            GuildFeature::Partnered => "PARTNERED",
            GuildFeature::Visionary => "VISIONARY",
        }
    }
}

/// What one account's instance lets it do. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct Capabilities {
    instance: Option<InstanceSnapshot>,
    experiments: ExperimentState,
    policy: PolicyDates,
}

impl Default for Capabilities {
    fn default() -> Self {
        Capabilities::new()
    }
}

impl Capabilities {
    /// Capabilities for an account whose instance is not known yet.
    pub fn new() -> Self {
        Capabilities {
            instance: None,
            experiments: ExperimentState::Pending,
            policy: POLICY_DATES,
        }
    }

    /// Takes in a discovery document. Whether anything changed.
    pub fn set_discovery(&mut self, d: &Discovery) -> bool {
        let next = InstanceSnapshot::of(d);
        if self.instance.as_ref() == Some(&next) {
            return false;
        }
        self.instance = Some(next);
        true
    }

    /// Takes in the state of the experiment assignments. Whether anything changed.
    pub fn set_experiments(&mut self, state: ExperimentState) -> bool {
        if self.experiments == state {
            return false;
        }
        self.experiments = state;
        true
    }

    /// Sets the policy dates (a test, or a build that knows them).
    pub fn set_policy_dates(&mut self, dates: PolicyDates) {
        self.policy = dates;
    }

    /// The experiment assignments' state.
    pub fn experiments(&self) -> ExperimentState {
        self.experiments
    }

    /// What is known of the discovery document (Fluxer's defaults before the first).
    pub fn instance(&self) -> InstanceSnapshot {
        self.instance.clone().unwrap_or_default()
    }

    /// Whether a discovery document has been taken in.
    pub fn has_discovery(&self) -> bool {
        self.instance.is_some()
    }

    fn snapshot(&self) -> std::borrow::Cow<'_, InstanceSnapshot> {
        match &self.instance {
            Some(s) => std::borrow::Cow::Borrowed(s),
            None => std::borrow::Cow::Owned(InstanceSnapshot::default()),
        }
    }

    /// Whether `cap` is on.
    pub fn has(&self, cap: Cap) -> bool {
        if cap.is_experiment() {
            return self.experiment_on(cap);
        }
        let s = self.snapshot();
        match cap {
            Cap::Voice => s.features.voice_enabled,
            Cap::Premium => s.features.premium_enabled,
            Cap::Stripe => s.features.stripe_enabled,
            Cap::Explore => s.features.discovery_enabled,
            Cap::Email => s.features.emails_enabled,
            Cap::EmailCodes => s.features.email_codes,
            Cap::SelfHosted => s.features.self_hosted,
            // Direct messages off also takes friend requests off, unless the instance
            // says it does not (the document's own rule, applied in the view).
            Cap::DirectMessages => !s.community.direct_messages_disabled,
            Cap::Friends => !s.community.friends_disabled,
            Cap::PersonalNotes => s.community.personal_notes_enabled,
            Cap::GuildCreate => s.community.guild_create_access,
            Cap::Gif => s.services.gif_enabled,
            Cap::YouTube => s.services.youtube_enabled,
            Cap::Bluesky => s.services.bluesky_enabled,
            Cap::ChannelThreads
            | Cap::GuildVisits
            | Cap::VoiceP2p
            | Cap::PlutoniumPage
            | Cap::DomainMigration => false,
        }
    }

    fn experiment_on(&self, cap: Cap) -> bool {
        match self.experiments {
            ExperimentState::Pending => cap == Cap::ChannelThreads,
            ExperimentState::Unavailable => false,
            ExperimentState::Loaded(e) => {
                let a = e.assignments;
                match cap {
                    Cap::ChannelThreads => a.channel_threads.is_some_and(|t| t.active),
                    Cap::GuildVisits => a.guild_visits == Some(true),
                    Cap::VoiceP2p => a.voice_p2p.is_some_and(|p| p.enabled),
                    Cap::PlutoniumPage => a.plutonium_page == Some(true),
                    Cap::DomainMigration => a.domain_migration == Some(true),
                    _ => false,
                }
            }
        }
    }

    /// The most participants of a peer-to-peer call, when the assignment is on.
    pub fn voice_p2p_max_participants(&self) -> Option<u32> {
        match self.experiments {
            ExperimentState::Loaded(e) => e
                .assignments
                .voice_p2p
                .filter(|p| p.enabled)
                .and_then(|p| p.max_participants),
            _ => None,
        }
    }

    /// The instance's branding.
    pub fn branding(&self) -> Branding {
        self.snapshot().branding.clone()
    }

    /// The instance's terms and privacy links.
    pub fn legal(&self) -> LegalLinks {
        self.snapshot().legal.clone()
    }

    /// The administrator's notice, whether or not it is live now.
    pub fn notice(&self) -> Option<InstanceNotice> {
        self.snapshot().notice.clone()
    }

    /// The notice, if it is live at `now_ms`.
    pub fn live_notice(&self, now_ms: i64) -> Option<InstanceNotice> {
        self.notice().filter(|n| n.is_live(now_ms))
    }

    /// The domain move is switched on in the discovery document and reaches this
    /// account. Fluxer shows its "moved" banner only then.
    pub fn domain_moved(&self) -> bool {
        self.snapshot().domain_migration.enabled && self.has(Cap::DomainMigration)
    }

    /// What the account's own flags ask the person to do.
    pub fn notices(&self, me: &CurrentUser) -> AccountNotices {
        let s = self.snapshot();
        let confirm_email = s.features.emails_enabled
            && !me.email_bounced()
            && me.verification_need() == Some("confirm_email");
        let email_bounced = s.features.emails_enabled && me.email_bounced();
        AccountNotices {
            confirm_email,
            email_bounced,
            limited: me.account_limited(),
            terms: self.terms_update(me),
        }
    }

    fn terms_update(&self, me: &CurrentUser) -> TermsUpdate {
        // Fluxer only asks on an instance that is not self-hosted.
        if self.snapshot().features.self_hosted {
            return TermsUpdate::None;
        }
        let outdated = |updated: Option<i64>, agreed: Option<i64>| {
            updated.is_some_and(|u| agreed.is_none_or(|a| a < u))
        };
        let terms = outdated(self.policy.terms_updated_ms, me.terms_agreed_at_ms());
        let privacy = outdated(self.policy.privacy_updated_ms, me.privacy_agreed_at_ms());
        match (terms, privacy) {
            (true, true) => TermsUpdate::Both,
            (true, false) => TermsUpdate::Terms,
            (false, true) => TermsUpdate::Privacy,
            (false, false) => TermsUpdate::None,
        }
    }

    /// Whether a server (with these `features`) has `feature`, as the instance lets it
    /// count: a server's visits need the visits experiment and no disabled invites, and
    /// the Explore listing needs the directory.
    pub fn guild_has(&self, features: &[String], feature: GuildFeature) -> bool {
        let raw = |f: GuildFeature| features.iter().any(|x| x == f.wire());
        match feature {
            GuildFeature::VisitsEnabled => {
                self.has(Cap::GuildVisits)
                    && raw(GuildFeature::VisitsEnabled)
                    && !raw(GuildFeature::InvitesDisabled)
            }
            GuildFeature::Discoverable => self.has(Cap::Explore) && raw(GuildFeature::Discoverable),
            other => raw(other),
        }
    }
}
