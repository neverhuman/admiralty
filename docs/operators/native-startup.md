# Native coordinator startup

`admiraltyd` serves the React dashboard, native fleet snapshots, SSE, and the
configured compatibility collector feed. A private native configuration enables
operator intake and scoped agent communication. Configuration does not authorize a task
process, candidate acceptance, publication, or a model invocation.

Use absolute paths. Keep the configuration directory mode `0700` and each
credential/configuration file mode `0600`, owned by the service account. The
loader rejects symlinks, multiple hard links, oversized files, unsafe ancestor
directories and ordinary in-place changes detected during a read. This does not
provide isolation from another process running as the same user or root.

```sh
admiraltyd --database /srv/admiralty/private/native.sqlite \
  --token-file /srv/admiralty/private/owner-token \
  --native-config /srv/admiralty/private/native.json \
  --assets /srv/admiralty/release/react --listen 127.0.0.1:18890 \
  --redirect-listen 127.0.0.1:18891
```

The optional redirect is served by Rust and preserves the path/query with HTTP
307. Bind addresses and targets must be loopback. Use an authenticated SSH tunnel
for remote access. Existing `serve`/`app` library callers retain their default
read-only configuration; native command routes cannot be installed in demo mode.

Example `native.json`, with deployment-selected identities:

```json
{
  "schema_version": 1,
  "operator": {"principal": "owner", "projects": ["project-a"]},
  "agents": [{
    "credential_id": "worker-1-read",
    "token_file": "/srv/admiralty/private/worker-1-token",
    "actor": "worker-1",
    "grants": [{
      "scope": {
        "project_id": "project-a",
        "recipient": "worker-1",
        "context_epoch": "context-1"
      },
      "action": "read_pending"
    }]
  }]
}
```

Owner and agent credentials must differ. Agent tokens authorize only their exact
project, recipient, epoch and action. Application ACK grants additionally require
an exact message ID and producer source identity. Credential replacement requires
loading a new authority; old in-process authenticated handles cannot be reused
against its replacement. Request fields do not select an actor or install grants.

The owner token authenticates `POST /v2/tasks` with `request_id` (canonical UUID)
and a complete validated `contract`. `POST /v2/tasks/lifecycle` accepts pending
cancellation or an unstarted contract revision with `expected_task_digest` of
the complete current TaskView. Exact retries are idempotent, including after a
later state transition. Reusing a request identity with a changed payload is an
integrity collision. No HTTP command accepts a candidate or closes an attempt.

Agent routes under `/v2/agents/inbox` are:

- `GET /pending`: exact scoped, bounded pending messages and continuation.
- `GET /context`: project/task/epoch and optional `known_contract_digest`, token
  budget, inbox limit and continuation. Returns the compact contract capsule
  when changed, task state/digest and bounded question/artifact references.
- `POST /application-ack`: an exact separately granted application receipt.

There is no public global history cursor. Compact replies expose a recovery
identity and task digest rather than another project's event volume. A cached
contract digest omits the unchanged capsule; it is not an ACK or an assignment.
Token estimates are not measured provider token usage. Durable question history
is explicitly reported as an outstanding requirement by this initial adapter.

For a native captain–admiral exchange, an agent may additionally have explicit
`routes` and an `acknowledge_pending` grant for its own exact inbox. `POST /send`
constructs the sender and envelope from authenticated identity and a preinstalled
task/revision/recipient route; `POST /ack-pending` constructs the receipt's source
identity from that actor and a canonical request UUID. These are separate grants;
`read_pending` alone permits neither operation. Both use durable exact retries.
See [native coordination setup and qualification](native-coordination.md) before
enabling them. Existing exact-message `application-ack` grants remain supported.

## Execution and recovery

The durable controller records one task claim and typed launch intent atomically,
claims effects through the Store writer, checks the current task/attempt/digest
and dispatch admission again, and records an exact fenced runtime result.
Malformed, denied, expired or uncertain effects remain held as unknown. A
successful child supplies a candidate, never task acceptance.

Native resource enforcement requires an explicitly configured qualified Linux
user systemd backend, an eligible CPU subset, memory/task limits and a current
trusted permit/reservation/lease. An absent backend or ambiguous observation
cannot grant execution. Actual runtime closure, independently executed checks,
protected policy and authenticated artifact custody must bind the same task,
attempt, contract, candidate, source commit/tree and verifier before a trusted
lifecycle authority can issue an acceptance command. Serialized claims cannot
substitute for these process-local proofs. Startup in this document installs
only operator authority; it does not install the execution controller authority.

`admiralty recover --help` exposes native online backup, verification, isolated
restore rehearsal and restart-admission proposals. Restore/restart proposals do
not authenticate external authority or reactivate held sessions. Use explicit
artifact closure inputs and create-only private outputs; keep the original
database and evidence until restoration is verified.

Full packet acceptance, deployment of a new binary and actual runtime admission
are separate observations. Report the exact source and executed checks for each.
