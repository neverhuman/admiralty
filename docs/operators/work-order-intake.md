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

This component defines previews, record/binding and delivery-requirement contracts
outside unchanged TaskContractv1. It does not implement public schema export,
the authenticated Store writer, enrichment UI, dispatch or actual delivery.
The separate WO2 writer must re-preview exact original bytes/format, verify every
selected outcome/contract/delivery/span and current authority/policy, then record
lineage/tasks/plan atomically. Artifact claims alone are not verified custody.
No parent packet is accepted by these nineteen component checks.
