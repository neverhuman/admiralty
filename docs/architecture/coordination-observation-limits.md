# Coordination observation component boundaries

These Rust components are tested building blocks for P05. They are not an
activated broker, authenticated agent API, durable subscription service or
application acceptance mechanism. Ordinary production execution remains disabled.
Existing Store validation and immutable event replay remain authoritative for
recorded domain events; in-memory observations cannot grant native authority.

## Recipient history

`coordination::mailbox::BoundedMailbox::new` rejects capacities outside 1–2000.
Project, recipient, capacity and contiguous watermark are private. The complete
16 KiB encoded message envelope is validated before insertion. Message ID,
sequence and full immutable contents must all match for an exact duplicate;
conflicting bindings return explicit collision outcomes without changing state.
Sequence zero is rejected, and watermark arithmetic cannot wrap.

ACK is local state, not proof of recipient understanding, task completion or a
durable transport ACK. ACKed messages retain collision evidence and consume the
same capacity. Once history fills, new messages require an explicitly authorized
resnapshot; nothing silently evicts evidence or resets an epoch. Exact replay and
ACK of retained messages remain available. Delivery views return at most128
messages. Caller authentication, task/grant checks and durable resnapshot
admission must be supplied by the future service.

## Topic directory

`TopicDirectory` requires a `TopicAuthorizer` for every subscription and read.
The trusted caller must authenticate the actor and evaluate real actor grants,
project/task membership, channel and target subscriber. Labels alone do not
provide authority; an arbitrary permissive callback is not a security boundary.
Denial and capacity failure leave containers and counters unchanged. Exact
subscription retries recheck authority even at capacity.

| Bound | Default | Hard maximum |
| --- | ---: | ---: |
| Topics |64 |256 |
| Total subscriptions |1024 |4096 |
| Subscribers per topic |64 |256 |
| Channel UTF-8 bytes |256 |256 |
| Returned subscribers per page |128 |128 |

Channels must be nonblank, trimmed and free of control characters. Custom limits
must be nonzero and below hard maxima. `resolve_page` seeks directly by typed
subscriber ID, inspects at most `limit+1` records and authorizes every returned
and lookahead target. A denial fails the whole page; it does not scan unbounded
records looking for allowed replacements. `next_cursor` is a lexical continuation
only. Concurrent subscription changes do not yield a stable snapshot or durable
recipient epoch. There is no persistence, delivery loop or subscription replay.

## Durable queue observation replay

`CoordinationService::queued_messages_since` replaces the unbounded global
snapshot/list helper. Each call requires a trusted `CoordinationReadAuthorizer`
for the actual actor, project, recipient and context epoch before querying Store.
It checks bounded scope/cursor fields before the Store can clone them. The
recovery generation is at most64 bytes; the sequence is a canonical nonnegative
signed64-bit decimal. Actual generation and head validation remains atomic in
the Store writer.

The request examines1–16 existing durable events, then returns only matching
`MessageQueued` observations. It never clones the global projection. The cursor
advances over the scanned durable page even when every event was filtered out.
Consumers must continue after an empty page with `scanned_events>0`; an empty
filtered page is not end-of-stream. `scanned_events==0` means no events at that
read, not a finished provider turn. Rebuild invalidates the old recovery cursor;
reopen preserves supported replay. Every page rechecks authorization.

This is immutable queue history: an ACKed message can still appear. It is not a
pending-delivery list, delivery attempt journal, application ACK or authenticated
HTTP command route. Full P05 still requires durable subscriptions, scoped replay
and expiry/unknown-delivery state, priority reserves/coalescing, service authority
and loss/restart qualification before activation.
