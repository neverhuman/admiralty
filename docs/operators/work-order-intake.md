# Work-order intake contracts

`admiralty_core::intake::preview_intake` is a pure, bounded preview API. Supply
original UTF-8 bytes, explicit `IntakeFormat`, source provenance and independently
authorized scope/policy. Those trusted inputs are not deserializable grants.
Imported requester/project claims and prose never authorize work.

PlainText preserves one request across paragraphs, fences and pasted metadata,
including JSON-looking text. Only explicit JsonBatch splits requests:

```json
{"schema_version":1,"items":[{"text":"First reviewed request","outcomes":[]},{"text":"Second reviewed request","outcomes":[]}]}
```

Empty outcomes and absent task contracts/checks/delivery facts remain unresolved.
Preview never invents acceptance criteria, task boundaries or source locations.
Raw source spans refer to original UTF-8 bytes, including JSON syntax/escapes;
they are not offsets into decoded text. Raw, semantic, outcome and manifest
digests have distinct domains. Revalidation requires the original bytes and
current independently authorized scope/policy.

The complete policy digest binds all limits and the revision label. Work-order
revisions retain the explicit input format. Public authority generations are
canonical positive decimal strings, preserving values above JavaScript's safe
integer range. They bind data; a future writer must reauthorize the current
generation and both work-order/plan compare-and-swap values.

Raw source is at most one MiB;256requests/1000outcomes are explicit maxima.
JSON item text has its own64KiB bound; plain text does not silently inherit it.
Duplicate/unknown keys, excessive depth/containers, malformed generations and
oversized collections fail closed before unbounded decode/hash/clone work.
Duplicate pages distinguish complete, partial and incomplete coverage; matching
content never automatically merges or rejects an intentional new request.

These contracts remain outside unchanged TaskContractv1. The Store now provides
`record_intake`, `get_work_order` and `find_duplicate_candidates` through its sole
writer. Trusted startup must install an `IntakeAuthority` using
`open_with_authorities`; default constructors install none and deny recording.
The authority resolves current independent scope/policy inside every record
writer turn, including historical retries. Public request snapshots are not grants.

Recording re-previews exact original bytes/format and checks every selected
outcome/contract/delivery/span plus work-order and plan CAS values. Original
source/policy, lineage, task bindings, plan revision and immutable receipt commit
in one transaction. Exact historical commands replay their original receipt;
changed content collides. Query results validate original records and trusted
scope before projecting lineage or pagination. Unknown historical coverage is
explicit. Open/rebuild/backup validate the stored bindings.

The enriched writer supports inline, explicitly enriched JsonBatch outcomes.
Unenriched PlainText cannot become a task through that writer. A separate
`capture_intake` operation now saves original PlainText or explicit JsonBatch,
provenance and unresolved preview before enrichment. Trusted startup installs an
`IntakeCaptureAuthority` through `open_with_intake_capture_authority`; it resolves
current independent scope/policy on every writer turn, including exact retries.
`captured_intake` takes a trusted read scope. Public snapshots grant no access.

Capture revisions require the current revision and exact parent receipt digest;
source identities and original receipts are immutable. Original UTF-8 is capped
at1MiB; escaped request/preview JSON has separate8MiB bounds. All persisted JSON
and UTF-8 lookup metadata count toward64MiB history, excluding SQLite page/index
overhead, with at most128captures and32revisions each. Capacity refuses new writes
without eviction. Open/rebuild/backup validate complete lineage. Captures create
no task, work-order, plan or outbox rows. Unverified artifact claims remain denied.

No production issuer, public route/schema export, enrichment UI, dispatch or
delivery is installed here. Thirty capture/enriched-writer/plan checks and
nineteen preview checks do not accept a parent packet or qualify autonomous
execution.
