//! The server on an ephemeral loopback port, spoken to over real sockets,
//! with values published the way the game publishes them.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use victory_mcp::{Config, Json, KIND_FIRST_BIND, RawFrame, Shared, Stats, parse_json, shot, start};

struct Reply {
    status: u16,
    head: String,
    body: String,
}

fn read_reply(stream: &mut TcpStream) -> Reply {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head_end, length) = loop {
        let n = stream.read(&mut chunk).expect("read");
        assert!(n > 0, "closed before a full head: {}", String::from_utf8_lossy(&buf));
        buf.extend_from_slice(&chunk[..n]);
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..at]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length: "))
                .map(|v| v.trim().parse::<usize>().unwrap())
                .expect("content-length");
            break (at + 4, length);
        }
    };
    while buf.len() < head_end + length {
        let n = stream.read(&mut chunk).expect("read body");
        assert!(n > 0, "closed in the body");
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    Reply { status, head, body: String::from_utf8_lossy(&buf[head_end..head_end + length]).into_owned() }
}

fn connect(addr: SocketAddr) -> TcpStream {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s
}

fn request(stream: &mut TcpStream, addr: SocketAddr, method: &str, path: &str, extra: &str, body: &str, close: bool) -> Reply {
    let text = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: {}\r\n\r\n{body}",
        body.len(),
        if close { "close" } else { "keep-alive" }
    );
    stream.write_all(text.as_bytes()).unwrap();
    read_reply(stream)
}

fn post(addr: SocketAddr, body: &str) -> Reply {
    request(&mut connect(addr), addr, "POST", "/mcp", "", body, true)
}

fn json(r: &Reply) -> Json {
    assert_eq!(r.status, 200, "{}", r.body);
    parse_json(r.body.as_bytes()).unwrap()
}

fn start_server(config: Config) -> (Arc<Shared>, victory_mcp::ServerHandle, SocketAddr) {
    let shared = Arc::new(Shared::new(config));
    let handle = start(shared.clone(), "127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = handle.addr();
    (shared, handle, addr)
}

#[test]
fn initialize_list_and_get_stats_over_a_socket() {
    let (shared, handle, addr) = start_server(Config::default());
    assert!(addr.ip().is_loopback());

    let init = json(&post(
        addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#,
    ));
    assert_eq!(init.get("result").and_then(|r| r.get("protocolVersion")).and_then(Json::as_str), Some("2025-06-18"));

    let accepted = post(addr, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    assert_eq!((accepted.status, accepted.body.as_str()), (202, ""));

    let list = json(&post(addr, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#));
    let Some(Json::Arr(tools)) = list.get("result").and_then(|r| r.get("tools")) else { panic!("{list:?}") };
    assert_eq!(tools.len(), 24);

    // Fake published data, as the frame meter publishes it.
    shared.publish_stats(Stats {
        frame: 777,
        frames: 13,
        fps: 12.7,
        ms_per_frame: 78.5,
        preparing_ms: 35.2,
        copies_ms: 10.5,
        gpu_ms: 40.0,
        submissions_per_frame: 6.0,
        ..Stats::default()
    });
    let stats = json(&post(addr, r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_stats","arguments":{}}}"#));
    let result = stats.get("result").unwrap();
    assert_eq!(result.get("isError"), Some(&Json::Bool(false)));
    let c = result.get("structuredContent").unwrap();
    assert_eq!(c.get("frame"), Some(&Json::Int(777)));
    assert_eq!(c.get("fps"), Some(&Json::Num(12.7)));
    assert_eq!(c.get("ms_per_frame"), Some(&Json::Num(78.5)));
    assert_eq!(c.get("preparing").and_then(|p| p.get("ms")), Some(&Json::Num(35.2)));
    assert_eq!(c.get("submissions_per_frame").and_then(Json::as_i64), Some(6));
    assert!(c.get("uptime_s").is_some());

    shared.record_event(KIND_FIRST_BIND, "deadbeef", Some("pixel"), None, 777, None);
    shared.push_log("   1.000s  INFO victory_gpu::pipeline: first bind hash=deadbeef");
    let events = json(&post(addr, r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_pipeline_events","arguments":{"n":5}}}"#));
    assert!(events.text().contains("deadbeef"));
    let log = json(&post(addr, r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_recent_log","arguments":{"filter":"first bind"}}}"#));
    assert!(log.text().contains("first bind hash=deadbeef"));

    handle.stop();
}

#[test]
fn keep_alive_serves_several_requests_on_one_connection() {
    let (_shared, handle, addr) = start_server(Config::default());
    let mut s = connect(addr);
    for id in 0..5 {
        let r = request(&mut s, addr, "POST", "/mcp", "", &format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping"}}"#), false);
        assert_eq!(r.status, 200);
        assert!(r.head.to_ascii_lowercase().contains("connection: keep-alive"));
        assert_eq!(json(&r).get("id"), Some(&Json::Int(id)));
    }
    handle.stop();
}

#[test]
fn http_level_refusals() {
    let (_shared, handle, addr) = start_server(Config::default());
    let mut s = connect(addr);
    let r = request(&mut s, addr, "GET", "/mcp", "", "", true);
    assert_eq!(r.status, 405);
    assert!(r.head.contains("Allow: POST"));
    let r = request(&mut connect(addr), addr, "POST", "/elsewhere", "", "{}", true);
    assert_eq!(r.status, 404);
    let r = request(&mut connect(addr), addr, "POST", "/mcp", "Origin: http://evil.example\r\n", "{}", true);
    assert_eq!(r.status, 403);
    let r = request(&mut connect(addr), addr, "POST", "/mcp", "Origin: http://localhost:3000\r\n", r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#, true);
    assert_eq!(r.status, 200);
    let r = post(addr, "this is not json");
    assert_eq!(r.status, 400);
    assert!(r.body.contains("-32700"));
    // Over the body limit: refused from its Content-Length alone.
    let mut big = connect(addr);
    big.write_all(b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 99999999\r\nConnection: close\r\n\r\n").unwrap();
    assert_eq!(read_reply(&mut big).status, 413);
    // Chunked bodies are not served.
    let mut chunked = connect(addr);
    chunked.write_all(b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap();
    assert_eq!(read_reply(&mut chunked).status, 411);
    // Garbage in place of a request line.
    let mut junk = connect(addr);
    junk.write_all(b"\x16\x03\x01 not http at all\r\n\r\n").unwrap();
    assert_eq!(read_reply(&mut junk).status, 400);
    // A request sent in dribbles is still read.
    let mut slow = connect(addr);
    let body = r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#;
    let text = format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    for piece in text.as_bytes().chunks(5) {
        slow.write_all(piece).unwrap();
        slow.flush().unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(json(&read_reply(&mut slow)).get("id"), Some(&Json::Int(7)));
    handle.stop();
}

#[test]
fn screenshot_is_taken_by_the_present_path_and_written_as_a_png() {
    let dir = tempfile::tempdir().unwrap();
    let (shared, handle, addr) = start_server(Config { shots_dir: dir.path().join("shots"), shot_timeout: Duration::from_secs(10), ..Config::default() });

    // A stand-in for the present path: once a frame, look for a request.
    let done = Arc::new(AtomicBool::new(false));
    let present = {
        let (shared, done) = (shared.clone(), done.clone());
        std::thread::spawn(move || {
            let mut frame = 100u64;
            while !done.load(Ordering::Relaxed) {
                frame += 1;
                if let Some(request) = shared.take_shot() {
                    // 4x2 pixels of B8G8R8A8 with row padding already removed.
                    let mut rows = Vec::new();
                    for i in 0..8u8 {
                        rows.extend([i * 10, 20, 30, 255]);
                    }
                    request.deliver(RawFrame { width: 4, height: 2, format: shot::format::B8G8R8A8_UNORM, frame, rows });
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    };

    let r = json(&post(addr, r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"screenshot","arguments":{"downscale":2}}}"#));
    done.store(true, Ordering::Relaxed);
    present.join().unwrap();
    let result = r.get("result").unwrap();
    assert_eq!(result.get("isError"), Some(&Json::Bool(false)), "{}", r.text());
    let c = result.get("structuredContent").unwrap();
    let path = c.get("path").and_then(Json::as_str).unwrap();
    assert!(path.ends_with(".png") && path.contains("-f1"), "{path}");
    assert_eq!((c.get("width"), c.get("height")), (Some(&Json::Int(2)), Some(&Json::Int(1))));
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(c.get("bytes"), Some(&Json::Int(bytes.len() as i64)));
    handle.stop();
}

#[test]
fn a_screenshot_with_nothing_presenting_answers_with_a_tool_error() {
    let dir = tempfile::tempdir().unwrap();
    let (_shared, handle, addr) = start_server(Config { shots_dir: dir.path().into(), shot_timeout: Duration::from_millis(100), ..Config::default() });
    let r = json(&post(addr, r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"screenshot","arguments":{}}}"#));
    let result = r.get("result").unwrap();
    assert_eq!(result.get("isError"), Some(&Json::Bool(true)));
    assert!(r.text().contains("no frame was presented"));
    handle.stop();
}
