//! The web view: a local page that draws the project as a graph.
//!
//! A small blocking HTTP server on a background thread, bound to loopback
//! only. Three routes — the page, the graph as JSON, and a callback the page
//! uses to ask tiny to open a file. That last one is what makes the graph a
//! way of moving around the project rather than just a picture of it.
//!
//! No web framework, no async runtime: this serves one page to one person on
//! their own machine, and a hundred lines of `std::net` covers it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::graph;

const PAGE: &str = include_str!("web/graph.html");

/// Shared between the server thread and the app.
struct Shared {
    root: PathBuf,
    options: Mutex<graph::Options>,
    /// A file the page asked tiny to open.
    open_request: Mutex<Option<PathBuf>>,
    shutdown: AtomicBool,
}

pub struct Server {
    addr: SocketAddr,
    shared: Arc<Shared>,
}

impl Server {
    /// Start serving. Port 0 lets the OS pick a free one.
    pub fn start(root: PathBuf, options: graph::Options, port: u16) -> Result<Self> {
        // Loopback only: this exposes the contents of your project, and has
        // no business being reachable from the network.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .with_context(|| format!("cannot listen on 127.0.0.1:{port}"))?;
        let addr = listener.local_addr()?;
        listener
            .set_nonblocking(true)
            .context("cannot poll the listener")?;

        let shared = Arc::new(Shared {
            root,
            options: Mutex::new(options),
            open_request: Mutex::new(None),
            shutdown: AtomicBool::new(false),
        });

        let worker = Arc::clone(&shared);
        thread::Builder::new()
            .name("tiny-web".into())
            .spawn(move || serve(listener, worker))
            .context("cannot start the web thread")?;

        Ok(Self { addr, shared })
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Take whatever file the page last asked to open.
    pub fn take_open_request(&self) -> Option<PathBuf> {
        self.shared.open_request.lock().ok()?.take()
    }

    /// Keep the served graph in step with the app's settings.
    pub fn update_options(&self, options: graph::Options) {
        if let Ok(mut o) = self.shared.options.lock() {
            *o = options;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Relaxed);
    }
}

fn serve(listener: TcpListener, shared: Arc<Shared>) {
    while !shared.shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                // One at a time is plenty for a single reader on localhost,
                // and it keeps the graph build off the app's thread.
                let _ = handle(stream, &shared);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(40));
            }
            Err(_) => break,
        }
    }
}

fn handle(mut stream: TcpStream, shared: &Shared) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    // Headers, only for the length of a body we actually intend to read.
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }

    let path = target.split('?').next().unwrap_or("/");
    match (method.as_str(), path) {
        ("GET", "/") => respond(
            &mut stream,
            200,
            "text/html; charset=utf-8",
            PAGE.as_bytes(),
        ),
        ("GET", "/graph.json") => {
            let options = shared.options.lock().map(|o| o.clone()).unwrap_or_default();
            let g = graph::build(&shared.root, &options);
            let body = serde_json::to_vec(&g).unwrap_or_else(|_| b"{}".to_vec());
            respond(&mut stream, 200, "application/json", &body)
        }
        ("POST", "/open") => {
            // Cap the body: nothing legitimate sends more than a path.
            let mut body = vec![0u8; content_length.min(4096)];
            reader.read_exact(&mut body)?;
            let rel = String::from_utf8_lossy(&body).trim().to_string();
            match safe_target(&shared.root, &rel) {
                Some(p) => {
                    if let Ok(mut slot) = shared.open_request.lock() {
                        *slot = Some(p);
                    }
                    respond(&mut stream, 200, "text/plain", b"ok")
                }
                None => respond(&mut stream, 400, "text/plain", b"outside the project"),
            }
        }
        _ => respond(&mut stream, 404, "text/plain", b"not found"),
    }
}

/// Resolve a path the page sent, refusing anything outside the project.
/// The page is local, but it is still input arriving over a socket.
fn safe_target(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() {
        return None;
    }
    let mut out = root.to_path_buf();
    for part in Path::new(rel).components() {
        match part {
            std::path::Component::Normal(p) => out.push(p),
            // Anything that could climb out is rejected outright rather than
            // normalised, since there is no reason for the page to send it.
            _ => return None,
        }
    }
    out.starts_with(root).then_some(out)
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
         Connection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    stream.flush()
}

/// Ask the desktop to open a URL. Best effort — the caller shows the address
/// either way, so a headless machine just reads it off the status line.
pub fn open_in_browser(url: &str) -> bool {
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else if cfg!(target_os = "windows") {
        &[("cmd", &["/C", "start", ""])]
    } else {
        &[("xdg-open", &[]), ("gio", &["open"]), ("wslview", &[])]
    };
    for (program, args) in candidates {
        let ok = std::process::Command::new(program)
            .args(*args)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .is_ok();
        if ok {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join("notes")).unwrap();
        std::fs::write(td.path().join("notes/a.md"), "see [[b]]\n").unwrap();
        std::fs::write(td.path().join("notes/b.md"), "# B\n").unwrap();
        td
    }

    /// A tiny HTTP client, so the tests exercise the real socket path.
    fn request(addr: SocketAddr, raw: &str) -> (u16, String) {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut buf = String::new();
        s.read_to_string(&mut buf).unwrap();
        let status = buf
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let body = buf.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }

    fn get(addr: SocketAddr, path: &str) -> (u16, String) {
        request(addr, &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n"))
    }

    #[test]
    fn serves_the_page_and_the_graph() {
        let td = fixture();
        let s = Server::start(td.path().to_path_buf(), graph::Options::default(), 0).unwrap();

        let (status, body) = get(s.addr, "/");
        assert_eq!(status, 200);
        assert!(body.contains("<canvas"), "the page should draw a canvas");

        let (status, body) = get(s.addr, "/graph.json");
        assert_eq!(status, 200);
        let g: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(g["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(g["edges"].as_array().unwrap().len(), 1);
        assert_eq!(g["edges"][0]["kind"], "wikilink");
    }

    #[test]
    fn binds_to_loopback_only() {
        let td = fixture();
        let s = Server::start(td.path().to_path_buf(), graph::Options::default(), 0).unwrap();
        assert_eq!(
            s.addr.ip(),
            Ipv4Addr::LOCALHOST,
            "must not be reachable off-box"
        );
        assert!(s.url().starts_with("http://127.0.0.1:"));
    }

    #[test]
    fn an_unknown_route_is_a_404() {
        let td = fixture();
        let s = Server::start(td.path().to_path_buf(), graph::Options::default(), 0).unwrap();
        assert_eq!(get(s.addr, "/secrets").0, 404);
    }

    #[test]
    fn the_page_can_ask_tiny_to_open_a_file() {
        let td = fixture();
        let s = Server::start(td.path().to_path_buf(), graph::Options::default(), 0).unwrap();
        assert!(s.take_open_request().is_none(), "nothing pending yet");

        let body = "notes/a.md";
        let (status, _) = request(
            s.addr,
            &format!(
                "POST /open HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert_eq!(status, 200);
        assert_eq!(
            s.take_open_request(),
            Some(td.path().join("notes/a.md")),
            "the app should be handed the resolved path"
        );
        assert!(s.take_open_request().is_none(), "and only once");
    }

    #[test]
    fn a_path_climbing_out_of_the_project_is_refused() {
        let td = fixture();
        let s = Server::start(td.path().to_path_buf(), graph::Options::default(), 0).unwrap();
        for evil in ["../../etc/passwd", "/etc/passwd", "notes/../../escape"] {
            let (status, _) = request(
                s.addr,
                &format!(
                    "POST /open HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{evil}",
                    evil.len()
                ),
            );
            assert_eq!(status, 400, "{evil} should be refused");
            assert!(
                s.take_open_request().is_none(),
                "{evil} must not reach the app"
            );
        }
    }

    #[test]
    fn safe_target_only_accepts_plain_relative_paths() {
        let root = Path::new("/project");
        assert_eq!(
            safe_target(root, "notes/a.md"),
            Some(PathBuf::from("/project/notes/a.md"))
        );
        assert_eq!(safe_target(root, ""), None);
        assert_eq!(safe_target(root, ".."), None);
        assert_eq!(safe_target(root, "a/../../b"), None);
        assert_eq!(safe_target(root, "/etc/passwd"), None);
    }

    #[test]
    fn the_graph_reflects_settings_it_is_given() {
        let td = fixture();
        std::fs::write(td.path().join(".hidden.md"), "x\n").unwrap();
        let s = Server::start(td.path().to_path_buf(), graph::Options::default(), 0).unwrap();
        let before: serde_json::Value =
            serde_json::from_str(&get(s.addr, "/graph.json").1).unwrap();
        assert_eq!(before["nodes"].as_array().unwrap().len(), 2);

        s.update_options(graph::Options {
            show_hidden: true,
            ..graph::Options::default()
        });
        let after: serde_json::Value = serde_json::from_str(&get(s.addr, "/graph.json").1).unwrap();
        assert_eq!(after["nodes"].as_array().unwrap().len(), 3);
    }
}
