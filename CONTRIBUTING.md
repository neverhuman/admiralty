# Contributing

Admiralty is a Rust workspace with a React dashboard and retained compatibility
tools. Read the relevant crate or app code, its tests, and the linked operator or
architecture guide before changing a boundary. The [execution ledger](docs/plans/admiralty-v2/EXECUTION.md)
records release scope and evidence; a component test or green check does not by
itself accept a parent work packet.

## Development setup

Install Rust 1.95.0 with `rustfmt` and `clippy` (pinned by
[`rust-toolchain.toml`](rust-toolchain.toml)), Node 24, pnpm 11.15.1 (pinned in
[`package.json`](package.json)), Bash and a native C build toolchain for
dependencies that compile native code. Python is not needed: serving,
ingestion, fleet tools, release qualification and repository automation are
Rust, and the few Node scripts are TypeScript.
Install JavaScript dependencies
from the lockfile without running dependency lifecycle scripts:

```sh
pnpm install --frozen-lockfile --ignore-scripts
```

Use an isolated checkout and keep generated build output out of commits. The
standard local validation entry point is:

```sh
pnpm check
```

It installs frozen JavaScript dependencies and runs the complete local matrix:
Rust formatting, strict locked workspace Clippy/tests (including the audit,
observer and tool parity suites), package/canary checks, actual native binaries and smoke checks,
Rust/JavaScript contracts, frontend unit tests/typechecks/build, and real
browser/accessibility journeys. Install Chrome or Chromium first;
`ADMIRALTY_BROWSER_BINARY` can select an existing browser. It does not invoke
paid providers. Logs and exact commands are retained in the private directory
printed by the command. See [local CI](docs/operators/local-ci.md) for prerequisites,
resource bounds and cache overrides.

Hosted Linux checks and the pinned Jankurai audit provide independent evidence.
A local pass does not establish a hosted result, deployment, native execution
qualification or a full release. An advisory audit pass does not close its
remaining conformance findings.

Run focused checks while developing, then the complete `pnpm check` before
pushing. Useful focused commands include:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
pnpm contracts:check
pnpm --dir packages/contracts test
pnpm --dir apps/dashboard typecheck
pnpm --dir apps/dashboard test --run
pnpm --dir apps/dashboard build
```

Set `ADMIRALTY_TARGET_DIR` if you need Cargo output in a private scratch path;
`cargo xtask check` honors it. Keep test data and private runtime files out
of the checkout. Do not weaken custody checks or replace source-bound fixtures
with hand-edited output to get a passing run. Runtime, tooling and automation
use Rust; UI and browser/contract scripts use TypeScript. The `python_policy`
test rejects any Python file and any new shell script (see the
[completed migration](PY_TO_RUST_PLAN.md)).

## Changes and review

Start from the base named by the maintainer or work packet. Keep changes within
the assigned paths; coordinate before touching shared integration files or
generated outputs. A worker candidate, merged commit, deployed binary and
qualified release are distinct states. Do not infer merge readiness from a
branch name, a green subset of tests, or an agent report.

Before requesting review, inspect the exact diff, run `git diff --check`, and
record any checks that fail or could not run. For assigned qualification work,
keep a source-bound receipt with the full base and tested commit, changed paths,
commands, working directory, exit status, relevant environment, raw log
references, test counts and limitations. Preserve failed drafts and their logs;
do not overwrite them with a repaired run or report nested assertions as
additional tests. Follow the packet's receipt location and validation rules.

Keep dependent changes in one integration pull request; coordinate with a
maintainer before starting a PR stack. Reuse an appropriate owned checkout,
keep one writer per checkout, and preserve dirty, ignored or foreign work.
Retire merged branches/worktrees only after confirming their work is integrated,
no process uses them, and unique/private state is preserved.

Open a pull request against the intended base and describe the behavior changed,
the exact source tested, validation, and remaining limits. Repository automation
may require an approval attached to the exact PR head; an approval for an older
head does not review later commits. That review gate is separate from merge
authority: only the maintainer or other person authorized by repository policy
should merge. Do not bypass required checks, force-push shared branches, or
assume that a bot approval authorizes a worker to merge. The project has used
`neverhumanbot` approval bound to an exact head; follow the current required
checks and maintainer process for the PR you are working on. Follow any stricter
instructions in the assigned work packet.

## Credentials and fixtures

Never commit credentials, private configuration, runtime databases, transcripts,
customer data, provider outputs, or unredacted security evidence. Do not pass
secrets in command-line arguments or URLs. Use private, owner-only scratch files
and directories for credentials and test data, and use only synthetic fixtures
unless a maintainer has explicitly authorized another source. Keep logs bounded
and remove secret values before sharing evidence. Read the
[native startup guide](docs/operators/native-startup.md) for current credential
and local trust limits.
