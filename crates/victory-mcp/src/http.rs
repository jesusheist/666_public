//! A minimal HTTP/1.1 request reader and response writer: a request line,
//! headers, a body of `Content-Length` bytes. Chunked bodies, upgrades and
//! trailers are not served.

use std::io::{self, Read, Write};

/// The request line and headers, together.
pub const MAX_HEAD: usize = 16 * 1024;
/// The body.
pub const MAX_BODY: usize = 1 << 20;
/// Header lines.
pub const MAX_HEADERS: usize = 64;

#[derive(Debug)]
pub struct Request {
    pub method: String,
    pub target: String,
    /// 0 for HTTP/1.0, 1 for HTTP/1.1.
    pub minor: u8,
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// The target without its query.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }

    pub fn keep_alive(&self) -> bool {
        let connection = self.header("connection").unwrap_or("").to_ascii_lowercase();
        let has = |token: &str| connection.split(',').any(|t| t.trim() == token);
        if self.minor >= 1 { !has("close") } else { has("keep-alive") }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HttpError {
    Bad(&'static str),
    HeadTooLarge,
    BodyTooLarge,
    LengthRequired,
    VersionNotSupported,
    /// No more bytes came while a request was half read.
    Timeout,
    /// The peer went away or the socket failed: nothing to answer.
    Closed,
}

impl HttpError {
    pub fn status(&self) -> u16 {
        match self {
            HttpError::Bad(_) => 400,
            HttpError::HeadTooLarge => 431,
            HttpError::BodyTooLarge => 413,
            HttpError::LengthRequired => 411,
            HttpError::VersionNotSupported => 505,
            HttpError::Timeout => 408,
            HttpError::Closed => 400,
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            HttpError::Bad(why) => why,
            HttpError::HeadTooLarge => "request head too large",
            HttpError::BodyTooLarge => "request body too large",
            HttpError::LengthRequired => "a Content-Length body is required (chunked is not served)",
            HttpError::VersionNotSupported => "HTTP/1.0 and HTTP/1.1 only",
            HttpError::Timeout => "request timed out",
            HttpError::Closed => "connection closed",
        }
    }
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Error",
    }
}

/// Reads requests one after another from a stream, keeping what was read
/// beyond one request for the next.
pub struct Reader<R> {
    inner: R,
    buf: Vec<u8>,
}

impl<R: Read> Reader<R> {
    pub fn new(inner: R) -> Self {
        Reader { inner, buf: Vec::new() }
    }

    /// More bytes into the buffer. `Ok(false)`: the stream ended or sat
    /// idle with nothing half read.
    fn fill(&mut self) -> Result<bool, HttpError> {
        let mut chunk = [0u8; 4096];
        loop {
            match self.inner.read(&mut chunk) {
                Ok(0) => {
                    return if self.buf.is_empty() { Ok(false) } else { Err(HttpError::Closed) };
                }
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    return Ok(true);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    return if self.buf.is_empty() { Ok(false) } else { Err(HttpError::Timeout) };
                }
                Err(_) => return Err(HttpError::Closed),
            }
        }
    }

    #[cfg(test)]
    pub fn next_request(&mut self) -> Result<Option<Request>, HttpError> {
        self.next_request_with(&mut || {})
    }

    /// The next request; `None` when the stream ended (or idled) between
    /// requests. `proceed` is run when the client asked `Expect:
    /// 100-continue` and its body is not all here yet.
    pub fn next_request_with(&mut self, proceed: &mut dyn FnMut()) -> Result<Option<Request>, HttpError> {
        // Blank lines before a request line are ignored.
        let end = loop {
            let blanks = self.buf.iter().take_while(|&&b| b == b'\r' || b == b'\n').count();
            if blanks > 0 {
                self.buf.drain(..blanks);
            }
            if let Some(end) = find(&self.buf, b"\r\n\r\n") {
                break end;
            }
            if self.buf.len() > MAX_HEAD {
                return Err(HttpError::HeadTooLarge);
            }
            if !self.fill()? {
                return Ok(None);
            }
        };
        if end + 4 > MAX_HEAD {
            return Err(HttpError::HeadTooLarge);
        }
        let mut request = parse_head(&self.buf[..end])?;
        let length = content_length(&request)?;
        let start = end + 4;
        if length > 0 && request.header("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) && self.buf.len() < start + length {
            proceed();
        }
        while self.buf.len() < start + length {
            if !self.fill()? {
                return Err(HttpError::Timeout);
            }
        }
        request.body = self.buf[start..start + length].to_vec();
        self.buf.drain(..start + length);
        Ok(Some(request))
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn parse_head(head: &[u8]) -> Result<Request, HttpError> {
    let text = std::str::from_utf8(head).map_err(|_| HttpError::Bad("head is not text"))?;
    let mut lines = text.split("\r\n");
    let line = lines.next().unwrap_or("");
    let mut parts = line.split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(HttpError::Bad("bad request line"));
    };
    if !is_token(method) {
        return Err(HttpError::Bad("bad method"));
    }
    if !target.starts_with('/') || target.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err(HttpError::Bad("bad target"));
    }
    let minor = match version {
        "HTTP/1.1" => 1,
        "HTTP/1.0" => 0,
        v if v.starts_with("HTTP/") => return Err(HttpError::VersionNotSupported),
        _ => return Err(HttpError::Bad("bad request line")),
    };
    let mut headers = Vec::new();
    for line in lines {
        if line.starts_with([' ', '\t']) {
            return Err(HttpError::Bad("folded header"));
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(HttpError::Bad("bad header"));
        };
        if !is_token(name) {
            return Err(HttpError::Bad("bad header name"));
        }
        if headers.len() >= MAX_HEADERS {
            return Err(HttpError::HeadTooLarge);
        }
        headers.push((name.to_ascii_lowercase(), value.trim_matches([' ', '\t']).to_owned()));
    }
    Ok(Request {
        method: method.to_owned(),
        target: target.to_owned(),
        minor,
        headers,
        body: Vec::new(),
    })
}

fn content_length(request: &Request) -> Result<usize, HttpError> {
    if request.header("transfer-encoding").is_some() {
        return Err(HttpError::LengthRequired);
    }
    let mut found: Option<usize> = None;
    for (name, value) in &request.headers {
        if name != "content-length" {
            continue;
        }
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return Err(HttpError::Bad("bad Content-Length"));
        }
        let n = value.parse::<usize>().map_err(|_| HttpError::BodyTooLarge)?;
        if found.is_some_and(|f| f != n) {
            return Err(HttpError::Bad("two Content-Lengths"));
        }
        found = Some(n);
    }
    let n = found.unwrap_or(0);
    if n > MAX_BODY {
        return Err(HttpError::BodyTooLarge);
    }
    Ok(n)
}

/// Writes a response with a JSON body (empty for 202).
pub fn write_response(
    out: &mut impl Write,
    status: u16,
    extra: &[(&str, &str)],
    body: &[u8],
    keep_alive: bool,
) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    head.push_str("Cache-Control: no-store\r\n");
    head.push_str(if keep_alive { "Connection: keep-alive\r\n" } else { "Connection: close\r\n" });
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let mut all = head.into_bytes();
    all.extend_from_slice(body);
    out.write_all(&all)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gives its bytes `step` at a time, then ends (or times out).
    struct Drip {
        data: Vec<u8>,
        at: usize,
        step: usize,
        then_timeout: bool,
    }

    impl Read for Drip {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.at >= self.data.len() {
                return if self.then_timeout {
                    Err(io::ErrorKind::TimedOut.into())
                } else {
                    Ok(0)
                };
            }
            let n = self.step.min(buf.len()).min(self.data.len() - self.at);
            buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }

    fn drip(text: &str, step: usize) -> Reader<Drip> {
        Reader::new(Drip { data: text.as_bytes().to_vec(), at: 0, step, then_timeout: false })
    }

    const POST: &str = "POST /mcp?x=1 HTTP/1.1\r\nHost: 127.0.0.1:1\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello";

    #[test]
    fn reads_a_request_however_it_arrives() {
        for step in [1, 2, 7, 4096] {
            let r = drip(POST, step).next_request().unwrap().unwrap();
            assert_eq!((r.method.as_str(), r.path(), r.body.as_slice()), ("POST", "/mcp", &b"hello"[..]));
            assert_eq!(r.header("host"), Some("127.0.0.1:1"));
            assert!(!r.keep_alive());
        }
    }

    #[test]
    fn reads_pipelined_requests_and_keep_alive() {
        let two = format!("{POST}\r\nGET / HTTP/1.1\r\n\r\n");
        let mut r = drip(&two, 3);
        assert_eq!(r.next_request().unwrap().unwrap().method, "POST");
        let get = r.next_request().unwrap().unwrap();
        assert_eq!(get.method, "GET");
        assert!(get.keep_alive());
        assert!(r.next_request().unwrap().is_none());
    }

    #[test]
    fn http_10_closes_unless_asked() {
        let r = drip("GET / HTTP/1.0\r\n\r\n", 9).next_request().unwrap().unwrap();
        assert!(!r.keep_alive());
        let r = drip("GET / HTTP/1.0\r\nConnection: Keep-Alive\r\n\r\n", 9).next_request().unwrap().unwrap();
        assert!(r.keep_alive());
    }

    #[test]
    fn a_half_request_is_an_error_an_idle_stream_is_not() {
        assert_eq!(drip("POST /mcp HT", 4).next_request().unwrap_err(), HttpError::Closed);
        assert_eq!(
            drip("POST /mcp HTTP/1.1\r\nContent-Length: 10\r\n\r\nabc", 4).next_request().unwrap_err(),
            HttpError::Closed
        );
        assert!(drip("", 4).next_request().unwrap().is_none());
        let mut idle = Reader::new(Drip { data: vec![], at: 0, step: 1, then_timeout: true });
        assert!(idle.next_request().unwrap().is_none());
        let mut slow = Reader::new(Drip { data: b"POST /mcp HT".to_vec(), at: 0, step: 4, then_timeout: true });
        assert_eq!(slow.next_request().unwrap_err(), HttpError::Timeout);
    }

    #[test]
    fn refuses_bad_input() {
        let cases: [(&str, HttpError); 10] = [
            ("GARBAGE\r\n\r\n", HttpError::Bad("bad request line")),
            ("GET / HTTP/1.1 extra\r\n\r\n", HttpError::Bad("bad request line")),
            ("GET nothing HTTP/1.1\r\n\r\n", HttpError::Bad("bad target")),
            ("GET / HTTP/2.0\r\n\r\n", HttpError::VersionNotSupported),
            ("GET / FTP/1.1\r\n\r\n", HttpError::Bad("bad request line")),
            ("GET / HTTP/1.1\r\nNoColon\r\n\r\n", HttpError::Bad("bad header")),
            ("GET / HTTP/1.1\r\n Folded: x\r\n\r\n", HttpError::Bad("folded header")),
            ("POST / HTTP/1.1\r\nContent-Length: -1\r\n\r\n", HttpError::Bad("bad Content-Length")),
            ("POST / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab", HttpError::Bad("two Content-Lengths")),
            ("POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n", HttpError::LengthRequired),
        ];
        for (text, want) in cases {
            assert_eq!(drip(text, 5).next_request().unwrap_err(), want, "{text:?}");
        }
    }

    #[test]
    fn refuses_oversize() {
        let big_body = format!("POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert_eq!(drip(&big_body, 4096).next_request().unwrap_err(), HttpError::BodyTooLarge);
        let huge = "POST / HTTP/1.1\r\nContent-Length: 99999999999999999999999\r\n\r\n";
        assert_eq!(drip(huge, 4096).next_request().unwrap_err(), HttpError::BodyTooLarge);
        let long_head = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD));
        assert_eq!(drip(&long_head, 4096).next_request().unwrap_err(), HttpError::HeadTooLarge);
        let endless = "a".repeat(MAX_HEAD * 2);
        assert_eq!(drip(&endless, 4096).next_request().unwrap_err(), HttpError::HeadTooLarge);
        let many = format!("GET / HTTP/1.1\r\n{}\r\n", "X: 1\r\n".repeat(MAX_HEADERS + 1));
        assert_eq!(drip(&many, 4096).next_request().unwrap_err(), HttpError::HeadTooLarge);
    }

    #[test]
    fn expect_continue_is_answered_once() {
        let text = "POST / HTTP/1.1\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\nok";
        let mut said = 0;
        let r = drip(text, 1).next_request_with(&mut || said += 1).unwrap().unwrap();
        assert_eq!((said, r.body.as_slice()), (1, &b"ok"[..]));
    }

    #[test]
    fn writes_a_response() {
        let mut out = Vec::new();
        write_response(&mut out, 405, &[("Allow", "POST")], b"{}", false).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"));
        assert!(text.contains("Content-Length: 2\r\n") && text.contains("Allow: POST\r\n"));
        assert!(text.contains("Connection: close\r\n") && text.ends_with("\r\n\r\n{}"));
    }
}
