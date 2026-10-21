# Running the Admiralty v2 foundation

The legacy dashboard now supports a read-only build-progress panel from a separately owned rollout manifest. Rust adds a tested durable task/message store, scoped coordination, context capsules, crash replay, backups, generated contracts and a read-only HTTP/SSE service. Native provider execution, enrollment, independent acceptance and publication through v2 remain disabled until their qualification packets pass. The existing fleet is being used to build those next slices.

From the repository root:

```sh
rtk proxy env CARGO_TARGET_DIR="$PWD/target" cargo run -p admiralty-cli -- demo --snapshot-only
rtk proxy env CARGO_TARGET_DIR="$PWD/target" cargo run -p admiralty-cli -- demo --listen 127.0.0.1:18892
rtk proxy env CARGO_TARGET_DIR="$PWD/target" cargo run -p admiralty-cli -- doctor
rtk proxy cargo xtask check
```

The demo owns an ephemeral database, uses synthetic tasks and messages, makes no model/forge calls and never produces accepted work. Its response labels simulation, unknown token consumption and unqualified native execution. Pass `--assets apps/dashboard/dist` after building the React application. Existing port 18890 stays the operational dashboard during rollout.

`admiraltyd --database <existing-db> --token-file <private-file> --listen 127.0.0.1:18891` serves private read-only projections. Use a high-entropy token with at least 32 characters; keep it outside Git and pass it as a Bearer header, never in URLs. Remote binding is refused until TLS/enrollment qualification exists. A single owner holds each database lock; offline CLI mutations require that owner to be stopped. This bootstrap does not advertise concurrent CLI/server writes as a managed command API.

Use `admiralty --help` for offline task submission/validation, context, mailbox, schema and backup commands. IDs are globally unique, validated strings; legacy project-local IDs must be mapped with provenance during import. Cross-project dependency contracts and authority-bearing acceptance are deliberately unavailable in this first slice.

Run `pnpm install --frozen-lockfile` for contributor tooling. Rust schemas generate TypeScript through `pnpm contracts:generate`; `pnpm contracts:check` rejects drift and external references. Node is contributor/frontend-build tooling, not a production orchestration service.

Track complete packets separately from prototype substeps. A passing candidate or agent final message cannot turn a full packet into verified/integrated work. The implementation ledger and per-candidate receipts preserve remaining gates.


## Current UI and bounded read path

`pnpm install --frozen-lockfile --ignore-scripts && pnpm demo` builds the React
preview and starts Rust's isolated synthetic server on 127.0.0.1:18892. The
launcher (`cargo xtask demo`) keeps Cargo artifacts in the checkout unless
ADMIRALTY_TARGET_DIR is set.
Production build assets are served by Rust; Vite is a development dependency.
The browser keeps authentication in memory, validates versioned envelopes, uses
one authenticated fetch/SSE stream and refreshes snapshots only on durable changes
or recovery. Retries are bounded and can be resumed manually. Idle changes are
signaled from the writer rather than polled per subscriber. The server admits at
most 16 event streams and 8 concurrent read operations; these are prototype bounds,
not qualified fleet throughput figures.

On Unix, newly created database directories use `0700` and database/lock/backup files
use `0600`. Existing files with group/other permissions, symlinks, multiple hardlinks
or group/world-writable parents are refused. Existing parent directories are never
silently chmodded. Back up and migrate a legacy database explicitly before reuse.
Same-user hostile processes and hostile filesystem/parent replacement are outside
this trusted local prototype's security boundary. Native runner isolation remains
a separate qualification gate.


## Connect the existing detailed dashboard

Run `admiraltyd` with its normal private database/token/assets options, plus
`--legacy-status-file <existing-status.json> --legacy-index-file <existing-index.html>`.
Reads are authenticated by default. `--legacy-public-read` explicitly preserves
an existing loopback dashboard's public read scope for those two legacy files;
it never bypasses authentication for native tasks or model configuration. The
synthetic demo refuses real legacy/model configuration. `/v1/legacy` returns the
producer's original facts and timestamps; the React view renders every project
through the same component. `/legacy/` and `/legacy/status.json` preserve the
original complete page as an exact fallback. Unknown legacy routes return404.

`--models-file <private-models.json>` enables authenticated Models settings.
The file's parent must be owned0700; model writes require the exact current
revision. Credentials remain private file references and are omitted from list
responses. CLI `admiralty models list|replace|select` uses the same registry and
selection logic. Native launch authority remains separately qualified.
