# Admiralty public contracts

Rust owns `schemas/*.schema.json`. Run `pnpm contracts:generate` to produce both
TypeScript types and browser-ready standalone validators. `pnpm contracts:check`
regenerates expected output in memory and fails on drift without writing files.
Use `pnpm --dir packages/contracts test` for runtime and generation checks.

The runtime entry exports guards for the twenty-two public Rust roots currently
covered by this pipeline:

- `TaskContract`, `CoordinationMessage`, `DomainEvent`, `DomainEventV2`
- `Workplan`, `VerificationReceipt`, `VerificationPolicy`, `InterfaceContract`
- `ConfigLayer`, `ConfigPatch`, `ConfigLayerReceipt`, `ProvenanceContribution`
- `ViewsSnapshot`
- `IntakeBatch`, `SourceProvenance`, `IntakePreview`, `WorkOrderRevision`
- `RecordIntakeRequest`, `RecordIntakeReceipt`, `DeliveryRequirement`
- `DuplicateQuery`, `DuplicatePage`

Each guard is named `validate<Type>`. The package also exports
`SUPPORTED_SCHEMA_VERSION` and source schema fingerprints. Root and nested types
are available from the package root, with explicit type-only subpaths listed in `package.json`.
Validators are generated with pinned AJV 8.20.0 in draft 2020-12 mode. Each guard
module has no runtime imports, AJV compiler, evaluation, network retrieval, coercion, default insertion, or removal of unknown properties. The
compatibility entry statically re-exports the original four guards from one
shared module and the eighteen additional roots from separate standalone modules.
The package declares these modules free of initialization side effects, letting
production bundlers omit unused roots while preserving the existing public API.
AJV remains a build tool.

Native release staging must explicitly select `generated/validators.mjs` and
all nineteen `generated/validators-*.mjs` siblings with their individual SHA-256
hashes. The CLI admits only the exact generated filenames and source-bound
module bytes, verifies the fixed local export closure, and rejects missing or
unknown dependencies before staging. Simple custom self-contained guards retain
a conservative source subset; escaped/template/loader syntax is unsupported.
This check stages artifacts and does not qualify an installer or grant authority.

The pipeline explicitly supports schema version **1**, including nested task
contracts and messages. Rust `uint16`, `uint32` and nullable `int32` format
annotations become numeric bounds in the validation copy; `protocol_version` retains the separate v2 value.
Source schema files remain unchanged by that normalization. Rust-derived schema
annotations retain collection caps, nonzero configured limits and bounded exit
codes. `DecimalU64` interface revisions remain canonical **decimal strings**
through `18446744073709551615`; numeric values, overflow, whitespace and leading
zeros are rejected without rounding. Missing optional fields and explicit nulls
retain their serde meaning and are never rewritten by the guards. Unknown fields
are rejected wherever Rust uses `deny_unknown_fields`; the existing extensible
metadata roots `ConfigLayerReceipt` and `ProvenanceContribution` retain their
serde behavior and permit extra properties.
Unknown formats fail generation so new wire formats require an explicit review.

These are complete **schema** guards, not domain acceptance or authorization.
Rust still validates identifier rules, nonempty goals, scoped ownership, check
bindings, transition prerequisites, provider authority and independent acceptance.
Private configuration policy, resolved private configuration, opaque validated
receipt results, and workplan preview results are deliberately not public input
schemas. The intake roots reuse the existing Rust wire models. `AuthorizedIntakeScope`
and `IntakePolicy` remain trusted non-deserializable composition inputs and are
not exported. Public `ScopeSnapshot`, attributed requester, policy digest, source
artifact/pin claims and record receipts are data; they grant no authenticated
scope, custody, dispatch, delivery or acceptance. Intake `authority_generation`
is a canonical **positive** decimal-u64 string: zero is rejected as well as
numeric JSON, leading zeros and overflow. The frozen v1 task schema is unchanged.

The schema guards enforce structure and serialization, including closed fields
and integer widths. Rust still enforces input byte/depth/item bounds, canonical
source/preview hashes, source spans, exact selected task/delivery bindings, current
policy digests, CAS and independently authorized Store actions. For example, a
well-shaped altered policy digest can pass a schema guard and still be rejected
by `RecordIntakeRequest::validate_with_policy`; an `ArtifactClaim` schema pass
never verifies its object or pin. Do not use validation as an action issuer.
The shared fixtures roundtrip actual Rust types and exercise the same standalone
JavaScript guards; a shaped provider claim is still untrusted. Collection boundary
fixtures prove schema limits only, not valid task identities, dependency graphs,
receipt policy consistency or the combined receipt-check cap.

Generated schema output cannot express every Rust semantic check; the guards
must never promote a candidate or substitute for that authority.

Bound inputs before invoking guards. The dashboard limits snapshot transport to
eight MiB, bounds JSON depth, nodes, collections and text before validation, and
retains its small fleet/cursor envelope guard until those API envelopes are also
generated from Rust. Domain unions, nested contracts and message payloads have
one schema source and no handwritten frontend shape mirror.

Generator implementation follows the official
[AJV standalone guide](https://ajv.js.org/standalone) and
[draft 2020-12 support](https://ajv.js.org/json-schema.html#draft-2020-12).
