# Node enrollment and durable transport

`admiralty node` and `admiralty-node` expose the same Unix commands. Every
state directory is explicitly selected and must be owned and mode 0700; key,
token and database files are private. There is no host discovery or automatic
enrollment. These commands receive observations and artifacts; they do not
launch tasks or grant publication, input, GPU or provider authority.

Create an inventory JSON file with `cpu_millis`, decimal-string `memory_bytes`
and `spool_bytes`, and a `runners` array. For example:

```json
{"cpu_millis":2000,"memory_bytes":"1073741824","spool_bytes":"16777216","runners":["isolated-oci"]}
```

Run `admiralty node init --state /private/node --inventory inventory.json
--name node.example`. It creates a persistent random node UUID, key,
certificate and public `enrollment.json`. The private key stays in
`identity.json`. Repeating initialization retains the same identity.

On the controller, issue a token with `admiralty node issue --state
/private/controller --scope scope.json --token-file /private/tokens/node.json`.
The scope contains the exact project, root, task and authority IDs. Token
lifetime defaults to 300 seconds, with a maximum of one hour. Only the digest
is persisted in the registry. Exchange the token and public enrollment request
through the installation's existing authenticated operator channel. Then run
`admiralty node enroll --state /private/controller --token-file
/private/tokens/node.json --request node-enrollment.json`. Tokens cannot enroll
a second binding. An identical lost-response retry returns the original node.
Bootstrap is an explicit local provisioning operation; the transport never
accepts anonymous remote enrollment.

Initialize a separate controller TLS identity with its actual DNS name. Give
nodes the public controller enrollment request over the same trusted channel.
Start `admiralty node serve --state /private/controller --identity
/private/controller-tls --listen 127.0.0.1:18900`. Select an appropriate explicit
address for remote use. TLS 1.3 requires certificates on both sides, validates
the controller DNS name and pins enrolled client certificates. ALPN fixes the
wire protocol to `admiralty-node/1`. Each request rechecks current enrollment,
scope, boot, generation and certificate inside the SQLite writer transaction.
Revocation therefore also fences previously authenticated connections.

Use `enqueue --state /private/outbox --identity /private/node --scope scope.json
--payload observation.json` before `flush --state /private/outbox --identity
/private/node --controller 127.0.0.1:18900 --controller-request
controller-enrollment.json --name controller.example`. The producer UUID,
sequence and unacknowledged records survive process restart. A response binds
the exact stream and its contiguous durable watermark. Gaps request replay;
changed bytes at an acknowledged sequence are rejected. ACKs acknowledge
receipt only. Observations are not domain-authorized state transitions.

`revoke --state /private/controller --node UUID` fences the current generation.
For a certificate rotation, initialize replacement credentials in a new private
directory, run `rotate` with the replacement public request on the controller,
save the returned public identity, and run `adopt --state /private/replacement
--identity returned-identity.json`. Adoption verifies the certificate/private
key binding. For a new boot after revocation, the enrollment request must use
the same node UUID, a fresh boot UUID and the next generation; a fresh one-time
token authorizes it. Preserve old credentials and pending outboxes as evidence.
An outbox cannot change incarnation while records remain unacknowledged.

Artifact transfer requires a local `grant-transfer` before accepting bytes.
The grant binds `transfer_id` (UUID), identity, scope, SHA-256 digest and exact
decimal-string size. `transfer` sends a JSON chunk containing that manifest,
decimal-string `offset`, byte array and `finish`. Repeated identical ranges
resume after a lost response or restart. Gaps, changed ranges, foreign custody,
revoked senders and incorrect final size/digest fail. Incomplete artifacts are
not visible through the verified artifact API. No remote filesystem path is
accepted.

Bounds are explicit: 64 KiB frames, 8 KiB observation payloads, 1,024 pending
producer records, 4,096 durable receiver records, 4 KiB chunks, 8 MiB per
artifact and 64 MiB/128 grants of artifact custody. Exhaustion rejects further
admission while preserving prior evidence. There is no implicit garbage
collection of acknowledged receipts or artifacts. Socket reads, writes and
connects use five-second timeouts. SQLite uses FULL synchronous WAL commits
before acknowledging observations or transfer ranges.

Run `cargo test -p admiralty-testkit --test fleet_transport --locked` to qualify
actual loopback mutual TLS, persistent enrollment, replay/gaps, lost ACKs,
revocation after handshake, rotation, reboot, custody, partial transfer and
bounded backpressure. Fixtures enroll only random local identities.
