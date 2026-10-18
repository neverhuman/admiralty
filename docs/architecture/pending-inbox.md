# Bounded pending inbox

`admiralty_store::inbox` observes unacknowledged messages from the Store writer's
existing message/ACK projection. Durability comes from the existing
`MessageQueued` and `MessageAcknowledged` SQLite events and their checked replay.
This pure helper introduces no second queue, SQL table or event type. It never
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

One serialized writer turn seeks the borrowed `BTreeMap` directly after the
cursor's message ID, examines at most 256 message records and clones at most 16
matching unacknowledged envelopes. It does not clone a snapshot/map or scan all
history in one call. One additional borrowed key lookahead determines whether a
continuation exists. `scanned_messages` counts examined records, not this
lookahead. A visited key/message-ID contradiction or ACK epoch mismatch fails
closed; malformed selected envelopes also fail, without returning partial data.
This is not a complete audit of unvisited projection records: durable replay is
responsible for the whole stored history.

Unrelated and already acknowledged messages advance the scan too. Therefore a
page may contain no messages yet supply `next_cursor`; continue it to finish that
pass. ACK suppression requires the stored ACK's epoch to equal the message's
epoch. The returned messages preserve original IDs, authority/causation, units,
negation and payload bytes without summarization.

Ordering is lexical message ID, not enqueue time, priority or causal order. A
continuation is a position in a live scan, not a stable snapshot, durable
consumer offset, delivery receipt or processing watermark. A new polling pass
or reconnect starts with no cursor. Messages inserted lexically before an
earlier continuation appear on the next fresh pass; persisting the continuation
as if it acknowledged preceding work could otherwise skip pending work.
Likewise, an ACK between pages can remove a message from that live observation.
Neither a read nor an empty/finished page records an ACK.

The per-call scan bound also limits writer occupancy for sparse scopes, but it
is not a recipient index. A scope with many unrelated or ACKed records ahead of
it may require multiple empty pages. This prototype does not claim qualified
fleet throughput or bounded total durable-history size.

The current continuation is for trusted internal callers. Its
`after_message_id` is the last globally scanned key and can therefore reveal a
message ID from another scope; `scanned_messages` also describes global scan
work. Exact payload filtering does not establish cross-project metadata
isolation. No new HTTP route exposes this cursor. An untrusted agent transport
requires an opaque server-owned continuation or a scope index, plus separately
qualified authentication and grants, before claiming scoped privacy.

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
