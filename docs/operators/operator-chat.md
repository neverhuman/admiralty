# Git Flow operator chat

Git Flow has a separate chat bubble for each project. Choose the Captain or Admiral destination, then send a message or an instruction. The bubble becomes opaque on hover or keyboard focus. It stays opaque on touch devices. Pause, stale/disconnected observations, and synthetic data disable sending.

Messages go to explicitly enrolled role inboxes. **Delivered means durably published in that inbox. It does not mean read, acknowledged, or executed.** A captain replies through the native CLI. Commands are text instructions for the agent to assess under its existing authority; the server never puts text into Screen, a shell, or a provider process.

## Protected enrollment

Create an existing owner-only directory for each role, on the server's local filesystem. For example:

```sh
rtk proxy mkdir -p /home/ubuntu/.local/state/admiralty/chat/dope-captain /home/ubuntu/.local/state/admiralty/chat/admiral
rtk proxy chmod 700 /home/ubuntu/.local/state/admiralty/chat /home/ubuntu/.local/state/admiralty/chat/dope-captain /home/ubuntu/.local/state/admiralty/chat/admiral
```

Use a private configuration file (0600), in an owner-only parent (0700), with protected ancestors and no symlink components:

```json
{
  "schema_version": 1,
  "targets": [
    {
      "id": "dope-captain-v1",
      "role": "captain",
      "label": "DOPE Captain",
      "projects": ["dope"],
      "directory": "/home/ubuntu/.local/state/admiralty/chat/dope-captain"
    },
    {
      "id": "fleet-admiral-v1",
      "role": "admiral",
      "label": "Fleet Admiral",
      "projects": ["dope", "admiralty"],
      "directory": "/home/ubuntu/.local/state/admiralty/chat/admiral"
    }
  ]
}
```

Target IDs are stable enrollment identities. Changing a role incarnation should use a new ID and preserve the previous inbox. One project has at most one route for each role. Each target has its own directory; several projects may share the same admiral target, with project filtering enforced by the server.

Add `--operator-chat-config /absolute/private/chat.json` to the reviewed `admiraltyd` startup command. Existing `--token-file` authenticates the owner. Missing configuration leaves chat disabled. Public legacy read access does not grant chat access. No HTTP request can install a route or choose a filesystem destination.

These instructions do not install configuration or restart a live service. Activation should use the normal reviewed deployment process and preserve existing native/intake startup options.

## Captain and admiral duties

At the start of a decision cycle, before integration/cleanup, and on a configured routine heartbeat, read the role's inbox:

```sh
rtk proxy admiralty chat inbox --directory /home/ubuntu/.local/state/admiralty/chat/dope-captain
```

Keep processed message IDs in the agent's normal durable work log. This command returns all retained records and is observational. Inspect `project_id`, `target_id`, `kind`, and full text before deciding. A `git_health` record asks for investigation and grants no Git write authority. A `command` does not bypass ownership, acceptance or repository rules.

Write a bounded UTF-8 reply to a local file, then acknowledge the exact message:

```sh
rtk proxy admiralty chat reply --directory /home/ubuntu/.local/state/admiralty/chat/dope-captain --message-id 00000000-0000-4000-8000-000000000001 --text-file /absolute/path/reply.txt
```

The example UUID must be replaced with the observed message ID. Replies are immutable. Repeating the same reply is safe; replacing it with different content fails. State what was read, decided, done, and what remains unresolved. A reply is an acknowledgment, not automatic proof of command execution or task acceptance.

The server polls no terminal and starts no model turn. A role needs its existing scheduled/active cycle to consume the inbox. Cross-host roles can use their existing authenticated SSH access to run the CLI on the server host; this release adds no remote filesystem transport or credentials. Direct live latency depends on that role's polling cycle.

## Retry, persistence, and limits

The browser creates one UUID per send intent. If delivery response is lost, retry the same ID, target, project, kind and text. The server returns the original receipt; different content with that ID returns 409. The UI also reconciles delivered messages through history. Do not retype an uncertain command as a fresh request until its original identity has been checked.

Private `msg-UUID.json` and `reply-UUID.json` files are the durable journal and inbox. Publication writes a temporary, syncs it, atomically renames under the shared directory lock, and syncs the directory. A write/fsync/lock/read error yields an uncertain outcome and preserves records. No terminal side effect needs replay. Temporary `.chat-*.tmp` files are not messages; investigate failed writes before removing owned scratch.

Text is limited to 8192 UTF-8 bytes. HTTP bodies and individual files are bounded. Each target admits at most 512 messages, reserving room for replies; total directory enumeration stops at 2048 entries. HTTP history shows the latest 100 outgoing records and 100 replies across the project's routes, with `history_truncated` when older records remain. The CLI exposes retained messages. Capacity errors block new publication until the operator preserves and archives acknowledged records.

Keep automatic Git-health records as durable deduplication receipts while the same fingerprint remains actionable. Archiving those records while a problem remains can cause its stable-ID notification to be published again. Do not delete uncertain or unacknowledged instructions. This version does not run automatic retention or cleanup.

## Git protector delivery

Enable explicit observer policy described in [git-health.md](git-health.md). The server reads the producer status once every five seconds into a shared cache. `GET /v2/git/health` reads that cache; it never invokes Git. The same loop publishes actionable reports to the configured project Captain even with no browser open.

A notification ID binds project, target and report fingerprint. Unchanged findings do not create additional messages, including after server restart. A failed scan keeps its old Git timestamp; a fresh producer generation can still notify that new failure. Producer status older than 90 seconds cannot trigger new captain messages. Cache/read/delivery failures remain visible as unavailable/stale health evidence. Unknown coverage never becomes healthy.

## Activation check

1. Check the deployed binary/source and dashboard bundle identity under normal release policy.
2. Confirm private configuration and each directory's owner/mode; enroll actual role identities and exact project IDs.
3. Ask each role to run `chat inbox` in its normal cycle. Send a bounded test message from that project's bubble, verify the exact ID in the intended inbox, and publish a reply. Confirm the reply appears only in the matching project's conversation.
4. Verify unauthorized access fails, public fallback cannot send, and no real chat transport exists in the synthetic demo.
5. Use a disposable repository to trigger dirty integration state, then confirm exactly one captain notification and no repeat after a server restart. Restore that fixture through its owner; do not manufacture a failure in a live project.
6. Record actual receipt and observed polling latency. Until this round trip is observed, report source support as implemented and live enrollment as unqualified.
