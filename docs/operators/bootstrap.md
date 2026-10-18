# Running the Admiralty v2 foundation

The legacy dashboard now supports a read-only build-progress panel from a separately owned rollout manifest. Rust adds a tested durable task/message store, scoped coordination, context capsules, crash replay, backups, generated contracts and a read-only HTTP/SSE service. Native provider execution, enrollment, independent acceptance and publication through v2 remain disabled until their qualification packets pass. The existing fleet is being used to build those next slices.

From the repository root:

```sh
rtk proxy env CARGO_TARGET_DIR="$PWD/target" cargo run -p admiralty-cli -- demo --snapshot-only
rtk proxy env CARGO_TARGET_DIR="$PWD/target" cargo run -p admiralty-cli -- demo --listen 127.0.0.1:18891
rtk proxy env CARGO_TARGET_DIR="$PWD/target" cargo run -p admiralty-cli -- doctor
rtk proxy bash scripts/check/all.sh
```

The demo owns an ephemeral database, uses synthetic tasks and messages, makes no model/forge calls and never produces accepted work. Its response labels simulation, unknown token consumption and unqualified native execution. Pass `--assets apps/dashboard/dist` after building the React application. Existing port 18890 stays the operational dashboard during rollout.

`admiraltyd --database <existing-db> --token-file <private-file> --listen 127.0.0.1:18891` serves private read-only projections. Use a high-entropy token with at least 32 characters; keep it outside Git and pass it as a Bearer header, never in URLs. Remote binding is refused until TLS/enrollment qualification exists. A single owner holds each database lock; offline CLI mutations require that owner to be stopped. This bootstrap does not advertise concurrent CLI/server writes as a managed command API.

Use `admiralty --help` for offline task submission/validation, context, mailbox, schema and backup commands. IDs are globally unique, validated strings; legacy project-local IDs must be mapped with provenance during import. Cross-project dependency contracts and authority-bearing acceptance are deliberately unavailable in this first slice.

Run `pnpm install --frozen-lockfile` for contributor tooling. Rust schemas generate TypeScript through `pnpm contracts:generate`; `pnpm contracts:check` rejects drift and external references. Node is contributor/frontend-build tooling, not a production orchestration service.

Track complete packets separately from prototype substeps. A passing candidate or agent final message cannot turn a full packet into verified/integrated work. The implementation ledger and per-candidate receipts preserve remaining gates.
