// SPDX-License-Identifier: AGPL-3.0-or-later
//! Demo entry for the embedded web view: with `INLET_DEMO_WEB` set, the demo opens the
//! web view window on a small local test page. Its links show each rule: an allowed page,
//! another host (blocked, the window closes), the end of a flow (intercepted, the window
//! closes), a download and a new window (both blocked), a `data:` link. What the helper
//! reports is logged as warnings.
//!
//! The helper needs the `test-loopback` build, which accepts plain `http` on loopback.
//! Build it next to the demo and name it:
//!
//! ```text
//! cargo build --profile ci -p inlet-web --features test-loopback
//! INLET_DEMO=1 INLET_DEMO_WEB=1 INLET_WEB_HELPER=target/ci/inlet-web target/ci/inlet
//! ```

use gpui::{App, WindowHandle};
use inlet_web::testkit::PageServer;

use crate::app::Inlet;
use crate::web::{self, WebFlow};

/// Opens the test page when `INLET_DEMO_WEB` is set.
pub fn open_if_requested(window: WindowHandle<Inlet>, cx: &mut App) {
    if std::env::var_os("INLET_DEMO_WEB").is_none() {
        return;
    }
    let Ok(parent) = window.update(cx, |_, window, _| web::parent_of(window)) else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name("web-demo".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a tokio runtime");
            runtime.block_on(run(parent));
        });
    if let Err(err) = spawned {
        log::warn!("web demo: {err}");
    }
}

async fn run(parent: inlet_web::protocol::Parent) {
    let server = match PageServer::plain() {
        Ok(server) => server,
        Err(err) => {
            log::warn!("web demo: no test server: {err}");
            return;
        }
    };
    let host = server.host_ip();
    let flow = WebFlow {
        url: format!("http://{host}/"),
        allow: vec![host.clone()],
        intercept: vec![format!("http://{host}/cb")],
        frames: vec![],
        title: "Web view demo".into(),
        size: (720, 520),
    };
    match web::open(flow, Some(parent)).await {
        Ok(mut session) => {
            while let Some(event) = session.next_event().await {
                log::warn!("web demo: {event:?}");
            }
            session.finished().await;
        }
        Err(err) => log::warn!("web demo: {err}"),
    }
}
