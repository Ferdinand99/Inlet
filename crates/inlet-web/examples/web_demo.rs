// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opens the helper on a local test page and prints what it reports. Click the links on
//! the page to see each rule at work.
//!
//! ```text
//! cargo build -p inlet-web --features test-loopback
//! cargo run -p inlet-web --features test-loopback --example web_demo
//! ```
//!
//! The helper is the `inlet-web` program in the same `target` folder.

use inlet_web::client::{helper_beside, ClientOptions, Session};
use inlet_web::protocol::{OpenRequest, Size};
use inlet_web::testkit::PageServer;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let server = PageServer::plain().expect("start the test server");
    let host = server.host_ip();
    let request = OpenRequest {
        url: format!("http://{host}/"),
        allow: vec![host.clone()],
        intercept: vec![format!("http://{host}/cb")],
        frames: vec![],
        title: "Inlet web view demo".into(),
        size: Size {
            width: 720,
            height: 520,
        },
        parent: None,
    };
    let exe = std::env::current_exe().expect("the example's path");
    // target/debug/examples/web_demo -> target/debug/inlet-web
    let helper = helper_beside(&exe.parent().and_then(|p| p.parent()).unwrap().join("x"));
    let options = ClientOptions::at(helper);
    let mut session = match Session::open(&options, request).await {
        Ok(session) => session,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };
    while let Some(event) = session.next_event().await {
        println!("{event:?}");
    }
    session.finished().await;
}
