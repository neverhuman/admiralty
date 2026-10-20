# Community readiness and release qualification

Admiralty currently publishes source for version `0.1.0`. The public repository
has an MIT license, contribution guidance, a credential-free synthetic demo and
private vulnerability reporting. A source preview is available; the complete
managed native major release is not yet qualified or published.

This page records the community-facing boundary on **20 October 2026**. The
[execution ledger](../plans/admiralty-v2/EXECUTION.md) owns source-specific
acceptance and supersedes historical summaries. See the
[operator release status](../operators/release-status.md) for current operational
gates.

## Platform support

| Surface | Demonstrated evidence | Remaining qualification |
| --- | --- | --- |
| macOS Apple Silicon | Rust source build, CLI inspection, synthetic demo, local application and browser/accessibility checks | General managed coding execution, complete installer and upgrade qualification |
| Linux x86_64 | Deployed Rust HTTP service/observer, locked Rust/contract checks, isolated browser journeys and deterministic task-loop evidence | Combined release qualification, general managed coding execution, complete clean-machine distribution |
| Windows | No current supported release or qualification receipt | Native build, installation, runtime, browser and recovery qualification |
| Browser | Chrome/Chromium journeys over the built React application | Other browser engines have no declared qualification receipt |
| Remote fleet execution | Component APIs and bounded Linux execution experiments | General enrollment-to-managed-coding-to-acceptance/recovery journey |

These rows describe executed evidence, not portability guarantees. Linux
systemd resource/isolation claims do not transfer to macOS. An ordinary worktree
or private file does not isolate another process running as the same OS user.

## Start from source

Install Rust **1.95.0** with rustfmt and Clippy, Node **24**, pnpm **11.15.1**,
Bash and a native C build toolchain. Use the root Rust and pnpm pins. Use Python
3.12+ for retained compatibility checks (3.12/Linux and 3.14/macOS are qualified).
The convenience `pnpm live` launcher also uses Python 3 for its
bootstrap; observation ingestion and the native HTTP runtime use Rust.

Follow the [README quickstart](../../README.md#quickstart). The first build may
fetch dependencies. The running demo uses synthetic data and does not contact a
coding provider. `admiralty doctor` reports this binary's capabilities and
qualification limits. Stop a foreground demo with Ctrl-C.

For local validation, install Chrome or Chromium and run `pnpm check`.
[Local CI](../operators/local-ci.md) documents its full matrix, private logs,
pinned browser support and build-cache selection. A successful local run does
not establish Linux-only tests, a hosted result or a deployed binary.

The separate security workflow provisions the pinned Rust toolchain and required
scanners in fresh job scratch. It checks dependency metadata and workflows,
then scans the complete GitHub event range, including merge-parent diffs.
A manual/local run without `ADMIRALTY_SECURITY_SCAN_BASE` checks the latest
commit and its merge diffs. An explicit base must be an available ancestor Git
object ID. The [qualification receipt](../../receipts/integration/security-fresh-runner-20261020.json)
preserves the original provisioning failure and synthetic false-pass control;
this lane does not claim all dependency or image scanners.

## Available workflows

- **Observe a fleet:** configure project/source identities, run the Rust
  observer, then connect the read-only console. Source times, partial coverage
  and unknown model/cost measurements remain explicit.
- **Inspect delivery:** use the detailed view, Git Flow and Fleet Atlas to
  inspect project, checkout, agent and evidence observations.
- **Use scoped coordination:** configure native task/agent authority and finite
  captain–admiral routes. Messages, exact retries and own-inbox application ACKs
  are durable; one actual production role cycle has a
  [sanitized receipt](../../receipts/integration/native-actual-role-cycle-20261019.json).
  Bootstrap wake-up still uses the legacy inbox.
- **Use offline tools:** preview/register workplan revisions, inspect contracts,
  compile context and rehearse backups/restores with exclusive database
  ownership. Recording work does not launch it.
- **Handle an explicit archive:** native stage/archive/private-version install
  commands verify supplied bytes and pins. They do not activate a service,
  authenticate a publisher or establish platform compatibility.

Production captures in the README are dated recordings, not live embeds or
proof of autonomous coding execution. Full-color WebP is losslessly encoded;
the GIF fallback has a maximum 256-color palette. Source data and timestamps
remain visible. The [capture guide](../operators/demo-capture.md) documents
reproduction and pixel comparisons.

## Major release gates

The managed major release requires every applicable criterion below to have
actual source-bound evidence. Keep failures and unknown outcomes until resolved.

| Gate | Evidence required before completion |
| --- | --- |
| Complete CI | Canonical local matrix on the exact integrated candidate, platform-specific tests and all required hosted checks; no skipped required checks presented as PASS |
| Managed coding execution | Current authenticated policy/credentials, physical capacity and budget admission, actual provider start/context delivery, source candidate publication, independent checks and trusted lifecycle acceptance |
| End-to-end dogfooding | The actual Admiral/captain/worker development workflow uses native task and message state, scoped context, execution, verification, integration and accounting; message transport alone is insufficient |
| Usage and settlement | Observed provider/model identity, recorded coordination and task usage, conservative unknown liabilities, qualified settlement and release |
| Recovery and upgrades | External fencing and owned-session/effect reconciliation, restart/partition behavior, upgrade with active jobs and original durable state preserved |
| Installation and distribution | A clean supported machine can install and run the intended mode from reviewed artifacts, with accurate support, provenance, activation/upgrade/rollback and documented integrity/authentication |
| Security and rights | Actual required secret/dependency/workflow checks, evidence-based finding dispositions, dependency/asset notices, reviewed license and release inventory |
| Community documentation | Tested quickstart and CLI examples, working links and media, current support/limits, contribution instructions and a usable private security-reporting channel |
| Full work packets | Reconcile P00–P31 against their actual criteria; only P00/P01 are currently accepted |

The release state cannot be promoted by changing an advisory audit profile,
counting component tests, renaming a stage, replaying a consumed provider grant,
or removing a failing assertion. The
[remaining-work audit](../plans/admiralty-v2/REMAINING_20261019.md) and
[work packets](../plans/admiralty-v2/WORK_PACKETS.md) retain the detailed criteria.

## Contributing and reporting

Use [CONTRIBUTING.md](../../CONTRIBUTING.md) for local checks and source ownership.
Keep one integration PR and preserve dirty, ignored, foreign and unmerged work.
Runtime databases, credentials, transcripts and raw security evidence stay out
of Git. Build output and generated files are not hand-edited source.

Use [private vulnerability reporting](https://github.com/neverhuman/admiralty/security/advisories/new)
for suspected security issues. This repository setting was enabled and verified
on 19 October 2026; [SECURITY.md](../../SECURITY.md) records the fallback if the
form is unavailable. No response-time guarantee is published.

