// SPDX-License-Identifier: AGPL-3.0-or-later
//! The embedded web view of Inlet.
//!
//! gpui has no web view, and the system's engines (WebView2, WKWebView, WebKitGTK) want
//! an event loop of their own, so a web page runs in a separate helper program,
//! `inlet-web`, that Inlet starts only when a flow needs one (a sign-in at an identity
//! provider, a payment, an OAuth consent) and that ends with its window. It cannot read
//! Inlet's keyring or settings: it is another process with a nearly empty environment,
//! and it is handed a URL and the rules for it, nothing else.
//!
//! - [`protocol`]: the messages, JSON lines on the helper's standard input and output;
//! - [`policy`]: which URLs the window may load, and which end the flow;
//! - [`client`] (feature `client`): starts the helper and turns its output into a
//!   stream of events;
//! - [`helper`] (feature `helper`): the helper itself, built on wry and tao.

pub mod datadir;
pub mod policy;
pub mod protocol;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "helper")]
pub mod helper;
#[cfg(feature = "loopback")]
pub mod testkit;

// Plain http on loopback and trusting any certificate are for tests and the demo. They
// must never ship.
#[cfg(all(feature = "loopback", not(debug_assertions)))]
compile_error!(
    "the `loopback` and `test-loopback` features are for tests and the demo and must not be in a release build"
);
