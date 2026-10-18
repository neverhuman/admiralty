<p align="center">
  <img src="docs/brand/admiralty-source.png" width="180" alt="Admiralty mascot" />
</p>

# Admiralty

**Fleet coordination for complex engineering work.**

Admiralty brings projects, task contracts, coding agents, model policy and delivery evidence into one control plane. Rust owns durable state and the HTTP service; React, TypeScript and Vite provide a dense, real-data console.

The operating model is **Owner → Admiral → Commodores → task workers**. The Admiral coordinates the fleet; each Commodore manages a project. Small, scoped context capsules and durable messages keep routine coordination compact, leaving implementation context with the workers doing the work. Requested models, observed processes, successful checks and accepted tasks retain distinct meanings.

[Quickstart](#quickstart) · [Live console](docs/operators/live-console.md) · [Native startup](docs/operators/native-startup.md) · [Model policy](docs/operators/models.md) · [Execution ledger](docs/plans/admiralty-v2/EXECUTION.md)

## What is here

- **A detailed fleet console.** Black backgrounds, bright model badges, ordinary project tabs, task stages, worker and Commodore observations, PR/CI/review evidence, logs, hierarchy and resource accounting. Admiralty itself is another project tab.
- **Thirteen temporal plots.** Cost index versus all-Luna, first-pass failures, task burn-up, in-progress work by project, model tiers, escalation, CPU load, disk, integration CI, hygiene, tokens, token yield and quota. Source periods, freshness, missing samples and accounting notes stay visible.
- **Durable coordination.** SQLite-backed events, exact retry identities, scoped inboxes, application acknowledgments, versioned contracts, full-task compare-and-swap and replayable task projections.
- **Compact worker context.** Task capsules carry the objective, scope, dependencies and named checks. Scoped agent reads can omit an unchanged capsule by its known contract digest; token budgets are estimates rather than claimed provider measurements.
- **Configurable fleet and models.** Node identities and model policies are configuration. Local providers can precede Luna, with explicit concurrency, capability, retry and escalation policy. A routing result is a proposal until execution capacity and authority are actually acquired.
- **Evidence-aware lifecycle and recovery.** Acceptance requires trusted verification and custody. Unknown launch outcomes remain held for reconciliation. Offline backup, verification, restore rehearsal and release staging preserve their inputs instead of overwriting live state.

These capabilities have different qualification levels. The native web service and component implementations are available; general autonomous native coding execution is still being completed. See [current release status](#current-release-status) before enabling a deployment.

## Quickstart

Use Rust **1.95.0** with rustfmt/Clippy, Node **24+**, pnpm **11.15.1**, Bash and a native C build toolchain. The repository pins its Rust and pnpm versions. Native ingestion and HTTP serving use Rust; Node builds the React assets. Historical migration utilities and compatibility tests remain in the repository.

```sh
git clone https://github.com/neverhuman/admiralty.git
cd admiralty
pnpm install --frozen-lockfile --ignore-scripts
pnpm demo
```

Open **http://127.0.0.1:18892/** and choose **Demo overview** or **Workplans**. This is an isolated synthetic fleet: no accounts, providers, machine enrollment or real projects are used. The detailed producer view needs an existing real feed. Stop with Ctrl-C.

The launcher builds React and Rust locally. `ADMIRALTY_TARGET_DIR` selects an alternative Cargo build directory; `ADMIRALTY_DEMO_LISTEN` selects another loopback listener. There is no supported one-command installer or published, qualified binary bundle yet.

To inspect the CLI without starting a server:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo run -p admiralty-cli --locked -- doctor
CARGO_TARGET_DIR="$PWD/target" cargo run -p admiralty-cli --locked -- demo --snapshot-only
CARGO_TARGET_DIR="$PWD/target" cargo run -p admiralty-cli --locked -- --help
```

## Connect real fleet observations

Build and configure the native observer using the [operator guide](docs/operators/native-observer.md) and [generic configuration](configs/examples/observe.json):

```sh
CARGO_TARGET_DIR="$PWD/target" cargo build --release -p admiralty-observe --locked
target/release/admiralty-observe --config /absolute/private/observe.json --validate
target/release/admiralty-observe --config /absolute/private/observe.json
```

The observer reads configured project ledgers, Git/PR evidence, Screen identities,
resource probes and measured usage directly. It publishes `status.json` and
bounded history without Python or model turns. It does not write project repos
or launch workers. Existing history can be imported with its original timestamps.
Missing or partial measurements remain explicit; local models are never priced
as free merely because no price was recorded.

Point the source launcher at the observer output and the retained detailed HTML page:

```sh
pnpm live \
  --status /absolute/path/status.json \
  --index /absolute/path/index.html
```

Open **http://127.0.0.1:18891/**. This uses real producer observations, preserves their timestamps and serves the original complete page at `/legacy/`. It does not start agents, enroll machines or authorize execution. `ADMIRALTY_LIVE_LISTEN` changes the loopback listener; `ADMIRALTY_STATE_DIR` changes the private preview state directory.

The launcher prints the private dashboard-token **file path**, never its value. Enter the token through **Session access** for native snapshots and model settings. This convenience launcher explicitly preserves public loopback reads of the existing detailed feed only. Native data and model configuration remain authenticated.

The React detailed view retains project evidence, screen tails, raw records, density controls, hierarchy and the complete chart strip. Repeated provenance gaps are grouped with affected packet IDs; stale evidence stays stale. Blue **checks recorded** counts do not mean integration or accepted completion. The owner's display name and small uploaded profile image can be configured through **Your profile**; see [profile storage and limits](docs/operators/profile-photo.md).

Native changes use one authenticated event stream. The compatibility feed refreshes every 15 seconds without routine model turns. Missing measurements remain unknown; an idle Screen is not evidence that a task finished.

## Run the native coordinator

Build the service, CLI and static dashboard:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo build --release \
  -p admiralty-server -p admiralty-cli -p admiralty-observe --locked
pnpm --dir apps/dashboard build
```

`target/release/admiraltyd` serves the dashboard and API directly; Vite and Node are build tools rather than production orchestration services. Configure an existing private directory, an owner token and the allowed operator/agent scopes as described in [native startup](docs/operators/native-startup.md).

```sh
target/release/admiraltyd \
  --database /absolute/private/native.sqlite \
  --token-file /absolute/private/owner-token \
  --native-config /absolute/private/native.json \
  --assets "$PWD/apps/dashboard/dist" \
  --listen 127.0.0.1:18890
```

Configuration directories must be owned and mode `0700`; credentials and private configuration must be owned regular files, mode `0600`. Use absolute paths. The server rejects unsafe private inputs. Owner and agent credentials differ, stay outside Git and travel in authorization headers rather than URLs. The browser keeps its access token in page memory. Use an authenticated SSH tunnel for remote access; the current service binds only to loopback.

An optional `--redirect-listen 127.0.0.1:18891` serves an old-port HTTP 307 redirect from the same Rust binary. Set `--legacy-status-file` to the Rust observer's `status.json` and `--legacy-index-file` to the retained detailed HTML page; these names preserve the existing feed contract. Reads are authenticated unless `--legacy-public-read` is explicitly selected. The observer is a separate Rust process with one exclusive output writer.

The private native configuration currently enables scoped operator task intake, pending cancellation and unstarted contract revision, plus scoped agent context/inbox reads and separately granted acknowledgments. It does **not** install a general provider executor or candidate-acceptance authority. Request payloads cannot install grants. See [the lifecycle contract](docs/architecture/native-lifecycle.md) for the exact state and authority boundaries.

## Configure models and project work

Create your model policy from [the example registry](configs/examples/models.json), then follow [model setup](docs/operators/models.md). The example names and endpoints are examples, not fleet constants. `--models-file /absolute/private/models.json` enables authenticated model settings on `admiraltyd`; the live launcher accepts `--models` for the same purpose.

The registry supports OpenAI-compatible endpoints, configured OpenCode SSH profiles and Codex CLI model definitions. Set your own model below Luna in tier order to prefer it. Capacity pressure waits by default; expensive fallback and escalation require explicit policy. Setup errors and missing input do not silently escalate. Endpoint or credential references do not establish a working provider, and self-hosted cost remains unknown until measured. See [OpenCode SSH qualification](docs/operators/opencode-ssh.md).

Workplans can be previewed and registered as immutable revisions in an exclusively owned offline database:

```sh
target/release/admiralty workplan preview --file /absolute/path/plan.json
target/release/admiralty workplan register \
  --database /absolute/private/plans.sqlite --file /absolute/path/plan.json
target/release/admiralty workplan show \
  --database /absolute/private/plans.sqlite --project my-project --plan my-plan
```

Registration preserves task identity and dependency order; it does not submit or launch tasks. Do not point offline CLI operations at a database currently owned by the server. [Durable workplans](docs/architecture/durable-workplans.md), [intake](docs/operators/workplan-import.md), [context governance](docs/architecture/context-governor.md) and `admiralty --help` describe the available operations.

## Architecture

| Area | Responsibility |
| --- | --- |
| `crates/admiralty-core` | Validated identities, contracts, lifecycle reducer, coordination and policy primitives. |
| `crates/admiralty-store` | Serialized durable writes, SQLite replay, idempotency, inbox/outbox, workplans and artifacts. |
| `crates/admiralty-runtime` | Session ownership, pinned workspaces, resource admission, executed checks and recovery primitives. |
| `crates/admiralty-server` | Authenticated HTTP/SSE, protected startup, native operator/agent adapters and static dashboard serving. |
| `crates/admiralty-observe` | Native project/fleet/usage ingestion, observed identities and bounded temporal history. |
| `crates/admiralty-cli` | Offline inspection, workplans, context, diagnostics, models, backup/rehearsal and release staging. |
| `apps/dashboard` | React/TypeScript fleet console and native projections, built with Vite. |
| `schemas` and `packages/contracts` | Rust-derived JSON Schemas and generated TypeScript contracts. |
| `dashboard`, `captains` and `tools` | Historical migration utilities, compatible detailed HTML, source adapters and fleet operating references. |

Admiral and Commodore are role labels, not model names or fixed machine identities. Each execution still needs an independently observed provider/model, an owned workspace, current admission and configured resource limits. Compatibility producer keys and Screen IDs keep their observed identities; changing a label does not rename a running process.

## Current release status

The primary web service is Rust/React. A real, isolated Linux deterministic integration exercised admission, owned service observation and closure, positive and negative checks, immutable custody, trusted Store acceptance, exact retry, revocation denial and reopen replay. Its [source-attributed receipt](receipts/integration/eb62f5f635bea46e98c76e91499b56b4c7673849.json) records exactly what ran.

General coding-provider execution, production controller/issuer composition, atomic model-budget/capacity admission and full rollout acceptance remain separate gates. Retiring old producers requires a verified single-writer cutover after native observations and history are checked. Full P00–P31 acceptance is not implied by passing component checks.

See [release scope and remaining gates](docs/operators/release-status.md), the [execution ledger](docs/plans/admiralty-v2/EXECUTION.md), [master plan](docs/plans/admiralty-v2/MASTER_PLAN.md) and [work packets](docs/plans/admiralty-v2/WORK_PACKETS.md). Historical deployment information remains in [the compatibility inventory](docs/operators/legacy-inventory.md).

Backups and isolated restore rehearsals are available through `admiralty recover`. Restart admission remains a proposal; it cannot reactivate an unknown session. `admiralty release stage` and [the deterministic packager](docs/operators/source-package.md) stage explicit inputs without installing, signing, publishing or qualifying a release.

## Develop and validate

```sh
bash scripts/check/all.sh
python3 scripts/release/package.py --self-test
python3 scripts/check/canary-smoke.py --self-test
```

The contributor checks cover Rust formatting, strict Clippy, workspace tests, retained compatibility fixtures, generated contracts and the dashboard's tests/typecheck/build. [CI](.github/workflows/ci.yml) additionally runs actual isolated-binary smoke checks on the shared self-hosted runner pool. Workflow selectors use pool labels rather than physical host names; actual job placement and results must be observed. See [runner operations](docs/operators/ci-runners.md).

Checks do not invoke paid providers. Real provider, fleet, failure-recovery and clean-machine release qualification require explicit owned fixtures and separate receipts. Preserve failed evidence, use disjoint work scopes and review the exact source being integrated.

## License

Project licensing is [pending an explicit repository license](docs/operators/release-status.md#license).
