//! Bounded local agent API client and MCP stdio facade. No shell, SQL or event append tools.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

pub const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_REPLY_BYTES: usize = 128 * 1024;
const DEADLINE: Duration = Duration::from_secs(2);
const PROTOCOL: &str = "2025-06-18";
const TOOL_NAMES: &[&str] = &[
    "task_context_get",
    "inbox_pending",
    "inbox_application_ack",
    "message_send",
    "inbox_ack_pending",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

fn fields(
    name: &str,
) -> Option<(
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
)> {
    match name {
        "task_context_get" => Some((
            "context",
            &["project_id", "task_id", "context_epoch"],
            &[
                "known_contract_digest",
                "max_estimated_tokens",
                "limit",
                "cursor_generation",
                "cursor_scope_digest",
                "cursor_after_message_id",
            ],
        )),
        "inbox_pending" => Some((
            "pending",
            &["project_id", "recipient", "context_epoch"],
            &[
                "limit",
                "cursor_generation",
                "cursor_scope_digest",
                "cursor_after_message_id",
            ],
        )),
        "inbox_application_ack" => Some((
            "application-ack",
            &[
                "project_id",
                "recipient",
                "context_epoch",
                "message_id",
                "producer_id",
                "incarnation",
                "sequence",
                "observed_utc",
            ],
            &[],
        )),
        "message_send" => Some((
            "send",
            &["request_id", "route_id", "payload"],
            &["causation_id"],
        )),
        "inbox_ack_pending" => Some((
            "ack-pending",
            &[
                "request_id",
                "project_id",
                "recipient",
                "context_epoch",
                "message_id",
            ],
            &[],
        )),
        _ => None,
    }
}

pub fn tool_catalog() -> Value {
    let tools: Vec<_> = TOOL_NAMES.iter().copied().map(|name| {
        let (_, required, optional) = fields(name).expect("fixed catalog");
        let properties: serde_json::Map<String, Value> = required.iter().chain(optional).map(|key| {
            let schema = match *key {
                "payload" => json!({"type":"object","description":"The versioned CoordinationMessage payload; validated by the native Rust contract.","required":["kind"],"properties":{"kind":{"enum":["task_assigned","question","answer","artifact_ready","handoff"]}}}),
                "limit" => json!({"type":"integer","minimum":1,"maximum":16}),
                "max_estimated_tokens" => json!({"type":"integer","minimum":1,"maximum":4096}),
                _ => json!({"type":"string","minLength":1,"maxLength":4096}),
            };
            ((*key).to_owned(), schema)
        }).collect();
        json!({"name":name,"description":match name {
            "task_context_get" => "Read the authorized task capsule and current pending questions at this context epoch.",
            "inbox_pending" => "Read a bounded authorized inbox page; retrieve the next page using its exact cursor.",
            "message_send" => "Send a typed message through a preinstalled exact task/recipient route. Preserve request_id for an exact retry; this cannot start or accept a task.",
            "inbox_ack_pending" => "Acknowledge application receipt in your separately granted own inbox. Preserve request_id for exact retries; this does not answer or accept work.",
            _ => "Record application receipt only; this does not answer a question or grant execution authority."
        },"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}})
    }).collect();
    json!({"tools":tools})
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

impl ToolCall {
    fn endpoint(&self) -> io::Result<&'static str> {
        admiralty_core::limits::bounded_json_size(self, MAX_REQUEST_BYTES)
            .map_err(|_| invalid("agent request exceeds limit"))?;
        let (path, required, optional) =
            fields(&self.name).ok_or_else(|| invalid("unknown agent tool"))?;
        let args = self
            .arguments
            .as_object()
            .ok_or_else(|| invalid("tool arguments must be an object"))?;
        if required.iter().any(|key| !args.contains_key(*key)) {
            return Err(invalid("missing tool argument"));
        }
        for (key, value) in args {
            if !required.contains(&key.as_str()) && !optional.contains(&key.as_str()) {
                return Err(invalid("unknown tool argument"));
            }
            let valid = match key.as_str() {
                "payload" => {
                    serde_json::from_value::<admiralty_core::MessagePayload>(value.clone()).is_ok()
                }
                "limit" => value.as_u64().is_some_and(|n| (1..=16).contains(&n)),
                "max_estimated_tokens" => value.as_u64().is_some_and(|n| (1..=4096).contains(&n)),
                _ => value
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && s.len() <= 4096 && !s.contains('\0')),
            };
            if !valid {
                return Err(invalid("invalid tool argument"));
            }
        }
        Ok(path)
    }
}

/// Credential is kept in memory and is never included in errors or command arguments.
pub struct AgentClient {
    server: SocketAddr,
    token: String,
}
impl AgentClient {
    pub fn new(server: SocketAddr, token: String) -> io::Result<Self> {
        if !server.ip().is_loopback()
            || !(32..=4096).contains(&token.len())
            || !token.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(invalid(
                "loopback address and valid agent credential required",
            ));
        }
        Ok(Self { server, token })
    }
    pub fn call(&self, call: &ToolCall) -> io::Result<Value> {
        let endpoint = call.endpoint()?;
        let mutation = matches!(endpoint, "application-ack" | "send" | "ack-pending");
        let body = if mutation {
            serde_json::to_vec(&call.arguments)?
        } else {
            Vec::new()
        };
        let query = if mutation {
            String::new()
        } else {
            let args = call
                .arguments
                .as_object()
                .ok_or_else(|| invalid("invalid arguments"))?;
            format!(
                "?{}",
                args.iter()
                    .map(|(key, value)| {
                        let value = value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string());
                        format!("{key}={}", percent_encode(value.as_bytes()))
                    })
                    .collect::<Vec<_>>()
                    .join("&")
            )
        };
        let deadline = Instant::now() + DEADLINE;
        let mut stream = TcpStream::connect_timeout(&self.server, DEADLINE)?;
        let request = format!(
            "{} /v2/agents/inbox/{endpoint}{query} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            if mutation { "POST" } else { "GET" },
            self.server,
            self.token,
            body.len()
        );
        let mut bytes = request.into_bytes();
        bytes.extend(body);
        let mut pending = bytes.as_slice();
        while !pending.is_empty() {
            stream.set_write_timeout(Some(remaining(deadline)?))?;
            let count = stream.write(pending)?;
            if count == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            pending = &pending[count..];
        }
        let mut reply = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            stream.set_read_timeout(Some(remaining(deadline)?))?;
            let count = stream.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            if reply.len() + count > MAX_REPLY_BYTES {
                return Err(invalid("agent reply exceeds limit"));
            }
            reply.extend_from_slice(&buffer[..count]);
        }
        decode_reply(&reply)
    }
}
fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::ErrorKind::TimedOut.into())
}
fn percent_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(b) {
                char::from(*b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn decode_reply(reply: &[u8]) -> io::Result<Value> {
    let split = reply
        .windows(4)
        .position(|s| s == b"\r\n\r\n")
        .filter(|n| *n <= 8192)
        .ok_or_else(|| invalid("invalid agent response headers"))?;
    let headers =
        std::str::from_utf8(&reply[..split]).map_err(|_| invalid("invalid agent response"))?;
    let mut lines = headers.split("\r\n");
    if lines.next().and_then(|line| line.split_whitespace().nth(1)) != Some("200") {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "agent API rejected the request; reconcile before retrying a mutation",
        ));
    }
    let mut length = None;
    for line in lines {
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| invalid("invalid response header"))?;
        if key.eq_ignore_ascii_case("transfer-encoding") {
            return Err(invalid("unsupported agent response framing"));
        }
        if key.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return Err(invalid("ambiguous response length"));
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| invalid("invalid response length"))?,
            );
        }
    }
    let body = &reply[split + 4..];
    if length != Some(body.len()) {
        return Err(invalid("incomplete agent reply"));
    }
    serde_json::from_slice(body).map_err(io::Error::other)
}

/// Each connection negotiates independently. Uncertified provider clients use CLI JSON.
#[derive(Default)]
pub struct McpSession {
    initialized: bool,
    ready: bool,
}
impl McpSession {
    pub fn handle(
        &mut self,
        request: Value,
        call: impl FnOnce(&ToolCall) -> io::Result<Value>,
    ) -> Option<Value> {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let error = |code, message| json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}});
        if admiralty_core::limits::bounded_json_size(&request, MAX_REQUEST_BYTES).is_err()
            || request.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || !request.is_object()
        {
            return Some(error(-32600, "Invalid bounded JSON-RPC request"));
        }
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        if request.get("id").is_none() {
            if method == "notifications/initialized" && self.initialized {
                self.ready = true;
            }
            return None;
        }
        if !(id.is_i64()
            || id.is_u64()
            || id.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 256))
        {
            return Some(error(-32600, "Invalid request ID"));
        }
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = match method {
            "initialize" if !self.initialized => {
                if params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .is_none()
                    || !params.get("capabilities").is_some_and(Value::is_object)
                    || !params.get("clientInfo").is_some_and(Value::is_object)
                {
                    return Some(error(-32602, "Invalid initialization parameters"));
                }
                self.initialized = true;
                json!({"protocolVersion":PROTOCOL,"capabilities":{"tools":{}},"serverInfo":{"name":"admiralty-agent-tools","version":env!("CARGO_PKG_VERSION")}})
            }
            "ping" => json!({}),
            _ if !self.ready => {
                return Some(error(
                    -32000,
                    "Initialize this MCP connection before calling tools",
                ));
            }
            "tools/list" => {
                if !params.as_object().is_some_and(|p| p.is_empty()) {
                    return Some(error(-32602, "Tool catalog has no continuation cursor"));
                }
                tool_catalog()
            }
            "tools/call" => {
                let tool: ToolCall = match serde_json::from_value(params) {
                    Ok(value) => value,
                    Err(_) => return Some(error(-32602, "Invalid tool call")),
                };
                if tool.endpoint().is_err() {
                    return Some(error(-32602, "Unknown tool or invalid arguments"));
                }
                match call(&tool) {
                    Ok(value) => {
                        json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false})
                    }
                    Err(_) => {
                        json!({"content":[{"type":"text","text":"Agent operation failed; refresh context and reconcile any message or application ACK before retrying."}],"isError":true})
                    }
                }
            }
            _ => return Some(error(-32601, "Unknown method")),
        };
        Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }
}

pub fn serve_stdio(
    client: &AgentClient,
    mut input: impl BufRead,
    mut output: impl Write,
) -> io::Result<()> {
    let mut session = McpSession::default();
    loop {
        let mut line = Vec::new();
        let count = input
            .by_ref()
            .take((MAX_REQUEST_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if count == 0 {
            return Ok(());
        }
        if line.len() > MAX_REQUEST_BYTES {
            return Err(invalid("MCP frame exceeds limit"));
        }
        let response = match serde_json::from_slice(&line) {
            Ok(request) => session.handle(request, |tool| client.call(tool)),
            Err(_) => Some(
                json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}),
            ),
        };
        if let Some(response) = response {
            admiralty_core::limits::bounded_json_size(&response, MAX_REPLY_BYTES * 2)
                .map_err(|_| invalid("MCP reply exceeds limit"))?;
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::TcpListener,
        sync::atomic::{AtomicUsize, Ordering},
        thread,
    };

    fn context() -> ToolCall {
        ToolCall {
            name: "task_context_get".into(),
            arguments: json!({"project_id":"project","task_id":"task","context_epoch":"epoch &next=foreign"}),
        }
    }
    fn ready() -> McpSession {
        let mut session = McpSession::default();
        let initialized = session.handle(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":PROTOCOL,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}), |_| panic!("initialization must not access the backend")).expect("initialize response");
        assert_eq!(initialized["result"]["protocolVersion"], PROTOCOL);
        assert_eq!(initialized["result"]["capabilities"], json!({"tools":{}}));
        assert!(
            session
                .handle(
                    json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                    |_| panic!("notification must not access backend")
                )
                .is_none()
        );
        session
    }
    #[test]
    fn mcp_requires_per_connection_initialization_and_has_only_narrow_tools() {
        let mut fresh = McpSession::default();
        assert!(
            fresh
                .handle(
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
                    |_| panic!("no backend access")
                )
                .expect("error")
                .get("error")
                .is_some()
        );
        let mut session = ready();
        let listed = session
            .handle(
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                |_| panic!("catalog is local"),
            )
            .expect("catalog");
        let names: Vec<_> = listed["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|tool| tool["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, TOOL_NAMES);
        for arguments in [
            json!({"project_id":"project","task_id":"task","context_epoch":"epoch","sql":"SELECT *"}),
            json!({"project_id":"project","task_id":"task","context_epoch":"epoch","limit":17}),
        ] {
            let reply = session.handle(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"task_context_get","arguments":arguments}}), |_| panic!("invalid input must not execute")).expect("rejection");
            assert_eq!(reply["error"]["code"], -32602);
        }
        let reply = session.handle(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"shell","arguments":{}}}), |_| panic!("no shell backend")).expect("rejection");
        assert_eq!(reply["error"]["code"], -32602);
    }
    #[test]
    fn mcp_preserves_cli_arguments_and_exact_results_without_retries() {
        let tool = context();
        let counter = AtomicUsize::new(0);
        let result = json!({"context_epoch":"epoch &next=foreign","contract_digest":"sha256:fixture","revision":"9007199254740993"});
        let reply = ready()
            .handle(
                json!({"jsonrpc":"2.0","id":"call","method":"tools/call","params":tool}),
                |received| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(received.arguments, context().arguments);
                    Ok(result.clone())
                },
            )
            .expect("tool reply");
        assert_eq!(reply["result"]["structuredContent"], result);
        assert_eq!(
            serde_json::from_str::<Value>(
                reply["result"]["content"][0]["text"]
                    .as_str()
                    .expect("JSON text")
            )
            .expect("structured parity"),
            result
        );
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        let failed = ready()
            .handle(
                json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":context()}),
                |_| Err(io::Error::other("private credential must not appear")),
            )
            .expect("failure reply");
        assert_eq!(failed["result"]["isError"], true);
        assert!(!failed.to_string().contains("private credential"));
    }
    #[test]
    fn local_http_client_encodes_scope_and_requires_complete_bounded_json() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("owned listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("client connect");
            stream.set_read_timeout(Some(DEADLINE)).expect("timeout");
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                assert!(request.len() < MAX_REQUEST_BYTES * 4);
                stream.read_exact(&mut byte).expect("request header");
                request.push(byte[0]);
            }
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\nConnection: close\r\n\r\n{\"observed\":true}").expect("response");
            String::from_utf8(request).expect("wire request")
        });
        let client =
            AgentClient::new(address, "test-token-with-sufficient-entropy".into()).expect("client");
        assert_eq!(
            client.call(&context()).expect("exact JSON response"),
            json!({"observed":true})
        );
        let wire = server.join().expect("server completed");
        assert!(wire.starts_with("GET /v2/agents/inbox/context?context_epoch=epoch%20%26next%3Dforeign&project_id=project&task_id=task HTTP/1.1\r\n"));
        assert!(wire.contains("Authorization: Bearer test-token-with-sufficient-entropy\r\n"));
        assert!(
            AgentClient::new(
                "192.0.2.1:80".parse().expect("address"),
                "test-token-with-sufficient-entropy".into()
            )
            .is_err()
        );
        assert!(
            AgentClient::new(address, "injected\r\nheader-with-sufficient-entropy".into()).is_err()
        );
        assert!(decode_reply(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{}").is_err());
        assert!(decode_reply(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{}").is_err());
        assert!(decode_reply(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 2\r\n\r\n{}").is_err());
    }
    #[test]
    fn native_message_tools_preserve_structured_payloads_and_use_post_without_query_strings() {
        for (name, endpoint, arguments) in [
            (
                "message_send",
                "send",
                json!({"request_id":"4ab239fa-ef42-4ef8-8724-4859d4db7822","route_id":"captain-route","payload":{"kind":"handoff","task_id":"task","candidate_ref":"commit:abc","next_action":"Do NOT merge until CI passes"}}),
            ),
            (
                "inbox_ack_pending",
                "ack-pending",
                json!({"request_id":"d474212d-c8de-4b71-9699-a949ae4b187c","project_id":"project","recipient":"captain","context_epoch":"epoch-1","message_id":"native-message"}),
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").expect("owned listener");
            let address = listener.local_addr().expect("address");
            let expected = arguments.clone();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().expect("client connect");
                stream.set_read_timeout(Some(DEADLINE)).expect("timeout");
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") {
                    assert!(header.len() < MAX_REQUEST_BYTES);
                    stream.read_exact(&mut byte).expect("request header");
                    header.push(byte[0]);
                }
                let header = String::from_utf8(header).expect("HTTP headers");
                assert!(
                    header.starts_with(&format!("POST /v2/agents/inbox/{endpoint} HTTP/1.1\r\n"))
                );
                let length = header
                    .lines()
                    .find_map(|line| line.strip_prefix("Content-Length: "))
                    .expect("length")
                    .parse::<usize>()
                    .expect("body size");
                assert!(length <= MAX_REQUEST_BYTES);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).expect("complete body");
                assert_eq!(
                    serde_json::from_slice::<Value>(&body).expect("typed payload"),
                    expected
                );
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 18\r\nConnection: close\r\n\r\n{\"duplicate\":true}").expect("receipt");
            });
            let client = AgentClient::new(address, "test-token-with-sufficient-entropy".into())
                .expect("client");
            let response = ready().handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}),|tool| client.call(tool)).expect("MCP reply");
            assert_eq!(
                response["result"]["structuredContent"],
                json!({"duplicate":true})
            );
            server.join().expect("server done");
        }
        for arguments in [
            json!({"request_id":"request","route_id":"route","sender":"owner","payload":{"kind":"handoff","task_id":"task","candidate_ref":"candidate","next_action":"review"}}),
            json!({"request_id":"request","route_id":"route","payload":{"kind":"execute","shell":"command"}}),
        ] {
            let rejected = ready().handle(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"message_send","arguments":arguments}}), |_| panic!("invalid commands must not access the server")).expect("rejection");
            assert_eq!(rejected["error"]["code"], -32602);
        }
    }
    #[test]
    fn stdio_rejects_oversized_frames_before_backend_access() {
        let client = AgentClient::new(
            "127.0.0.1:1".parse().expect("address"),
            "test-token-with-sufficient-entropy".into(),
        )
        .expect("client");
        let mut output = Vec::new();
        assert!(
            serve_stdio(
                &client,
                io::Cursor::new(vec![b' '; MAX_REQUEST_BYTES + 1]),
                &mut output
            )
            .is_err()
        );
        assert!(output.is_empty());
    }
}
