# Native captain–admiral communication

The native daemon can deliver typed assignments, questions, answers, evidence
references and handoffs between authenticated agents without Screen injection or
Markdown inbox writes. The existing Store writer checks current task membership,
revision, authority and recipient before durable enqueue. Reading a message,
acknowledging application receipt, and accepting a task are distinct operations.

This is an opt-in coordination path. It does not install a provider controller,
launch an agent, answer a question automatically, or qualify autonomous delivery.
Production dogfooding requires the actual supervisors to use this path; fixture
exchanges and increasing message counters do not establish that cutover.

## Install exact routes

Use the protected configuration and credential rules in [native startup](native-startup.md).
Create the real task through owner intake first. Read its current contract digest
through `task_context_get`; never use the digest of the entire TaskView in a route.
Add a route to the **sending** agent's configuration. Its subscription identifies
the **receiving** inbox, one task, channel, revision and contract digest:

```json
{
  "credential_id": "admiral-native",
  "token_file": "/srv/admiralty/private/admiral-token",
  "actor": "admiral",
  "grants": [
    {
      "scope": {"project_id": "admiralty", "recipient": "admiral", "context_epoch": "release-1"},
      "action": "read_pending"
    },
    {
      "scope": {"project_id": "admiralty", "recipient": "admiral", "context_epoch": "release-1"},
      "action": "acknowledge_pending"
    }
  ],
  "routes": [{
    "subscription": {
      "subscription_id": "captain-release",
      "project_id": "admiralty",
      "task_id": "release-task",
      "recipient": "captain",
      "context_epoch": "release-1",
      "channel": "release",
      "contract_revision": 1,
      "contract_digest": "sha256:REPLACE_WITH_CURRENT_64_LOWERCASE_HEX_DIGITS",
      "expires_at_utc": null
    },
    "authority_ref": "REPLACE_WITH_EXACT_TASK_AUTHORITY_REF"
  }]
}
```

This object belongs in `agents` within `native.json`; it is not a request body.
Replace both placeholders with exact current task facts. Install a distinct
captain credential, own-inbox read/ACK grants and a reverse route to the admiral.
Do not reuse the owner token. A route authorizes sending only; inbox access needs
its own grant. ACK permission cannot name another principal's inbox. All agents
and routes are bounded; duplicate route IDs with different definitions fail
startup before Store open.

The deployment owner loads the new configuration when starting the qualified
binary. HTTP callers cannot add routes or grants. Existing configurations without
routes retain their previous permissions. Route definitions are immutable in the
durable subscription journal; use a new subscription ID for a changed revision,
epoch, recipient or expiry. Expired or stale routes deny sends and exact retries;
already queued messages remain visible until separately acknowledged.

## Use CLI or MCP

The same narrow tools are available through CLI JSON and MCP stdio:

```sh
admiralty agent-tools list
admiralty agent-tools --server 127.0.0.1:18890 \
  --token-file /srv/admiralty/private/admiral-token \
  call --file /srv/admiralty/private/assignment.json
admiralty agent-tools --server 127.0.0.1:18890 \
  --token-file /srv/admiralty/private/captain-token mcp
```

The token file is read with the startup loader's owner-only, bounded, no-follow
checks. `ADMIRALTY_AGENT_TOKEN` remains a compatibility alternative when no file
is specified. Credentials stay out of request JSON, URLs and command arguments.
Remote agents need an authenticated local tunnel; the client accepts only
loopback addresses. MCP clients must initialize each connection before tool use.

An `assignment.json` ToolCall has this shape. Generate and retain a fresh canonical
UUID for `request_id`; replace the contract digest with the same current value
used by the installed route:

```json
{
  "name": "message_send",
  "arguments": {
    "request_id": "4ab239fa-ef42-4ef8-8724-4859d4db7822",
    "route_id": "captain-release",
    "payload": {
      "kind": "task_assigned",
      "task_id": "release-task",
      "revision": 1,
      "contract_digest": "sha256:REPLACE_WITH_CURRENT_64_LOWERCASE_HEX_DIGITS"
    }
  }
}
```

The service supplies a stable message ID bound to principal and request UUID, authenticated
sender, project, recipient, epoch and authority. The complete envelope remains
limited to 16 KiB. Large logs belong in scoped evidence references. A successful
send receipt means durably queued, not delivered or processed. It explicitly
reports `execution_granted: false` and `acceptance_granted: false`.

The captain uses `inbox_pending` and `task_context_get` to read its exact scope.
After applying the message, it can issue a separate receipt:

```json
{
  "name": "inbox_ack_pending",
  "arguments": {
    "request_id": "d474212d-c8de-4b71-9699-a949ae4b187c",
    "project_id": "admiralty",
    "recipient": "captain",
    "context_epoch": "release-1",
    "message_id": "REPLACE_WITH_MESSAGE_ID_FROM_SEND_RECEIPT"
  }
}
```

Reply through the reverse route with `message_send`, an optional `causation_id`,
and a typed `handoff`, `question`, `answer` or `artifact_ready` payload. Preserve
failed checks and next actions in the handoff. A candidate reference or green
CI claim does not issue acceptance or merge authority.

## Retries, revocation and recovery

Persist each request before sending. After an uncertain reply, retry the exact
original request with the same UUID; a successful duplicate returns the original
event sequence. Reusing that identity with changed content records an integrity
collision and returns HTTP 409. Do not generate a new UUID to hide uncertainty.
The stable replay namespace follows the principal across credential rotation
and daemon restart. Principals therefore coordinate their request IDs across
credentials. A send UUID and an ACK UUID must also differ.

Every send requires a currently active credential and the exact configured route,
including on retry. A credential read guard spans the Store operation so successful
revocation cannot race the commit. Contention, missing membership, stale contracts
and expiry deny the operation. A configured read token cannot acknowledge receipt.
The existing exact-message `inbox_application_ack` tool remains available for
deployments that grant each complete message/source identity individually.

Pending messages, application receipts, subscriptions and source-collision
evidence survive Store reopen. A pending read does not ACK. An ACK does not answer
a question, record independent verification, release resources or accept a task.
Follow [recovery procedures](recovery.md) for backup/restore; do not copy the live
SQLite file or replay an unknown provider run.

## Qualification and release boundary

Run the source-bound native exchange and denial cases:

```sh
cargo test -p admiralty-server coordination_routing --locked
cargo test -p admiralty-runtime agent_tools --locked
```

The exchange test uses actual protected startup, owner HTTP intake, CLI/MCP's
shared HTTP client, both role directions, application receipt, lost-response
retry and two complete server/Store reopens. Other cases cover forged actors,
wrong routes/tasks/epochs, read-only ACK denial, bounded bodies, current revision
checks and retained collision evidence. These are synthetic local fixtures.

For a production cutover receipt, identify the source and binary hashes, sole
deployment owner, exact task/revision/routes, actual recipient applications and
ACKs, retained failed/passing CI evidence, restart observations and remaining
holds. Never commit private credentials, runtime databases or transcripts.
General execution, independent completion, settlement, recovery, all packet
criteria and distribution remain required for the full release; see
[release status](release-status.md).
