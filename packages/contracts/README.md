# Admiralty public contracts

Rust owns `schemas/*.schema.json`. Run `pnpm contracts:generate` to produce both
TypeScript types and browser-ready standalone validators. `pnpm contracts:check`
regenerates expected output in memory and fails on drift without writing files.
Use `pnpm --dir packages/contracts test` for runtime and generation checks.

The runtime entry exports `validateTaskContract`, `validateCoordinationMessage`,
`validateDomainEvent`, `SUPPORTED_SCHEMA_VERSION`, and source schema fingerprints.
Validators are generated with pinned AJV 8.20.0 in draft 2020-12 mode. The output
has no runtime imports, AJV compiler, evaluation, network retrieval, coercion,
default insertion, or removal of unknown properties. AJV remains a build tool.

The pipeline explicitly supports schema version **1**, including nested task
contracts and messages. Rust `uint16` and `uint32` format annotations become
numeric bounds in the validation copy. Source schema files remain unchanged.
Unknown formats fail generation so new wire formats require an explicit review.

These are complete **schema** guards, not domain acceptance or authorization.
Rust still validates identifier rules, nonempty goals, scoped ownership, check
bindings, transition prerequisites, provider authority and independent acceptance.
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
