//! A minimal blocking HTTP/1.1 client with a whole-request deadline.
//!
//! The response parser follows CPython 3.13 `http.client` framing rules
//! (status line, `email`-style header parsing, `Content-Length`, chunked and
//! close-delimited bodies). Every blocking read or write is bounded by the
//! remaining request time and re-checks the deadline afterwards, and an
//! independent watchdog shuts the connection down at the deadline, so a
//! slow-dripping peer cannot extend a request.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::py::{parse_int, split_whitespace};

const MAXLINE: usize = 65536;
const MAXHEADERS: usize = 100;
const BUFFER: usize = 8192;

/// Seconds on a process-wide monotonic clock (`time.monotonic()`).
pub fn monotonic() -> f64 {
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    ANCHOR.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// A connected byte stream.
pub(crate) trait Wire: Read + Write + Send {
    /// Bound the next blocking read/write.
    fn set_io_timeout(&mut self, timeout: Duration) -> io::Result<()>;
    /// A handle that shuts the stream down from another thread.
    fn shutdown_handle(&self) -> io::Result<Box<dyn FnOnce() + Send>>;
}

impl Wire for TcpStream {
    fn set_io_timeout(&mut self, timeout: Duration) -> io::Result<()> {
        let timeout = timeout.max(Duration::from_micros(1));
        self.set_read_timeout(Some(timeout))?;
        self.set_write_timeout(Some(timeout))
    }

    fn shutdown_handle(&self) -> io::Result<Box<dyn FnOnce() + Send>> {
        let clone = self.try_clone()?;
        Ok(Box::new(move || {
            let _ = clone.shutdown(Shutdown::Both);
        }))
    }
}

/// A cancellable timer.
pub(crate) trait Watchdog {
    /// Cancel before it fires.
    fn cancel(self: Box<Self>);
}

/// Clock, connector and timer used by [`exchange`]; replaceable in tests.
pub(crate) trait HttpRuntime {
    /// `time.monotonic()`.
    fn monotonic(&self) -> f64;
    /// `socket.create_connection((host, port), timeout)`.
    fn connect(&self, host: &str, port: u16, timeout: Duration) -> io::Result<Box<dyn Wire>>;
    /// `threading.Timer(delay, abort).start()`.
    fn start_watchdog(&self, delay: f64, abort: Box<dyn FnOnce() + Send>) -> Box<dyn Watchdog>;
}

/// The real clock, TCP connector and timer thread.
pub(crate) struct SystemRuntime;

struct ThreadWatchdog {
    cancel: mpsc::Sender<()>,
}

impl Watchdog for ThreadWatchdog {
    fn cancel(self: Box<Self>) {
        let _ = self.cancel.send(());
    }
}

impl HttpRuntime for SystemRuntime {
    fn monotonic(&self) -> f64 {
        monotonic()
    }

    fn connect(&self, host: &str, port: u16, timeout: Duration) -> io::Result<Box<dyn Wire>> {
        let timeout = timeout.max(Duration::from_micros(1));
        let mut last_error =
            io::Error::new(io::ErrorKind::NotFound, "getaddrinfo returns an empty list");
        let addresses: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
        for address in addresses {
            match TcpStream::connect_timeout(&address, timeout) {
                Ok(stream) => return Ok(Box::new(stream)),
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    }

    fn start_watchdog(&self, delay: f64, abort: Box<dyn FnOnce() + Send>) -> Box<dyn Watchdog> {
        let (sender, receiver) = mpsc::channel::<()>();
        let delay = Duration::from_secs_f64(delay.max(0.0));
        std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = receiver.recv_timeout(delay) {
                abort();
            }
        });
        Box::new(ThreadWatchdog { cancel: sender })
    }
}

/// Why an exchange failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HttpFailure {
    /// The request deadline elapsed (`check_deadline`).
    Deadline,
    /// `OSError` / `http.client.HTTPException` / `TimeoutError`.
    Transport(String),
    /// `Content-Length` was not ASCII decimal digits.
    InvalidContentLength,
    /// Declared or received body exceeded the byte bound.
    TooLarge,
    /// EOF before the declared `Content-Length`.
    Truncated,
}

/// A complete response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// Headers with lowercased names (last occurrence wins).
    pub headers: BTreeMap<String, String>,
    /// Body bytes.
    pub body: Vec<u8>,
}

/// One request description.
pub(crate) struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    /// Value of the `Host` header.
    pub host_header: String,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: Option<&'a [u8]>,
}

struct Deadline<'a> {
    runtime: &'a dyn HttpRuntime,
    at: f64,
    expired: Arc<AtomicBool>,
}

impl Deadline<'_> {
    fn check(&self) -> Result<f64, HttpFailure> {
        let remaining = self.at - self.runtime.monotonic();
        if self.expired.load(Ordering::SeqCst) || remaining <= 0.0 {
            return Err(HttpFailure::Deadline);
        }
        Ok(remaining)
    }
}

fn transport(error: impl std::fmt::Display) -> HttpFailure {
    HttpFailure::Transport(error.to_string())
}

/// `http.client` `fp`: a buffered reader whose every raw read is bounded by
/// the remaining request time and followed by a deadline check.
struct BoundedReader<'a> {
    wire: Box<dyn Wire>,
    deadline: &'a Deadline<'a>,
    buffer: Vec<u8>,
    position: usize,
}

impl BoundedReader<'_> {
    fn raw_read(&mut self, into: &mut [u8]) -> Result<usize, HttpFailure> {
        let remaining = self.deadline.check()?;
        self.wire
            .set_io_timeout(Duration::from_secs_f64(remaining))
            .map_err(transport)?;
        let count = self.wire.read(into).map_err(transport)?;
        self.deadline.check()?;
        Ok(count)
    }

    fn fill(&mut self) -> Result<bool, HttpFailure> {
        if self.position < self.buffer.len() {
            return Ok(true);
        }
        let mut chunk = vec![0u8; BUFFER];
        let count = self.raw_read(&mut chunk)?;
        chunk.truncate(count);
        self.buffer = chunk;
        self.position = 0;
        Ok(count > 0)
    }

    /// `readline(limit)`.
    fn readline(&mut self, limit: usize) -> Result<Vec<u8>, HttpFailure> {
        let mut line = Vec::new();
        while line.len() < limit {
            if !self.fill()? {
                break;
            }
            let available = &self.buffer[self.position..];
            let wanted = limit - line.len();
            let take = match available.iter().position(|&byte| byte == b'\n') {
                Some(newline) if newline < wanted => newline + 1,
                _ => available.len().min(wanted),
            };
            line.extend_from_slice(&available[..take]);
            self.position += take;
            if line.last() == Some(&b'\n') {
                break;
            }
        }
        Ok(line)
    }

    /// `read1(n)`: buffered bytes, or at most one raw read.
    fn read1(&mut self, amount: usize) -> Result<Vec<u8>, HttpFailure> {
        if amount == 0 {
            return Ok(Vec::new());
        }
        if self.position < self.buffer.len() {
            let take = amount.min(self.buffer.len() - self.position);
            let chunk = self.buffer[self.position..self.position + take].to_vec();
            self.position += take;
            return Ok(chunk);
        }
        let mut chunk = vec![0u8; amount];
        let count = self.raw_read(&mut chunk)?;
        chunk.truncate(count);
        Ok(chunk)
    }

    /// `_safe_read(n)`: exactly `n` bytes or `IncompleteRead`.
    fn safe_read(&mut self, amount: usize) -> Result<Vec<u8>, HttpFailure> {
        let mut out = Vec::with_capacity(amount);
        while out.len() < amount {
            let chunk = self.read1(amount - out.len())?;
            if chunk.is_empty() {
                return Err(HttpFailure::Transport("IncompleteRead".into()));
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// Lines split like `email.feedparser` (`\r\n`, `\r` or `\n`), ends kept.
fn crack_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                let end = if bytes.get(index + 1) == Some(&b'\n') {
                    index + 2
                } else {
                    index + 1
                };
                lines.push(&text[start..end]);
                start = end;
                index = end;
            }
            b'\n' => {
                lines.push(&text[start..=index]);
                start = index + 1;
                index += 1;
            }
            _ => index += 1,
        }
    }
    if start < bytes.len() {
        lines.push(&text[start..]);
    }
    lines
}

fn header_line_matches(line: &str) -> bool {
    if line.starts_with("From ") || line.starts_with([' ', '\t']) {
        return true;
    }
    for character in line.chars() {
        if character == ':' {
            return true;
        }
        let code = character as u32;
        if !(0x21..=0x7e).contains(&code) {
            return false;
        }
    }
    false
}

/// `email.parser.Parser(policy=compat32).parsestr(block)` header items.
fn parse_header_block(block: &str) -> Vec<(String, String)> {
    let mut lines = Vec::new();
    for line in crack_lines(block) {
        if !header_line_matches(line) {
            break;
        }
        lines.push(line);
    }
    let mut headers = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let flush = |current: &mut Vec<&str>, headers: &mut Vec<(String, String)>| {
        if let Some((name, value)) = current.first().and_then(|first| first.split_once(':')) {
            let mut value = value.trim_start_matches([' ', '\t']).to_owned();
            for continuation in &current[1..] {
                value.push_str(continuation);
            }
            let value = value.trim_end_matches(['\r', '\n']).to_owned();
            headers.push((name.to_owned(), value));
        }
        current.clear();
    };
    for line in lines {
        if line.starts_with([' ', '\t']) {
            if current.is_empty() {
                continue;
            }
            current.push(line);
            continue;
        }
        flush(&mut current, &mut headers);
        if line.starts_with("From ") {
            // A unix-from envelope, trailing body line or misplaced envelope
            // header; none of them becomes a header.
            continue;
        }
        match line.find(':') {
            Some(0) | None => continue,
            Some(_) => current.push(line),
        }
    }
    flush(&mut current, &mut headers);
    headers
}

fn first_header<'h>(headers: &'h [(String, String)], name: &str) -> Option<&'h str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn all_headers(headers: &[(String, String)], name: &str) -> Option<String> {
    let values: Vec<&str> = headers
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
        .collect();
    (!values.is_empty()).then(|| values.join(", "))
}

struct Response<'a> {
    reader: BoundedReader<'a>,
    closed: bool,
    chunked: bool,
    chunk_left: Option<i128>,
    length: Option<i128>,
}

impl Response<'_> {
    fn read_status(reader: &mut BoundedReader<'_>) -> Result<u16, HttpFailure> {
        let raw = reader.readline(MAXLINE + 1)?;
        if raw.len() > MAXLINE {
            return Err(transport("LineTooLong: status line"));
        }
        let line = latin1(&raw);
        if line.is_empty() {
            return Err(transport(
                "RemoteDisconnected: Remote end closed connection without response",
            ));
        }
        let mut parts = split_whitespace(&line, 2);
        if parts.len() < 3 {
            parts = split_whitespace(&line, 1);
        }
        let version = if parts.len() >= 2 { parts[0] } else { "" };
        if !version.starts_with("HTTP/") {
            return Err(transport(format!("BadStatusLine: {line:?}")));
        }
        let status = parse_int(parts[1], 10, false)
            .filter(|status| (100..=999).contains(status))
            .ok_or_else(|| transport(format!("BadStatusLine: {line:?}")))?;
        if !(version == "HTTP/1.0" || version == "HTTP/0.9" || version.starts_with("HTTP/1.")) {
            return Err(transport(format!("UnknownProtocol: {version}")));
        }
        u16::try_from(status).map_err(transport)
    }

    fn read_header_lines(reader: &mut BoundedReader<'_>) -> Result<Vec<u8>, HttpFailure> {
        let mut block = Vec::new();
        let mut count = 0;
        loop {
            let line = reader.readline(MAXLINE + 1)?;
            if line.len() > MAXLINE {
                return Err(transport("LineTooLong: header line"));
            }
            count += 1;
            if count > MAXHEADERS {
                return Err(transport(format!("got more than {MAXHEADERS} headers")));
            }
            let end = line.is_empty() || line == b"\r\n" || line == b"\n";
            block.extend_from_slice(&line);
            if end {
                return Ok(block);
            }
        }
    }

    fn next_chunk_size(&mut self) -> Result<i128, HttpFailure> {
        let line = self.reader.readline(MAXLINE + 1)?;
        if line.len() > MAXLINE {
            return Err(transport("LineTooLong: chunk size"));
        }
        let line = match line.iter().position(|&byte| byte == b';') {
            Some(position) => &line[..position],
            None => &line[..],
        };
        let text = latin1(line);
        match parse_int(&text, 16, true) {
            Some(size) if size >= 0 => Ok(size),
            _ => {
                self.closed = true;
                Err(transport("IncompleteRead: invalid chunk size"))
            }
        }
    }

    fn chunk_left(&mut self) -> Result<Option<i128>, HttpFailure> {
        let mut left = self.chunk_left;
        if left.unwrap_or(0) == 0 {
            if left.is_some() {
                self.reader.safe_read(2)?;
            }
            let size = self.next_chunk_size()?;
            if size == 0 {
                loop {
                    let line = self.reader.readline(MAXLINE + 1)?;
                    if line.len() > MAXLINE {
                        return Err(transport("LineTooLong: trailer line"));
                    }
                    if line.is_empty() || line == b"\r\n" || line == b"\n" {
                        break;
                    }
                }
                self.closed = true;
                left = None;
            } else {
                left = Some(size);
            }
            self.chunk_left = left;
        }
        Ok(left)
    }

    /// `HTTPResponse.read1(n)`.
    fn read1(&mut self, amount: usize) -> Result<Vec<u8>, HttpFailure> {
        if self.closed {
            return Ok(Vec::new());
        }
        if self.chunked {
            let Some(left) = self.chunk_left()? else {
                return Ok(Vec::new());
            };
            if amount == 0 {
                return Ok(Vec::new());
            }
            let wanted = usize::try_from(left).map_or(amount, |left| amount.min(left));
            let chunk = self.reader.read1(wanted)?;
            self.chunk_left = Some(left - chunk.len() as i128);
            if chunk.is_empty() {
                return Err(transport("IncompleteRead"));
            }
            return Ok(chunk);
        }
        let mut wanted = amount;
        if let Some(length) = self.length {
            wanted = usize::try_from(length).map_or(amount, |length| amount.min(length));
        }
        let chunk = self.reader.read1(wanted)?;
        if chunk.is_empty() && wanted > 0 {
            self.closed = true;
        } else if let Some(length) = self.length {
            let left = length - chunk.len() as i128;
            self.length = Some(left);
            if left == 0 {
                self.closed = true;
            }
        }
        Ok(chunk)
    }
}

fn request_bytes(request: &Request<'_>) -> Vec<u8> {
    let mut head = format!("{} {} HTTP/1.1\r\n", request.method, request.path);
    let names: Vec<String> = request
        .headers
        .iter()
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect();
    let has = |name: &str| names.iter().any(|candidate| candidate == name);
    if !has("host") {
        head.push_str(&format!("Host: {}\r\n", request.host_header));
    }
    if !has("accept-encoding") {
        head.push_str("Accept-Encoding: identity\r\n");
    }
    if !has("content-length") && !has("transfer-encoding") {
        match request.body {
            Some(body) => head.push_str(&format!("Content-Length: {}\r\n", body.len())),
            None if matches!(
                request.method.to_ascii_uppercase().as_str(),
                "PATCH" | "POST" | "PUT"
            ) =>
            {
                head.push_str("Content-Length: 0\r\n");
            }
            None => {}
        }
    }
    for (name, value) in request.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let mut bytes = head.into_bytes();
    if let Some(body) = request.body {
        bytes.extend_from_slice(body);
    }
    bytes
}

/// Perform one request on a fresh connection with the whole-request deadline
/// `request_deadline` (monotonic seconds) and a body bound of `max_body`.
pub(crate) fn exchange(
    runtime: &dyn HttpRuntime,
    connect_host: &str,
    port: u16,
    request: &Request<'_>,
    request_deadline: f64,
    max_body: usize,
) -> Result<HttpResponse, HttpFailure> {
    let expired = Arc::new(AtomicBool::new(false));
    let deadline = Deadline {
        runtime,
        at: request_deadline,
        expired: Arc::clone(&expired),
    };
    let remaining = request_deadline - runtime.monotonic();
    if remaining <= 0.0 {
        return Err(HttpFailure::Deadline);
    }
    let mut wire = runtime
        .connect(connect_host, port, Duration::from_secs_f64(remaining))
        .map_err(transport)?;
    deadline.check()?;
    let shutdown = wire.shutdown_handle().map_err(transport)?;
    let abort_flag = Arc::clone(&expired);
    let watchdog = runtime.start_watchdog(
        (request_deadline - runtime.monotonic()).max(0.001),
        Box::new(move || {
            // Shutdown may surface as EOF; the flag is checked after all I/O.
            abort_flag.store(true, Ordering::SeqCst);
            shutdown();
        }),
    );
    let result = (|| {
        let remaining = deadline.check()?;
        wire.set_io_timeout(Duration::from_secs_f64(remaining))
            .map_err(transport)?;
        wire.write_all(&request_bytes(request)).map_err(transport)?;
        wire.flush().map_err(transport)?;
        deadline.check()?;
        let mut reader = BoundedReader {
            wire,
            deadline: &deadline,
            buffer: Vec::new(),
            position: 0,
        };
        let status = loop {
            let status = Response::read_status(&mut reader)?;
            if status != 100 {
                break status;
            }
            Response::read_header_lines(&mut reader)?;
        };
        let block = Response::read_header_lines(&mut reader)?;
        let headers = parse_header_block(&latin1(&block));
        let chunked = first_header(&headers, "transfer-encoding")
            .is_some_and(|value| value.to_lowercase() == "chunked");
        let mut length = None;
        if !chunked
            && let Some(value) = first_header(&headers, "content-length").filter(|v| !v.is_empty())
        {
            length = parse_int(value, 10, false).filter(|length| *length >= 0);
        }
        if status == 204
            || status == 304
            || (100..200).contains(&status)
            || request.method == "HEAD"
        {
            length = Some(0);
        }
        deadline.check()?;
        if let Some(size) = all_headers(&headers, "Content-Length") {
            if size.is_empty() || !size.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(HttpFailure::InvalidContentLength);
            }
            let declared = size.trim_start_matches('0');
            let too_large = declared.len() > 20
                || (!declared.is_empty()
                    && declared
                        .parse::<u128>()
                        .map_or(true, |n| n > max_body as u128));
            if too_large {
                return Err(HttpFailure::TooLarge);
            }
        }
        let expected_length = length;
        let mut response = Response {
            reader,
            closed: request.method == "HEAD",
            chunked,
            chunk_left: None,
            length,
        };
        let mut data = Vec::new();
        loop {
            deadline.check()?;
            let amount = (16 * 1024).min(max_body + 1 - data.len());
            let chunk = if response.closed {
                Vec::new()
            } else {
                response.read1(amount)?
            };
            deadline.check()?;
            if chunk.is_empty() {
                if expected_length.is_some_and(|expected| expected != data.len() as i128) {
                    return Err(HttpFailure::Truncated);
                }
                break;
            }
            data.extend_from_slice(&chunk);
            if data.len() > max_body {
                return Err(HttpFailure::TooLarge);
            }
        }
        deadline.check()?;
        let headers = headers
            .into_iter()
            .map(|(name, value)| (name.to_lowercase(), value))
            .collect();
        Ok(HttpResponse {
            status,
            headers,
            body: data,
        })
    })();
    watchdog.cancel();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_block_follows_email_parser_rules() {
        let headers =
            parse_header_block("Content-Length: 2 \r\nX: a\r\n b\r\nbad line\r\nY: z\r\n\r\n");
        assert_eq!(
            headers,
            vec![
                ("Content-Length".to_owned(), "2 ".to_owned()),
                ("X".to_owned(), "a\r\n b".to_owned()),
            ]
        );
        let repeated = parse_header_block("Content-Length: 1\r\ncontent-length: 2\r\n\r\n");
        assert_eq!(
            all_headers(&repeated, "Content-Length").as_deref(),
            Some("1, 2")
        );
        assert_eq!(first_header(&repeated, "content-length"), Some("1"));
    }

    #[test]
    fn request_bytes_match_http_client_layout() {
        let request = Request {
            method: "PUT",
            path: "/v1/models",
            host_header: "127.0.0.1:1234".into(),
            headers: &[("Content-Type", "application/json")],
            body: Some(b"{}"),
        };
        assert_eq!(
            String::from_utf8_lossy(&request_bytes(&request)),
            "PUT /v1/models HTTP/1.1\r\nHost: 127.0.0.1:1234\r\nAccept-Encoding: identity\r\nContent-Length: 2\r\nContent-Type: application/json\r\n\r\n{}"
        );
        let get = Request {
            method: "GET",
            path: "/healthz",
            host_header: "[::1]:9".into(),
            headers: &[],
            body: None,
        };
        assert_eq!(
            String::from_utf8_lossy(&request_bytes(&get)),
            "GET /healthz HTTP/1.1\r\nHost: [::1]:9\r\nAccept-Encoding: identity\r\n\r\n"
        );
    }
}
