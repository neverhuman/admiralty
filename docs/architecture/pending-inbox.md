# Bounded pending inbox

`admiralty_store::inbox` observes unacknowledged messages from the Store writer's
existing message/ACK projection. Durability comes from the existing
`MessageQueued` and `MessageAcknowledged` SQLite events and their checked replay.
The derived index introduces no second durable queue, SQL table or event type. It never
reads a clock, authenticates a caller, launches a provider, records delivery or
acknowledges processing.

`InboxScope` matches the exact project, recipient and context epoch. Epochs are
nonblank, control-free UTF-8 strings of at most 16,384 bytes; typed project and
principal IDs retain core identifier validation. No scope value is normalized or
truncated. Admission bounds do not establish that a complete message with a
maximum-length epoch fits the core 16 KiB envelope; selected envelopes separately
pass `CoordinationMessage::validate()` before copying.

`validate_request` must run before cloning/enqueueing a Store request, and the
writer repeats that admission in `pending_page`. Page limits are 1..=16. Cursor
generation strings and the current writer generation must be nonblank,
control-free and at most 64 UTF-8 bytes. A cursor's scope digest uses the core
`content_digest` format (`sha256:` plus 64 lowercase hexadecimal digits) and binds
all scope fields exactly. Invalid or differently scoped cursors reject;
recovery-generation mismatch requires a fresh snapshot/pass. A matching digest
is an identity check, not a grant.

The writer owns a derived `BTreeMap<InboxScope, BTreeSet<MessageId>>` containing
only pending IDs. Open/replay validates the complete message/ACK projection and
reconstructs the index once. Each append prepares its validated index delta from
the post-reduce projection before committing SQL; only a successful commit
applies that delta. Failed transactions and exact-source replay do not alter it.
Explicit projection rebuild constructs the replacement before publishing its
new recovery generation. The index has no independent persistence or authority.

One serialized writer turn seeks only the requested scope's pending IDs after
the cursor, examines/clones at most the requested 1..16 envelopes and borrows one
additional own-scope key to determine continuation. It does not clone a snapshot,
rebuild the index or scan global history per read. `scanned_messages` counts
visited own-scope pending records, excluding the borrowed lookahead. Foreign and
ACKed histories do not change returned cursor IDs, counts or page boundaries.
An absent scope returns zero examined records and no continuation.

Every selected index reference is checked against the authoritative projection;
missing or foreign records, malformed envelopes and ACK contradictions fail the
whole page with a generic error. This does not claim to detect arbitrary missing
index entries in constant time: private writer ownership and precommit updates
maintain completeness; checked replay/rebuild reconstructs it from durable facts.
The returned messages preserve original IDs, authority/causation, units, negation
and payload bytes without summarization.

Ordering is lexical message ID, not enqueue time, priority or causal order. A
continuation is a position in a live scan, not a stable snapshot, durable
consumer offset, delivery receipt or processing watermark. A new polling pass
or reconnect starts with no cursor. Messages inserted lexically before an
earlier continuation appear on the next fresh pass; persisting the continuation
as if it acknowledged preceding work could otherwise skip pending work.
Likewise, an ACK between pages can remove a message from that live observation.
Neither a read nor an empty/finished page records an ACK.

The index removes global-scan metadata and sparse empty-page amplification. Its
memory grows with pending scopes/IDs; durable history and the full projection
retain their existing growth policy. Startup replay and the reducer's full-state
clone are unchanged. This component does not establish fleet throughput or a
bounded total durable-history size. The helper is crate-private so callers use
the writer-owned index through `Store::pending_messages`. No new HTTP route
exposes cursors; actual agent authentication and grants still require separate
qualification before untrusted transport is enabled.

`Store::pending_messages` validates admission before entering the existing
bounded request queue and invokes the helper while holding writer ownership, so
append/rebuild cannot interleave between generation comparison and page
construction. `Store::acknowledge_scoped_message` validates exact project,
recipient and epoch in that same owned writer before normal append, including
before returning a historical source-replay receipt. Only a new committed ACK
emits the existing change hint; reads and duplicate ACKs do not.

`CoordinationService::pending_messages` and `acknowledge_application` require a
trusted injected, action-aware authorizer before Store access. Read permission
does not imply application-ACK permission, and ACK policy must bind the producer
to the actual actor. The injected actor/policy is a trusted service dependency,
not authenticated agent transport. The shared private-server bearer token alone
does not establish an agent's project/recipient/epoch grant. No new HTTP route
or runtime delivery loop is enabled by these seams.

Existing historical queue replay remains a different operation: it can return
already ACKed queue observations and uses an event-sequence cursor. Existing
offline CLI operations also retain their current contracts. This component adds
no HTTP mutation route, subscription persistence, recipient transport, delivery
attempt/unknown-outcome journal, expiry, retry loop, priority reserve or task
acceptance. Native execution and full P05 activation remain unqualified.
