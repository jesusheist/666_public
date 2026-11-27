//! The listener: loopback only, one short-lived thread per connection.

use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::http::{self, HttpError, Reader, Request};
use crate::protocol::{self, Outcome};
use crate::{RESERVED_PORTS, Shared};

/// Connections served at once; more are answered 503.
const MAX_CONNECTIONS: usize = 16;
/// Requests one connection may make.
const MAX_REQUESTS: usize = 1000;
/// How long a connection may sit idle or stall in a request.
const IO_TIMEOUT: Duration = Duration::from_secs(15);

pub struct ServerHandle {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl ServerHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops accepting (connections in progress finish). The game never
    /// calls this; tests do.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the accept.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(500));
    }
}

fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
    match addr {
        SocketAddr::V4(a) if *a.ip() == Ipv4Addr::LOCALHOST => {}
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "only 127.0.0.1 is served")),
    }
    if RESERVED_PORTS.contains(&addr.port()) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "ports 59340-59450 are reserved"));
    }
    if addr.port() != 0 {
        return TcpListener::bind(addr);
    }
    // An ephemeral port, but not one of the reserved ones.
    for _ in 0..50 {
        let listener = TcpListener::bind(addr)?;
        if !RESERVED_PORTS.contains(&listener.local_addr()?.port()) {
            return Ok(listener);
        }
    }
    Err(io::Error::new(io::ErrorKind::AddrNotAvailable, "no free port outside the reserved range"))
}

/// Binds `addr` (127.0.0.1 only) and serves on a thread of its own.
pub fn start(shared: Arc<Shared>, addr: SocketAddr) -> io::Result<ServerHandle> {
    let listener = bind(addr)?;
    let addr = listener.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::Builder::new().name("victory-mcp".into()).spawn(move || {
        let active = Arc::new(AtomicUsize::new(0));
        for stream in listener.incoming() {
            if flag.load(Ordering::Relaxed) {
                break;
            }
            let Ok(stream) = stream else { continue };
            if !stream.peer_addr().is_ok_and(|p| p.ip().is_loopback()) {
                continue;
            }
            if active.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                active.fetch_sub(1, Ordering::AcqRel);
                let _ = http::write_response(&mut &stream, 503, &[], br#"{"error":"too many connections"}"#, false);
                continue;
            }
            let (shared, active, port) = (shared.clone(), active.clone(), addr.port());
            let spawned = std::thread::Builder::new()
                .name("victory-mcp-conn".into())
                .stack_size(1 << 20)
                .spawn({
                    let active = active.clone();
                    move || {
                        let _ = serve(stream, &shared, port);
                        active.fetch_sub(1, Ordering::AcqRel);
                    }
                });
            if spawned.is_err() {
                active.fetch_sub(1, Ordering::AcqRel);
            }
        }
    })?;
    Ok(ServerHandle { addr, stop })
}

fn serve(stream: TcpStream, shared: &Shared, port: u16) -> io::Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let _ = stream.set_nodelay(true);
    let mut reader = Reader::new(stream.try_clone()?);
    let mut out = &stream;
    for _ in 0..MAX_REQUESTS {
        let request = reader.next_request_with(&mut || {
            let _ = (&stream).write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
        });
        match request {
            Ok(Some(request)) => {
                let keep = request.keep_alive();
                let (status, extra, body) = route(&request, shared, port);
                http::write_response(&mut out, status, &extra, body.as_bytes(), keep)?;
                if !keep {
                    break;
                }
            }
            Ok(None) | Err(HttpError::Closed) => break,
            Err(e) => {
                let body = protocol_error_body(e.message());
                http::write_response(&mut out, e.status(), &[], body.as_bytes(), false)?;
                break;
            }
        }
    }
    Ok(())
}

fn protocol_error_body(message: &str) -> String {
    crate::Json::obj([("error", crate::Json::str(message))]).text()
}

/// The status, extra headers and body for a request.
fn route(request: &Request, shared: &Shared, port: u16) -> (u16, Vec<(&'static str, &'static str)>, String) {
    if !host_ok(request.header("host"), port) {
        return (403, vec![], protocol_error_body("Host must be 127.0.0.1 or localhost"));
    }
    if let Some(origin) = request.header("origin")
        && !origin_ok(origin)
    {
        return (403, vec![], protocol_error_body("Origin not allowed: only pages on this PC may call"));
    }
    if request.path() != "/mcp" {
        return (404, vec![], protocol_error_body("the server is at /mcp"));
    }
    if request.method != "POST" {
        return (405, vec![("Allow", "POST")], protocol_error_body("POST JSON-RPC to /mcp (no event stream is served)"));
    }
    match protocol::handle_body(shared, &request.body) {
        Outcome::Response(r) => (200, vec![], r.text()),
        Outcome::Accepted => (202, vec![], String::new()),
        Outcome::Error(status, r) => (status, vec![], r.text()),
    }
}

/// The host part of `host` (`name`, `name:port`, `[::1]:port`).
fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or("");
    }
    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

fn loopback_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1" || name == "::1"
}

/// A request without a Host header (HTTP/1.0) passes; one naming another
/// host (a DNS name pointed at this PC) does not.
fn host_ok(host: Option<&str>, _port: u16) -> bool {
    host.is_none_or(|h| loopback_name(host_name(h)))
}

/// A browser page calling from another origin does not pass.
fn origin_ok(origin: &str) -> bool {
    let rest = origin.strip_prefix("http://").or_else(|| origin.strip_prefix("https://"));
    rest.is_some_and(|r| loopback_name(host_name(r.split('/').next().unwrap_or(""))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_origins() {
        assert!(host_ok(None, 1));
        for h in ["127.0.0.1:47820", "localhost:47820", "LOCALHOST", "[::1]:47820", "127.0.0.1"] {
            assert!(host_ok(Some(h), 1), "{h}");
        }
        for h in ["evil.example:47820", "127.0.0.1.evil.example", "10.0.0.5:1", ""] {
            assert!(!host_ok(Some(h), 1), "{h}");
        }
        assert!(origin_ok("http://localhost:3000"));
        assert!(origin_ok("http://127.0.0.1:47820"));
        for o in ["null", "http://evil.example", "https://localhost.evil.example", "file://"] {
            assert!(!origin_ok(o), "{o}");
        }
    }

    #[test]
    fn only_loopback_and_unreserved_ports_bind() {
        let shared = Arc::new(Shared::new(crate::Config::default()));
        for bad in ["0.0.0.0:0", "192.168.1.2:0", "127.0.0.1:59340", "127.0.0.1:59450", "[::1]:0"] {
            assert!(start(shared.clone(), bad.parse().unwrap()).is_err(), "{bad}");
        }
        let h = start(shared, "127.0.0.1:0".parse().unwrap()).unwrap();
        assert!(h.addr().ip().is_loopback() && !RESERVED_PORTS.contains(&h.addr().port()));
        h.stop();
    }
}
