// SPDX-License-Identifier: AGPL-3.0-or-later
//! Demo data for the banners: what the instance says (a notice, flags), what the status
//! page says, and the demo account's own flags, per scene. The scenes that show nothing
//! of this send nothing, so the other screenshots are unchanged.

use inlet_protocol as p;
use inlet_state::ExperimentState;
use serde_json::json;

use super::scenes::Scene;
use crate::backend::instance::status_page::{Incident, Maintenance, MaintenanceState, Status};
use crate::backend::instance::Ev;
use crate::backend::{messages as msgs, Ui};

/// The demo instance's discovery document, with its notice.
fn discovery(notice: bool) -> p::Discovery {
    let mut doc = json!({
        "api_code_version": 1,
        "endpoints": {"api_public": "https://chat.example.org/api", "gateway": "wss://chat.example.org/gateway"},
        "features": {"emails_enabled": true, "voice_enabled": true},
        "app_public": {"branding": {"product_name": "Hedgerow Chat"}}
    });
    if notice {
        doc["notice"] = json!({
            "id": "demo-notice",
            "message": "Planned maintenance on Saturday at 22:00 UTC. The service may be unavailable for up to an hour.",
            "url": "https://status.example.org/maintenance",
            "starts_at": null, "ends_at": null
        });
    }
    serde_json::from_value(doc).expect("the demo discovery document")
}

/// The demo account with a flag of its own set.
fn user_with(ready: &p::Ready, flag: &str, value: serde_json::Value) -> p::Dispatch {
    let mut me = ready.user.clone();
    me.user.extra.insert(flag.into(), value);
    p::Dispatch::UserUpdate(Box::new(me))
}

/// Whether a scene opens the updated-terms window over the main screen.
pub fn terms_scene() -> bool {
    super::scenes::current() == Scene::Terms
}

/// Sends what `scene` shows of the instance, after the demo's READY.
pub fn start(events: &Ui, scene: Scene, ready: &p::Ready) {
    let send = |ev: Ev| {
        let _ = events.unbounded_send(ev);
    };
    let update = |dispatch: p::Dispatch| {
        let _ = events.unbounded_send(msgs::Ev::Dispatch {
            dispatch: Box::new(dispatch),
            rendered: Vec::new(),
        });
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
    match scene {
        // The address is not confirmed, and the administrator has a notice.
        Scene::Banners => {
            send(Ev::Discovery(Box::new(discovery(true))));
            update(user_with(
                ready,
                "verification_need",
                json!("confirm_email"),
            ));
        }
        // Messaging is paused, and maintenance is planned.
        Scene::BannersMaintenance => {
            send(Ev::Discovery(Box::new(discovery(false))));
            update(user_with(ready, "account_limited", json!(true)));
            send(Ev::StatusPage(Status {
                maintenance: Some(Maintenance {
                    id: "demo-maintenance".into(),
                    name: "Database upgrade".into(),
                    state: MaintenanceState::Scheduled,
                    start_ms: Some(now + 3 * 3_600_000),
                    duration_minutes: 90,
                    url: Some("https://status.example.org/maintenance".into()),
                }),
                incident: None,
            }));
        }
        // The connection is down, and the status page names an incident.
        Scene::BannersConnection => {
            send(Ev::Discovery(Box::new(discovery(true))));
            send(Ev::StatusPage(Status {
                maintenance: None,
                incident: Some(Incident {
                    id: "demo-incident".into(),
                    name: "Gateway trouble: some people cannot connect".into(),
                    url: Some("https://status.example.org/incident".into()),
                }),
            }));
            let _ = events.unbounded_send(crate::backend::accounts::Ev::GatewayDown(
                "The connection was closed.".into(),
            ));
        }
        // The updated-terms window (the banner and the window are opened by the view).
        Scene::Terms => {
            send(Ev::Discovery(Box::new(discovery(false))));
            send(Ev::Experiments(ExperimentState::Pending));
        }
        _ => {}
    }
}
