// SPDX-License-Identifier: AGPL-3.0-or-later
//! A small web server for the tests and the demo (feature `loopback`): a handful of
//! pages that try the helper's rules, and a record of which requests came in.
//!
//! The server is generic over the connection, so a test can serve the same pages over
//! TLS. It answers every request with `Connection: close`, one thread per connection.
//!
//! Pages (`PORT` is the server's, `OTHER` is the same server under its other loopback
//! name, `localhost` or `127.0.0.1`, which is another host to the helper):
//!
//! | path | what it does |
//! |---|---|
//! | `/` | links to try by hand |
//! | `/next`, `/popup`, `/blocked`, `/redir-target` | plain pages |
//! | `/cb` | a plain page; the flows intercept it |
//! | `/file` | answers with `Content-Disposition: attachment` |
//! | `/redirect` | `302` to `/redir-target` on the other host |
//! | `/redirect-cb` | `302` to `/cb?code=abc` on the same host: the way an identity provider sends the user back |
//! | `/auto/frame` | embeds a frame of `/frame` on the other host |
//! | `/auto/next` | goes to `/next` by itself |
//! | `/auto/blocked` | goes to `/blocked` on the other host by itself |
//! | `/auto/intercept` | goes to `/cb?code=abc` by itself |
//! | `/auto/download` | goes to `/file` by itself |
//! | `/auto/window` | calls `window.open('/popup')` |
//! | `/auto/window-flood` | calls `window.open` thirty times |
//! | `/auto/data` | goes to a `data:` URL |

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A connection the server reads a request from and writes the answer to.
pub trait Conn: Read + Write + Send {}
impl<T: Read + Write + Send> Conn for T {}

/// Turns an accepted TCP connection into the connection the server talks over (a TLS
/// stream, say). `None` drops it.
pub type Accept = dyn Fn(TcpStream) -> Option<Box<dyn Conn>> + Send + Sync;

/// One request the server saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The `Host` header, with the port.
    pub host: String,
    /// The path with its query.
    pub path: String,
}

/// The server. Stops when dropped.
pub struct PageServer {
    addr: SocketAddr,
    hits: Arc<Mutex<Vec<Hit>>>,
    stop: Arc<AtomicBool>,
}

impl PageServer {
    /// Serves plain http on a free loopback port.
    pub fn plain() -> io::Result<PageServer> {
        PageServer::start("http", Arc::new(|tcp| Some(Box::new(tcp) as Box<dyn Conn>)))
    }

    /// Serves on a free loopback port. `scheme` is what the pages use in the links they
    /// build (`http` or `https`).
    pub fn start(scheme: &'static str, accept: Arc<Accept>) -> io::Result<PageServer> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let hits = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (hits, stop) = (hits.clone(), stop.clone());
            std::thread::spawn(move || {
                for tcp in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(tcp) = tcp else { continue };
                    let (hits, accept) = (hits.clone(), accept.clone());
                    std::thread::spawn(move || {
                        if let Some(conn) = accept(tcp) {
                            let _ = serve(conn, scheme, &hits);
                        }
                    });
                }
            });
        }
        Ok(PageServer { addr, hits, stop })
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The `127.0.0.1:PORT` form of the server's host.
    pub fn host_ip(&self) -> String {
        format!("127.0.0.1:{}", self.port())
    }

    /// The `localhost:PORT` form: the same server as another host.
    pub fn host_name(&self) -> String {
        format!("localhost:{}", self.port())
    }

    /// Everything requested so far.
    pub fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Whether a request for `path` (a prefix of path and query) came in on `host`.
    pub fn saw(&self, host: &str, path: &str) -> bool {
        self.hits()
            .iter()
            .any(|hit| hit.host == host && hit.path.starts_with(path))
    }
}

impl Drop for PageServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wakes the accept loop so it sees the flag.
        let _ = TcpStream::connect(self.addr);
    }
}

fn serve(mut conn: Box<dyn Conn>, scheme: &str, hits: &Mutex<Vec<Hit>>) -> io::Result<()> {
    let (path, host) = {
        let mut reader = BufReader::new(&mut conn);
        let mut first = String::new();
        if reader.by_ref().take(8192).read_line(&mut first)? == 0 {
            return Ok(());
        }
        let path = first.split(' ').nth(1).unwrap_or("/").to_string();
        let mut host = String::new();
        loop {
            let mut line = String::new();
            if reader.by_ref().take(8192).read_line(&mut line)? == 0 || line.trim().is_empty() {
                break;
            }
            if let Some(value) = line
                .to_ascii_lowercase()
                .strip_prefix("host:")
                .map(|v| v.trim().to_string())
            {
                host = value;
            }
        }
        (path, host)
    };
    hits.lock().unwrap_or_else(|e| e.into_inner()).push(Hit {
        host: host.clone(),
        path: path.clone(),
    });
    let port = host.rsplit(':').next().unwrap_or("");
    let other = if host.starts_with("localhost") {
        format!("127.0.0.1:{port}")
    } else {
        format!("localhost:{port}")
    };
    let answer = answer(&path, scheme, &other);
    conn.write_all(answer.as_bytes())?;
    conn.flush()
}

fn answer(path: &str, scheme: &str, other: &str) -> String {
    let route = path.split(['?', '#']).next().unwrap_or("/");
    let go = |url: &str| format!("<script>location.href = {url:?};</script>");
    let (status, extra, body) = match route {
        "/" => ("200 OK", String::new(), menu(scheme, other)),
        "/file" => (
            "200 OK",
            "Content-Disposition: attachment; filename=\"test.txt\"\r\n".to_string(),
            "a file".to_string(),
        ),
        "/redirect" => (
            "302 Found",
            format!("Location: {scheme}://{other}/redir-target\r\n"),
            String::new(),
        ),
        "/redirect-cb" => (
            "302 Found",
            "Location: /cb?code=abc\r\n".to_string(),
            String::new(),
        ),
        "/auto/frame" => (
            "200 OK",
            String::new(),
            format!("<iframe src=\"{scheme}://{other}/frame\"></iframe>"),
        ),
        "/auto/next" => ("200 OK", String::new(), go("/next")),
        "/auto/blocked" => (
            "200 OK",
            String::new(),
            go(&format!("{scheme}://{other}/blocked")),
        ),
        "/auto/intercept" => ("200 OK", String::new(), go("/cb?code=abc")),
        "/auto/download" => ("200 OK", String::new(), go("/file")),
        "/auto/data" => ("200 OK", String::new(), go("data:text/html,blocked")),
        "/auto/window" => (
            "200 OK",
            String::new(),
            "<script>window.open('/popup');</script>".to_string(),
        ),
        "/auto/window-flood" => (
            "200 OK",
            String::new(),
            "<script>for (let i = 0; i < 30; i++) window.open('/popup' + i);</script>".to_string(),
        ),
        other_path => ("200 OK", String::new(), format!("page {other_path}")),
    };
    let html = format!("<!doctype html><meta charset=utf-8><title>test</title><body>{body}</body>");
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    )
}

fn menu(scheme: &str, other: &str) -> String {
    format!(
        "<h1>Inlet web view test page</h1><ul>\
         <li><a href=\"/next\">An allowed page (same host)</a></li>\
         <li><a href=\"{scheme}://{other}/blocked\">Another host (blocked, the window closes)</a></li>\
         <li><a href=\"/cb?code=demo\">The end of a flow (intercepted, the window closes)</a></li>\
         <li><a href=\"/file\">A download (blocked)</a></li>\
         <li><a href=\"/popup\" target=\"_blank\">A new window (blocked)</a></li>\
         <li><a href=\"data:text/html,hello\">A data: link (blocked, the window closes)</a></li>\
         </ul>"
    )
}
