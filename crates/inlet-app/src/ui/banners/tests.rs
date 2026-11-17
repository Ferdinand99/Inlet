// SPDX-License-Identifier: AGPL-3.0-or-later
// The whole file is test code (`#[cfg(test)] mod tests;` in mod.rs); the catalogue check
// of visible text stops reading a file at the first such marker.
//! The banners: Fluxer's choice of which ones show, and every flag that feeds them
//! switched on and off through the real view with an account state.

use gpui::{div, prelude::*, Context, Render, TestAppContext, Window};
use inlet_protocol as p;
use inlet_state::{AccountState, Cap, ExperimentState, PolicyDates};
use serde_json::{json, Value};

use super::*;
use crate::backend::instance::status_page::{Incident, Maintenance, MaintenanceState, Status};
use crate::backend::BackendHandle;

#[test]
fn of_the_banners_that_apply_one_firm_and_one_dismissible_show_in_order() {
    use Banner::*;
    // The firm ones: the lowest priority number wins, the others wait.
    assert_eq!(select(&[Email, Terms]), vec![Terms]);
    assert_eq!(select(&[AccountLimited, Email]), vec![AccountLimited]);
    assert_eq!(select(&[Email, Connection, Terms]), vec![Connection]);
    // The dismissible ones: one of them, the lowest.
    assert_eq!(select(&[InstanceNotice, Maintenance]), vec![InstanceNotice]);
    // One of each kind, in order of priority.
    assert_eq!(
        select(&[InstanceNotice, Email]),
        vec![Email, InstanceNotice]
    );
    assert_eq!(select(&[]), Vec::<Banner>::new());
    assert_eq!(select(&[Email, Email]), vec![Email]);
}

#[test]
fn texts_from_the_instance_are_cleaned_and_cut() {
    assert_eq!(tidy("Down\nfor a bit\u{202E}!", 100), "Down for a bit!");
    assert_eq!(tidy("a\u{200B}b", 100), "ab");
    assert_eq!(
        tidy(&"x".repeat(50), 10),
        format!("{}\u{2026}", "x".repeat(10))
    );
    assert_eq!(tidy("  hi  ", 10), "hi");
}

#[test]
fn a_maintenance_length_is_said_in_the_largest_even_unit() {
    assert_eq!(duration_text(1), "1 minute");
    assert_eq!(duration_text(45), "45 minutes");
    assert_eq!(duration_text(60), "1 hour");
    assert_eq!(duration_text(120), "2 hours");
    assert_eq!(duration_text(90), "90 minutes");
    assert_eq!(duration_text(1440), "1 day");
    assert_eq!(duration_text(0), "1 minute");
}

#[test]
fn dismissals_keep_the_newest_and_are_looked_up_by_id() {
    let mut prefs = Prefs::default();
    for i in 0..(MAX_DISMISSED + 10) {
        prefs.dismiss(format!("notice|https://a.example|{i}"));
    }
    assert_eq!(prefs.dismissed.len(), MAX_DISMISSED);
    assert!(!prefs.is_dismissed("notice|https://a.example|0"));
    assert!(prefs.is_dismissed(&format!("notice|https://a.example|{}", MAX_DISMISSED + 9)));
    // The same one again moves to the end instead of twice into the list.
    prefs.dismiss(format!("notice|https://a.example|{}", MAX_DISMISSED));
    assert_eq!(prefs.dismissed.len(), MAX_DISMISSED);
    let text = serde_json::to_string(&prefs).unwrap();
    assert_eq!(serde_json::from_str::<Prefs>(&text).unwrap(), prefs);
    // A file of another shape gives the defaults for what is missing.
    assert_eq!(
        serde_json::from_str::<Prefs>("{}").unwrap(),
        Prefs::default()
    );
}

struct Probe;

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

const ME: &str = "41";

fn key(origin: &str) -> AccountKey {
    AccountKey::new(origin, ME).expect("an account key")
}

fn ready(user_extra: Value) -> p::Ready {
    let mut user = json!({"id": ME, "username": "me", "global_name": "Mia"});
    for (k, v) in user_extra.as_object().unwrap() {
        user[k] = v.clone();
    }
    serde_json::from_value(json!({
        "session_id": "s", "user": user, "guilds": [], "users": [],
        "private_channels": [], "read_states": [], "user_guild_settings": []
    }))
    .unwrap()
}

fn discovery(patch: Value) -> Box<p::Discovery> {
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
            _ => doc[k] = v.clone(),
        }
    }
    Box::new(serde_json::from_value(doc).unwrap())
}

/// An `Inlet` on the main screen with an account, no backend thread, on `origin`.
fn with_inlet(
    cx: &mut TestAppContext,
    origin: &str,
    user: Value,
    body: impl FnOnce(&mut Inlet, &mut Context<Inlet>),
) {
    let origin = origin.to_string();
    let window = cx.add_window(|_, _| Probe);
    let inlet = window
        .update(cx, |_, window, cx| {
            cx.new(|cx| Inlet::new_with(window, cx, |_| BackendHandle::detached()))
        })
        .unwrap();
    window
        .update(cx, |_, _, cx| {
            inlet.update(cx, |this, cx| {
                this.screen = Screen::Main;
                this.switcher.active = Some(key(&origin));
                this.account = Some(cx.new(|_| AccountState::from_ready_at(&ready(user), 1)));
                body(this, cx);
            })
        })
        .unwrap();
}

fn instance_says(this: &mut Inlet, origin: &str, ev: Ev, cx: &mut Context<Inlet>) {
    this.on_instance_event(Some(key(origin)), ev, cx);
}

const HOME: &str = "https://fluxer.opland.net";

#[gpui::test]
fn the_email_banner_follows_the_instance_flag_and_the_account(cx: &mut TestAppContext) {
    with_inlet(
        cx,
        HOME,
        json!({"verification_need": "confirm_email"}),
        |this, cx| {
            // The instance's emails are off: nothing to confirm.
            instance_says(
                this,
                HOME,
                Ev::Discovery(discovery(json!({"features": {"emails_enabled": false}}))),
                cx,
            );
            assert_eq!(this.visible_banners(cx), vec![]);
            // Switched on.
            instance_says(
                this,
                HOME,
                Ev::Discovery(discovery(json!({"features": {"emails_enabled": true}}))),
                cx,
            );
            assert_eq!(this.visible_banners(cx), vec![Banner::Email]);
            // Switched off again.
            instance_says(
                this,
                HOME,
                Ev::Discovery(discovery(json!({"features": {"emails_enabled": false}}))),
                cx,
            );
            assert_eq!(this.visible_banners(cx), vec![]);
        },
    );
    // An account with nothing to confirm has no banner, whatever the instance says.
    with_inlet(cx, HOME, json!({}), |this, cx| {
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(json!({"features": {"emails_enabled": true}}))),
            cx,
        );
        assert_eq!(this.visible_banners(cx), vec![]);
    });
}

#[gpui::test]
fn the_limited_account_banner_follows_the_account(cx: &mut TestAppContext) {
    with_inlet(cx, HOME, json!({"account_limited": true}), |this, cx| {
        assert_eq!(this.visible_banners(cx), vec![Banner::AccountLimited]);
        // The account's USER_UPDATE lifts the limit: the banner goes.
        let update = p::Dispatch::parse("USER_UPDATE", json!({"id": ME, "username": "me"}));
        this.account
            .clone()
            .unwrap()
            .update(cx, |s, _| s.apply(&update));
        assert_eq!(this.visible_banners(cx), vec![]);
    });
}

#[gpui::test]
fn the_terms_banner_shows_when_the_account_is_behind(cx: &mut TestAppContext) {
    with_inlet(cx, HOME, json!({}), |this, cx| {
        assert_eq!(this.visible_banners(cx), vec![]);
        let account = this.account.clone().unwrap();
        account.update(cx, |s, _| {
            s.set_policy_dates(PolicyDates {
                terms_updated_ms: Some(86_400_000),
                privacy_updated_ms: None,
            })
        });
        assert_eq!(this.visible_banners(cx), vec![Banner::Terms]);
        // A self-hosted instance never asks.
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(json!({"features": {"self_hosted": true}}))),
            cx,
        );
        assert_eq!(this.visible_banners(cx), vec![]);
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(json!({"features": {"self_hosted": false}}))),
            cx,
        );
        assert_eq!(this.visible_banners(cx), vec![Banner::Terms]);
        // The accepted terms (the account as the instance returns it) take it away.
        let accepted = p::Dispatch::parse(
            "USER_UPDATE",
            json!({"id": ME, "username": "me", "terms_agreed_at": "2026-11-16T12:00:00.000Z"}),
        );
        account.update(cx, |s, _| s.apply(&accepted));
        assert_eq!(this.visible_banners(cx), vec![]);
    });
}

fn notice(id: &str, starts: Value, ends: Value) -> Value {
    json!({"notice": {"id": id, "message": "Planned downtime", "url": null,
                      "starts_at": starts, "ends_at": ends}})
}

#[gpui::test]
fn the_instance_notice_shows_while_live_and_until_it_is_dismissed(cx: &mut TestAppContext) {
    with_inlet(cx, HOME, json!({}), |this, cx| {
        // Live with no times.
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(notice("n1", Value::Null, Value::Null))),
            cx,
        );
        assert_eq!(this.visible_banners(cx), vec![Banner::InstanceNotice]);
        // Not started yet, and already over: nothing.
        let future = "2099-01-01T00:00:00.000Z";
        let past = "2000-01-01T00:00:00.000Z";
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(notice("n2", json!(future), Value::Null))),
            cx,
        );
        assert_eq!(this.visible_banners(cx), vec![]);
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(notice("n3", Value::Null, json!(past)))),
            cx,
        );
        assert_eq!(this.visible_banners(cx), vec![]);
        // The administrator removes it.
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(json!({"notice": null}))),
            cx,
        );
        assert_eq!(this.visible_banners(cx), vec![]);
        // Dismissed: gone, and a new text (a new ID) shows again.
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(notice("n4", Value::Null, Value::Null))),
            cx,
        );
        assert_eq!(this.visible_banners(cx), vec![Banner::InstanceNotice]);
        this.dismiss_banner(Banner::InstanceNotice, cx);
        assert_eq!(this.visible_banners(cx), vec![]);
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(notice("n5", Value::Null, Value::Null))),
            cx,
        );
        assert_eq!(this.visible_banners(cx), vec![Banner::InstanceNotice]);
    });
}

fn maintenance(state: MaintenanceState) -> Status {
    Status {
        maintenance: Some(Maintenance {
            id: "m1".into(),
            name: "Database upgrade".into(),
            state,
            start_ms: Some(now_ms() + 3_600_000),
            duration_minutes: 90,
            url: Some("https://status.example.org/m1".into()),
        }),
        incident: None,
    }
}

#[gpui::test]
fn maintenance_shows_per_stage_and_is_dismissed_per_stage(cx: &mut TestAppContext) {
    with_inlet(cx, HOME, json!({}), |this, cx| {
        instance_says(
            this,
            HOME,
            Ev::StatusPage(maintenance(MaintenanceState::Scheduled)),
            cx,
        );
        assert_eq!(this.visible_banners(cx), vec![Banner::Maintenance]);
        this.dismiss_banner(Banner::Maintenance, cx);
        assert_eq!(this.visible_banners(cx), vec![]);
        // It starts: a new stage, shown again.
        instance_says(
            this,
            HOME,
            Ev::StatusPage(maintenance(MaintenanceState::InProgress)),
            cx,
        );
        assert_eq!(this.visible_banners(cx), vec![Banner::Maintenance]);
        // It ends.
        instance_says(this, HOME, Ev::StatusPage(Status::default()), cx);
        assert_eq!(this.visible_banners(cx), vec![]);
    });
}

#[gpui::test]
fn a_notice_and_the_email_banner_show_together_but_a_lost_connection_comes_first(
    cx: &mut TestAppContext,
) {
    with_inlet(
        cx,
        HOME,
        json!({"verification_need": "confirm_email"}),
        |this, cx| {
            instance_says(
                this,
                HOME,
                Ev::Discovery(discovery({
                    let mut d = notice("n1", Value::Null, Value::Null);
                    d["features"] = json!({"emails_enabled": true});
                    d
                })),
                cx,
            );
            // One firm and one dismissible, in order of priority.
            assert_eq!(
                this.visible_banners(cx),
                vec![Banner::Email, Banner::InstanceNotice]
            );
            // The gateway stopped: the firm slot goes to the connection, the notice stays.
            this.chat.gateway_down = Some("closed".into());
            assert_eq!(
                this.visible_banners(cx),
                vec![Banner::Connection, Banner::InstanceNotice]
            );
            this.chat.gateway_down = None;
            assert_eq!(
                this.visible_banners(cx),
                vec![Banner::Email, Banner::InstanceNotice]
            );
        },
    );
}

#[gpui::test]
fn the_connection_banner_waits_three_seconds(cx: &mut TestAppContext) {
    with_inlet(cx, HOME, json!({}), |this, cx| {
        assert_eq!(this.visible_banners(cx), vec![]);
        // Reconnecting, just now: not yet.
        this.chat.status = t!("status-gateway-reconnecting-in", seconds = 4u64);
        assert_eq!(this.visible_banners(cx), vec![]);
        // Still down after the grace: the banner shows, whatever the counting text says.
        let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(5);
        this.banners.connection_seen = Some((this.chat.status.clone(), long_ago));
        this.chat.status = t!("status-connecting");
        assert_eq!(this.visible_banners(cx), vec![Banner::Connection]);
        // Connected again: gone, and the next outage starts its own clock.
        this.chat.status = t!("status-connected");
        assert_eq!(this.visible_banners(cx), vec![]);
        this.chat.status = t!("status-disconnected");
        assert_eq!(this.visible_banners(cx), vec![]);
    });
}

#[gpui::test]
fn an_incident_explains_the_lost_connection(cx: &mut TestAppContext) {
    with_inlet(cx, HOME, json!({}), |this, cx| {
        let incident = Status {
            maintenance: None,
            incident: Some(Incident {
                id: "i1".into(),
                name: "Gateway trouble".into(),
                url: Some("https://status.example.org/i1".into()),
            }),
        };
        instance_says(this, HOME, Ev::StatusPage(incident), cx);
        // Not down: an incident alone is no banner.
        assert_eq!(this.visible_banners(cx), vec![]);
        this.chat.gateway_down = Some("closed".into());
        assert_eq!(this.visible_banners(cx), vec![Banner::Connection]);
    });
}

#[gpui::test]
fn the_domain_move_shows_no_banner_as_fluxers_own_desktop_app_has_none(cx: &mut TestAppContext) {
    let official = "https://fluxer.app";
    with_inlet(cx, official, json!({}), |this, cx| {
        instance_says(
            this,
            official,
            Ev::Discovery(discovery(json!({"domain_migration": {"enabled": true}}))),
            cx,
        );
        instance_says(
            this,
            official,
            Ev::Experiments(ExperimentState::Loaded(inlet_protocol::Experiments {
                assignments: inlet_protocol::Assignments {
                    domain_migration: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            })),
            cx,
        );
        // The capability is on, and nothing is drawn for it.
        assert!(this
            .account
            .clone()
            .unwrap()
            .read(cx)
            .capabilities()
            .domain_moved());
        assert_eq!(this.visible_banners(cx), vec![]);
    });
}

#[gpui::test]
fn no_banner_shows_off_the_main_screen_or_without_an_account(cx: &mut TestAppContext) {
    with_inlet(cx, HOME, json!({"account_limited": true}), |this, cx| {
        assert_eq!(this.visible_banners(cx), vec![Banner::AccountLimited]);
        this.screen = Screen::SignIn;
        assert_eq!(this.visible_banners(cx), vec![]);
        this.screen = Screen::Main;
        this.account = None;
        assert_eq!(this.visible_banners(cx), vec![]);
    });
}

#[gpui::test]
fn an_answer_that_comes_before_the_account_state_is_given_to_it_later(cx: &mut TestAppContext) {
    with_inlet(cx, HOME, json!({}), |this, cx| {
        let first = this.account.take();
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(json!({"features": {"voice_enabled": true}}))),
            cx,
        );
        // READY has not come: there is no state to hold it, the answer waits.
        assert!(!this.has_cap(cx, Cap::Voice));
        // The state is made (the first READY); the next sync gives it the answer.
        this.account = Some(cx.new(|_| AccountState::from_ready_at(&ready(json!({})), 1)));
        this.sync_instance_info(cx);
        assert!(this.has_cap(cx, Cap::Voice));
        // A background account's answer is kept for when it becomes the active one.
        let other = AccountKey::new("https://other.example", "77").unwrap();
        this.on_instance_event(
            Some(other.clone()),
            Ev::Discovery(discovery(json!({"features": {"stripe_enabled": true}}))),
            cx,
        );
        assert!(!this.has_cap(cx, Cap::Stripe));
        this.switcher.active = Some(other);
        this.account = Some(cx.new(|_| AccountState::from_ready_at(&ready(json!({})), 1)));
        this.sync_instance_info(cx);
        assert!(this.has_cap(cx, Cap::Stripe) && !this.has_cap(cx, Cap::Voice));
        drop(first);
    });
}

/// A READY whose guild "100" has threads on (a `threads` list) or not.
fn ready_with_guild(threads: bool) -> p::Ready {
    let mut guild = json!({
        "id": "100", "properties": {"id": "100", "name": "Guild", "owner_id": "1"},
        "channels": [], "roles": [], "members": [], "member_count": 1
    });
    if threads {
        guild["threads"] = json!([]);
    }
    serde_json::from_value(json!({
        "session_id": "s", "user": {"id": ME, "username": "me"}, "guilds": [guild],
        "users": [], "private_channels": [], "read_states": [], "user_guild_settings": []
    }))
    .unwrap()
}

#[gpui::test]
fn the_thread_button_follows_the_guilds_own_threads_list_and_not_the_experiment(
    cx: &mut TestAppContext,
) {
    with_inlet(cx, HOME, json!({}), |this, cx| {
        this.selected_guild = Some("100".into());
        this.selected_channel = Some("200".into());
        this.channels = vec![crate::model::Channel {
            id: "200".into(),
            name: "general".into(),
            kind: crate::model::ChannelKind::Text,
            parent_id: None,
            position: 0,
            topic: None,
            slowmode_secs: 0,
        }];
        let button = |this: &Inlet, cx: &mut Context<Inlet>| {
            crate::ui::threads::header_button(this, cx).is_some()
        };
        // The guild arrived without a `threads` list: the instance has threads off for it.
        this.account = Some(cx.new(|_| AccountState::from_ready_at(&ready_with_guild(false), 1)));
        assert!(!button(this, cx));
        // With the list the button shows, whatever `/experiments` says or does not say:
        // before the first answer, with the threads assignment gone, or without the route.
        this.account = Some(cx.new(|_| AccountState::from_ready_at(&ready_with_guild(true), 1)));
        assert!(button(this, cx));
        for state in [
            ExperimentState::Loaded(inlet_protocol::Experiments::default()),
            ExperimentState::Unavailable,
            ExperimentState::Pending,
        ] {
            instance_says(this, HOME, Ev::Experiments(state), cx);
            assert!(button(this, cx), "{state:?}");
        }
        // A guild that is not on screen, or a direct message, has none.
        this.selected_guild = Some("999".into());
        assert!(!button(this, cx));
        this.selected_guild = None;
        assert!(!button(this, cx));
    });
}

#[gpui::test]
fn the_about_view_names_the_instance_by_its_branding(cx: &mut TestAppContext) {
    with_inlet(cx, HOME, json!({}), |this, cx| {
        // No branding: the address names it.
        let line = this.about_instance(cx).unwrap();
        assert_eq!(
            (line.name.as_str(), line.host.as_str()),
            ("fluxer.opland.net", "fluxer.opland.net")
        );
        assert!(line.status_page.is_none());
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(json!({"app_public": {"branding": {
                "product_name": "Opland
Social", "status_page_url": "https://status.opland.net"}}}))),
            cx,
        );
        let line = this.about_instance(cx).unwrap();
        assert_eq!(line.name, "Opland Social");
        assert_eq!(
            line.status_page.as_deref(),
            Some("https://status.opland.net")
        );
        // A status page address that is not https to a host name is not offered.
        instance_says(
            this,
            HOME,
            Ev::Discovery(discovery(json!({"app_public": {"branding": {
                "product_name": "Opland Social", "status_page_url": "http://192.168.1.2/status"}}}))),
            cx,
        );
        assert!(this.about_instance(cx).unwrap().status_page.is_none());
    });
}
