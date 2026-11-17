// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `inlet-web` helper program: one window with one web page in it.
//!
//! Inlet starts it when a flow needs a real web page (a sign-in at an identity provider,
//! a payment, an OAuth consent) and the helper ends with the window. It reads one `open`
//! request on standard input, writes events on standard output (see [`crate::protocol`])
//! and keeps these rules, whatever the page does:
//!
//! - no script of its own goes into the page: nothing is injected and the page has no
//!   channel to the helper;
//! - no developer tools;
//! - private mode: the engine keeps nothing on disk (Linux and macOS), or, where it must
//!   have a folder (Windows, WebView2), a data folder that is new for this window and
//!   removed at its end;
//! - no new windows, no downloads, no permission (camera, microphone, location,
//!   notifications ...) and nothing dropped onto the window is opened;
//! - a navigation to a URL the rules do not allow is stopped, reported as `blocked`, and
//!   the window closes; a navigation to an `intercept` URL is stopped, reported as
//!   `intercepted`, and the window closes. The window may only go to `allow`; frames of
//!   the page may go to `allow` and `frames`, on every platform (see [`crate::gate`]);
//! - a page that asks for more than a few new windows or downloads ends the flow;
//! - HTTPS only (see [`crate::policy`]);
//! - the window is closed when Inlet closes the helper's input.
//!
//! The policy is checked for every navigation the engine reports and again for every
//! page load it reports, so a redirect the first check did not see is still caught.
//! Subresources (images, scripts, requests the page makes itself) are not navigations
//! and are not filtered: the page is as trusted as the hosts in `allow`.
//!
//! What this protects: the helper holds no secret of Inlet's (it is started with none),
//! and its own code never touches Inlet's keyring or settings. Web content is kept apart
//! from the rest of the user's session by the engine's own process sandbox: Chromium's
//! on Windows, the WebContent sandbox on macOS. WebKitGTK's sandbox is not switched on
//! yet on Linux (a follow-up), so there a renderer exploit would run with the user's
//! rights; that is why the helper is not given anything to find.

use std::cell::Cell;
use std::io::{BufRead, Read, Write};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event as LoopEvent, WindowEvent};
use tao::event_loop::{EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder};
use wry::{NewWindowResponse, PermissionResponse, WebContext, WebView, WebViewBuilder};

use crate::datadir::{self, DataDir};
use crate::gate::{Decision, Gate, Refusal};
use crate::policy::{Policy, PolicyOptions};
use crate::protocol::{
    decode_request, encode_event, ErrorKind, Event, OpenRequest, ProtocolError, Request,
    MAX_LINE_LEN,
};

/// Present only in a test build, so a release check can tell one by its bytes.
#[cfg(feature = "test-loopback")]
#[used]
static TEST_BUILD_MARKER: &str = "inlet-web test-loopback build";

/// How long the helper waits for the `open` request before it gives up.
const OPEN_WITHIN: Duration = Duration::from_secs(10);

/// What the event loop is told from other threads and from the web view's callbacks.
enum Wake {
    /// A request from Inlet.
    Request(Request),
    /// Something on the input is not a request.
    BadInput(ErrorKind),
    /// Inlet closed the input (it went away).
    InputClosed,
    /// No request came in time.
    NoRequest,
    /// The flow is over (intercepted or blocked): close the window.
    Finish,
}

/// Runs the helper and returns its exit code.
pub fn run() -> i32 {
    let mut args = std::env::args_os().skip(1);
    match args.next() {
        None => {}
        Some(arg) if arg == "--version" => {
            println!("inlet-web {}", env!("CARGO_PKG_VERSION"));
            return 0;
        }
        Some(_) => {
            // No argument carries anything: the request comes on standard input.
            eprintln!("usage: inlet-web (reads its request on standard input)");
            return 2;
        }
    }
    if let Err(kind) = preflight() {
        emit(&Event::Error { kind });
        return 1;
    }

    let event_loop = EventLoopBuilder::<Wake>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let opened = Arc::new(AtomicBool::new(false));
    spawn_reader(proxy.clone());
    spawn_deadline(proxy.clone(), opened.clone());

    let mut session: Option<Session> = None;
    event_loop.run(move |event, target, control_flow| {
        *control_flow = tao::event_loop::ControlFlow::Wait;
        match event {
            LoopEvent::UserEvent(Wake::Request(Request::Open(open))) => {
                opened.store(true, Ordering::SeqCst);
                if session.is_some() {
                    fail(session.take(), ErrorKind::InvalidRequest);
                }
                match start(&open, target, &proxy) {
                    Ok(started) => {
                        session = Some(started);
                        emit(&Event::Ready);
                    }
                    Err(kind) => fail(None, kind),
                }
            }
            LoopEvent::UserEvent(Wake::Request(Request::Close))
            | LoopEvent::UserEvent(Wake::InputClosed | Wake::Finish)
            | LoopEvent::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => finish(session.take()),
            LoopEvent::UserEvent(Wake::BadInput(kind)) => fail(session.take(), kind),
            LoopEvent::UserEvent(Wake::NoRequest) => {
                fail(session.take(), ErrorKind::InvalidRequest)
            }
            _ => {}
        }
    })
}

/// What must hold before anything is created.
fn preflight() -> Result<(), ErrorKind> {
    // Linux: GTK stops the program if there is no display, which Inlet could not tell
    // apart from a crash.
    #[cfg(target_os = "linux")]
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err(ErrorKind::WindowFailed);
    }
    // Windows: the WebView2 runtime comes with Windows 10 and 11 but can be removed.
    wry::webview_version()
        .map(|_| ())
        .map_err(|_| ErrorKind::RuntimeMissing)
}

/// Reads requests from standard input and wakes the event loop with them.
fn spawn_reader(proxy: EventLoopProxy<Wake>) {
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        loop {
            let mut line = Vec::new();
            let read = (&mut input)
                .take(MAX_LINE_LEN as u64 + 2)
                .read_until(b'\n', &mut line);
            let wake = match read {
                Ok(0) | Err(_) => Wake::InputClosed,
                Ok(_) if line.last() != Some(&b'\n') => {
                    if line.len() > MAX_LINE_LEN {
                        Wake::BadInput(ErrorKind::InvalidRequest)
                    } else {
                        Wake::InputClosed
                    }
                }
                Ok(_) => {
                    line.pop();
                    match String::from_utf8(line).map(|text| decode_request(&text)) {
                        Ok(Ok(request)) => {
                            if proxy.send_event(Wake::Request(request)).is_err() {
                                return;
                            }
                            continue;
                        }
                        Ok(Err(ProtocolError::UnsupportedVersion(_))) => {
                            Wake::BadInput(ErrorKind::UnsupportedVersion)
                        }
                        _ => Wake::BadInput(ErrorKind::InvalidRequest),
                    }
                }
            };
            let _ = proxy.send_event(wake);
            return;
        }
    });
}

/// Gives up when no request comes: the program that started the helper is gone or broken.
fn spawn_deadline(proxy: EventLoopProxy<Wake>, opened: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        std::thread::sleep(OPEN_WITHIN);
        if !opened.load(Ordering::SeqCst) {
            let _ = proxy.send_event(Wake::NoRequest);
        }
    });
}

/// Writes one event. When Inlet is not reading any more, the helper has no reason to
/// stay.
fn emit(event: &Event) {
    let line = encode_event(event);
    let mut out = std::io::stdout().lock();
    if out
        .write_all(line.as_bytes())
        .and_then(|()| out.flush())
        .is_err()
    {
        datadir::remove_registered();
        std::process::exit(1);
    }
}

/// The window is closed: `closed`, clean up, exit.
fn finish(session: Option<Session>) -> ! {
    // The window and the web view first, so the user sees the window go before anything
    // slow happens; then the event; then the folder, which the engine may hold for a
    // moment.
    let data_dir = session.map(Session::into_data_dir);
    emit(&Event::Closed);
    drop(data_dir);
    std::process::exit(0);
}

/// The helper could not do what it was asked: `error`, clean up, exit.
fn fail(session: Option<Session>, kind: ErrorKind) -> ! {
    let data_dir = session.map(Session::into_data_dir);
    emit(&Event::Error { kind });
    drop(data_dir);
    datadir::remove_registered();
    std::process::exit(1);
}

/// Everything of the open window. Field order is drop order: the web view goes before
/// the window it sits in, and the data folder (removed on drop) goes last.
struct Session {
    webview: WebView,
    /// Only where the engine needs a data folder (Windows).
    context: Option<WebContext>,
    window: Window,
    data_dir: Option<DataDir>,
}

impl Session {
    /// Closes the window and the web view and hands back the data folder to remove.
    fn into_data_dir(self) -> Option<DataDir> {
        let Session {
            webview,
            context,
            window,
            data_dir,
        } = self;
        drop(webview);
        drop(context);
        drop(window);
        data_dir
    }
}

/// What the web view's callbacks share.
struct Shared {
    gate: Gate,
    /// The flow is over; nothing more is reported.
    done: Cell<bool>,
    proxy: EventLoopProxy<Wake>,
}

impl Shared {
    /// Whether the engine may go to `url`; ends the flow when it may not.
    fn navigate(&self, url: &str) -> bool {
        if self.done.get() {
            return false;
        }
        match self.gate.navigation(url) {
            Decision::Load => true,
            Decision::Stop(event) => {
                self.end(&event);
                false
            }
        }
    }

    /// Whether a frame may go to `url` (Windows: the engine's frame callback).
    #[cfg(windows)]
    fn navigate_frame(&self, url: &str) -> bool {
        if self.done.get() {
            return false;
        }
        match self.gate.frame(url) {
            Decision::Load => true,
            Decision::Stop(event) => {
                self.end(&event);
                false
            }
        }
    }

    /// A document (the window's or a frame's) is about to be requested from the network
    /// (Windows: WebView2's resource callback, which runs before the request is sent).
    /// Frames and the window are not told apart here, so what the rules allow for neither
    /// (not `allow`, not `frames`) or end the flow with (`intercept`) is stopped; the rest
    /// is left to the navigation callbacks.
    #[cfg(windows)]
    fn document_request(&self, url: &str) -> bool {
        self.navigate_frame(url)
    }

    /// The window loads `url`: the second look at what `navigate` let through.
    fn page_load(&self, url: &str) {
        if self.done.get() {
            return;
        }
        if let Some(event) = self.gate.page_load(url) {
            self.end(&event);
        }
    }

    /// A new window or a download was refused; the page stays, unless it keeps asking.
    fn refuse(&self, url: &str) {
        if self.done.get() {
            return;
        }
        match self.gate.refusal(url) {
            Refusal::Report(event) => emit(&event),
            Refusal::EndFlow(event) => self.end(&event),
        }
    }

    fn end(&self, event: &Event) {
        self.done.set(true);
        emit(event);
        let _ = self.proxy.send_event(Wake::Finish);
    }
}

/// Opens the window and starts loading the first page.
fn start(
    open: &OpenRequest,
    target: &EventLoopWindowTarget<Wake>,
    proxy: &EventLoopProxy<Wake>,
) -> Result<Session, ErrorKind> {
    let policy = Policy::for_request(open, PolicyOptions::of_this_build())
        .map_err(|_| ErrorKind::InvalidRequest)?;
    // The engine's private mode keeps nothing on disk on Linux (an ephemeral context) and
    // macOS (a non-persistent store), so a data folder and a context of ours would only
    // be a place for stray files. WebView2 on Windows always needs a folder.
    #[cfg(windows)]
    let data_dir = Some(DataDir::create().map_err(|_| ErrorKind::WindowFailed)?);
    #[cfg(not(windows))]
    let data_dir: Option<DataDir> = None;
    let shared = Rc::new(Shared {
        gate: Gate::new(policy, cfg!(not(windows))),
        done: Cell::new(false),
        proxy: proxy.clone(),
    });

    let window = build_window(open, target)?;
    let mut context = data_dir
        .as_ref()
        .map(|dir| WebContext::new(Some(dir.path().to_path_buf())));
    let webview = build_webview(&window, context.as_mut(), &shared)?;
    webview
        .load_url(&open.url)
        .map_err(|_| ErrorKind::WindowFailed)?;
    Ok(Session {
        webview,
        context,
        window,
        data_dir,
    })
}

/// A window of Inlet's style: its own title (the page cannot change it), above Inlet's
/// main window and centred over it.
///
/// Where the window has a parent (Windows: an owner window) it is kept above Inlet and
/// nothing else, so it does not cover the user's password manager or authenticator.
// Deviation: where a window cannot be tied to another process's window (macOS, Linux) it is
// always on top of every window, the only way to keep it above Inlet. The Windows owner
// window ties the two threads' input queues, so a stall in one can freeze input in both;
// a stale handle falls back to no owner.
fn build_window(
    open: &OpenRequest,
    target: &EventLoopWindowTarget<Wake>,
) -> Result<Window, ErrorKind> {
    let (width, height) = (f64::from(open.size.width), f64::from(open.size.height));
    let mut base = WindowBuilder::new()
        .with_title(open.title.clone())
        .with_inner_size(LogicalSize::new(width, height))
        .with_min_inner_size(LogicalSize::new(
            f64::from(crate::protocol::MIN_WINDOW),
            240.0,
        ))
        .with_resizable(true)
        .with_visible(true)
        .with_focused(true);
    if let Some(bounds) = open.parent.and_then(|parent| parent.bounds) {
        let x = f64::from(bounds.x) + (f64::from(bounds.width) - width) / 2.0;
        let y = f64::from(bounds.y) + (f64::from(bounds.height) - height) / 2.0;
        base = base.with_position(LogicalPosition::new(x, y));
    }
    let parent_handle = open
        .parent
        .and_then(|parent| parent.handle)
        .filter(|h| *h != 0);
    let build = |parent_handle: Option<u64>| {
        let mut builder = base.clone();
        #[cfg(windows)]
        let owned = if let Some(handle) = parent_handle {
            // Tied to Inlet's main window: above it, minimised with it.
            use tao::platform::windows::WindowBuilderExtWindows;
            builder = builder.with_owner_window(handle as isize);
            true
        } else {
            false
        };
        #[cfg(not(windows))]
        let owned = {
            let _ = parent_handle;
            false
        };
        if !owned {
            builder = builder.with_always_on_top(true);
        }
        builder.build(target)
    };
    // A handle that has gone stale must not stop the flow: without a parent the window
    // is on top of everything instead.
    build(parent_handle)
        .or_else(|_| build(None))
        .map_err(|_| ErrorKind::WindowFailed)
}

fn build_webview(
    window: &Window,
    context: Option<&mut WebContext>,
    shared: &Rc<Shared>,
) -> Result<WebView, ErrorKind> {
    let on_navigate = shared.clone();
    let on_load = shared.clone();
    let on_download = shared.clone();
    let on_window = shared.clone();
    let builder = match context {
        Some(context) => WebViewBuilder::new_with_web_context(context),
        None => WebViewBuilder::new(),
    }
    // The rules of the module documentation.
    .with_devtools(false)
    .with_incognito(true)
    .with_clipboard(false)
    .with_autoplay(false)
    .with_hotkeys_zoom(false)
    .with_navigation_handler(move |url| on_navigate.navigate(&url))
    .with_on_page_load_handler(move |_, url| {
        // The second look: a load the navigation callback did not see.
        on_load.page_load(&url);
    })
    .with_new_window_req_handler(move |url, _| {
        on_window.refuse(&url);
        NewWindowResponse::Deny
    })
    .with_download_started_handler(move |url, _| {
        on_download.refuse(&url);
        false
    })
    .with_permission_handler(|_| PermissionResponse::Deny)
    // Files dropped onto the window are not opened.
    .with_drag_drop_handler(|_| true);

    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window.default_vbox().ok_or(ErrorKind::WindowFailed)?;
        builder.build_gtk(vbox)
    };
    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(window);

    let webview = webview.map_err(|_| ErrorKind::WindowFailed)?;
    // Frames must be held to the same rules as everywhere else, or the flow does not start.
    #[cfg(windows)]
    {
        hook_frames(&webview, shared)?;
        hook_requests(&webview, shared)?;
    }
    #[cfg(all(feature = "test-loopback", target_os = "linux"))]
    test_loopback_linux(&webview);
    Ok(webview)
}

/// Windows: wry hooks the window's navigations only, so the frames' go through WebView2's
/// own frame callback into the same rules ([`Gate::frame`]). When the hook cannot be set
/// the flow does not start: frames would be open.
#[cfg(windows)]
fn hook_frames(webview: &WebView, shared: &Rc<Shared>) -> Result<(), ErrorKind> {
    use webview2_com::{take_pwstr, NavigationStartingEventHandler};
    use windows_core::PWSTR;
    use wry::WebViewExtWindows;

    let shared = shared.clone();
    let handler = NavigationStartingEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        let url = {
            let mut uri = PWSTR::null();
            // SAFETY: WebView2 fills `uri` with a string it allocated; `take_pwstr` copies
            // it and frees it.
            unsafe { args.Uri(&mut uri)? };
            take_pwstr(uri)
        };
        let allow = shared.navigate_frame(&url);
        // SAFETY: `args` is the event's own argument object, valid for this call.
        unsafe { args.SetCancel(!allow)? };
        Ok(())
    }));
    let mut token = 0;
    // SAFETY: the handler is a live COM object and the token a valid pointer for the call.
    unsafe {
        webview
            .webview()
            .add_FrameNavigationStarting(&handler, &mut token)
    }
    .map_err(|_| ErrorKind::WindowFailed)
}

/// Windows: WebView2 raises `NavigationStarting` after the request for a redirect or a
/// script's navigation may already be on its way, so a URL that must never be fetched
/// (a blocked host, or the callback with the authorisation code) is also stopped where
/// the engine asks for a document: `WebResourceRequested` runs before the network and
/// can answer in its place. The navigation callbacks stay as they are: this only adds.
#[cfg(windows)]
fn hook_requests(webview: &WebView, shared: &Rc<Shared>) -> Result<(), ErrorKind> {
    use webview2_com::Microsoft::Web::WebView2::Win32::COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT;
    use webview2_com::{take_pwstr, WebResourceRequestedEventHandler};
    use windows_core::{HSTRING, PWSTR};
    use wry::WebViewExtWindows;

    let core = webview.webview();
    let env = webview.environment();
    let shared = shared.clone();
    let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else {
            return Ok(());
        };
        // SAFETY: the arguments are the event's own objects, valid for this call; the
        // string WebView2 hands out is copied and freed by `take_pwstr`.
        unsafe {
            let request = args.Request()?;
            let mut uri = PWSTR::null();
            request.Uri(&mut uri)?;
            let url = take_pwstr(uri);
            if !shared.document_request(&url) {
                let response = env.CreateWebResourceResponse(
                    None,
                    403,
                    &HSTRING::from("Blocked"),
                    &HSTRING::from(""),
                )?;
                args.SetResponse(&response)?;
            }
        }
        Ok(())
    }));
    let mut token = 0;
    // SAFETY: the handler is a live COM object and the token a valid pointer for the call.
    unsafe {
        core.AddWebResourceRequestedFilter(
            &HSTRING::from("*"),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT,
        )
        .and_then(|()| core.add_WebResourceRequested(&handler, &mut token))
    }
    .map_err(|_| ErrorKind::WindowFailed)
}

/// For the tests against a local server (never in a release build): trust the server's
/// self-signed certificate, and let a script open a window, so the refusal of new
/// windows can be tried without a mouse click.
#[cfg(all(feature = "test-loopback", target_os = "linux"))]
// WebKitGTK 4.1 still has the call that the 6.0 API moved; the test build uses it.
#[allow(deprecated)]
fn test_loopback_linux(webview: &WebView) {
    use webkit2gtk::{SettingsExt, TLSErrorsPolicy, WebContextExt, WebViewExt};
    use wry::WebViewExtUnix;
    let view = webview.webview();
    if let Some(context) = view.context() {
        context.set_tls_errors_policy(TLSErrorsPolicy::Ignore);
    }
    if let Some(settings) = WebViewExt::settings(&view) {
        settings.set_javascript_can_open_windows_automatically(true);
    }
}

#[cfg(test)]
mod tests {
    /// The helper's source, without this test module.
    fn source() -> &'static str {
        let text = include_str!("helper.rs");
        &text[..text.find("#[cfg(test)]").unwrap()]
    }

    #[test]
    fn nothing_is_injected_into_the_page() {
        // The page gets no script from the helper, no channel to it and no way to be
        // asked to run one: the only way in is the URL.
        for forbidden in [
            "initialization_script",
            "evaluate_script",
            "with_ipc_handler",
            "with_custom_protocol",
            "with_asynchronous_custom_protocol",
            "with_html",
            "with_headers",
            "with_url_and_headers",
            "load_url_with_headers",
            "with_user_agent",
            "set_cookie",
            "cookies",
            "open_devtools",
        ] {
            assert!(
                !source().contains(forbidden),
                "the helper uses `{forbidden}`"
            );
        }
    }

    #[test]
    fn the_web_view_is_built_with_every_rule() {
        let source = source();
        for required in [
            ".with_devtools(false)",
            ".with_incognito(true)",
            ".with_navigation_handler(",
            ".with_on_page_load_handler(",
            ".with_new_window_req_handler(",
            "NewWindowResponse::Deny",
            ".with_download_started_handler(",
            ".with_permission_handler(|_| PermissionResponse::Deny)",
            ".with_drag_drop_handler(|_| true)",
            ".with_always_on_top(true)",
        ] {
            assert!(source.contains(required), "the helper lacks `{required}`");
        }
        // The new window handler never lets one open, whatever it is given.
        assert!(!source.contains("NewWindowResponse::Allow"));
        assert!(!source.contains("NewWindowResponse::Create"));
    }

    #[test]
    fn the_test_loopback_code_is_only_in_the_test_build() {
        let source = source();
        let at = source.find("fn test_loopback_linux").unwrap();
        // The attributes just above the function.
        let above: Vec<&str> = source[..at].trim_end().lines().rev().take(4).collect();
        assert!(
            above.contains(&"#[cfg(all(feature = \"test-loopback\", target_os = \"linux\"))]"),
            "the loopback tweaks must be behind the test-loopback feature"
        );
    }
}
