# Contributor routing

Read these before changing source:

1. `docs/plans/admiralty-v2/MASTER_PLAN.md` — architecture and packet criteria.
2. `docs/plans/admiralty-v2/EXECUTION.md` — integrated source and actual evidence.
3. `docs/operators/release-status.md` — demonstrated behavior and remaining gates.
4. `captains/COMMON.md` — coordination, resource, review and merge rules.

Runtime/CLI/tools use Rust. The UI uses Vite, TypeScript and React. New Python
requires the maintainer's exception; existing retained sources and retirement
tasks are inventoried in `PYTHON_EXCEPTIONS.md` and `PY_TO_RUST_PLAN.md`.

Use one writer per checkout and one active integration PR. Inspect status first;
preserve foreign, dirty, unmerged, ignored and custody-held work. Reuse an
appropriate checkout before creating another. Freeze base, exclusive paths,
dependencies, checks and receipt paths before assigning concurrent work.
The integrator owns shared Store/migrations, public contracts, startup/auth and
shared application wiring; obtain an explicit source scope before overlapping.

`packages/contracts/generated/` and generator-owned exports are generated from
`schemas/` and `scripts/contracts/`. Change source inputs, run
`pnpm contracts:generate`, then `pnpm contracts:check`; do not patch generated
outputs by hand. Preserve historical receipts and original failures.

Run `pnpm check` before pushing. It invokes `scripts/check/all.sh` with the full
local application matrix; see `docs/operators/local-ci.md` for Node24,
pnpm11.15.1, native/browser prerequisites and retained logs. Use RTK when
available. Compiler/test concurrency and private scratch are bounded by the
script; do not remove assertions or weaken limits to make a run pass.
Linux-only qualification and the pinned Jankurai audit are separate evidence
when they cannot execute on the local platform. Review audit findings against
source and tests; do not erase negative tests or hide genuine findings.

Return exact source/dirty state, commands, tool versions, exits, test counts,
log/artifact hashes and material limits. Local passes do not imply hosted CI
green, deployment, native provider authority, task acceptance or a full release.
Never invent receipts or acceptance. Production has one deployment/configuration
writer; reviews and local tests do not authorize another service controller.
Do not reuse consumed provider grants or install request-issued authority.

The owner removed mandatory independent/second-reviewer approval on 21 October
2026. Review the source and findings, require passing current-head checks, then
use ordinary `gh pr merge --rebase --match-head-commit`. Do not wait for a
`neverhumanbot` approval or fabricate one. No force pushes, admin bypass, squash
merges or merge commits. Follow explicit maintainer authorization; do not add a
new user approval step for already-authorized reversible work.
