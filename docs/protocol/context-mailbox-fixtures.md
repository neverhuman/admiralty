# Context and mailbox fixture proof matrix

This integration test uses only the existing public `admiralty-core` APIs:
`TaskContract`, `compile_task_capsule`, `CoordinationMessage`, `DomainEvent`,
and `reduce`. Fixtures are in memory, deterministic, and use no clock, random
input, filesystem state, provider, or external service.

## Covered facts and assertions

| Fixture | Existing API result asserted |
| --- | --- |
| Submit and claim | Project, root, task, revision, contract digest and owner remain exact in the projection. |
| Compile capsule | Task, revision, epoch, digest, project/root, authority, negation, units, forbidden paths, acceptance criterion and argv survive rendering. |
| Small or escaped budget | Complete capsule is retained when it fits; too-small or escaped JSON byte budget returns an error instead of truncating. |
| Queue assignment | Existing task project, revision, digest and authority must match; recipient and context epoch remain on the queued message. |
| Near-scope substitutions | Similar project/task IDs, mismatched project or authority, and a digest from a different root fail queueing. Project/root changes produce different capsule text and contract digests. |
| ACK | Wrong recipient or wrong epoch fails; the exact message recipient and epoch succeeds. A new epoch gets a freshly compiled complete contract capsule. |
| Duplicate message ID | The projection reducer rejects an exact duplicate and changed bytes under an already present message ID. It does not return a replay receipt. |
| Declared dependency | A dependency included in the contract is rendered and remains in the capsule after changing epoch. |

The compiler estimates tokens as UTF-8 bytes divided by four. This is a size
estimate only: it is not provider tokenization, observed token usage, context
fidelity qualification, or a savings result.

## Existing API limits and unactivated behavior

`compile_task_capsule` takes only a task contract, epoch and byte-estimate
budget. It cannot retrieve an active question, evidence artifact, recipient
baseline or prior capsule. The test checks contract facts and epoch replacement
only; active-question/evidence recovery remains unsupported here. The
`CoordinationMessage` API has no root field. Root binding in this fixture is
therefore checked through the contract digest, not claimed as a separate
message field.

The core `reduce` projection has no producer identity, source sequence,
durable inbox, or replay ledger. It rejects any duplicate message ID, so this
packet cannot prove producer replay idempotency or distinguish an exact replay
receipt from a changed-byte collision. Those semantics remain unactivated and
require a test at the owning durable store/API boundary. No assertion in this
fixture treats reducer rejection as idempotent delivery.

Likewise, dependency compilation proves only that a declared dependency is
rendered. This API does not prove dependency closure or retrieve dependency
evidence. Those remain explicit integration blockers; no closure or evidence
is fabricated by the test.
