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

The existing contract exporter includes nine intake roots, originally bringing
the public inventory to22; the additive v2 coordination header brings it to23.
Run `admiralty contracts
check` and `node scripts/contracts/generate.mjs --check` to detect drift. The
native packager explicitly includes every generated validator sibling and tests
the staged entry with actual Node imports. Original public schemas and leaf
validators retain their bytes. Structural guards do not authenticate scope,
validate source custody or issue grants; Rust semantic checks remain required.

`StoreAuthorities` composes optional lifecycle, enriched-intake, raw-capture,
question, attention and native-usage policies through
`open_with_startup_authorities`. Existing constructors
retain their behavior; omitted policies deny their protected operations. The
question journal retains original contract/provenance and immutable retries,
while resolving current authority for every writer turn and read.

`admiralty_server::services::work_order_intake::WorkOrderIntakeService` now
composes scoped preview, capture, enriched recording and duplicate queries over
these actual Store methods. Trusted startup must explicitly supply its current
credential registry and independent policy, and install the paired
`WorkOrderIntakeAdmissionBridge` in both intake authority slots. The service
checks current grants before enqueueing; the bridge rechecks on the writer and
after policy callbacks, including exact historical retries. Read-only inbox
credentials, requester claims and matching hashes never authorize intake.

Preview policy identity covers the complete bounded original source, format,
provenance, scope and policy. Equal-length requests have distinct identities.
Inflight mutation tickets are bounded, expire and retain nonce identity; a
retired ticket cannot remove a replacement. Raw PlainText remains unresolved,
and unverified artifact claims remain unavailable. This module registration
alone does not install a production registry or enrichment UI.

## Enable the private intake API

Use the [configuration template](../../configs/examples/intake.json), replacing
the project/root, token-file path, principal, allowed actions and exact local
origins. Save it as an owned0600 file in an owned0700 directory. The separate
intake token must have32–4096 printable ASCII characters and differ from the
owner token and every configured inbox credential. Tokens stay in private files,
never in this template, request JSON, URLs or logs.

Start `admiraltyd` with the existing `--native-config` and the additional
`--intake-config /absolute/private/intake.json`. The loader validates the complete
closed configuration before opening the database. Omit this option to leave
intake unavailable. Public demo mode cannot enable it. The native executable
composes the same admission bridge into both Store writer policies and the HTTP
service; an owner credential by itself grants no intake action.

The policy catalog names explicit caps/revisions. A credential scope grant and
an independent action rule must both allow an operation, with exact principal,
project, root, authority reference/generation and policy binding. Missing rules
deny. Counters/caps are canonical positive decimal strings; `schema_version`
remains numeric1. Startup configuration is a static snapshot: changing its file
does not hot-reload or revoke a running grant. Existing in-process registry
revocation is rechecked on service/writer/replay turns. This private-file trust
boundary does not isolate root or other processes with the same UID.

All requests require both `Authorization: Bearer <owner-token>` and
`X-Admiralty-Intake-Authorization: Bearer <intake-token>`, plus
`X-Admiralty-Requested-With: intake`. The Origin and Host must match one configured
loopback pair exactly. No CORS permission, profile fallback or request-issued
grant is installed. Browser clients send same-origin JSON POST requests; their
Origin is supplied by the browser. Explicit-Origin GET reads remain available
to configured nonbrowser clients. Ordinary same-origin GET can omit Origin
under the [Fetch standard](https://fetch.spec.whatwg.org/#origin-header), so
browser reads use the bounded POST variants.

| Path under `/v2/intake` | Method and input |
| --- | --- |
| `/preview` | POST project/root, format, provenance and original source payload |
| `/captures` | POST exact `CaptureIntakeRequest` built from the preview |
| `/captures/{id}` | GET scope/revision query, or POST closed scope/revision JSON with no URL query |
| `/duplicates/query` | POST project/root and bounded `DuplicateQuery` |
| `/work-orders` | POST explicit enriched `RecordIntakeRequest` |
| `/work-orders/{id}` | GET scope/revision query, or POST closed scope/revision JSON with no URL query |

Successful replies contain `schema_version:1`, `data` with the actual typed result,
`execution_granted:false` and `acceptance_granted:false`; an absent authorized
read returns `data:null`. Authentication/unavailability is checked before body
extraction. Body bytes are capped at1MiB with a five-second read deadline, queries
at8KiB, and concurrent admitted requests at8. GET reads require no framed body.
Responses have a separate16MiB bound; larger historical offline captures fail
closed rather than changing Store/source limits. Unknown or stale authority,
ambiguous headers, identity collisions, malformed input and unverified artifact
claims produce fixed non-secret errors. Exact retries still recheck current
policy and return the original receipt.

HTTP preview/capture/recording invokes no tool, provider, verifier, publication or
task dispatch. PlainText stays one unresolved request; enriched recording still
requires explicit outcomes/contracts/checks and both current CAS values. The
enrichment UI and complete request-to-delivery runtime qualification remain
release work. Focused service/API tests do not accept a full parent packet.
