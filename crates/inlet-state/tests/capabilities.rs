//! What an account's instance lets it do: every capability switched on and off from
//! the discovery document, the experiment assignments and the account's own flags.

mod common;

use inlet_protocol::api::experiments::{Assignments, ChannelThreads, VoiceP2p};
use inlet_protocol::{Discovery, Experiments};
use inlet_state::{
    AccountState, Cap, Capabilities, ExperimentState, GuildFeature, PolicyDates, TermsUpdate,
};
use serde_json::{json, Value};

/// A discovery document with `patch` merged over an otherwise empty one.
fn discovery(patch: Value) -> Discovery {
    let mut doc = json!({
        "api_code_version": 1,
        "endpoints": {"api_public": "https://x.example/api", "gateway": "wss://x.example/gw"},
        "features": {}
    });
    for (k, v) in patch.as_object().unwrap() {
        match (doc.get_mut(k), v) {
            (Some(Value::Object(have)), Value::Object(add)) => {
                for (k2, v2) in add {
                    have.insert(k2.clone(), v2.clone());
                }
            }
            _ => {
                doc[k] = v.clone();
            }
        }
    }
    serde_json::from_value(doc).unwrap()
}

fn caps_of(patch: Value) -> Capabilities {
    let mut c = Capabilities::new();
    c.set_discovery(&discovery(patch));
    c
}

fn loaded(a: Assignments) -> ExperimentState {
    ExperimentState::Loaded(Experiments {
        assignments: a,
        ..Experiments::default()
    })
}

/// Each discovery flag: the document that turns it on and the one that turns it off.
fn flag_cases() -> Vec<(Cap, Value, Value)> {
    let f = |k: &str, v: bool| json!({"features": {k: v}});
    let community = |k: &str, v: bool| json!({"community": {k: v}});
    let service = |k: &str, v: bool| json!({"services": {k: v}});
    vec![
        (
            Cap::Voice,
            f("voice_enabled", true),
            f("voice_enabled", false),
        ),
        (
            Cap::Premium,
            f("premium_enabled", true),
            f("premium_enabled", false),
        ),
        (
            Cap::Stripe,
            f("stripe_enabled", true),
            f("stripe_enabled", false),
        ),
        (
            Cap::Explore,
            f("discovery_enabled", true),
            f("discovery_enabled", false),
        ),
        (
            Cap::Email,
            f("emails_enabled", true),
            f("emails_enabled", false),
        ),
        (
            Cap::EmailCodes,
            f("email_codes", true),
            f("email_codes", false),
        ),
        (
            Cap::SelfHosted,
            f("self_hosted", true),
            f("self_hosted", false),
        ),
        (
            Cap::DirectMessages,
            community("direct_messages_disabled", false),
            community("direct_messages_disabled", true),
        ),
        (
            Cap::Friends,
            community("friends_disabled", false),
            community("friends_disabled", true),
        ),
        (
            Cap::PersonalNotes,
            community("personal_notes_enabled", true),
            community("personal_notes_enabled", false),
        ),
        (
            Cap::GuildCreate,
            community("guild_create_access", true),
            community("guild_create_access", false),
        ),
        (
            Cap::Gif,
            service("gif_enabled", true),
            service("gif_enabled", false),
        ),
        (
            Cap::YouTube,
            service("youtube_enabled", true),
            service("youtube_enabled", false),
        ),
        (
            Cap::Bluesky,
            service("bluesky_enabled", true),
            service("bluesky_enabled", false),
        ),
    ]
}

#[test]
fn every_discovery_flag_switches_its_capability_on_and_off() {
    for (cap, on, off) in flag_cases() {
        assert!(caps_of(on).has(cap), "{cap:?} on");
        assert!(!caps_of(off).has(cap), "{cap:?} off");
    }
    // Every capability that is not an experiment has a case.
    for cap in Cap::ALL.into_iter().filter(|c| !c.is_experiment()) {
        assert!(flag_cases().iter().any(|(c, _, _)| *c == cap), "{cap:?}");
    }
}

#[test]
fn a_flag_switches_when_a_later_document_changes_it() {
    let mut c = caps_of(json!({"features": {"voice_enabled": true}}));
    assert!(c.has(Cap::Voice));
    // The same document again is no change; a new value is, and says so.
    assert!(!c.set_discovery(&discovery(json!({"features": {"voice_enabled": true}}))));
    assert!(c.set_discovery(&discovery(json!({"features": {"voice_enabled": false}}))));
    assert!(!c.has(Cap::Voice));
    assert!(c.set_discovery(&discovery(json!({"features": {"voice_enabled": true}}))));
    assert!(c.has(Cap::Voice));
}

#[test]
fn a_missing_flag_has_the_default_the_schema_gives_it() {
    let c = Capabilities::new();
    assert!(!c.has_discovery());
    for (cap, expected) in [
        (Cap::Voice, false),
        (Cap::SelfHosted, false),
        (Cap::Email, false),
        (Cap::Gif, false),
        (Cap::Explore, true),
        (Cap::DirectMessages, true),
        (Cap::Friends, true),
        (Cap::PersonalNotes, true),
        (Cap::GuildCreate, true),
    ] {
        assert_eq!(c.has(cap), expected, "{cap:?}");
    }
    // A flag with the wrong type counts as missing.
    let odd = caps_of(json!({"features": {"voice_enabled": "yes", "discovery_enabled": 3}}));
    assert!(!odd.has(Cap::Voice) && odd.has(Cap::Explore));
}

#[test]
fn friends_are_off_with_direct_messages_unless_the_instance_says_otherwise() {
    let both = caps_of(json!({"community": {"direct_messages_disabled": true}}));
    assert!(!both.has(Cap::DirectMessages) && !both.has(Cap::Friends));
    let friends_only = caps_of(
        json!({"community": {"direct_messages_disabled": true, "friends_disabled": false}}),
    );
    assert!(!friends_only.has(Cap::DirectMessages) && friends_only.has(Cap::Friends));
}

fn experiment_cases() -> Vec<(Cap, Assignments, Assignments)> {
    let none = Assignments::default();
    vec![
        (
            Cap::ChannelThreads,
            Assignments {
                channel_threads: Some(ChannelThreads {
                    active: true,
                    config_version: 1,
                }),
                ..none
            },
            Assignments {
                channel_threads: Some(ChannelThreads {
                    active: false,
                    config_version: 1,
                }),
                ..none
            },
        ),
        (
            Cap::GuildVisits,
            Assignments {
                guild_visits: Some(true),
                ..none
            },
            Assignments {
                guild_visits: Some(false),
                ..none
            },
        ),
        (
            Cap::VoiceP2p,
            Assignments {
                voice_p2p: Some(VoiceP2p {
                    enabled: true,
                    max_participants: Some(4),
                }),
                ..none
            },
            Assignments {
                voice_p2p: Some(VoiceP2p {
                    enabled: false,
                    max_participants: Some(4),
                }),
                ..none
            },
        ),
        (
            Cap::PlutoniumPage,
            Assignments {
                plutonium_page: Some(true),
                ..none
            },
            Assignments {
                plutonium_page: Some(false),
                ..none
            },
        ),
        (
            Cap::DomainMigration,
            Assignments {
                domain_migration: Some(true),
                ..none
            },
            Assignments {
                domain_migration: Some(false),
                ..none
            },
        ),
    ]
}

#[test]
fn every_experiment_switches_its_capability_on_and_off() {
    for (cap, on, off) in experiment_cases() {
        let mut c = Capabilities::new();
        c.set_experiments(loaded(on));
        assert!(c.has(cap), "{cap:?} on");
        c.set_experiments(loaded(off));
        assert!(!c.has(cap), "{cap:?} off");
        // An answer without the assignment turns it off, whatever the others say.
        c.set_experiments(loaded(on));
        c.set_experiments(loaded(Assignments::default()));
        assert!(!c.has(cap), "{cap:?} missing");
    }
    for cap in Cap::ALL.into_iter().filter(|c| c.is_experiment()) {
        assert!(
            experiment_cases().iter().any(|(c, _, _)| *c == cap),
            "{cap:?}"
        );
    }
}

#[test]
fn before_the_first_answer_only_threads_are_assumed_on_and_without_the_route_none() {
    let mut c = Capabilities::new();
    for cap in Cap::ALL.into_iter().filter(|c| c.is_experiment()) {
        assert_eq!(c.has(cap), cap == Cap::ChannelThreads, "pending {cap:?}");
    }
    assert!(!c.set_experiments(ExperimentState::Pending), "no change");
    assert!(c.set_experiments(ExperimentState::Unavailable));
    for cap in Cap::ALL.into_iter().filter(|c| c.is_experiment()) {
        assert!(!c.has(cap), "unavailable {cap:?}");
    }
}

#[test]
fn the_peer_to_peer_limit_comes_with_the_assignment() {
    let mut c = Capabilities::new();
    assert_eq!(c.voice_p2p_max_participants(), None);
    c.set_experiments(loaded(Assignments {
        voice_p2p: Some(VoiceP2p {
            enabled: true,
            max_participants: Some(6),
        }),
        ..Assignments::default()
    }));
    assert_eq!(c.voice_p2p_max_participants(), Some(6));
    c.set_experiments(loaded(Assignments {
        voice_p2p: Some(VoiceP2p {
            enabled: false,
            max_participants: Some(6),
        }),
        ..Assignments::default()
    }));
    assert_eq!(c.voice_p2p_max_participants(), None);
}

#[test]
fn the_domain_move_needs_the_switch_and_the_assignment() {
    let on = json!({"domain_migration": {"enabled": true}});
    let off = json!({"domain_migration": {"enabled": false}});
    let assigned = loaded(Assignments {
        domain_migration: Some(true),
        ..Assignments::default()
    });
    let mut c = caps_of(on.clone());
    assert!(!c.domain_moved(), "the switch alone is not enough");
    c.set_experiments(assigned);
    assert!(c.domain_moved());
    c.set_discovery(&discovery(off));
    assert!(!c.domain_moved(), "the assignment alone is not enough");
    c.set_discovery(&discovery(on));
    c.set_experiments(loaded(Assignments {
        domain_migration: Some(false),
        ..Assignments::default()
    }));
    assert!(!c.domain_moved());
}

#[test]
fn the_instance_notice_is_live_between_its_times() {
    let c = caps_of(json!({"notice": {
        "id": "n1", "message": "Maintenance", "url": null,
        "starts_at": "2026-11-20T10:00:00.000Z", "ends_at": "2026-11-20T12:00:00.000Z"
    }}));
    let n = c.notice().unwrap();
    let start = n.starts_at_ms.unwrap();
    assert!(c.live_notice(start - 1).is_none());
    assert_eq!(c.live_notice(start).unwrap().id, "n1");
    assert!(c.live_notice(n.ends_at_ms.unwrap()).is_none());
    // The administrator removes it: the next document has none.
    let mut c = c;
    assert!(c.set_discovery(&discovery(json!({"notice": null}))));
    assert!(c.notice().is_none());
}

fn me(extra: Value) -> inlet_protocol::CurrentUser {
    let mut user = json!({"id": "10", "username": "me", "discriminator": "0001",
                          "global_name": null, "avatar": null, "flags": 0});
    for (k, v) in extra.as_object().unwrap() {
        user[k] = v.clone();
    }
    serde_json::from_value(user).unwrap()
}

#[test]
fn the_email_banner_follows_the_instance_and_the_account() {
    let needs = me(json!({"verification_need": "confirm_email"}));
    let mut c = caps_of(json!({"features": {"emails_enabled": true}}));
    assert!(c.notices(&needs).confirm_email);
    // Emails off on the instance: nothing to confirm.
    c.set_discovery(&discovery(json!({"features": {"emails_enabled": false}})));
    assert!(!c.notices(&needs).confirm_email);
    c.set_discovery(&discovery(json!({"features": {"emails_enabled": true}})));
    assert!(
        !c.notices(&me(json!({"verification_need": null})))
            .confirm_email
    );
    assert!(
        !c.notices(&me(json!({"verification_need": "add_email"})))
            .confirm_email
    );
    // A bounced address is its own banner, not the confirm one.
    let bounced = me(json!({"verification_need": "confirm_email", "email_bounced": true}));
    let n = c.notices(&bounced);
    assert!(n.email_bounced && !n.confirm_email);
}

#[test]
fn the_limited_account_banner_follows_the_account() {
    let c = Capabilities::new();
    assert!(c.notices(&me(json!({"account_limited": true}))).limited);
    assert!(!c.notices(&me(json!({"account_limited": false}))).limited);
    assert!(!c.notices(&me(json!({}))).limited);
}

#[test]
fn updated_terms_are_asked_only_when_the_account_is_behind_and_the_instance_is_not_self_hosted() {
    let day = 86_400_000;
    let mut c = caps_of(json!({"features": {"self_hosted": false}}));
    // Fluxer's build has no policy dates today: nothing to take in.
    assert_eq!(c.notices(&me(json!({}))).terms, TermsUpdate::None);
    c.set_policy_dates(PolicyDates {
        terms_updated_ms: Some(10 * day),
        privacy_updated_ms: None,
    });
    assert_eq!(c.notices(&me(json!({}))).terms, TermsUpdate::Terms);
    // Agreed before the change: behind. After it: fine.
    let old = me(json!({"terms_agreed_at": "1970-01-05T00:00:00.000Z"}));
    assert_eq!(c.notices(&old).terms, TermsUpdate::Terms);
    let new = me(json!({"terms_agreed_at": "1970-01-20T00:00:00.000Z"}));
    assert_eq!(c.notices(&new).terms, TermsUpdate::None);
    c.set_policy_dates(PolicyDates {
        terms_updated_ms: Some(10 * day),
        privacy_updated_ms: Some(12 * day),
    });
    assert_eq!(c.notices(&new).terms, TermsUpdate::Privacy);
    assert_eq!(c.notices(&old).terms, TermsUpdate::Both);
    // A self-hosted instance never asks.
    c.set_discovery(&discovery(json!({"features": {"self_hosted": true}})));
    assert_eq!(c.notices(&old).terms, TermsUpdate::None);
}

#[test]
fn a_servers_features_count_as_the_instance_lets_them() {
    let feats = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut c = caps_of(json!({"features": {"discovery_enabled": true}}));
    let visits = feats(&["VISITS_ENABLED"]);
    // Visits need the assignment.
    assert!(!c.guild_has(&visits, GuildFeature::VisitsEnabled));
    c.set_experiments(loaded(Assignments {
        guild_visits: Some(true),
        ..Assignments::default()
    }));
    assert!(c.guild_has(&visits, GuildFeature::VisitsEnabled));
    // And not with invites disabled.
    assert!(!c.guild_has(
        &feats(&["VISITS_ENABLED", "INVITES_DISABLED"]),
        GuildFeature::VisitsEnabled
    ));
    assert!(c.guild_has(&feats(&["INVITES_DISABLED"]), GuildFeature::InvitesDisabled));
    // Explore listing needs the directory.
    let listed = feats(&["DISCOVERABLE"]);
    assert!(c.guild_has(&listed, GuildFeature::Discoverable));
    c.set_discovery(&discovery(
        json!({"features": {"discovery_enabled": false}}),
    ));
    assert!(!c.guild_has(&listed, GuildFeature::Discoverable));
    assert!(!c.guild_has(&[], GuildFeature::Verified));
    assert!(c.guild_has(&feats(&["VERIFIED"]), GuildFeature::Verified));
}

fn ready_event() -> inlet_protocol::Dispatch {
    common::d(
        "READY",
        common::ready_json(
            common::guild_ready(
                common::default_roles(),
                common::default_channels(),
                &[common::MOD_ROLE],
            ),
            json!([]),
            json!([]),
        ),
    )
}

#[test]
fn the_account_state_keeps_its_capabilities_across_a_new_ready_and_reports_changes() {
    let mut state = common::state();
    assert!(!state.capabilities().has_discovery());
    let c = state.set_discovery(&discovery(json!({"features": {"voice_enabled": true}})));
    assert!(c.capabilities);
    assert!(state.capabilities().has(Cap::Voice));
    assert!(
        !state
            .set_discovery(&discovery(json!({"features": {"voice_enabled": true}})))
            .capabilities
    );
    let c = state.set_experiments(loaded(Assignments {
        guild_visits: Some(true),
        ..Assignments::default()
    }));
    assert!(c.capabilities && state.capabilities().has(Cap::GuildVisits));

    // A new READY rebuilds the state; what the instance said stays.
    let changes = state.apply(&ready_event());
    assert!(changes.reset);
    assert!(state.capabilities().has(Cap::Voice) && state.capabilities().has(Cap::GuildVisits));

    // The account's own USER_UPDATE changes its banners.
    assert!(!state.notices().limited);
    let changes = state.apply(&common::d(
        "USER_UPDATE",
        json!({"id": common::ME, "username": "me", "account_limited": true}),
    ));
    assert!(changes.capabilities);
    assert!(state.notices().limited);
    // Server features go through the state.
    assert!(!state.guild_has(common::g(), GuildFeature::Verified));
}

#[test]
fn an_account_state_built_alone_has_the_defaults() {
    let state: AccountState = common::state();
    assert!(state.capabilities().has(Cap::ChannelThreads));
    assert!(state.capabilities().has(Cap::DirectMessages));
    assert!(!state.capabilities().has(Cap::Voice));
    assert_eq!(state.notices(), Default::default());
}
