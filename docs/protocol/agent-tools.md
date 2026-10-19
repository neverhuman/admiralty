# Narrow agent tools

`admiralty agent-tools list` prints the same closed tool catalog exposed by the Rust MCP stdio facade:

| Tool | Existing authenticated API | Result |
| --- | --- | --- |
| `task_context_get` | `GET /v2/agents/inbox/context` | Bounded task capsule, current digest, pending questions and artifact references |
| `inbox_pending` | `GET /v2/agents/inbox/pending` | Scoped inbox page and exact continuation cursor |
| `inbox_application_ack` | `POST /v2/agents/inbox/application-ack` | Durable application receipt bound to the configured message and producer identity |
| `message_send` | `POST /v2/agents/inbox/send` | Typed message through an installed exact task/revision/recipient route; sender and envelope are service-authenticated |
| `inbox_ack_pending` | `POST /v2/agents/inbox/ack-pending` | Own-inbox application receipt under a separate exact-scope ACK grant; service supplies the producer identity |

The application ACK grants no question decision, execution, publication or acceptance authority. The facade exposes no shell, SQL, arbitrary URL or raw event append operation. The existing server credential authority checks each call; read grants cannot authorize ACKs. Delivery timing remains the caller's safe-turn responsibility, and each request retains its explicit context epoch.

Use `--token-file /absolute/private/agent-token` or set `ADMIRALTY_AGENT_TOKEN` to a separately configured agent credential. The client accepts an explicit loopback socket address, keeps credentials out of command arguments and protocol errors, and does not follow redirects or retry network calls. Requests are bounded to 16 KiB, HTTP replies to 128 KiB, and calls to a two-second absolute network deadline. Unknown tools, unknown argument fields, invalid page budgets, incomplete replies and unsupported response framing are rejected.

`message_send` and `inbox_ack_pending` require a retained canonical UUID `request_id`.
After an uncertain reply, reconcile or retry that exact request; a changed-payload
identity collision is not a successful retry. The actor's currently installed
route and grants remain mandatory after restart or credential rotation. Neither
tool launches, verifies, accepts or merges work. See [native coordination](../operators/native-coordination.md)
for protected route configuration, typed examples, current-revision checks and
the distinction between synthetic qualification and production adoption.

For the CLI JSON fallback, write a request such as:

```json
{"name":"task_context_get","arguments":{"project_id":"example","task_id":"producer","context_epoch":"worker-epoch-1","limit":16}}
```

Then run:

```sh
admiralty agent-tools --server 127.0.0.1:18890 call --file request.json
```

The same configured connection can serve newline-delimited MCP JSON-RPC on stdin/stdout:

```sh
admiralty agent-tools --server 127.0.0.1:18890 mcp
```

The facade implements the MCP [2025-06-18 lifecycle](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle) and [tools messages](https://modelcontextprotocol.io/specification/2025-06-18/server/tools). Each connection must initialize and send `notifications/initialized` before tool access. Returned JSON is available both as structured content and exact JSON text. Providing the current contract digest omits the repeated capsule; a changed or missing digest retrieves it again. A stale context epoch still requires fresh authorization. Inbox continuation retains its recovery generation and scope digest.

The local tests qualify the synthetic MCP client, CLI argument/result parity, a real authenticated Axum/Store round trip, immutable schema references, context compaction, stale-epoch rejection and exact durable ACK retries. Provider-specific MCP clients remain individually uncertified; use the CLI JSON fallback until each client has its own interoperability evidence. The tests do not start a provider or deploy a service.
