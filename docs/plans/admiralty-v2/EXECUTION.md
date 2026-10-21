# Admiralty execution ledger

Receiving-main browser follow-up: PR156 merged to5512531 with all three reviewed
head checks passing. Receiving-main Rust/contracts passed, but browser run39302569957
failed21/23: a10s document-ready wait and a public-view30s whole-test override.
Source9c36435 adds explicit bounded navigation readiness/error diagnostics,
an actual Chromium navigation-denial control and awaited public-fixture cleanup
after a deadline. Final actual Mac browser suite passes24/24; fresh hosted and
receiving-main results remain separate. Full logs, failed invocation and exact
pins: [browser-navigation-20261019.json](../../../receipts/integration/browser-navigation-20261019.json).
The sole deployment owner rolled back the5512531 daemon launch; native supervisor
adoption and full release remain open. This does not promote parent acceptance.

Planning baseline: `1e8ed17eff27ed3ab80dc6860fc8eda301406aee`. Implementation started on 18 October 2026. PR #9 was merged by the owner at head `9debe5dd` on 18 October at 13:58 UTC. Subsequent main commits contain the offline workplan registry, controlled Codex harness and private scratch correction. The read-only dashboard still runs its separately pinned binary/assets; merging source does not deploy it. Existing coding projects retain their lanes and ownership. Native v2 execution remains disabled pending qualification.

Read [MASTER_PLAN.md](MASTER_PLAN.md) and [WORK_PACKETS.md](WORK_PACKETS.md). Integrator assigns one current base/interface version and exact write scope per packet. Update this table from evidence; do not infer completion from agent messages.

Latest full remaining-work matrix: [19 October, source b2b9e7a6](REMAINING_20261019.md), with later source-specific corrections below. The native Rust dashboard/observer are now primary; P00/P01 foundation criteria are accepted at `ebc5a8dc`. P02–P31 remain open. Older merge-window, deployment and queued-CI notes are historical. Dashboard shared-workspace `verified` labels never establish whole-packet acceptance or general autonomous execution qualification.

Native release priority (owner steering, 18 October): I0 scoped pending index is
merged in [PR63](https://github.com/neverhuman/admiralty/pull/63), receiving main
`e8f11153f5d0432f6833eb3c52a432f7c15463e2`. I1 now implements the
[authoritative lifecycle seam](../../architecture/native-lifecycle.md): version2
CAS decisions, exact optional-effect authorization, closure-bound retry,
conservative revisions and accepted-dependency release. It remains an inactive
component until actual runtime authority, admitted execution, trusted checks,
React v2 support and isolated recovery/cutover gates are integrated. Existing
Screen workers have concrete disjoint production packets for authentication,
execution, checks, UI, workspace preparation, resource admission, recovery and
controller integration. Pointer delivery and passing component tests do not
establish completed workers or parent-packet acceptance. The owner authorizes
additional Luna fleet workers and hard cutover after operational qualification;
registered resource limits, unrelated jobs and preserved work remain in force.

| Packet | State | Owner | Integrated commit or candidate | Evidence and next action |
| --- | --- | --- | --- | --- |
| P00 | accepted | evidence lane + independent integration review | `ebc5a8dc081185d2824f653fc3924147d66d11ed` | 37 pinned hashes, 11 explicitly synthetic cases, distinct 449/503 baselines, unknown-explicit inventory and actual legacy-import checks; criterion receipt and archived atomic rollout update below |
| P01 | accepted | integration + independent quality review | `ebc5a8dc081185d2824f653fc3924147d66d11ed` | Named harnesses, pinned toolchains/CI, deterministic fake clock/fault/forge and actual effect-free binary help verified; 1,061 receiving-main Rust checks; production execution remains a separate gate |
| P02 | in_progress | domain + integration review | reviewed `03772f94` | Typed contracts, pure reducers and schema/TS checks are integrated; complete authority/policy negotiation remains pending |
| P03 | in_progress | local store agent + integration reviewer | reviewed `29acc013` | Atomic domain/outbox admission, immutable request/effect associations, replay and receive-time retry compatibility tested; durable executor admission loop pending |
| P04 | in_progress | artifact + custody lanes + strong review | reviewed `29acc013` | Transactional asset/pin catalog with full observation binding, fixed clock policy and receipt closure; trusted filesystem observation/startup activation pending |
| P05 | in_progress | local coordination team + strong review | reviewed `318ac713` | Mailbox collision/history/envelope bounds, authorized bounded topics and scoped queue-history pages reviewed and tested; trusted service authentication and durable delivery/subscription activation pending |
| P06 | in_progress | local context team | PR prototype | Constraint-preserving capsules, epochs and bounded attention governor; durable governor/outbox activation and retrieval pending |
| P07 | in_progress | remote session lane + local reviewer | runtime `e2b540fb`; fixture `661bf36f` | Private socket fixture now bounds framing, connect, I/O and owned-child cleanup; same-daemon client reconnect passes; control-service restart adoption and admitted fleet activation unqualified |
| P08 | in_progress | provider + integration review | reviewed `455ee5f4`; Round 7 `b28a7f7`; Round 8 `8c588ad`, `d755b8f`, `d069cf8` | Round 7 provider fault tests and Round 8 worktree isolation/publication/Codex capture qualifications are batched on `codex/batch-item2-p08-p09-p12`; exact-main focused checks and full branch gates pass. Main merge is held for the coordinated window; packet remains unaccepted |
| P09 | in_progress | remote workspace lane + local reviewer | candidate `f0d15a82`, imported `d637bfe`; Round 7 `3a71e82`; Round 8 `35810f5`, `75d07d4`, `a8c1885` | Round 7 runner-isolation fault cases and Round 8 P09/P16, P09/P20, and P09/P25/P30 qualifications are batched on `codex/batch-item2-p08-p09-p12`; workspace-registry checks and full branch gates pass. Main merge is held for the coordinated window; packet remains unaccepted |
| P10 | in_progress | usage/reservation lanes + strong review | candidate `99f05b0`, imported `61e888b` | Segmented usage plus conservative scoped reservations reviewed; runtime reservation activation pending |
| P11 | in_progress | API + integration review | reviewed `17c7a76d` | Authenticated bounded read API/SSE and offline CLI are tested; live command envelopes, terminal input and agent tools pending |
| P12 | in_progress | frontend + integration review | live assets `9debe5dd`; Round 7 `a39a7ab`; Round 8 `93ea210`, `453d49e`, `7c6b3ee`, `23fa33d` | Round 7 integration-generation clock-skew test and Round 8 P12/P18, P12/P23, P12/P26/P31, and cross-packet packaging qualifications are batched on `codex/batch-item2-p08-p09-p12`; dashboard and full branch gates pass. Main merge is held for the coordinated window; packet remains unaccepted |
| P13 | in_progress | store + integration review | reviewed `455ee5f4` | Durable immutable JSON revisions, CAS/retry, reciprocal task identity, writer/CLI and backup/rebuild tests; TOML/Markdown, import UI and live admission pending |
| P14 | in_progress | scheduler lane + strong review | candidate `4aa3abe`, imported `2ef27f4` | Pure scheduler independently hardened at `03772f94`; durable scheduling and launches remain pending |
| P15 | in_progress | verifier lane + strong review | candidate `a4c358`, imported `1f8b32c` | Scoped evidence consistency hardened at `03772f94`; trusted independent verifier execution pending |
| P16 | in_progress | Luna audit lane + strong review | candidate `e8b9321`, imported `10fe48e` | Staged audit candidate under independent review; no Jankurai execution qualification |
| P17 | in_progress | Luna interface lane + strong review | candidate `fb366ab`, imported `3df778c` | Scoped interface reducer reviewed; peer tools/durable transport pending |
| P18 | in_progress | Luna conflict lane + strong review | candidate `54d57e0`, imported `03d42fe` | Bounded conservative conflict graph reviewed; merge train/runtime integration pending |
| P19 | in_progress | Luna publication lane + strong review | candidate `c3e3289`, imported `4a7eee0` | Frozen request/receipt publication fences reviewed; credential gateway/forge qualification pending |
| P20 | in_progress | Luna retirement lane + strong review | candidate `164aee7`, imported `94336b5` | Scope/pin/identity/replay retirement proposals reviewed; no destructive executor or permission |
| P21 | in_progress | provider implementation + independent cross-review | PR26 `661bf36f`, offline compiled/tested | 22 registered tests and shared strict decoder address offline counterexamples; unadmitted launch/configuration/journal/real transport and nested usage qualification remain pending |
| P22 | in_progress | provider implementation + independent cross-review | PR26 `661bf36f`, offline compiled/tested | 12 registered matrix tests cover portable fixtures, truthful identity hints, strict incremental framing and shared launch envelopes; native identity/journal/transport qualification remains pending |
| P23 | in_progress | Luna enrollment lane + strong review | candidate `e3a9125`, imported `a6637a6` | Scope/clock/revocation enrollment contracts reviewed; real enrollment/replay transport pending |
| P24 | in_progress | Luna placement lane + strong review | candidate `a791470`, imported `08c3a38` | Scoped fresh ownership/continuation placement proposals reviewed; real partition qualification pending |
| P25 | in_progress | Luna recovery lane + strong review | candidate `fa77ebd`, imported `4494af9` | Snapshot/fence/closure recovery bindings reviewed; executor restore/cutover qualification pending |
| P26 | in_progress | Luna economics lane + strong review | candidate `4aec329`, imported `78688d4` | Exact scoped usage/correction/coverage economics reviewed; explorer/service integration pending |
| P27 | in_progress | Luna excursions lane + strong review | candidate `c3a1895`, imported `1fe0bb2` | Canonical bounded scoped excursion state reviewed; durable proposal loop activation pending |
| P28 | in_progress | Luna trials lane + strong review | candidate `142bd2a`, imported `87b0a94` | Fixed cohort/seed/oracle trial analysis reviewed; independent live trials pending |
| P29 | in_progress | diagnostics + frontend review | reviewed `17c7a76d` | Bounded recorded diagnostic CLI tested; durable action/job, assets/fleet UX and named accessibility/E2E gates pending |
| P30 | in_progress | release + integration review | reviewed `17c7a76d` | Explicit-input archive builder passes 10 checks; clean-install, signed distribution, rights and complete operator paths pending |
| P31 | in_progress | integration + qualification review | reviewed `455ee5f4` | Isolated binary smoke and offline controlled provider harness are integrated; real provider, soak, savings, restore and fleet cutover gates remain open |

## Git Flow viewport UX — 21 October 2026 candidate

New user-authorized feature lane `codex/git-flow-viewport` starts at
`5d4e656bbe7438a25728aefbac78b2576e1732ed`. Root owns UI integration;
helper worker delivered pure camera math and nine tests in disjoint new files.
The existing Git Flow owner verified no overlapping source writer and is routing
Captain review designation/Admiral tracking through existing coordination logs.
Actual reviewer adoption, exact-head approval, local/hosted qualification,
ordinary merge and serialized rollout remain separate evidence gates.

The [viewport scope record](GITFLOW_VIEWPORT_2026-10-21.md) tracks explicit100%
snap, anchored zoom, project focus/return and responsive
canvas/gutter math. Original dirty checkout, runtime/generated contracts,
private access/chat boundaries and sole deployment controls remain preserved.
Preview tests are candidate evidence; no new source merge or deployment is
claimed by this entry. The final PR/qualification receipt records later results.

The human's subsequent correction makes **per-repository investigation** authoritative: choose a repository, Expand project, fit both loaded axes with padding, then continuously zoom/pan/drill/inspect and return with a remembered camera. A fleet-wide fullscreen map is superseded. The same PR166 retains sole source ownership and existing Admiral/Captain coordination. Current source extends the viewport record with fullscreen integration, per-instance camera, bounded source/disabled-health explanations and actual CDP drag qualification. No merge, final review, hosted success or deployment is claimed by this source snapshot.

## Git Flow and Fleet Atlas — qualified source, 19 October 2026 UTC

Merged [PR95](https://github.com/neverhuman/admiralty/pull/95) introduced the base
views at `35e5890`; [PR96](https://github.com/neverhuman/admiralty/pull/96), branch
`codex/live-view-review-repairs`, implements the worker-ready
[LIVE_VIEWS_PLAN](LIVE_VIEWS_PLAN.md) and [operator contract](../../operators/live-views.md).
Source `a21532a3147e7a30d47caf02f18d27fa05fb5545`, based on main
`99f4e26cebfab97afa661ed05aea6192bfd5935c`, was qualified directly from a clean
checkout. The [final LV receipt](receipts/LV/a21532a3147e7a30d47caf02f18d27fa05fb5545.json)
stores exact commands, computed hashes, failed runs/repairs and screenshots.
Earlier PR86/88/93 candidates and the `9cde56d` receipt remain historical evidence.
The final source preserves the upstream native task-loop probe, `sha2`
dependency, browser form deadlines and CI resource limits. Published histories
are preserved. PR95 merged at `35e5890`; PR96 merged at `5126af0`, preserving
upstream Store attention. Historical live-view qualification is source-bound;
aggregate current-main crate/CI objects include separate upstream changes.

Git Flow retains original ordered parents, shallow boundaries, refs and
main/linked/detached/unborn checkouts with bounded working-state observations.
Fleet Atlas retains configured command roles, actual process identities,
native task assignments, unassigned agents, exact model evidence and its
original timestamps. Both routes share authenticated current-state SSE,
inspectors, URL navigation, complete loaded inventories and full-window mode.

Direct checks pass 1,164 Rust tests (five existing ignored), 114 dashboard tests,
eleven contract runtime tests, legacy qualification and all nineteen browser
checks. These include real temporary Git → observer → server → built UI,
a synthetic 500-agent census, and nine pinned axe surfaces with zero violations
and zero incomplete. Package/canary self-tests pass. Local macOS proof does not
establish hosted Linux CI success. Hosted PR96 browser/contracts passed; Rust
failed on six owned view-fixture permissions and a separate native-executor
controlled-child deadline. Six failures reproduce under umask `0002`. The
fixture-only repair creates `0700` scratch directories atomically and preserves
Store guards and production views. Source `f4432a2`, on current main `f655b15`,
is pinned in the [fixture receipt](receipts/LV/f4432a204204d2848052580d6f4f0355a8e9ed7d.json)
with targeted format, strict server Clippy, server-library and build proof under
umask `0002`. The authorized read-only reviewer found no actionable issues in
the identical fixture change `4432c832`. No full-suite rerun is inferred.
Exact-final-head neverhumanbot approval and normal merge gates remain required.

The authorized read-only reviewer found no additional actionable findings after
repairing conflicting project/manager claims and Git Follow latest after
Inventory updates. Reversed-order Rust fixtures and a failing-before/passing-after
browser counterexample cover those repairs. A plain scroll-roundtrip theory was
disproved and retracted; initial browser setup failures remain explicit.

Independent review repairs cover each linked worktree's effective filters,
retained Git on matching native projects, manager deduplication, ambiguous
identity joins, independent model timestamps and explicit schema rejection.
Native command production code is unchanged. Earlier failures, Cargo overlap
resolution and the test-only HTTP fixture repair are retained. This change does
not close any parent P00–P31 packet or qualify production rollout, opaque
provider completeness, remote Git transport, arbitrary history replay or
performance/soak targets.

## Receipt template

Save at `receipts/<packet>/<actual-commit>.json`; do not create an example file with invented commit hashes. Large raw output belongs in artifacts, not this table.

```json
{
  "packet": "P00",
  "state": "review",
  "owner": "assigned principal",
  "base_sha": "computed full OID",
  "head_sha": "computed full OID",
  "contract_version": "actual version",
  "changed_paths": [],
  "criteria": [{"name": "criterion", "result": "pass|fail|not_run", "evidence": []}],
  "validation": [{"argv": [], "cwd": "actual path", "exit_code": 0, "artifact": "computed reference"}],
  "environment": {},
  "decisions": [],
  "limitations": [],
  "blockers": [],
  "next_action": "bounded action"
}
```



### Queue item 2 batch (2026-10-18)

P08, P09, and P12 Round 7 worker commits are staged together on
`codex/batch-item2-p08-p09-p12`, based on `origin/main` `c513a4e`. The batch
contains provider fault coverage (`b28a7f7`), runner-isolation fault coverage
(`3a71e82`), and clock-skew integration-generation coverage (`a39a7ab`). Exact-main
focused checks passed on `c513a4e`; full workspace format, clippy, tests, and
contract checks passed on the batch code. P08/P09/P12 remain in progress and
not packet-accepted. Main merge is held for the coordinated window; PR #38
remains local-CI-only until its main workflow is green.



### Queue item 2 Round 8 consolidation (2026-10-18)

All 12 Round 8 worker commits named in the captain order are integrated as
single-parent commits on `codex/batch-item2-p08-p09-p12`, based on
`c513a4e38092ac6c2e4e8464537aa1988744c02f`. The P08/P09/P12 packet receipts
record their worker commit mapping and the full workspace qualification at
`b7a0ceeb01d51dca5861eeada5b2dfbf79809614`. Formatting, workspace clippy,
workspace tests (712 passed, 0 failed, 1 ignored), and `pnpm contracts:check`
passed using the specified shared acceptance target and private scratch path.
Round 8 receipts are supplemental and leave existing rollout packet statuses
unchanged; main merge is held for the 17:00Z–17:05Z batched window.

The receipt is engineering evidence, not product acceptance. Integrator verifies actual resulting tree/tests and records the integrated SHA. Reconcile planning estimates after P08/P15 establish real provider/verifier throughput; preserve earlier estimates as history rather than silently changing a committed delivery promise.

## Live build visibility

The existing legacy dashboard has a generic rollout summary and expanded agent
table. Rust and React now serve real producer observations on loopback18891;
the original dashboard stays available on18890 and under `/legacy/`. React assets
are pinned to `9debe5dd`; the legacy roster bridge is `b2523eb2` and the running Rust
binary is `17c7a76d`. These are immutable, separately deployed canary revisions, now represented in merged source. The default page
is the detailed real fleet; synthetic demo now defaults to18892.

A bounded observer joins captain reports into runtime JSON every15 seconds
without model turns. It independently checks owned `adm_` Screen identities and
bounded candidate-path/HEAD metadata; producer report age is preserved. The
collector consumes that projection on its next cycle. The initial three GNU Screens
ran Luna with enforced CPU pairs104–109 in isolated worktrees. Startup trust
prompts were independently observed and cleared for this checkout before edits
began. Those candidates and the subsequent scheduler, verifier and config batch
are imported for independent review. Later pool results are recorded below. Process
presence, file edits and passing candidate tests never complete parent packets.

Current native read path: max 16 SSE bodies, max 8 concurrent reads; a coalesced
writer notification wakes subscribers, which then re-read durable cursor pages.
These bounds and offline tests do not constitute fleet-scale performance or
paid-provider qualification. Existing project scheduling/authority remains in
the legacy system until the planned qualification gates pass.

Prototype integration evidence at `109563722a4c03941b18e79d180a31c9225369fb`: 95 Rust tests and strict workspace Clippy pass. Partial receipts under `receipts/P02`, `P03`, `P05`, `P06`, `P07` and `P11` name the tested code commit and remaining acceptance gaps.


## Owner steering: detailed UI, own models and more parallel workers

React now preserves detailed legacy project cards and side panels and treats
Admiralty as an ordinary project tab. A configured original-page fallback stays
available. Private model configuration supports own-model tiers below Luna,
provider-native identity, caps and explicit escalation, with setup/input failures
waiting for repair. One corrected Ornith/OpenCode coding task passed8 independent
methods and80 seeded cases in30.039s; its first attempt is retained as a checkout
setup/permission failure. This qualifies that observed helper tuple, not the
entire native managed execution/recovery path or all model task classes.

The remote captain has moved the next workplan, question-lifecycle and workspace
packets to actual GNU Screen+taskset lanes in its reserved CPU scope. The prior
artifact-file and segmented-usage candidates have been imported for review.
Additional independent local workers implement a captain attention governor and
an outbox component while the UI/model/provider integration is deployed. Parent
packets remain partial; no full-fleet cutover or merge gate is bypassed.


## Dense console restoration and expanded execution

At code `617e9127`,184 Rust tests, strict workspace Clippy and formatting pass.
Partial component receipts name the tested commit and keep parent acceptance
false. UI code `8474dec9` restores the black fleet console, bright model badges,
compact hierarchy, model-mix and usage tables, plus all13 expandable temporal
metrics.65 React tests, typecheck and production build pass; this exact asset
revision is deployed on18891 and verified against real producer history.
The user explicitly requested aggressive Admiralty-first Luna parallelism; an
expanded pool is being admitted within the existing worker CPU reservation.
Record actual starts in the private lane manifest before describing workers as
running. Strong integration review and existing CI remain in effect.

At code `03772f94`, the independently hardened scheduler/config/verifier components
pass223 Rust checks (including one compile-fail doctest), strict workspace Clippy
and formatting. Receipts under P02/P14/P15 retain the runtime activation gaps.
At11:20UTC,12 newLuna providerPIDs were independently verified on workerCPUs104–109;
the private manifest retains3 earlier candidates separately. No other project
received dispatch changes.

## First expanded pool: candidates and review

All12 initial expanded Luna lanes produced clean candidates within their assigned
write scopes. Four completed lanes were reused for staged audits, recorded
operator diagnostics, explicit-input packaging and bounded canary smoke checks;
all four produced candidates too. These16 candidates are imported for review.
The12 earlier component candidates now have stronger independent review fixes;
staged audit and operator CLI review continue. Packaging and smoke checks are
being integrated into CI. No full parent packet is accepted by this count.

The owner confirmed Admiralty first. The next isolated pool contains six real
integration-test slices (task lifecycle, mailbox restart, backup reopen, HTTP
access/projection, offline CLI round-trip and combined context/mailbox fixtures)
and three implementation slices (atomic domain/outbox admission, transactional
asset pins and durable question journal). Only observed starts belong in the
private live lane manifest; prepared work is not described as running. The
original projects receive no dispatch changes.

At code `17c7a76d`,357 Rust checks (including two compile-fail doctests),
strict workspace Clippy,66 React tests/typecheck/build,73 legacy Python tests
(one platform skip), five contract runtime checks and both contract/schema drift
checks pass. The package builder passes10 self-tests; the bounded smoke harness
passes16 self-tests and 15 actual isolated binary HTTP/CLI checks. The latter
caught and fixed model endpoint authentication occurring after JSON extraction.
Partial receipts for16 component packets and an aggregate integration receipt
record remaining activation and qualification gaps. GitHub jobs were queued at
the last observation; no hosted CI success is claimed.

The exact live roster bridge is now `b2523eb2`; it validates activity timestamps
against the observation after Screen capture rather than the earlier manifest
read, while retaining unknown/future/rollback safeguards. The subsequently upgraded
canary runs binary `17c7a76d` with React assets `0b8b8cb2`. No other project was
dispatched or upgraded.

## Current review and manager identity

All six follow-up integration-test lanes and all three durable storage lanes
produced scoped candidates. Their imports received stronger review and root
combined-tree validation. Store hardening is committed at `29acc013`; its partial
P03/P04/P05 receipts preserve activation gaps. Session-host and Codex adapter
follow-ups are reviewed at `997513f6`: shared prompt-envelope limits and explicit
final capture watermarks close the two identified seams. Durable launch-intent
admission, trusted custody, actual provider execution and fleet recovery still
require integration/qualification before native activation.

React assets `0b8b8cb2` pass75 tests, typecheck and production build. The live
Commodore card now uses the explicitly configured, observed manager's exact
Screen ID/PID rather than a hardcoded naming assumption. The actual Gemini 4
Argon manager was independently observed and verified in the browser; the older
Luna session remains a worker observation. Missing/mismatched managers stay
unknown. Admiral/Commodore UX labels, the Grokbot asset and browser-local profile
photo upload are deployed. These are display/selection changes, not model-name
substitution or native provider qualification.

Smoke harness `5e1bc679` fixes deadline shutdown appearing as successful EOF and
rejects truncated HTTP bodies. All18 harness checks and 15 actual isolated binary
HTTP/CLI checks pass. The native canary remains read-only; no complete packet,
production execution or hosted CI success is inferred from these checks.

At source `997513f6`, the Mac combined tree passes 439 Rust checks (including
two compile-fail doctests), strict workspace Clippy, formatting and 15 actual
isolated binary HTTP/CLI smoke checks. Raw local logs and hashes are retained in
`target/quality/997513f6a0ad832b810864807fdc961070dc0974/`.
The unchanged React source passes 75 tests/typecheck/build; legacy Python has 73
passes and one platform skip. Store 62 and smoke-harness 18 checks also pass on
exact clean Linux source `5e1bc679`. Linux runtime at `997513f6` failed four
cleanup EOF checks (117 passed); the failed receipt is retained. The native
process-group correction `e2b540fb` replaces platform-specific signal utility
arguments with guarded Rustix calls and adds two real regressions. The Mac
combined tree now passes 441 checks, strict Clippy and formatting. Exact-source
Linux runtime passes all 125 checks (123 library, one binary and one doctest),
including all four earlier failures. Both raw failure and passing evidence are
retained; the aggregate receipt and partial P07/P08 receipts name the tested
source. GitHub jobs remain queued at the last observation.


## Thirty-minute implementation and quality checkpoint

At code `ebec880b`, durable offline JSON workplan registration now goes through
one owned Store writer with immutable revisions, command replay, CAS, retained
task identities and reciprocal submission guards. Batched startup/rebuild checks
validate shared plan history once and reject corrupt bindings before changing a
recovery generation. Offline CLI preview/register/show/list preserves the original
preview syntax. TOML/Markdown adapters, import UI, live commands and task admission
remain open P13 work.

The same commit adds P08Q's opt-in controlled exact-fixture Codex transport harness.
It uses the existing prepared launch, owned session, fenced EOF, journal decoder
and fixed capture watermark. Raw executable hashes, protected fixture checks and
failed cleanup evidence are retained. Its offline example tests are registered
in ordinary workspace CI. No paid provider run or native qualification occurred.

Exact-source Linux validation caught default temporary directory permissions
inheriting umask `0002`, correctly rejected by Store custody. The failed `ebec880b`
receipt/log is preserved. Correction `455ee5f4` requests private mode `0700` at
creation for the demo and affected fixtures; an owned child regression exercises
that umask without changing the test process globally. The Mac combined tree
passes 464 Rust checks, strict workspace Clippy, formatting, schemas and 15 actual
isolated binary smoke checks under umask `0002`. Linux passes 466 Rust checks (including all six offline
fake-provider example checks), binary build, schemas and 15 isolated binary smoke checks at that same
source. The global `/tmp/.git` directory caused an additional retained environment
failure; the passing run uses a fresh owned 0700 temporary root outside Git.
Neither the custody rule nor the foreign checkout was changed.

At that review baseline, P21 candidate `9cbd0210` and P22 candidate `f5a34805`
were not imported. Subsequent main imports retain them as unregistered, inactive
source prototypes, with the same review blockers. Strong review reproduces 14 P21 counterexamples despite all four original
tests passing; P22 static review identifies test registration, identity, framing
and launch-envelope gaps. Their review receipts name exact source and next fixes.
All 32 parent contracts are reconciled in `receipts/reconciliation/`; none is fully
accepted by component counts. At the initial fleet snapshot, fifteen idle
Luna processes held registered lanes; no new overlapping process or terminal input was introduced.
Three scoped local subagents advanced implementation/review within this chat.

The live canary remains binary `17c7a76d` with UI assets `9debe5dd`, real data,
Admiral/Commodore labels, the observed Gemini manager, profile and temporal plots.
No production checkout, other project, native execution or merge scope changed.
Hosted CI's unassigned queue is distinct from these independent source checks.

A fresh 14:03:49 UTC observation finds 15 owned Luna processes/Screens: six active
and nine idle, determined from provider-session task boundaries. The configured
Gemini 4 Argon Commodore is observed again; /healthz is 200 with native qualification
false. The 14:01:57 producer timestamp and stale 13:51:31 manifest timestamp remain
explicit in the receipt. No idle lane is treated as released capacity.


## Main reconciliation and conservative progress checks

Main `9227e8b` now contains the offline registry/harness and private scratch fix
through rebased commits `0b20ea7`/`9227e8b`. All 18 implementation paths match the
independently tested `455ee5f4` blobs exactly. Historical tested SHAs are not
ancestors of rebased main; the new reconciliation records actual ancestry and
explicit blob equivalence instead of carrying those earlier ancestry claims
forward. Registered Rust inputs, UI/contracts and packaging/smoke sources are
unchanged. The six newly imported provider module/test source files are dormant;
their modules and test targets remain unregistered. Their retained review findings
still require fixes. No full new-head Rust test pass is inferred from import.

At source `b6622f8`, the collector and audit no longer promote bare or inconsistent
receipt claims, override blocked holds with activity/main observations, or treat
`unverified` as a closed lane. Real failure verdicts remain visible. Fourteen new
collector regressions and eleven audit regressions receive separate independent
review. macOS discovers 88 legacy tests (87 pass, one platform skip) and passes
44 audit tests; clean Linux passes all 88 legacy and 44 audit tests.

The first Linux fixture step failed because the verifier invoked the local `rtk`
helper absent from a minimal CI environment. That failed receipt/log is retained.
Source `3312ba0` invokes Git directly; its clean Linux verifier passes all 37 pinned
sources and 11 synthetic cases, explicitly reporting 18 current drift paths.
Unchanged passing Python tests are reused with exact object/log hashes rather
than rerun. CI now includes the audit regression suite and creates private scratch
outside the checkout. Preexisting test-fixture ResourceWarnings are retained.

The successor review branch is `codex/admiralty-quality-checkpoint`. Detailed
current evidence is in `receipts/integration/3312ba03b5ddaaed6818f68c5b0fe4febac3ac5d.json`
and `receipts/reconciliation/2026-10-18T1425Z-3312ba03b5ddaaed6818f68c5b0fe4febac3ac5d.json`.
All 32 parent contracts still have remaining gates, with zero full acceptance.
Main GitHub run 39134086804 was queued without assigned runners at observation;
independent source checks do not substitute for hosted checks or exact-head review.
No new production binary, provider activation, native execution or fleet cutover
occurred. The pinned real-data dashboard, manager identity and other projects stay
intact. Next ready work is scoped provider hardening, resource-permit reconciliation
and live workplan command authority/import UI; complete those before activation.


## Registered offline providers and independent claim reconciliation

PR26 source `a9ba930e` compiles the Claude, AGY, Grok and generic-process
prototypes and registers the two provider integration targets in ordinary Cargo
workspace checks. No server provider route or model-selection enum was added.
A shared bounded incremental JSONL decoder rejects recursive duplicate keys,
scalar roots, partial final frames and sticky failures. Four decoder tests,
22 Claude tests and 12 provider-matrix tests pass and receive independent review.
The four new synthetic Claude JSONL fixtures are explicitly tracked, so clean
checkout checks exercise the same inputs. See
[offline provider observations](../../architecture/provider-observations.md).

Claude now requires one initialized, session-matched complete parent turn and
keeps malformed/absent terminal usage distinct from earlier parent/opaque-child
observations. Multiline user input stays inside the session's encoded 4096-byte
wire bound. Launch proposals explicitly remain unadmitted, with configuration,
resume, nested accounting and native qualification flags false. AGY/Grok wrappers
keep semantic failures sticky; generic argv/model hints never become observed
identity. Complete launch envelopes enforce shared total/serialized bounds and
reject option-like requested values. Real CLI transport, trusted identity,
journal/watermark binding and native admission remain pending P21/P22 gates.

The frozen macOS combined tree passes 502 Rust checks, strict workspace Clippy,
formatting, build, schemas and 15 actual isolated binary smoke checks. Its first
smoke attempt refused the macOS `/var` symlink alias; the failed log is retained.
The same source passes with the canonical `/private/var` scratch path, without
changing the guard or repeating passing Rust suites. Clean Linux at exact commit
`a9ba930e` independently passes 504 Rust checks (including all six fake Codex
example checks), strict workspace Clippy, formatting, build, schemas and 15 actual
isolated smoke checks. Both smoke runs terminated their two owned children.
Unchanged Python/UI/contracts/package/harness checks are reused through explicit
Git-object identity, with source/log hashes, not presented as fresh executions.

The 14:40 UTC independent fleet receipt observes 15 owned workers with reported
open task boundaries, not proven useful progress. Declared candidate receipts
and SHAs had not advanced since 14:03. P21/P22 worktree declarations still resolve
to older scheduler/verifier candidates, so no dispatch/adoption was made through
those pointers. CPU/conversation reservations remain retained; native permit
configuration is distinct from the legacy pool. The actual configured manager
is again visibly Gemini 4 Argon; native execution attestation remains unavailable.

Six new legacy whole-packet claims were independently inspected. P00's artifact
criteria are technically satisfied; a complete live inventory or native provider
qualification is not an additional P00 requirement. Its current receipt now names
a hashed independent execution log, leaving authorized engineering closeout
separate from this observation. P01/P02/P03/P06/P11 have genuine component test
logs but specific complete-contract gaps; they do not close from passing those
subsets. The dashboard projection quarantines the self-claims. All 32 contracts
are reconciled again without issuing native acceptance or silently promoting
parent status. Missing P01 fake-clock/fault/forge/cumulative-reset harness work,
live command/coordination authority and provider admission remain ready follow-ups.

Current evidence: `receipts/integration/a9ba930e6eb3b46b76797582427cce6bacefdaab.json`,
P00/P21/P22 partial receipts at that source, and
`receipts/reconciliation/2026-10-18T1450Z-a9ba930e6eb3b46b76797582427cce6bacefdaab.json`.
Hosted checks and configured exact-head review remain pending independently of
these source checks. No merge, production deployment, native activation or
other-project dispatch occurred.


## Combined packet harness checkpoint and later-main review

Frozen source `661bf36f` integrates main's `8366b581` registration of all 30 named
packet harnesses with the independently hardened provider implementations. The
three wiring conflicts were resolved without losing registered targets or
reverting the strict shared decoder. Five newly active fixtures needed repairs:
canonical private directories created with mode 0700; bounded response framing,
connection and I/O deadlines; short portable Unix-socket names; and owned child
cleanup. The package self-test child now has a 30-second deadline and 64 KiB
output caps. An exceptional unreaped child is reported explicitly and handed to
an eventual reaper; foreground cleanup does not claim guaranteed descendant
termination. The session fixture exercises client reconnects to the same daemon
and child, not a control-service restart. Independent cross-review cleared the
five scoped repairs after identifying and correcting the last two deadline gaps.

At exact source `661bf36f`, macOS passes 558 Rust checks and clean Linux passes
560, including doctests and all 30 registered integration targets. Both pass
formatting, strict workspace Clippy, binary build, schemas, the 37-source/11-case
legacy fixture verifier, and 15 actual isolated HTTP/CLI smoke checks. Both smoke
children terminate. Tests inherit umask 0002 and use canonical owned private
scratch outside Git. The ignored session worker helper is explicitly spawned by
its owning continuity test. Earlier failed preflight output is retained; one
initial failure survives only as an explicitly labelled excerpt, and earlier
session EINVAL failures have transcript references but no saved raw-file hash.
Unchanged React, contracts, legacy/audit Python, packaging and smoke-harness
checks are reused through exact Git-object identity. No repeat of those passing
suites or fresh Node run is claimed.

The 15:12–15:13 UTC fleet observation finds 15 retained owned workers: 11 reported
completed turns and four open turns with recent tool calls/results. No candidate
HEAD or receipt advanced since 14:40. No confirmed stalled worker was found.
Three task/worktree bindings and the stale workqueue need owner reconciliation
before reuse; idle processes do not release their retained CPU/conversation
reservations. The configured manager remains visibly Gemini 4 Argon. Its new
32/32 integrated claim lacks verification/integration provenance; the live
collector correctly displays 32 claimed and zero integrated. Root sent the
manager the contradictory evidence and remaining contract gates. This checkpoint
reconciles all 32 parents again and issues no whole-packet or native acceptance.
P00's technically verified artifact criteria stay distinct from engineering
closeout; native provider qualification is not an extra P00 prerequisite.

After the frozen checkpoint, another lane merged main `429595d` (P05 helpers)
and `a73fc557` (audit changes). They are excluded from the 661 proof. Independent
static review of exact P05 objects finds silent changed-payload ID/sequence
collision suppression, unbounded ACKed history, sequence-zero/overflow hazards,
missing complete-envelope validation, unbounded insufficiently authorized topic
routing, and an unpaged global-snapshot wrapper. Existing durable Store checks
remain intact; the new wrappers do not activate durable subscription/replay,
delivery/expiry or authenticated commands. Repair those bounded components with
negative regressions before accepting P05 or integrating that later source here.
Do not infer full packet acceptance from a harness with the packet's name.

GitHub PR26 run 39138996888 was cancelled by the owner account before any test
steps. No hosted success or exact-head configured review is recorded. Root has
not merged a PR, deployed a new binary/UI, changed other projects or enabled
native execution. The browser still shows real data, 13 temporal plots, bright
models, the ordinary Admiralty tab, owner profile and Admiral/Commodore hierarchy.

Evidence: `receipts/integration/661bf36f0a401c79c87e2166fd75be325984e09c.json`,
`receipts/reconciliation/2026-10-18T1515Z-661bf36f0a401c79c87e2166fd75be325984e09c.json`
and `receipts/reviews/429595d196fcb6b730d5683e04d479faa1ec968c-P05.json`.
Next ready work is the scoped P05 repair, stale lane/permit reconciliation,
missing P01 deterministic fault/clock/forge/reset harnesses, and live command
admission. Hosted CI and exact-head review remain separate merge gates.


## Coordination hardening checkpoint

Source `318ac713` incorporates the reviewed main P05 additions through a clean
local merge, then repairs the concrete offline blockers. Two disjoint agents own
mailbox/envelope and topic files; integration owns the server wrapper and shared
harness. Mailboxes now reject invalid capacities, sequence zero and full-envelope
errors; exact replay requires the same ID, sequence and complete immutable
message. Conflicts leave state unchanged. ACKed history retains collision
evidence within the same bound, with explicit sticky resnapshot on exhaustion
and checked prefix arithmetic. Topic subscriptions and pages require injected
trusted-service policy, hard byte/container/page limits and authorization before
mutation. Reads inspect at most limit+1 targets and return no unbounded clones.

The unpaged global-snapshot message wrapper is replaced by recipient/epoch queue
observation replay over 1–16 durable events. Every page checks actual caller
policy before Store access; scope and canonical cursor fields are bounded before
cloning/enqueueing. Independent review found and closed that last cursor-bound
seam. Empty filtered pages still advance the scanned cursor, so consumers cannot
mistake them for EOF. ACKed observations remain historical facts. Real Store
reopen and recovery-generation tests cover the supported semantics. These are
building blocks, not an authenticated agent route, pending-delivery queue or
activated durable broker. See
[component boundaries](../../architecture/coordination-observation-limits.md).

At exact source `318ac713`, macOS passes 582 Rust checks and clean Linux passes
584, including all 30 named integration targets, 19 new mailbox/envelope/topic
unit regressions and six coordination integration checks. Both pass formatting,
strict workspace Clippy, schemas, binary build, the pinned 37-source/11-case
fixture verifier, 50 audit tests and 15 actual isolated binary HTTP/CLI smokes.
Both owned smoke children terminate. Fresh legacy checks discover 88 on macOS
(87 pass, one platform skip) and pass all 88 on Linux. The legacy/audit rerun is
justified by the merged tools tree change. Unchanged React/contracts/package and
smoke-harness checks are reused by exact object identity. The intermediate
concurrent topic Default lint warning was corrected; its original output has no
saved raw log, while both final strict logs and hashes are retained.

The 15:23 fleet snapshot observes 15 newly started turns but no new candidate
HEADs/receipts. Bounded sanitized latest-input classification finds only
review/verification/status nudges, with no explicit implementation directive;
source actor and authority remain unknown. Do not call those implementation
progress. The manager remains visibly Gemini 4 Argon. Its correction application
is unproven: all 32 whole-packet claims, three stale worktree bindings and the
11:16 workqueue remain unchanged. Their legacy CPU reservations are still held;
no new dispatch, process interruption or native permit was introduced by root.
The live collector continues to quarantine all 32 claims, and health remains OK
with native qualification false. The UI retains its real-data console and plots.

Current evidence: `receipts/P05/318ac71330d23b2cc57824a06cad8456d74eaa05.json`,
`receipts/integration/318ac71330d23b2cc57824a06cad8456d74eaa05.json` and
`receipts/reconciliation/2026-10-18T1535Z-318ac71330d23b2cc57824a06cad8456d74eaa05.json`.
The source proof excludes the later hourly data export `f85725f`, which changes
only 11 history inputs. Hosted run 39140691489 is still queued with no assigned
runner/steps at observation; inspected APIs expose no cause. Required hosted
checks and configured exact-head review remain separate gates. No PR merge,
production binary/UI deployment or native cutover occurred. Next ready work is
manager task/permit reconciliation, the missing P01 deterministic fault matrix,
and the durable coordination service authority/delivery interface and executor.


## External PR26 merge and receipt successor

While root published the 318-source receipts, another integrator pushed
`703c9c2` to PR26 and merged it as main `0d7d77e` at 15:36:31 UTC. A configured
neverhumanbot APPROVED review is bound to exact head `703c9c2`; the hosted jobs
were cancelled, so no hosted pass is inferred from its local-check summary.
Root's stale push was rejected without overwriting remote work. Root preserved
its committed receipts and moved them onto `codex/admiralty-coordination-evidence`
from that main; root did not perform the PR merge or a force push.

The only changed code since tested source `318ac713` is eight finite timing values in
`codex_qualification.rs`'s cfg(test) fixtures. Independent review verifies the
production prefix and every assertion are byte-identical, including the 20 ms
negative and false native/task-acceptance assertions. Exact successor source
`67f2ccde` passes five Mac and six clean Linux qualification-example checks,
strict example Clippy, formatting and the fixture verifier. Other implementation,
UI, CI and test source objects are unchanged; the full 582/584, 50 audit, legacy
and 15 actual smoke results remain explicitly attributed to318 and reused only
through that object identity. Example reruns are not additional unique tests.
The fresh verifier covers the 11 later history-export inputs. No paid provider,
production deployment, active-session change or native activation was introduced.

Evidence: `receipts/integration/67f2ccde6c12f377aeefd243c2f4e7657d7d6af4.json`
and its 32-row reconciliation in `receipts/reconciliation/`. The successor PR
contains the final source-bound receipts and ledger; the functional fixes are
already on main through the external PR26 merge. Full P00–P31 gates and the
manager's stale bindings/status-only fanout remain the next implementation work.

## Deterministic testkit and current harness checkpoint

Source `50d0abe` fills the missing P01 offline clock, occurrence-fault and forge
fixture infrastructure, plus valid split UTF-8 and cumulative unknown/reset
provider observations. Three agents own disjoint helper files; root owns shared
wiring and a five-test harness over the real mailbox, strict JSONL decoder and
usage accumulator. Independent review checks replay bounds and ambiguity. The
forge remains simulated: admitted intent/reply loss, cached outcomes and CAS
grant no execution, publication or acceptance. Eight cumulative frames contain
six semantic events; opening usage stays unknown. See
[fixture scope](../../architecture/deterministic-testkit.md).

Review also repairs latest-main regressions: the watchdog no longer treats
zero/completed/prose/quoted/fenced/old monitor status as current background work;
newer completed/zero UI status overrides old running status within the original
raw-line recency window. Screen hints still establish no durable task progress.
Canonical directories are private at creation, an owned-child exit observation
is bounded before waiting, and negative tests assert the actual size guard and
valid-digest collision instead of passing through malformed input. The latest
main `0f6b1b6` adds component boundary/replay checks; it is integrated through a
normal merge, preserving those assertions and root's fixes. No force push.

At exact combined source `880b685`, macOS passes 670 Rust checks and clean Linux
passes 672, including all 31 named integration targets. Both pass formatting and
strict workspace Clippy. The full `50d0abe` gate separately passes 639/641 Rust
checks, 67 audit tests, the pinned 37-source/11-case fixture verifier, schemas,
binary builds and 15 actual isolated HTTP/CLI smokes per platform; both smoke
children terminate. Legacy discovers 88 checks (Mac 87 pass/one skip, Linux all
88 pass). Eighteen exact input objects are identical from 50 to 880, so the
unaffected non-Rust-suite checks are reused; combined Rust checks are fresh.
Unchanged React 75, contract 5, package 10 and smoke-harness 18 checks retain
their prior source attribution through nine verified object identities.

The initial 50 Mac workspace attempt fails an existing FIFO three-second elapsed
assertion once. Its unchanged targeted recheck passes in 0.28 seconds, then the
full 50 and combined 880 workspace suites pass. Cause is not established; no
timeout or assertion was relaxed. Initial inherited-target permission failure,
strict unwrap lint and three watchdog negative-test failures are retained or
explicitly identified where only tool output exists. Counts are suite totals,
not additive unique tests.

The 16:04 fleet observation verifies all 15 exact PID/start identities, clean
tracked candidate trees and zero HEAD advances since 15:23. Session activity was
not scanned, so no new implementation progress is inferred. The manager's
visible frame explicitly says Gemini 4 Argon; backend attestation is unavailable.
At 16:07, three current top-level lane rows still disagree with actual branch/HEAD
(P21A, P22A, P04B); historical nested metadata is excluded from that conclusion.
The 11:16 workqueue remains stale. Their legacy CPU holds remain reserved; root
does not release, reset or redispatch them. A compact manager inbox handoff asks
for checkout/task/permit reconciliation and disjoint implementation contracts,
and asks it to hold further shared harness churn during integration.

Live port 18891 remains healthy with native qualification false. Rendered browser
state confirms the 13 temporal cards, detailed real-data console, colored models,
owner/Grokbot images and Admiral/Commodore labels. It exposes PR35 as the current
source reference; 32 verified labels remain observational receipt references,
not whole-packet acceptance. The production checkout and deployed 17c7a76d
canary/UI are preserved. Root performs no model call, PR merge or native cutover.

PR [#35](https://github.com/neverhuman/admiralty/pull/35) contains this source and
its source-bound evidence. Hosted jobs on 880 are queued without assigned runner
or steps at observation; prior main jobs were cancelled. Local proof does not
substitute a hosted success or configured exact-head review. Evidence:
`receipts/P01/880b685be4f003028aa377798a6058014ad5e7e3.json`,
`receipts/integration/880b685be4f003028aa377798a6058014ad5e7e3.json` and the 32-row
`receipts/reconciliation/2026-10-18T1615Z-880b685be4f003028aa377798a6058014ad5e7e3.json`.
All full parent acceptance gates remain separate; P00's prior technical artifact
verification is retained. Next ready implementation is the authenticated bounded
durable coordination command/delivery seam, after manager ownership reconciliation.

## Independent review and continued implementation

Root independently reviews the testkit code, boundary cases, replay semantics,
watchdog and current PR diff. Another integrator rebases PR35 onto the later
`e1710ca` harness additions and merges exact head `bea32822` as `2f693b1a` at
16:19:08 UTC, after neverhumanbot approves that head at 16:19:02. Root preserves
its original history and reconciles the external source through normal merges;
root performs no force push or PR35 merge. Earlier 880 evidence remains scoped
to 880 and is not relabeled as testing the newer harnesses.

The independent review fixes two further concrete issues at source `f16fc759`:
quoted/fenced foreground cancel hints cannot suppress idle detection, and a
newer completed status overrides an older foreground spinner. The new artifact
corruption test now uses the canonical directory private at creation and changes
one byte without changing length; a size mismatch can no longer falsely prove
digest verification. Five failing watchdog subcases are retained before repair.

Exact source `f16fc759` passes macOS 684/Linux 686 Rust checks, strict workspace
Clippy and formatting, 69 audit checks on each platform, the 37-source/11-case
fixture verifier and fresh legacy 88 discovery (one Mac skip). The unchanged
binary/schema/actual smoke and frontend/package inputs retain their earlier
source-attributed proof. Receipt:
`receipts/integration/f16fc759e4d4a9a39364f9670923de0ae7be9d68.json`.
Full packet acceptance and native activation remain separate. Implementation
continues while hosted CI routing is repaired; root is not awaiting user review.
The next bounded pending-inbox Store component is being built with disjoint
ownership while the authorized todoq study and existing general-runner setup run
in parallel.

## Work-order intake and delivery plan amendment — 18 October 2026

Reviewed the supplied “Work Order → ToDo → Work Tracking” specification against
source `c0ebc0935657c21adb254ff57c777c701ff10498`, the existing master/packet plan
and the preceding pinned private todoq study. The attachment is descriptive
material, not authority to operate its named installation or queues. No source
queue, task body or private credential is published by this amendment.

[WORK_ORDER_REVIEW.md](WORK_ORDER_REVIEW.md) records the value-add decisions,
concrete current read/edit paths, six ordered subpackets, frozen interface and
write boundaries, negative cases, commands, receipts and activation limits.
MASTER_PLAN and P02/P13/P14/P19/P25/P29/P31 now explicitly require request origin
and rationale, safe multi-paragraph intake, bounded all-state duplicate discovery,
configurable timezone-aware scheduling, inert todoq observation and separate
merge/install/running/release evidence with a Needs attention view. The initial
master-plan header is corrected to implementation in progress; its old
“unstarted” statement was stale.

WO1–WO6 are planned refinements, not completed implementations. The existing
P00–P31 identities and acceptance states are retained. Immediate implementation
priority remains completing bounded pending communication and one durable task
attempt; intake preview/contracts can proceed in disjoint scope after interface
freeze. CI capacity and runner repair continue independently of implementation.

## PR reconciliation and publication-fixture cleanup — 18 October 2026

Observed PR40/42/43/44/46 merged by the concurrent integrator. PR40 exact source
`9d07244d` has a passing general-pool run `39148202646` (709 Rust checks plus
legacy/audit/frontend and actual isolated smoke). ARM64 qualification at
`deda67f` passes on the shared xbabe4 general pool. PR44's bounded pending inbox
and scoped application ACK component is integrated at `cc51d140`; it remains a
trusted internal service, without authenticated transport/native activation.

Independent review of the publication regressions found unqualified temporary
path custody, inherited Git overrides/unbounded output, and a positive fixture
whose review/check evidence described the base rather than its candidate. The
post-merge repair preserves the existing regressions, adds canonical private
fixtures, bounded hermetic owned-child Git execution with positive/negative
controls, and exact-candidate evidence plus changed-candidate rejection. This
is offline fixture coverage, not P08/P19 native acceptance. The CI scratch guide
is corrected to match the actual canonical `/tmp` implementation.

Combined source `cf0e4a1723f487b54d635005e710d8a91ad6b866` passes 726 macOS Rust
checks, workspace strict Clippy/formatting, actual isolated binary smoke,
schemas, retained fixtures and legacy/audit checks. The helper/harness matches
the independently cleared `9986686e` candidate. Receipt:
`receipts/integration/cf0e4a1723f487b54d635005e710d8a91ad6b866.json`.
Source verification and every full parent/native rollout gate remain separate.

The user's orphan cleanup also found fourteen replacement legacy workers in
different owned Admiralty worktrees. Their old Screen PID/branch bindings were
stale, not evidence those new processes were dead. Private verified bundles,
create-only HEAD anchors and staged/unstaged patches preserve both Git commons,
including active dirty/unpushed work. Current lane identity observations are
reconciled without inventing task acceptance, model identity or CPU permits;
active/ambiguous/queue-referenced assets remain protected during retirement.

## Queue Item 3 batched candidate qualification — 2026-10-18T17:12:12Z

The linear batch branch `codex/batch-item3-p04-p07-p18-p21-p24-p25-p26` is based on `origin/main` `3927b3f9a824b97027fa9db46500441a96361e27` and contains the three remaining Round 8 commits, all eleven named Round 9 Item-3 commits, and seven additional scoped Item-3 worker commits. Two overlapping extra cherry-picks were empty and were skipped. No merge commits are present.

Final tested source commit: `9531ba089abb44c962538d8bf1159db05a6a9a91`. Exact checks pass: `cargo fmt --all -- --check`; `cargo test --workspace --locked` (721 passed, 0 failed, 1 ignored across 66 suites); `cargo clippy --workspace --all-targets --locked -- -D warnings`; and `pnpm contracts:check` (three Rust schemas match; no generated files changed). Logs and hashes are recorded in `receipts/P04/9531ba089abb44c962538d8bf1159db05a6a9a91.json` and companion P07/P18/P21/P24/P25/P26 receipts.

P04, P07, P18, P21, P24, P25 and P26 remain candidate evidence only; no main merge or acceptance transition is claimed. The batch is held for the 17:30Z-17:35Z merge window.

## Queue Item 4 Round 10–12 batch qualification — 2026-10-18

Created `codex/batch-item4-round10-12` from requested `origin/main` `24937f669495043abbfdd1f10bbb40c7db950ac0`. Integrated the Round 10–12 worker audit harnesses from the 15 `origin/codex/accept-*` lanes and kept the batch linear. Two duplicate source patches were empty after earlier lane content was applied and were skipped. Exact worker refs and final workspace evidence are recorded in `/home/ubuntu/admiralty-v2-build/receipts/MAIN-ACCEPTANCE/24937f669495043abbfdd1f10bbb40c7db950ac0.json`.

Final tested code head `302f3bc99d410ac519c431374b6ec7060b39c91b` passes `cargo fmt --all -- --check`, strict workspace Clippy, `pnpm contracts:check`, and the full workspace test suite (800 passed, 0 failed, 2 ignored across 85 suites; serialized test threads used while worker lanes shared the target directory). Build logs are under `/home/ubuntu/admiralty-v2-build/targets/main-acceptance/item4-24937/`. This is candidate qualification evidence; it does not change packet acceptance or claim a main merge. `origin/main` advanced to `a68643b5eb087b56325248bb0a76df418c0d9da7` during qualification; this batch remains based on the explicitly requested `24937f...` head for the scheduled window.

## Native lifecycle integration and primary web switch — 2026-10-18T21:24Z

I1 is merged in PR66 (reviewed head `dd419ae`, receiving main
`f7ae3c1c86b46911051931dcbad0178ca4075d81`): explicit v2 task lifecycle,
full-view CAS and serialized current authority checks. PR70 integrates the
controller/outbox, bounded process adapters, actual check executor, resource
permit boundary, protected server startup/agent scope, operator intake and
pending cancel/revise, CLI recovery/intake/staging and terminal task UI.
PR70 reviewed head `4b4e0e5df8d09769236c3d40cef3d794f7e95851` was approved
by neverhumanbot and merged with exact-head rebase at 21:13:54 UTC, receiving
main `f6f5ea57b79a1cafcf7987ff0bb8196b04b7b8d5`.

The source-attributed implementation receipt is
[4b4e0e5](../../../receipts/integration/4b4e0e5df8d09769236c3d40cef3d794f7e95851.json).
At parent `533e9afa175a727cdb9aab2e8c02347b689ce986`, 987 Rust tests pass
(three ignored, 129 suites), plus 93 frontend tests, production bundle,
contract/schema, retained legacy/audit/fixture and packaging checks. The sole
final code delta restores the default listener announcement used by the actual
smoke harness; the 31 affected server tests, strict Clippy, formatting and
rebuilt 15-check actual isolated binary smoke pass at final reviewed head.
Initial failures and their log hashes remain in the receipt; these are not
relabeled as passing runs.

The user's hard-switch order supersedes the earlier staged deployment wait.
The Commodore is the sole deployment writer. Root independently observed the
actual running Rust binary at receiving main `f6f5ea57...`, SHA-256
`e82361aa05ce6048bf7882b08d1276f492f3566073f1ee8d948ce9678eab49b6`,
serving React on the primary port at 21:20:57 UTC. The alternate port uses the
same Rust binary's HTTP 307 redirect. Health, fresh producer data and retained
detailed HTML respond 200; unauthenticated native fleet responds 401. The
Commodore's private receipt also records real operator submit/cancel and exact
lost-response replay, scoped agent context, unchanged collector and backup.
This verifies the primary web switch, not general native worker execution.

Parallel ownership is explicit: Grok S1 owns command journey UI/tests, S2 owns
browser/a11y harness/workflow and S3 owns model-selection accounting. Root owns
backend integration, server/module and App wiring after review. The Commodore
owns deploys. P09 owns workspace repair; strong review owns the executor's
recovery repairs and the resource gate's bounded diagnostics. The shared fleet
claim records these scopes; no task waits for user sign-off.

Remaining native execution work is concrete. A newly configured owned Linux
slice was exercised once under a protected, exact one-run authorization; the
child started but the adapter could not authenticate its observation before
exit. That consumed authorization is not retried, and resource closure remains
unknown. Read-only diagnosis also found systemd represents no pending Job as
an empty value; repair accepts only explicit empty/zero, retaining all identity
and enforcement checks. Bounded diagnostics preserve the original observation
failure. A separate deterministic computation probe will require fresh exact
authorization and prove actual closure, checks, immutable custody, acceptance
and replay; no successful proof is yet claimed here.

Workspace candidate `9113020` failed the registered macOS compile (platform
`st_dev` type mismatch). Independent review also found surviving detached pipe
workers, directory inode admission undercount and SHA256 format handling gaps.
It is preserved but not merged. Concrete executor candidate `48faff6` requires
non-Linux denial, observed identity persistence, observation recovery, opaque
closure handoff and per-run session lifetime repairs before registration can
qualify execution. Grok model routing still needs authenticated cumulative
settlement and receipt independence fixes; command UI needs live writes and
mounting; browser tests need real pinned axe, v2 scenarios and qualified browser
execution. These repairs proceed in disjoint scopes while receiving-main CI
runs.

Every full P00–P31 packet remains subject to its original acceptance criteria.
Component counts, idle Screens, mocks, passing unit tests and deployment do not
by themselves complete those packets. General native execution is explicitly
reported false until the real admission/provider/check/recovery path is proven.

## Actual native integration proof and continued cleanup — 2026-10-18T21:46Z

The actual Linux deterministic integration at source
`eb62f5f635bea46e98c76e91499b56b4c7673849` passes (exit zero). It observes the
real owned service and kernel limits, captures opaque closure, executes the
positive check and required wrong-oracle negative control, verifies immutable
source/oracle/check/closure custody, accepts through the real Store writer and
confirms exact retry, revocation denial and reopened projection equality.
The fixture candidate Git commit is separate from the Admiralty build commit.
[Sanitized proof receipt](../../../receipts/integration/eb62f5f635bea46e98c76e91499b56b4c7673849.json)
records both identities and artifact hashes; private detailed evidence remains
in the owned deployment proof area.

Three failed fresh authorizations are preserved and never replayed. Diagnostics
first isolated CPU enforcement: transient systemd AllowedCPUs reported two CPUs
but the actual service cpuset and affinity allowed six. The original resource
owner verified an empty owned probe slice and tightened only that slice from
0–5 to 0–1; ancestors and other workloads were unchanged. The passing run proves
a fixed inherited CPU restriction. It does not qualify arbitrary per-service
CPU assignment, exclusive reservations or a working transient property on this
host. Another real failure revealed successful show(not-found) returns exit zero
with empty invocation identity; closure now requires its exact property shape
and two independent positive absence checks in addition to retained live
identity, exited owned client, unchanged boot and empty/gone observed cgroup.
Generic failed/malformed manager probes remain unknown.

Workspace preparation is repaired at `35e20a7`: bounded nonblocking Git pipe I/O,
materialized directory/file quotas, explicit unsupported SHA256 source rejection
and actual macOS pinned-directory traversal. The final Mac run passes all 13
preparation tests and the runtime package. Combined source `35e20a7` passes
1,002 Rust tests, four intentional helpers ignored, across 129 suites. The sole
later production delta is the closure helper; its 11 resource tests, strict
workspace Clippy and formatting pass at `eb62f5f` with unchanged source hashes
during each check. A sequential harness's original attribution of later Clippy
to the starting commit was corrected and its original retained, then exact-head
checks rerun.

Hosted CI at `eb62f5f` exposes three executor test fixtures created beneath the CI
checkout's shared/differently owned ancestry. Production journal/source guards
correctly refuse that custody. The narrow test repair creates fixtures mode0700
at creation under the canonical temporary directory; it does not relax any
production check. The failed CI log is retained and the affected Linux tests
are rerun on the repaired source before integration.

Root also refreshed the old browser document through the native redirect. It
now reads the primary origin with fresh producer samples and all 13 temporal
plots rather than retaining its stale old-origin snapshot after failed fetches.
Grok's command UI remains with S1 for digest ordering, exact lost-response retry
and full-contract revision preservation; actual Rust golden fixtures are
provided without editing those files. S3's exact protected replay metadata clears
proposal-component registration; correction-after-settlement replay and atomic
writer integration still require work. S2 retains browser sandbox protection
while qualifying the actual browser/axe/v2 journeys.

This proof does not install the production controller/provider issuer, qualify
a coding provider, automate model-budget/capacity transactions, publish source
candidates or complete the P00–P31 parent packets. General native execution
remains explicitly false. Those implementation steps continue under their
existing owners; the primary Rust/React dashboard is already live.

## Dashboard provenance correction and PR72 closeout — 2026-10-18T21:57Z

The user-reported 32 identical red alerts came from the bootstrap input manifest,
not a stopped collector. It declared all packets integrated while omitting the
required integration receipt, target ref and observation time. Its shared
component qualification did not complete the parent acceptance criteria. The
existing loader correctly downgraded those claims; React then flattened the same
warning into 32 alerts and colored recorded checks green.

The dashboard now groups identical warnings into one counted notice with all
affected packet IDs and the original evidence available on expansion. Distinct
failures remain separate. Stale producer timestamps remain explicit, including
the separate rollout timestamp when a project already has another source.
Recorded component checks use blue styling and a `checks recorded` label;
accepted completion retains its source meaning. All 97 frontend tests and the
TypeScript/Vite production build pass, and independent review found no blocker.

The owned observer input was atomically reconciled at 21:56:53 UTC against this
ledger at source `30319e096bfaec6d87ae16ab5d0205a2d8214ee4`: P00 review and P01–P31
in progress. Original bytes and historical claims are preserved under
`admiralty-v2-build/receipts/rollout-corrections/20261018T215653Z/`. Before hash:
`9eb3c954e0f57ab2ddeb34542e350bc055e91c447c31a131d8462d026b504696`;
after hash: `ea07efad8d6def60c1905da7a489f08708d374252875f8ec2f63ddef9bc72f21`.
The actual deployed loader validated the replacement. No integration provenance
was invented, collector code changed or service restarted. Its observation age
continues to increase naturally; future reconciliation must use fresh evidence.

The final affected Linux checks at `30319e0` pass: three executor tests, 13
workspace preparation tests and strict server Clippy. The hermetic fixture
repair normalizes only newly owned test Git metadata under permissive caller
umasks; production custody checks remain strict. The earlier failure is retained.
[Sanitized Linux receipt](../../../receipts/integration/30319e096bfaec6d87ae16ab5d0205a2d8214ee4-linux-affected-checks.json)
records source and log hashes. This is separate from the scoped real native
lifecycle proof at `eb62f5f`; neither establishes general provider execution or
full P00–P31 acceptance. PR72 packages both implementation and honest evidence
presentation. Commodore remains the single deployment owner.

## Native Rust producer replacement — 2026-10-18T22:33Z

The web cutover still depended on the historical Python collector and rollout
observer. The user explicitly rejected that runtime dependency. A new registered
`admiralty-observe` crate now reads configured project ledgers, local Git/PR
evidence, fleet resources, real Screen/process identities, usage events and
rollout inputs directly, without Python or model turns. Node/project identities
and all deployment paths remain configuration. The source checkout is read-only.

Three disjoint workers implemented project adapters, fleet observations and
usage/history; independent integration review found and repaired retained-HTML
shape failures, omitted rollout history/Commodore fields, excessive embedded
history, malformed identity pins and nested credential-field exposure. The real
47-task tracker uses its configured Git ref, not a drifting worktree. Requested
models never replace missing observed footer identity. Screen presence never
establishes task progress or acceptance.

Bounded private usage offsets/baselines and observed events survive restarts.
Partial source scans and initially truncated logs leave full windows, aggregate
cost and yield unknown, with measured lower bounds kept separately. Historical
samples retain original timestamps; display embedding is bounded separately
from the retained 4,096-row disk history. These observations do not grant task
acceptance or qualify autonomous provider execution.

The final local candidate passes 47 registered observer tests, strict observer
all-target Clippy, workspace formatting, 101 frontend tests and the TypeScript/
Vite production build. Commands now mounts the merged command component in App;
the production bundle contains its lazy chunk. Native cost weighting and ledger
source age remain distinct from historical per-agent/publication measurements.
Actual Linux source tests and a real separate-output fleet observation are being
qualified before the deployment owner replaces the feed and retires exact old
owned processes. Do not interpret this entry as a deployment receipt.

PRs71,72,74,75 have exact-head review and are merged. PR69 remains actively
owned for its browser/isolated-native-server repairs; existence-only build reuse
or optional skipped backend coverage does not qualify an exact release. New
work starts from current main rather than stacking unmerged branches. Shared
Admiral/ticker, other projects, GPU jobs, runners and preserved data stay outside
the observer retirement scope. General native controller/provider/candidate
publication and atomic model-admission composition remain separate open work.

Final real-source qualification at 22:37:39 UTC passes 49 observer tests on Mac
and Linux and strict Linux all-target Clippy. Live observation reaches all four
configured nodes and the actual Admiralty manager footer reads Gemini 4 Argon,
high effort. Native history now imports the original only at initialization;
later legacy writes cannot suppress native samples or overwrite prior data.
The final feed appends its own current sample at 22:37:35 UTC while retaining
the original historical timestamps. Both shipped React rendering (all thirteen
plots) and retained HTML render the actual feed, including null costs and unknown
resources. Eight additional unknown/identity render checks pass; 88 legacy tests
pass with one Linux-only check skipped on Mac. These are collection/rendering
proofs, not autonomous execution or full parent-packet acceptance.

The [sanitized native observer receipt](../../../receipts/integration/native-observer-20261018T223739Z.json)
pins tested source-file, binary, log, configuration and snapshot hashes. The
Commodore must deploy the new retained HTML as well as the React assets and Rust
producer. Old process retirement remains a distinct recorded deployment action.

## Native primary deployment and foundation closeout — 2026-10-18T23:30Z

PR76 received exact-head review and merged to `fe158305d0ea49fd905fd1e7d888aa4b477e4412`.
The Commodore deployed that source to primary port18890 at 22:42 UTC. Independent
checks confirm the actual daemon and observer executables come from its pinned
deployment directory; port18891 redirects to the primary origin. The feed is
produced by Rust, reaches the four configured permitted nodes and observes the
Admiralty manager as Gemini 4 Argon. All thirteen plots, original historical
timestamps, retained details and mounted Commands are preserved.

The exact old Admiralty-owned Python collector, rollout observer and restart
wrapper were archived and retired. The owned hourly history-export cron was
also archived and removed; unrelated cron entries were unchanged. Later process
checks found no respawn. Shared fleet Admiral/ticker, unrelated project services,
GPU jobs and runners are outside this retirement. The original private cutover
receipt and its host-field correction remain separate evidence:
`ADMIRALTY_NATIVE_PR76_RUST_CUTOVER_RECEIPT_20261018T2245Z.json`
(`f5934e9670be5c0e8a91b83ab3760e0656f058a59158fdd5adb9f108c3d7c93c`)
and `ADMIRALTY_NATIVE_PR76_RUST_CUTOVER_RECEIPT_CORRECTION_20261018T2249Z.json`
(`2e818a375aeb10969612dfcb4221f43e116a19ef23ec77e6225c6cfa358f6b62`).
The public observer receipt predates deployment and is not rewritten as if it
had observed these later actions.

The exact receiving main passes 1,061 Rust tests across 132 suites, with four
intentional ignored helpers, strict workspace Clippy, 101 frontend tests and
the production TypeScript/Vite build. Main CI run39173192206 is successful at
that SHA. These results are reused only for unchanged source; later changes
receive affected checks and eventual final-source validation.

Criterion-specific review now closes the remaining technical foundation gaps.
P00's immutable verifier checks 37 source blobs, counts, mirrors and secret
absence; all eleven synthetic cases remain explicitly reconstructed. Its
read-only inventory permits explicit unknowns. Thirteen actual legacy-import
tests pass. P01's deterministic fake-clock/fault/forge suite passes five tests;
its CI/toolchain/fixture/ignored-zone requirements are present. The node binary
now implements effect-free help while normal startup remains disabled. All
four actual binaries return zero for both help flags in an empty private
directory, which remains unchanged. No live enrollment or provider call is a
foundation criterion. The engineering acceptance and integration revision for
P00/P01 will be recorded in the owned rollout after this reviewed fix lands;
the old partial-review receipts and all historical claims remain intact.

The executor also retains the original descriptor-backed `PreparedWorkspace`
through pending/recovery ownership and consumes it together with actual closure
evidence. Fallible closure, journal and capacity checks precede owner removal;
unknown paths retain the host, admission and workspace. Three affected executor
tests pass, one child helper is intentionally ignored, and strict server/runtime
all-target Clippy passes. Independent static review finds no blocker. The
[source-pinned handoff/foundation receipt](../../../receipts/integration/native-handoff-foundation-20261018T2330Z.json)
separates these results from the unchanged main's broader tests.

This handoff still needs a real Linux successful-execution-to-source-candidate
proof. General native execution remains false: production launch/completion,
durable model admission, provider credentials, source custody, trusted checks
and Store acceptance must be composed. PR77's caller-asserted closure authority
and expired resource leases require repair; PR78 must retain original workspace
identity, disable Git text conversion helpers and compile portably. Their
owners have exact findings and disjoint scopes. PR69's owner has the minimal
UI/CSS accessibility repair scope and must pass real axe/browser/native-backend
checks. Neither green component CI nor Screen presence completes the other
thirty parent packets.

## Receiving-main foundation acceptance — 2026-10-18T23:40Z

PR79 merged by matched-head rebase after neverhumanbot approved exact
`b8a4fb21c0647c7830116073ad44bfe07012c4bb`; receiving main is
`ebc5a8dc081185d2824f653fc3924147d66d11ed`. All source/artifact hashes in the
handoff/foundation receipt were independently checked. The clean receiving
source then passed the full workspace: 1,061 tests, zero failures and four
intentional ignored helpers across 132 suite results. Log SHA-256:
`739f5a6d631f52baa4b683cd49d0a72a63a832768b35e7f77511cf56b373e31a`.
The [receiving-main receipt](../../../receipts/integration/native-main-ebc5a8d-20261018T2340Z.json)
keeps that actual run distinct from the prior base's CI. CI for the new main was
still queued at its last observation; no success is invented.

Root recorded engineering acceptance for P00 and P01 against their complete
foundation criteria, with independent criterion, code and receipt reviews.
The owned rollout was updated atomically with a source-hash compare, original
bytes archived, both packet-specific acceptance receipts hashed, and every
other packet preserved exactly. Archive:
`admiralty-v2-build/receipts/rollout-corrections/20261018T2340Z/`.
Before SHA-256: `ea07efad8d6def60c1905da7a489f08708d374252875f8ec2f63ddef9bc72f21`;
after: `cdfbd0af46fdf938976a6a0f6f73f5b63e5422ae2cdfd33541c2e4f9a7ca4b54`.
Acceptance receipts are in `receipts/foundation-acceptance/20261018T2340Z/`
inside the owned build area: P00 hash
`4f878b9f3a03bd2fe8af29c24ad6728724242760fcc5d343bcd9cc27671cdffc`;
P01 hash `d252f0fe021ee65dfcd5b2ac4adcc9b529bc8360f042fcd5711b9755a4272f81`.
These are engineering rollout records, not domain task acceptance grants.

Thirty parent packets remain open. Fresh criterion review of P02–P08 identifies
specific unfinished requirements: public/WO1 contract coverage; observation and
artifact availability transactions; durable subscriptions/questions/delivery
states; four missing capsule compilers and persisted render baselines; actual
control-service restart through the independent session host; and a real
qualified provider tuple/task receipt. Existing passing components do not erase
these requirements. Three disjoint local agents now implement observed-start
writer composition, deterministic role capsules and existing-type public schema
exports. Grok's three repair lanes keep their original exclusive scopes. The
Commodore remains the sole deployment writer; native general execution stays
explicitly unqualified until the complete actual chain passes.

## Durable starts, compact capsules and contract closure — 2026-10-19T00:10Z

The next combined source records actual native starts through a required trusted
`StartedSink`: the executor journals the observation, then the Store sink appends
the fully scoped `TaskStarted` event before completion can be handed off. Sink
failure retains the original admission, session host and descriptor-backed
workspace. Exact retry/reopen, mismatched binding, capacity and callback-failure
tests pass. Unstarted plans have a lock-protected withdrawal path; unknown or
already prepared launches cannot silently release their retained owners.

Four additional deterministic capsule compilers cover decisions, handoffs,
project exceptions and fleet exceptions. Protected facts, exact decimal budgets,
immutable references and unknowns survive bounded encoding. The decoder checks
the independently retained role, epoch and protected-fact digest. These payloads
grant no authority; persisted render baselines and measured context savings are
still open. Eight existing domain roots now join the original four Rust-derived
public schemas, TypeScript contracts and runtime guards, including exact u64
decimal-string bounds. No serialization policy or private authority type is
made public by this export.

Independent integration caught an initial dashboard bundle increase from the
additional guards. Splitting unused roots restores the initial bundle while
preserving the original four executable guards. Native package staging now
requires the exact nine hash-bound sibling modules; missing/unknown dependencies
fail before staging. Tests actually import the staged closure in Node and
exercise all twelve guards. The Rust CI job therefore installs the same pinned
Node24 action already used by the contract job. This is explicit component
staging, not a JavaScript sandbox, signed bundle or supported installer.

The [aggregate receipt](../../../receipts/integration/native-composition-20261019T0010Z.json)
distinguishes the earlier full workspace run at `56663d1c` (1,076 passed,
four ignored) from later changes. After PR77, all 51 server tests pass. Final
source `6b7d7173dc564645ffb7a015902b271718e14468` passes all 48 CLI tests,
101 frontend tests, ten contract/actual TypeScript-consumer tests, generation
drift, strict CLI Clippy, formatting and the production TypeScript/Vite build.
Its main JS is 496.23kB raw / 114.60kB Vite gzip; the earlier controlled base
comparison is recorded separately. None of these results is silently attributed
to a later receiving-main SHA.

PR77's conservative repair is merged at `19c42ffb`: settlement requires an
explicit trusted verifier, default unknown closure retains liability/capacity,
and linked admission checks current authority/time. The component is not an
installed atomic production model-budget issuer. PR69 is merged at `37a1d2ab`
after exact-head browser CI run39178216770 passed all thirteen tests with pinned
axe and a mandatory actual Rust backend. Its original owner receipt contained
an incorrect explanatory axe hash; the original is retained with a separate
correction, while approval relies on the actual successful CI log. Fixture
lost-response retries remain distinct from real native API coverage.

The prior `ebc5a8dc` main CI job hit its 15-minute limit after passing formatting,
Clippy and many tests; no assertion failure was observed. A read-only audit
found concrete progress in newer jobs and substantial runner I/O/speed variance.
No tests, limits or unrelated runner services were removed or weakened. The
deployment owner has advanced the live service to `ebc5a8dc`; later merged source
must be deployed with its own source-pinned build evidence. PR78's new bounded
source-candidate revision remains under exact-head review. The complete real
provider → immutable changed source → independent checks → Store acceptance
chain, restart continuity, durable accounting/coordination and remaining P02–P31
criteria stay open. No general execution qualification or new parent acceptance
is claimed.

## Scoped dispatch and durable context composition — 2026-10-19T00:30Z

PR78 is merged at `196b873b80f12f36eeffe9a555749b962ea4f815` after exact-head
neverhumanbot approval of `9ad8c9fc0181159c626f4f74529a8fa8c18b2138` and
matched-head rebase. Independent Mac checks pass all 35 workspace cases,
including 13 source-candidate and 14 preparation tests, strict runtime Clippy and
formatting. Bounded reads now cover the earlier preparation config path too;
Git directory traversal checks admission before retaining child paths. None of
this is substituted for actual successful Linux executor-to-candidate proof.

The next integration at `d7ced28bbcb71471a105752f0e035e4f714abc27` passes the
entire registered Rust workspace: **1,128 passed, zero failures, five intentional
controlled child helpers ignored, zero filtered**, across 132 suite results.
Strict workspace all-target Clippy and formatting pass on the same clean source.
The [coordinator/context receipt](../../../receipts/integration/native-task-coordinator-20261019T0030Z.json)
pins actual production sources and logs, each imported worker revision and the
independent static reviews. This full run includes the newly merged source
candidate, exact-key Store claims, ownership retention and durable context
storage; it is not attributed to a later source revision.

Exact-key claim and expiry now run on Store's sole writer without changing a
foreign project's pending or expired intent. The global monotonic clock and
existing fences remain intact. Registration rejection returns the original
opaque plan. Known pre-spawn errors retain plan/reservation, and even an
uncertain journal write with no visible file fences retry/withdrawal. Actual or
ambiguous spawned handles continue through the existing pending registry.

`NativeTaskLoop` constructs its own actual executor and StoreStartedSink against
the same Store and immutable scope. It registers before atomic claim/intake,
retains frozen requests after ambiguity, and dispatches only the retained exact
launch key. A separately retained effect-acknowledgment state survives completion
buffering; unconfirmed closed proof cannot be handed off for acceptance. Bounded
backpressure precedes one-shot transfer. There is no setter that turns an
unknown effect into success, nor an HTTP/startup activation added by this change.
The current lifetime registry retains at most one attempt per task; retirement,
new-attempt continuation and explicit unknown-effect reconciliation remain open.

Role renders now persist exact protected text, scope/role/epoch/fact digests and
explicit byte/estimated-token metadata through the sole Store writer. Immutable
records, bounded pages/history, recipient baseline CAS and historical exact
command replay survive reopen, rebuild and SQLite backup. Lost-ACK replay returns
the historical receipt without rewinding the latest baseline. Corrupt stored
history fails closed before backup publication. The 120 actual Store tests
include eleven context cases. Application acknowledgment is a trusted caller's
recorded observation; no authenticated delivery, provider memory or token-saving
proof is invented. Live route/governor/delivery integration remains separate.

The Commodore deployed PR80 main `c2300024848fd921031dee47f82288423afb6fda` at
00:20 UTC. Independent verification checks all twelve manifest artifacts and
actual daemon/observer executable paths. Primary18890 returns200; compatibility
18891 returns307; Rust collection records zero model turns and no acceptance
authority. The freshly loaded production React bundle has all thirteen metric
controls and original history; old Admiralty Python producers have not respawned.
Manifest SHA-256: `edb27988b3fa5c23f35c446b961177fc69d47285124e853bc73097d9372f43b8`.

Current-main Rust/contracts CI at both c230 and196 is successful. Browser CI
has two preserved receiving-main failures: c230 startup exceeded15 seconds;
196 passed12 cases but its4/20/100-project functional journey exceeded30 seconds.
The corresponding PR browser runs passed13/13; no failed main run is called
green. S2 owns diagnosis/repair with sandbox, mandatory real backend and pinned
axe unchanged. PR78's separate contracts failure reported a null generator child
status after about33 seconds; discarded error/signal details prevent a definite
cause. The follow-up retains the30-second bound, adds bounded termination/stage
diagnostics and requires actual exit1 for expected rejection. All eleven contract
tests, including actual interrupted-child controls, pass locally on Node25;
this is not a new Node24 Linux CI result or a claim of intermittent CI repair. These are
actionable harness findings, not a reason to stop unrelated implementation.

All old PRs were cleared at 00:18:55 UTC; new work starts from the actual current
main. Three new disjoint surge packets cover custody-preserving candidate errors,
browser failure repair and a trusted local evidence catalog. A separate isolated
Linux real-task-loop proof is being implemented; it will use a fixed child and
fresh exact authorization, not a paid provider or reused failed permit. Native
general execution remains false. P00/P01 remain the only accepted parent packets.

## Task-loop Linux proof and independent review repairs — 2026-10-19T00:50Z

PR81 merged at 00:36:49 UTC after exact-head neverhumanbot approval of
`b7d7597a80ba1fc49d3902d55dcba5512e642a39` and matched-head rebase. Receiving
main is `3fdb10189322962bb91eadfbcb111f483697a688`; all eight production Rust
hashes in the coordinator receipt match that revision. The Commodore remains
the sole deployment writer and deployed it at 00:42:35 UTC. Independent checks
at 00:47:12 UTC verify actual daemon/observer executable paths, all twelve
manifest files, primary200, redirect307 and the Rust producer. Manifest hash:
`e90f8944190d01d35ccbc1c8be8e9eaac9c2836c589446076aff86e0b91110c4`.
The actual browser retains thirteen temporal controls and one profile control.
The deploy receipt's zero plots/false profile fields are unsupported feed-schema
inferences; the original is retained and an explicit correction was requested.
Receiving-main browser CI39181566301 succeeds; pending Rust/contracts checks are
not described as green.

An isolated example now composes actual `NativeTaskLoop` admission/dispatch,
observed-start Store sink, acknowledged closure, original prepared workspace,
changed-source candidate, independent compilation/checks and lifecycle acceptance.
It prepares a new bounded exact request, requires external private authorization,
and creates a one-run fence before runtime Git or launch. Its fixed deterministic
child is not a coding provider. Independent review caught that generic checker
exit17 could satisfy the old negative control. The repair reserves exit23 for a
private typed oracle mismatch after successful compilation, execution and complete
capture; setup/compiler/capture failures remain17. This also limits the older
eb62 proof's wrong-oracle claim; its original receipt is preserved.

Actual Linux source `dbec672d0fa0fd4137200d148d67115f2edb29a3` ran once with a
fresh protected authorization, owned empty probe slice, CPUs0–1, memory256MiB,
tasks16 and deadline90s. It recorded the real claim, observed start and effect ACK,
captured actual successful resource closure and materialized changed source. Its
positive trusted check then failed: rustc created a mode0775 executable under the
inherited host umask, and the unchanged program guard correctly refused it. The
failed fixture, journals, authorization and output remain in the owned private
proof area; no acceptance or passing proof is claimed, and authorization is not
reused. The owned probe slice is empty after closure. The repair retains the
new output-directory descriptor, refuses aliases and replacement, and applies0700
only through the exact owned compiler-output file descriptor. Independent static
review and nine component tests cover the permission repair; a fresh build and
authorization are required for the next actual Linux run.

PR82 at `c3e1066` is held for two concrete defects: an attempted freeze could lose
its unknown-state fence if diagnostic OIDs were unavailable, and its new error
diagnostics invoked Git after metadata preflight rejected that repository.
Thirty-six actual focused workspace tests and strict runtime Clippy passed but
did not cover these defects. S1 owns the repair and regressions. PR83 at `fb979f7`
passes its browser CI, but independent review found cleanup could return before
an existing asynchronous termination completed and retry options admitted
nonfinite counts. S2 owns shared cleanup-promise and bounded-option repairs.
Neither old reviewed head is approved for merge.

The unregistered S3 catalog candidate `88af5f6` remains held. Its real closure cwd
check confused the original execution workspace with the detached candidate;
its path-only Git revalidation was weaker and unbounded; revocation failure on
capacity/lock contention could leave known-drift authority active. S3 owns these
three repairs. Component fixture checks are not a real opaque-proof positive path.
No weaker catalog authority has been installed.

Actual component/source/deployment reconciliation updated rollout.json at
00:48:23 UTC, preserving every parent status and unrelated packet. Archived proof
records before hash `e7a65ad3de83a8e7c843c515f0e31829b98713c159f1e1e12599806194a270e6`
and after hash `1edbdda027b9942b1a007e8e3e9baa63c5d21f199bc44e1e815e12d8933d253e`.
Display staleness now follows the authorized thirty-minute engineering heartbeat
plus five-minute grace; original producer/history timestamps and actual ages stay
intact. No runtime permit, lease, admission or security limit changed. P00/P01
alone remain accepted; all other thirty parent packets and general autonomous
coding execution remain unqualified. The existing heartbeat remains active every
thirty minutes with current owners and source evidence, without duplicate monitors.

The fresh Linux proof at source `661e5c5294fe8fe3fe968ce2070b0d71951be397`
subsequently passes with exit0. The [sanitized task-loop receipt](../../../receipts/integration/native-task-loop-linux-20261019T0053Z.json)
pins the source archive, executable, failed predecessor and independently verified
evidence objects. Actual positive compilation/check exits0; the independently
compiled wrong-oracle control exits23. Store records one real claim and one actual
observed start, retained admission replay matches, duplicate dispatch does not
attempt another runtime, effect ACK is recorded, acceptance is event5, exact replay
matches, revocation is denied and reopened projection agrees. The owned slice is
empty afterward. All nine example tests also pass on Linux with zero ignored or
filtered. The fixed deterministic workload still does not qualify a coding
provider, install production startup authority or close a whole parent packet.
One additional preparation copied the prior executable before the new build
finished; it was never authorized or launched, was preserved, and was superseded
by the independently checked new binary and fresh fixture.

PR83's cleanup-promise and finite-option repairs pass independent review at
`98c4bcfcb6468eab8db4c428fe5434e65c491a05`. Its exact Linux local proof passes
all13 browser cases and TypeScript; original failures remain preserved. Under the
authorized `local-CI only` path, exact-head neverhumanbot approval precedes
matched-head rebase at 00:55:14 UTC, receiving main
`a3b0b661a2071cab1e87e64956b7bd88de6771e8`. Queued hosted checks are not relabeled
green. The small remaining form-loop deadline correction is assigned to S2 on a
fresh branch; it affects latency/diagnostics, not successful-check interpretation.

## Provider-plan composition and repaired integration queue — 2026-10-19T01:30Z

PR84 received main `e0421cedf457530405234269fa77ea532b1e3c2d`; PR82 received `b2b9e7a6c33bd57fd7b56079a846482f7ea25964`. The fresh actual Linux task-loop proof and custody error repairs are integrated. PR87 received `616ce667054aa7b1354bbfbc6790c6f8494c1cf8`, preserving one total browser form-stage deadline. PR85 received `9893516024c855a2c0e28503bc35c471a6ae9635`: registered bounded local evidence catalog, original execution cwd distinct from candidate checkout, existing descriptor-backed custody revalidation, permanent revocation tombstones and fail-closed contention latch. Independent review caught an in-flight latch race before the final repaired head was approved. Nine dedicated catalog tests pass; this process-local component proof is not a production opaque-catalog or restart qualification. All merges followed exact-head neverhumanbot approval and matched-head rebase.

The registered native Codex plan adapter now calls the actual provider preparation API, retains the original prepared workspace and binds exact launch argv/environment, source, capsule, selected model, reservation, lease and live resource claims. Rejections return the original handles and resource inputs. It neither issues admission nor starts a process. [The component receipt](../../../receipts/integration/native-codex-plan-20261019T0130Z.json) records fourteen actual dedicated tests across the registered adapter and catalog, strict server all-target Clippy and formatting. Independent review covers the source; declared model IDs remain distinct from observed identities. Actual tagged capture and a separately authorized coding-provider proof remain active disjoint work.

Main3fdb101 Rust job117835221056 actually hit its fifteen-minute job timeout while compiling; no test assertion failure was observed. Its complete raw log is preserved with SHA-256 `72f7009ffd8316259226bdf6dfcc19a3be2e23cba07692dd3f83e88828009b4f`. PR89 received `b24c84b8aa829d8e130fe31ca0655b3417a17799`, setting Cargo build concurrency to two for that job without skipping tests, extending deadlines or changing shared runners. A later completed CI result is still required; queued/pending checks are not green. PR88 live Git/Fleet views remains under exact-head changes-requested review for linked-worktree filter execution and source/freshness/identity correctness. Its newer head must be reviewed rather than inheriting approval.

The new [remaining-work audit](REMAINING_20261019.md) accounts for every P02–P31 parent against source b2b9e7a6, with explicit gaps, dependencies, disjoint proposed handoffs and stop criteria. P00/P01 alone remain accepted. Native Rust web/observer cutover is complete, but general autonomous coding execution and full major-release acceptance remain false. Commodore alone owns redeployment; source integration does not imply a newer live binary.

### Actual tagged capture handoff — 2026-10-19T01:34Z

The executor now retains an opaque, non-reconstructible completed capture with original stdout/stderr tags, contiguous offsets and final actual SessionHost status. Exact exited producer identity, complete EOF watermark, bounded bytes/chunks and an unchanged final status are checked before transfer. Invalid, truncated, unknown or overflowing capture leaves existing execution custody pending. A genuinely silent completed child retains the legacy empty output but provides no authenticated provider transcript. The original completion APIs remain compatible; a new consuming tuple transfers capture with the original workspace and closure.

[Source-bound evidence](../../../receipts/integration/native-tagged-capture-20261019T0134Z.json) records sixteen actual focused executor tests, one pre-existing ignored child entry helper, strict server all-target Clippy and formatting. Independent review is clear at source SHA-256 `3dacb9f30f67766dfab8e7becb22155c607d78b477216130d72903a9a7514499`. The worker preserved an initial portability failure: non-Linux SessionHost deliberately has no process-start identity, so the final code compares its actual optional identity without inventing one. Linux resource closure is independently required. No provider execution or general qualification is claimed.

### Explicit coding-provider probe and actual preflight repair — 2026-10-19T01:43Z

PR90 merged to `1ecdd19fb3cda9e9bc5b5be069c356d94831c768` and PR91 to `779a008f9660e3fa1c8e9f308aae8229a263e547`, each exact-head approved and matched-head rebased. A separately reviewed extension of the existing task-loop example now composes actual Codex preparation, exact one-run resource authorization, NativeTaskLoop, original tagged capture, source candidate, trusted compiler checks and Store acceptance/replay. Its [operator handoff](../../operators/native-codex-task-loop.md) requires a fresh reviewed request and external0400 authorization. Passing source/check/Store and provider-protocol qualification are separate outcomes; missing model/effort/usage never become observed identity. There is no production controller activation or general admission.

Actual preflight found the safe installed supported CLI is296865480bytes, larger than the old128MiB object-read path. The repaired probe streams a raw fingerprint through64KiB buffers with separate384MiB/30s metadata bounds and exact descriptor/path identity checks. It does not enlarge ArtifactStore, generic-read, HOME, executable or runtime limits. Fifteen actual component tests and strict server all-target Clippy/fmt pass; [the source-bound component receipt](../../../receipts/integration/native-codex-probe-component-20261019T0143Z.json) preserves the original blocker and worker evidence. Independent static review is clear. No actual provider generation is recorded by this component receipt; the Linux qualification build/run remains separate.

Rollout reconciliation at01:32:53Z pins merged source779a008 and the real fixed-toy/factory/capture receipts. Before/after hashes are `1edbdda027b9942b1a007e8e3e9baa63c5d21f199bc44e1e815e12d8933d253e` and `91a647293f72eeb6af54a0c1450b8672ee4edb610077bc620bf6b7405f170307`. P00/P01 original criterion receipts were independently hash-verified; only their recorded reference status is normalized from verified_packet_criteria to verified with explicit criterion kind/scope, archived original reference and original verification timestamp. This repairs the existing sc_audit enum mismatch without changing parent acceptance or claiming a new check. Every parent status and unrelated packet stays intact. P00/P01 alone accepted;30 parents and general coding execution remain open.

### CI efficiency follow-up and provider prerequisite failure — 2026-10-19T01:52Z

The616ce667 Rust job117844998810 also exceeded its fifteen-minute job deadline, after formatting and strict workspace Clippy passed. This is preserved as a timeout, not an assertion failure. A narrow reviewed follow-up omits dev/test line-debug data while retaining assertions, overflow checks, unoptimized profiles, every check, jobs2 and the same deadline. An actual isolated Cargo1.95 fixture proves dev assertions and overflow101 plus both test assertions/overflow cases; actionlint and semantic workflow comparison pass. [Evidence](../../../receipts/integration/ci-rust-debug-info-20261019T0152Z.json) does not claim hosted performance or green CI.

PR92 merged at01:43:26Z to99f4e26cebfab97afa661ed05aea6192bfd5935c. The actual Linux probe build pins7159a0218f1105e125f8ccc310abfa3492982a5e, executable SHA-256d02361038373591ee6a7073251589e9704a1013cde61c2c484aa6a54aada07e3. An earlier mistyped embedded source stamp was caught before any preparation or authorization and archived unused. First fresh preparation/request had17700bytes, SHA-256c6914bd8beed10d6e76fc4c0ef05a6026784c9823e7c112b613debc9315f34df. Its one-run attempt exited17 before provider spawn because default host umask002 made Git metadata0664; the unchanged workspace guard correctly refused it. Fixture, logs and consumed authorization stay private and are never reused. No provider execution or acceptance is claimed. A fresh fixture is preparing with umask077; no authorization exists for it.

The Commodore reports primary779a008 cutover01:43:28Z with twelve pinned artifacts. Root independent hash/process recheck was interrupted by a reset SSH connection; subsequent banner exchanges and the dashboard tunnel time out. Last earlier health check returned native_execution_qualified=false/status=ok. Do not assert either full deployment requalification or a proven service crash from a tunnel failure. Hold provider authorization while connection/source/owned-slice state is unknown. Primary/observer control remains with Commodore, unrelated runners/GPU/services untouched.

At01:54Z root cancelled six superseded merged-PR workflow runs only after re-reading each run and every job as queued/pending. [The queue receipt](../../../receipts/integration/ci-superseded-queue-20261019T0154Z.json) preserves admission and actual cancelled outcomes. Current-main/open-PR work and every individually running job remain untouched; no shared runner or unrelated service was controlled. Cancelled results are not green checks. PR88 is closed in favor of PR93; its current9cde56d5 head needs current-main conflict resolution and final exact-source receipt from its existing owner. PR94 contains this reviewed CI follow-up; exact-head forge approval remains required. The configured reviewer helper is on currently unreachable xbabe2, absent on checkedxbabe1/3. No identity or approval bypass is used. Public6969 and configured fleet-LAN22 both time out at banner exchange; a fresh provider fixture still has no authorization and must be inspected, then superseded if metadata age expired, when connection recovers.

### Git/Fleet integration and durable attention — 2026-10-19T02:26Z

SSH recovered. The0149 fixture completed preparation without authorization;
metadata is stale and it will never be run. Root independently checked live779a008
at02:10:16Z: all twelve manifest hashes match and both Rust processes execute
source-pinned deployment binaries. Health is ok/native_execution_qualified=false.
No retired Admiralty Python producer matched the scoped process check.

PR95 merged02:18:49Z to35e58904b2e5f9215d48329962f1514a2cf86610 after exact32dea091
neverhumanbot approval and matched-head rebase. Source-bound local build and
all18actual-backend browser checks pass; hosted browser/contracts pass. Hosted
Rust fails four private-directory fixtures plus an inherited controlled-child
capture deadline. Original logs are archived in the
[single deferred cleanup backlog](../../../CI_CLEANUP_BACKLOG.md). Jepson's
02:12Z code-first ruling authorizes this local-CI-only cutover merge; no red check
is green. Commodore alone owns the new Git/Fleet deployment.

The registered Store attention journal persists observations and delivery
decisions, atomically creates new wake intents, preserves immutable historical
receipts and unknown effects, and validates history on open/rebuild/backup.
Nine actual registered integration tests pass; the frozen worker's affected
Store129-test checks and independent source/registration review are preserved in
[the component receipt](../../../receipts/integration/durable-attention-20261019T0224Z.json).
Current authenticated scope/clock authority and safe-turn transport remain
separate. Accepted parents remain P00/P01 alone,2/32;30 parents remain open.

Parallel disjoint local work advances exact-policy/precision-safe intake,
atomic request-to-plan recording and an explicitly authorized attention service
bridge. A fresh0224 Codex fixture/request716ce940 was independently inspected;
its exact external0400 one-run authorization started02:25:12Z after a1.5second
host CPU sample45.04% and empty owned slice. Source7159a02/binaryd0236103,
CPU0–1,512MiB,32tasks,120seconds/maxconcurrency1 remain pinned. This start alone
establishes no provider identity, acceptance or qualification; outcomes must be
read from actual capture/check/Store evidence. Prior failed grants remain consumed.

### Intake, authenticated attention and bounded provider startup — 2026-10-19T02:50Z

PR97 received23cb1ec70474f1febb7c325eb1128a8082c9ce55, PR96 received
5126af0e388061690aa45bf3c758afe5b9db80a6 and PR94 received
f655b15922810c0e8155c5714e8125b11ad0794d, all after exact-head neverhumanbot
approval and matched-head rebase. Ready CI-profile work was rebased with both
ledger histories preserved; no new CI cleanup worker or suite repetition was
used. At this snapshot the repository has no open PRs. Commodore reports PR95
35e58904 live since02:23:42Z,14manifest entries,13plots/profile; this is distinct
from root's historical independent779 verification. Latest deployment recheck
and further current-main redeployment remain Commodore's single-writer scope.

Registered intake now preserves one PlainText request, splits only explicit
JsonBatch, binds every policy cap, uses exact decimal generation strings and
retains immutable format metadata. Nineteen registered tests pass. The new
attention bridge requires an opaque authenticated actor, current credential
scope, separate default-deny action policy and exact stored context/facts/task
binding; five real Store/auth/context/outbox tests and server build pass.
Request-context and pending batch references are distinct, and policy is checked
before enqueue rather than atomically with Store commit. These activation gaps
remain explicit; no route/provider transport or startup action issuer is enabled.

The0224 actual Codex attempt failed CLI startup at02:25:31Z with nucleo
ThreadPoolBuildError/WouldBlock; no model identity or acceptance was established.
[Failure evidence](../../../receipts/integration/native-codex-startup-failure-20261019T0225Z.json)
preserves hashes and consumed grant. A narrow independently reviewed child-local
TOKIO_WORKER_THREADS=2 request is bound in fresh authorization; only canonical1/2
is accepted. It does not loosen32tasks/512MiB/CPU0–1/120seconds or host85%cap.
Twelve resource-gate tests and fifteen actual example component tests pass.
Linux sourceefc67de built successfully to7cf7eb35; code trees are identical after
rebasing ontof655b159, as computed in
[the component receipt](../../../receipts/integration/intake-attention-thread-budget-20261019T0245Z.json).

Fresh0245 request17727bytes/afe58fab was independently inspected and authorized
once at02:46:54Z after measured CPU51.30% and empty owned slice. It also failed
CLI startup with the same nucleo error, exiting17 at02:47:16Z. That grant is
consumed and fixture retained; the requested async-pool setting did not resolve
this observed case. No provider turn, qualified identity, immutable coding
candidate or acceptance is claimed. Source/help diagnosis continues without new
provider grants or changes to resource limits. Pure source checks are not a
substitute for that missing execution chain.

WO2 atomic inline enriched-JSON record has11new Store and4existing workplan
checks, but independent review found duplicate-query integrity validation missing
before returned candidates. The owner is repairing that exact read path with
post-open corruption regressions before integration. Artifact claims and raw
unenriched prose remain explicit unsupported cases, not inferred tasks. Accepted
parents remain2/32, general autonomous execution false, and CI cleanup remains
deferred under the current human ruling.

### Atomic work-order source integration — 2026-10-19T02:59Z

PR99 received2c0c669bd4b98a93162074b2701bf46ef49de6b2 at02:52:39Z; PR98
receivedf590837abb85ae428761688218cb9dc940031b42 at02:55:21Z. Both exact heads
were neverhumanbot-approved and matched-head rebased. PR98's test-only owned
0700 fixture repair preserves production guards; all17artifacts and12qualified
objects were independently hash-checked, with actual72server tests underumask0002.
That historical local result is not a current full-source/hosted-green claim.

The WO2 Store writer is now registered through startup-only authority injection.
It reauthorizes each writer turn/replay, re-previews original inline source with
explicit format/current policy, verifies exact selected outcomes/contracts/
delivery/spans and atomically commits original source, work-order lineage,
selected task bindings, plan revision and immutable receipt. Existing standalone
workplan transactions preserve their semantics; the narrow shared helper allows
one caller-owned transaction without nested commit. Historical retries retain
old receipts; changed command/source identities collide, competing CAS has one
winner, and late SQL faults leave no partial plan or lineage.

Independent review caught duplicate-query reliance on denormalized scope and
partial binding data. Both post-open corruption counterexamples failed the
original source. Final queries validate full original revisions and independent
scope before returning/paginating candidates; before-clone identifier/reference
bounds are also repaired. All14actual registered intake tests plus4existing
plan tests pass after root registration; worker strict StoreClippy/fmt and
independent repaired-source review are recorded in
[the source-bound receipt](../../../receipts/integration/atomic-work-order-intake-20261019T0258Z.json).
Default startup installs no intake issuer. Explicit enriched inline JSON is the
supported component scope; raw unenriched prose/artifact claims fail without
partial writes. Public routes, enrichment UX and dispatch are not activated.

Primary CLI source diagnosis found that its own CODEXHOME/.env loader can replace
initial environment variables before runtime creation; neither root nor reviewer
read that file or concluded it caused the two startup failures. At02:55:45Z a new
owned0700 minimalHOME/.codex with only an explicit reference to existing0600
ChatGPT auth passed actual safeCLI login-status metadata. No credential bytes
were read/copied by root, no .env/userconfig was inherited, and no provider task
or authorization was issued. The auth reference remains shared writable: normal
CLI OAuth refresh can write the external authfile, so credential/network
isolation is not claimed. Written Fleet Admiral scope ACK is requested before
any fresh one-run qualification with this profile. Accepted parents2/32 and
native_execution_qualified=false remain unchanged.

### Raw intake integration and live reconciliation — 2026-10-19T03:23Z

PR100 merged at03:08:21Z receiving90e76d881689d36226c176a9493c1b8736cce071.
PR101 merged at03:14:50Z receiving1d00b08f07b53bf32c6d2e513185b6a741da726f,
repairing a strict Clippy failure in the attention test policy. All5affected
attention cases, strict server all-target Clippy and formatting pass. The
original failure and receiving-main browser diagnostic mismatch are retained
on the canonical CI cleanup backlog; no cleanup worker was assigned.

Root independently checked the live f590837a deployment at03:10:25Z: all 14
manifest hashes, both actual native process executables, health/UI/feed200,
18891redirect307 and Rust producer. No retired owned Python producer was found.
The Mac SSH tunnel had no listener; root restored owned loopback18890/18891
forwarding without service control. Fresh browser observation confirms13plots,
profile and Git/Atlas navigation. Anonymous native Git feed401 is expected;
no authenticated live-browser qualification is inferred. Source evidence is in
[the deployment receipt](../../../receipts/integration/native-f590837-independent-20261019T0310Z.json).

Sole-writer rolloutCAS at03:17:04.915Z advanced91a64729 to4971756c from actual
new merged source and hash-checked receipts, retaining every parent state,
byte-identical foundation packets and original source/history timestamps.
[Reconciliation proof](../../../receipts/integration/rollout-reconciliation-20261019T0317Z.json)
records2/32accepted and30open, not a clock-only freshness change.

The raw-capture Store module is registered at98a8ada4a0aa362d1594f074f40625aff0a14c24.
It saves original inline source and unresolved preview before enrichment,
requires startup-only current authority on every command/replay, retains exact
immutable receipts, and enforces parent-receipt revisionCAS, independently trusted
read scope and complete bounded lineage integrity at read/open/rebuild/backup.
It creates no task, work-order, plan or outbox effect. Root read the complete
source/registration; actual12capture+14enriched-intake+4workplan tests pass with
zero failures/ignored/filtered. Worker strict Store Clippy and all original
failures remain in [the source-bound receipt](../../../receipts/integration/raw-intake-capture-20261019T0320Z.json).
No public route, production issuer, artifact pin or enrichment is activated.

Parallel implementation now exports the frozen intake contracts through the
existing generator and adapts the already-existing question journal to the sole
Store writer; no duplicate question journal/migration is being introduced.
Production coding execution remains false. Both previous provider grants are
consumed. Minimal auth-profile metadata passes, but no new provider grant was
issued while coordinating its existing dedicated slice with the newly observed
fleetcap wrapper requirement. Other implementation continues independently.

### Questions, public intake and native package closure — 2026-10-19T03:40Z

Source6f76ed29e2d11278d9834be408f11f98a1b9c019 composes the existing question
journal with startup-only current authority on the sole Store writer. Original
contract/provenance and immutable retry receipts survive contract revisions;
fresh stale actions/reads deny. `StoreAuthorities` preserves the lifecycle,
enriched-intake and raw-capture policies alongside questions. No production
issuer, route, autoanswer, effective decision or outbox dispatch is activated.

Nine frozen intake roots now use the existing Rust schema, TypeScript and
runtime-guard pipeline. All36 prior schema/type/individual-validator outputs
retain their bytes. The native packager adds nine exact validator siblings,
retains unknown loader/path rejection and imports the real staged22-root entry
in Node. The generator now distinguishes a domain field named `format` from a
schema annotation; genuine unsupported annotations still fail.

The composed source passes40 focused Store checks,10 native-package checks,
13 contract runtime checks including the actual TypeScript consumer, both
22-root drift checks, strict Store/CLI Clippy and formatting. Independent review
clears the question module, composed startup plumbing and paired generator/
package closure. [Source-bound evidence](../../../receipts/integration/question-intake-package-20261019T0340Z.json)
retains earlier failed evidence and the narrower scope of historical tests.

The [P02 criterion audit](../../../receipts/integration/p02-criterion-audit-20261019T0330Z.json)
does not accept P02: deterministic reducer/configuration and the22-root export
pass, while the complete coordination/cursor/interface inventory and scheduling/
delivery amendments remain open. Local disjoint workers own the additive v2
envelope, bundled-timezone scheduling and verified-dialect read-only todoq
import. The existing Gemini S1 lane owns pure delivery/attention projections.
Root owns shared module/dependency/schema integration. No CI cleanup worker is
assigned; CI failures remain on the canonical backlog.

Root independently verified live40501925ae2a8906532ed5f709cda83a5898c675
at03:37:53Z: all14 deployment hashes, both current executable paths/start ticks,
health/UI/feed200 and18891redirect307. Earlier90e PID disappearance coincided
with newer deployments, not a proven crash. No retired owned Python producer
was found. [Independent deployment evidence](../../../receipts/integration/native-40501925-independent-20261019T0337Z.json)
leaves general coding execution false and accepted parents2/32 unchanged.

### Core integration and actual provider failure — 2026-10-19T04:05Z

Qualified source04d334650fda018ee7acc88980c5c3f143948a6d composes the additive
v2 coordination header/public root, pure bundled-timezone scheduling and inert
todoq snapshot importer. Root tests pass73 core cases,10 native-package cases,
3 CLI-public fixtures and14 contract runtime cases with a real TypeScript
consumer, strict core/CLI Clippy, formatting and both23-root drift checks.
Independent aggregate review verifies all27 receipt artifact hashes,63 unchanged
prior generated leaves and unchanged prior dependency version/checksum tuples.
[Source-bound receipt](../../../receipts/integration/core-contracts-20261019T0405Z.json)
retains failed bootstrap/schema-representation attempts and worker regressions.
Review reproduced and repaired three scheduling zone/range failures and the
importer's page-local source binding defect. No bus, scheduler, credential,
provider or publication service is activated by these pure contracts.

Root independently verified deployed8e99eaac at04:01:42Z: all14 file hashes,
both actual native process paths/start ticks, health/UI/feed200 and18891redirect307.
No retired owned Python producer was found. [Live proof](../../../receipts/integration/native-8e99eaac-independent-20261019T0403Z.json)
is separate from the newer source integration.

Admiral SC-DEC-133 explicitly resolved the single fixed-toy two-tier containment
route: parent through fleetcap, child under unchanged native user-slice caps.
Root built source8e99eaac through fleetcap and inspected one fresh0349 fixture.
The exact fe2e9000 request was authorized once03:52:34Z at hostCPU69.1064% and
MemAvailable49373416KiB, with CPUs0,1/512MiB/32tasks/120s/concurrency1.
The minimal-HOME CLI starts and completes a turn, but three shell commands fail
unified-exec EAGAIN. It creates no changed source; the probe exits17 at03:53:03Z.
Its grant is consumed and will not be reused. [Failure evidence](../../../receipts/integration/native-codex-minimal-failure-20261019T0353Z.json)
retains exact request/binary/journal hashes and reported usage. Requested model
remains distinct from unverified observed identity. Retired cgroups retain no
counter proving the resource cause; no cap was raised or automatic retry issued.

Current disjoint lanes: S1 delivery/attention projection; S2 scoped work-order
service with current writer-turn authorization; local writer-bound attention
proof, bounded native resource diagnostics and Rust native archive. Root owns
shared registration/dependencies/source integration/rollout; Commodore alone
deploys. Parent acceptance remains2/32 and native coding execution false. CI
cleanup stays deferred; current hosted results never replace source-bound proof.

### Writer attention and advisory resource composition — 2026-10-19T04:25Z

Sourceee47a0b84b0fecb6f2afed36f97727cee4285089 composes current opaque actor/
policy authorization on Store's sole attention writer with the paired native
adapter. Exact historical retries recheck current permission; final denial rolls
back staged journal, budget and outbox changes. Prior history and receipt
formulas remain readable. No production issuer, pending-batch compiler or
provider delivery is activated. Independent review clears the exact Store,
credential helper, adapter and retained legacy test assertions.

The same source retains bounded first/latest/final original-cgroup diagnostics
without changing authority, caps or launch bindings. Explicit version2 full
journals separate advisory bytes from the original closure authority digest.
Executor sampling is bounded to250ms; both probe authority comparisons retain
the original closure serialization. Independent review verifies this separation,
including the final formatting-only test delta. Actual root qualification in
an exclusive fresh target passes55 cases:12authority,9prior journal,7bridge,
1versioned journal,11resource and15example tests. Strict server/Store/runtime
all-target Clippy and formatting pass. [Source-bound receipt](../../../receipts/integration/attention-resource-20261019T0440Z.json)
preserves the initial incompatible shared-target build; final checks rebuild
the actual source in an exclusive target. No new provider turn/grant occurs.

The [04:09 rollout CAS](../../../receipts/integration/rollout-reconciliation-20261019T0409Z.json)
archives original bytes and source/history times and records PR102–104 without
parent promotion. SC-DEC139 acknowledges the correction: partial same-source
tests cannot replace full P02/P03/P05/P12/P13 contracts. Accepted parents remain
2/32, general coding execution false, prior0349 grant consumed. Commodore
reports9c4ba020 deployed04:16:32Z; root public health returns200/ok/false.
An additional root full-hash check is queued behind the fleet compute gate;
it is not yet an independent qualification of that deployment.

S1 delivery candidate85c432ba is held for four independently reproduced
failures: loose evidence binding, order-dependent package/stage joins,
foreign-attempt running sessions and silent attention-fact truncation. Its12
original passing tests do not clear these counterexamples. Findings and four
actual failing-before cases go to the existing S1 owner; no competing edits.
Rust archive review likewise reproduced three guard-closure bypasses and the
owner repaired them with existing staging validation; CLI integration remains
owned work. S2 continues the exact current-writer work-order service. The next
context lane owns a bounded authorized original-wake/source read and protected
batch compiler; no claimed digest substitutes for original journal custody.
Actual hosted failures are retained on the single CI cleanup backlog, with no
cleanup lane before implementation under the02:12 human ruling.

### Native Rust archive integration — 2026-10-19T04:36Z

PR105 merged04:27:34Z after exact aee74b5 approval; receivingmain is
6e1c8f577bc7a4cf95251941a30f1ac701882423. Root has now consumed the queued
[independent9c4 proof](../../../receipts/integration/native-9c4ba020-independent-20261019T0424Z.json):
its actual observation time is04:24:18Z, all14 manifest files and both process
paths/start ticks match, HTTP200/307/feed200, Rust producer/modelturns0, and no
retired owned Python PID. First historical-row timestamp was not measured.
The nominal0431 filename in the coordination directory does not change that
observation time. Likewise attention receipt0440 records actual04:25:49Z.
Commodore alone owns deployment of newer receivingmain; root controls no service.

Source413aef4ef8bc745908ac4762f9b8986a14dd3820 registers
`admiralty release archive` over explicit reviewed native stages. All34 actual
cases pass:20 archive,4 CLI child and10 unchanged native-stage/real-Node guard
tests. Strict CLI all-target Clippy and formatting pass. Prior157 dependency
version/source/checksum tuples remain unchanged;7 new packages provide pinned
Rust tar/miniz compression. Independent review verifies manifest hash domains,
no-follow retained handles, bounded streaming/inventory, unchanged guard policy,
deterministic metadata and atomic no-replace/uncertain outcomes. Root also adds
pre/post publication private-parent checks with actual permission-change tests.
[Integrated receipt](../../../receipts/integration/native-archive-20261019T0436Z.json)
preserves three failing-before guard-bypass cases and the repaired source.
README now points to the [Rust archive command](../../operators/native-archive.md).
The historical Python packager remains compatibility reference; no supported
Linux archive, real release payload, installer, signature, publication or full
P30 qualification is inferred from local synthetic archives.

S2 review reproduced two further component failures atcb496c9: credentials
revoked inside writer policy still allowed a real capture commit, and different
equal-length preview texts shared one policy digest. Both actual cases failed
before repair; the existing S2 author owns corrections and current-main rebase.
S1 retains its four failing-before delivery cases. Neither candidate is merged
or accepted from earlier passing smoke tests. The new protected original-wake/
source-read context componentb9e455f has9 actual tests and independent review;
root registration/composition remains next, with no production read issuer or
provider transport enabled. Parent acceptance2/32 and general coding false remain
unchanged. Failed CI stays on the one cleanup backlog without delaying ready code.

### Protected pending-source context — 2026-10-19T04:52Z

Sourceee2018d60fa9bdb74a5a00a03ac976b3fbb01049 composes the original-Wake
reader and compiler with an independent current-reader native policy. Store's
sole writer replay-validates the retained Wake and every first original Observe,
the actual full source renders and current task contracts. Independent expected
receipt, batch, ordering, epoch, render and explicit baseline pins must match.
Complete queued contextA and request contextB survive together; missing custody,
unknown baseline or bounds fail without truncation. Current read permission is
separate from recording and is checked for each full namespace with the opaque
current actor. A historical source credential cannot grant today's reader access.

The [integrated receipt](../../../receipts/integration/safe-turn-context-20261019T0452Z.json)
records actual root9 compiler tests and13 native bridge cases, strict Store/server
all-target Clippy and formatting. Seven original mutation tests retain their
bodies; six new bridge cases exercise independent read grants and actual registry
revocation. Independent source review clears both components. The read policy
defaults DENY; production startup/router issuers and provider transport remain
uninstalled. Existing Wake recording permission is still separately required;
denial does not mint another wake or reset attention budgets. No send, application
ACK, baseline change, measured savings, provider run or parent promotion occurred.

PR106 merged04:39:59Z into b86439f248c3d70398f1757f3ec07676e1a54df6.
Commodore reports6e1c8f5 live04:35:19Z and is the only primary deploy writer;
newer deployment qualification is recorded separately. Owned Linux instrumented
probe build and synthetic archive tests obey actual fleetcap freeze/thaw and
memory limits. Resource admission delays are not source failures or proven CI
causes. An archive runner's first admitted attempt lacked Cargo on PATH and
launched zero tests; evidence is retained for an absolute-toolchain retry.
S1/S2 repairs remain with their existing owners; their earlier passing tests do
not clear the independently failing counterexamples. Accepted parents remain2/32,
general native coding false. Non-build CI remains on the single cleanup backlog.

### PR107 merge, independent deployment and rollout reconciliation — 04:55Z

PR107 merged04:53:21Z after exact cb21e6f approval at04:53:08Z; receivingmain
is586f8c91770bcd2f0da0b90e441c2be2ccb28141. Root's
[independent b864 proof](../../../receipts/integration/native-b86439f2-independent-20261019T0453Z.json)
observes04:53:41.003Z: all14 manifest hashes, exact daemon443726/start57090988
and observer441687/start57090827 executable paths, health/UI/feed200,
18891redirect307, Rust observer0 model turns,256 displayed history rows and no
retired owned Python PID. First original history timestamp remains unmeasured.
This verifies the running b864 release, separately from newer desired586f8c9.
Commodore's Screen footer labels Gemini4Argon; the same hardcopy reports quota
exhaustion. Root notified the Admiral for sole-deployer coverage without paid
upgrade, service control or substituting a configured label for qualified model
identity. Healthy b864 remains the last independently observed deployment.

The [04:55 rollout CAS](../../../receipts/integration/rollout-reconciliation-20261019T0455Z.json)
archives the prior bytes, checks all four transferred receipt hashes, and records
PR105–107 component checkpoints55/34/22 actual local cases. All32 parent states,
foundation packets, unrelated packets and historical evidence remain preserved.
These checkpoints expose implementation progress without converting partial
components to full P02–P31 acceptance. Accepted parents remain2/32 and native
coding qualification false; the older0349 provider grant stays consumed.

Next bounded work retains S1's delivery and S2's intake repairs with their
existing owners. Root delegates a separate immutable provider-reported native
usage/correction journal; no normalization, billing, model qualification,
budget reservation, settlement, dispatch or production issuer is inferred.
Migration0008 and shared Store registration remain exclusively root-owned.
Linux instrumented-probe build-only retry is serialized behind the current
synthetic archive qualification, with unchanged source,600-second limit,
fleetcap and fresh resource gates. No new provider request or grant is prepared.

### Delivery projection integration and further negative cases — 05:07Z

Root integrates S1's frozen4cac3ab candidate, whose actual base isb86439f248c3
(the original handoff printed a different suffix). Its original four independent
counterexamples now pass. Further review reproduced two additional failures:
a real validated receipt for sourceA could be paired with mutable sourceB inputs,
and an optional unmerged repository blocked a satisfied required target. Root
repairs both in the integration branch. The validator privately retains its
actual checked-input digest without changing the original six-field serialized
report; delivery compares that digest with the complete sidecar. Optional and
pre-merge target diagnostics remain visible without adding an unselected merge
requirement. Unknown external effects still block, including optional targets.

The [integrated receipt](../../../receipts/integration/delivery-projection-20261019T0507Z.json)
pins source39eb43d51f70ca5e84d1a5608ecc24e7e4586d2d:22 actual delivery cases,
14 receipt-validator cases,4 compile-fail docs, strict Core all-target Clippy,
formatting and independent final-source review pass. Original red logs and the
new receipt-rebinding/optional-target failures remain retained. Root's two added
tests initially accessed private fields; their E0616 log is preserved and the
tests now use the existing getters, with no visibility or authority changes.
Content-pin constructors are explicitly consistency helpers; independently
trusted caller admission/custody remains required. This registers a pure module,
not a Store/router/publisher/dashboard activation or WO5/parent acceptance.

The instrumented6e Linux probe build now succeeds,140.47seconds on the second
serialized capped attempt, binarySHA256
63ff88d6a32f0546d4aec528e38bd473348e6ebf033da9878082b6173254fe56.
No binary/provider execution, preparation, fresh grant or qualification follows.
SC-DEC133 authorized exactly one consumed run, not a blanket permit; the observed
Admiral and Commodore quota failures do not extend it. Other source work advances.
Linux archive testing atb864 instead found12passed/8failed: a manifest created
underumask077 is0600 while the unchanged archive validator correctly requires0644.
The owned fix uses the retained manifest FD before sync and is separately reviewed
and tested; Linux archive/install qualification remains pending. The preserved
passing Mac archive receipt remains historical and does not overwrite this result.

### Intake, native usage and manifest-mode integration — 05:21Z

PR108 merged05:09:39Z after exact c270a36 approval at05:09:08Z; receivingmain
is7e282c3c1df5f7c618a63679441d7054706f9ab5. Root's next code source
619a00544475bb064899a296dfc3e5830baf1eec composes three cleared candidates:
S2 scoped intake80f2054e, native usage843c22aa and manifest-mode236e647c.
The [integrated receipt](../../../receipts/integration/native-intake-usage-package-20261019T0520Z.json)
records57 actual affected cases:10 usage,11 intake integration,1 intake ticket,
20 archive,5 actual CLI and10 package. Strict Store/server and CLI all-target
Clippy, formatting and independent final shared-source review pass. Packaging
checks run under an owned shell'sumask077; no global user/service setting changes.
The initial CLI check requested a nonexistent library target and ran zero tests;
its log remains preserved before the corrected actual binary target passed20.

The GitHub S2 branch still pointed to oldcb496c9 despite the finished local repair.
Root verified a create-only Git bundle of actual80f2054e, sole parentb86439f2,
SHA2566a71ed0630b46a538186401ed95d9f2575f6f5ecb92a86a6059785c3730feec6,
and imported only its two matching files. The original clone, refs and dirty-work
custody were preserved. No cause for the stale remote is inferred. The repaired
service checks exact current grants after policy callbacks and binds complete
preview bytes; the original two failing cases now pass on registered root source.

Migration0008 and sole-writer usage requests retain immutable provider-reported
counters and amount-only corrections with original contracts, current independent
opaque read/write policies, exact retries/CAS, scoped snapshots and bounded replay.
Unknown model/unit/opening values remain unknown. No normalization, billing,
qualified identity, reservation, settlement or provider ingestion is installed.
The independent intake/attention policies retain default denial; service module
registration creates no production issuer, HTTP route, enrichment UI or dispatch.

The manifest fix normalizes only its newly created retained FD to0644 before
sync in the private stage. Strict archive/source guards remain unchanged. The
fresh619 Linux retest is prepared; its final05:19:49Z caller refused CPU86.28%
before run-capped. Zero wrapper/Cargo/test invocation occurred; an earlier agent
start report was corrected before publication. The unused invocation waits lawful
admission. No Linux pass, clean install, signing or publication follows from Mac
tests. Accepted parents remain2/32 and native execution false. Both manager quota
failures preserve the existing sole-deployer handoff and consumed one-run permit;
root performs no service control, new provider grant or paid retry.

### Native intake HTTP and explicit startup configuration — 05:53Z

PR109 merged05:24:42Z to066706b1d563d7554e19b9ffb0b2137cf66f14e8.
Commodore deployed it05:32:51Z; root's
[independent proof](../../../receipts/integration/native-066706b1-independent-20261019T0540Z.json)
observes05:36:32.886Z and checks all14manifest/file hashes, actual daemon/observer
executable hashes and process start identities, health/UI/feed200 and18891redirect307.
The producer is Rust,0model turns,256history rows, no retired owned Python PID.
The unchanged React bundle hash is retained; no fresh browser/control-count or
first original-history timestamp measurement is claimed. Manager access briefly
resumed for this deploy; fresh hardcopies again show Gemini4Argon quota errors.
Root never operated primary services or changed the sole-deployer handoff.

The exact619 Linux packaging follow-up completed05:28:38Z with35passes
(20archive/5actualCLI/10package),0failures, one capped invocation385.66seconds.
[Its receipt](../../../receipts/integration/native-archive-linux-20261019T0530Z.json)
preserves original b864failure and preflight refusal, checks940source files/
1081entry names against the pinned archive and retains deterministic hashes.
This proves the scoped synthetic workflow under its recorded077environment;
it is not installation/signing/publication/P30 acceptance. Different hosted
directory-permission failures remain on the single CI backlog.

Root's [05:39 reconciliation](../../../receipts/integration/rollout-reconciliation-20261019T0538Z.json)
archives the prior rollout, verifies transferred receipts and records actual
PR108/109 component progress plus independently running066. All32parent states,
P00/P01, unrelated packets and original evidence/timestamps remain preserved.
The refreshed [remaining audit](REMAINING_20261019.md) pins066; its historical
b864deployment paragraph is superseded by the independent proof above.

New sourcead5ea2ed59b5ba3f02dee99451f5499b46b3f511 composes frozen HTTP15423e33
and private configuration19d2bb44 with actual native startup/main. The
[integrated receipt](../../../receipts/integration/native-intake-http-20261019T0553Z.json)
records26actual cases (9config,5startup,3priorstartup,9HTTP), strict server
all-target Clippy, formatting and independent exact-commit review. One case
launches the actual `admiraltyd` binary with cleared environment/private example
files, previews/captures/reads exact multiline source, then closes only its owned
child and reopens the Store with no hidden task admission. Initial root ID
conversion and router-state compile failures remain preserved as zero-test
attempts before repair.

`--intake-config` requires `--native-config`; all closed protected-file policy,
scope/action intersections and credential-domain checks occur before DB open.
The same bridge supplies both writer authority slots. Existing entrypoints keep
None/default denial. Every HTTP operation requires main bearer plus independent
intake credential, exact Origin/Host/non-simple marker and bounded body/query/
response/capacity. Review caught early permit release, framed GET body acceptance
and browser GET Origin incompatibility. The repaired adapter retains a permit
through response materialization and blocking work after disconnect; bounded
POST read variants preserve strict guards. No CORS or profile/inbox fallback.

This adds an explicit optional startup issuer, not a production configuration or
provider/check/acceptance/dispatch grant. No live intake token/file was created
or enabled. Enrichment UI, real coding-provider qualification, durable budget/
model admission, recovery and full parent criteria remain. SC-DEC133 is consumed;
no new provider fixture/metadata/grant/run follows from the successful6e build.
All30open parents remain open; accepted2/32, native autonomous coding false.

### Private native installation and stage permissions — 06:08Z

PR110 merged05:55:38Z to b976b4f5e2fa69685d59a7ca4c83da258418f5b3 after
actual neverhumanbot approval of exact39649ce89439495acf9a7a8f3e598e88dfeeb6fc.
The optional scoped intake startup/HTTP composition above is now on main;
no live intake credential or production intake configuration was created.

Root source9d4dbb0c497efe005520cec5b6b95f6bca7b3365 adds the explicit private
`release install` command and [operator guide](../../operators/native-install.md).
The [source-bound receipt](../../../receipts/integration/native-private-install-20261019T0608Z.json)
records68actual Mac checks:26installer,7actual installer CLI,5archive CLI,
10package and20archive, plus strict CLI all-target Clippy and formatting.
Independent final-source review is clear. Failed original evidence is retained.

Red-team review reproduced an actual old CLI accepting a nonsticky0777 ancestor
above a private leaf. The repaired installer checks source and destination
ancestors through retained identities, refuses untrusted writable ancestors and
uses no-replace publication. An unexpected rename error preserves uncertain
destination/staging identities rather than deleting potentially published bytes.
The CLI emits installed_bytes:null for that unknown outcome. A private operation
seam tests a lost rename return; no real disk EIO or other-UID attack is claimed.
Integrity pins authenticate bytes only, not a publisher. Nothing runs package
payloads, changes services, sets PATH/current links or grants execution/acceptance.

Separately, a real002-umask counterexample reproduced newly staged directories
being0775 and correctly refused by archive guards. New directory creation now
requests0755 filtered by the caller mask; existing directories/source permissions
and all guards remain unchanged. Actual child-process077/002/000 roundtrips pass.
Root reran the composed68cases under an owned002 shell. The one admitted Linux
qualification of exact9d began06:09:34Z under run-capped with observed0002 mask;
it is compiling, not a Linux pass or hosted-CI result at this entry.

A Luna subagent has a separate frozen two-file raw-intake React candidate7a8b716e,
with8focused tests, typecheck and build passing. Independent review and root-owned
navigation wiring remain; those component checks do not qualify production intake
or autonomous coding. The installer likewise leaves full P30 activation, signed
release publication, clean-machine bootstrap and runtime criteria open. Accepted
parents remain2/32 and native execution false.

### Private install merged and Linux qualification — 06:17Z

PR111 merged06:14:13Z to36aa0ebc98e378eb8de69e71016aef034544fafa,
after actual neverhumanbot approval06:13:59Z of exact
52ea60ab050faf7e27b8a39e950c5dd8da8dd8e8. The source checked on Mac was9d4dbb0;
the final commit added only its receipt and execution entry. No hosted queue wait.

The separate [Linux receipt](../../../receipts/integration/native-install-linux-9d4dbb0-20261019T0617Z.json)
records actual68passes at exact9d4dbb0:26installer,7actual installer CLI,
5archive CLI,10package and20archive. One admitted capped invocation exited0
at06:15:58.888Z after385.067seconds, with observed0002 mask. All954source files/
1095entries match before and after. Final scope observation06:17:30Z shows
inactive/dead, empty control group and absent owned job PID. No second invocation,
provider, production installation/service activation, real EIO, Linux strict lint
or full-workspace result is claimed.

Commodore separately deployedb976b4f at06:05:27Z. Root's
[independent observation](../../../receipts/integration/native-b976b4f-independent-20261019T0613Z.json)
at06:13:05.712Z checks all14manifest/file/proc executable hashes, actual daemon
1140458/start57537898 and observer1138335/start57537748, healthy public endpoints,
Rust producer0turns and256history rows. No retired owned Python PID is present.
The same React bundle hash preserves prior static control evidence; this observation
does not count controls, original-history timestamp or qualify a browser anew.
Root performed no primary service control and created no live intake issuer.

A read-only hosted snapshot shows the exact36aa0eb dashboard E2E/accessibility
workflow39208749754 passed, contracts passed and Rust workflow39208749813 still
running. Earlier b976Rust and066 failures remain evidence; no full green claim,
rerun, runner/service change or CI cleanup lane follows from that snapshot.

### Dashboard raw request intake — 06:28Z

Root source3935fe3699617d6c57de6b836c03ed1d035c222e wires the repaired Luna
candidate200b13037db47ed95966274f41a7313c0cfa520f into a lazy Intake tab and
direct `#/intake` route. The [composition receipt](../../../receipts/integration/native-intake-ui-20261019T0628Z.json)
records123actual UI cases in13files, including9intake cases, typecheck and a
production build with68modules and an actual Intake chunk. Independent final
component and root wiring review is clear. The author build without App wiring
is explicitly distinct from the integrated production build.

The form uses actual preview/capture/exact-revision POST routes, separate
in-memory credentials and generated preview validation. It preserves paragraphs
as one unresolved original request. A lost reply retains the exact serialized
command; source editing stays locked until retry or explicit local abandonment.
The latter cannot undo a possibly committed server record. Confirmed receipts
have a separate read-only recovery action. Pending state survives dashboard tab
changes; reload/close still loses the local envelope, as the guide explains.

Original independent counterexamples remain preserved: a1MiB preview became an
oversized capture that still fetched; a same-length changed source passed partial
readback checks. The final component bounds the complete capture before freezing
or sending, guards construction/error handling and checks the full frozen request
and receipt on readback. It also removes a controlled password value prop after
review of installed React showed defaultValue reflecting it into an HTML attribute.
Secure UUID absence is a visible refusal, never a weak-random fallback.

No production issuer/token, task dispatch, enrichment, duplicate-query or delivery
UI follows from this component. A separate worker owns only a new actual native
child/browser journey; it is not run or counted as qualified here. Following the
code-first ruling, that follow-up does not hold locally built/reviewed component
integration. No additional CI cleanup lane or unchanged-suite rerun is started.
All30open parents remain open and native autonomous execution remains false.

### Real native intake browser and accessibility proof — 06:48Z

PR112 merged06:30:14Z to6395f8c76146e5b74e2a8ab3c039987ac70aecb8 after
neverhumanbot approved exactc388f7c035b7f42a18521d319653f6aa73b76aef at06:30Z.
Root sent the sole Commodore a new file pointer requesting this build or verified
successor; root performed no primary service control or live issuer creation.

The [first browser receipt](../../../receipts/integration/native-intake-browser-20261019T0636Z.json)
records one actual private native-server/browser case passing6.51seconds. It
commits the original117-byte Unicode source before deliberately dropping that
response, navigates away/back, retries identical bytes/command ID, observes
replayed:true and reads the exact original revision. Actual native tasks and
coordination messages stay zero; both authority flags remain false. Separate
credentials never appear in markup/storage/URLs or saved transport proof. That
first case did not run axe and is preserved with its exact earlier source.

Adding the existing pinned axe scan then failed the new form: Owner access and
Preview exact source inherited light text on browser-default light buttons,
ratio1.18:1. The actual failed run, bounded failure transport and owned-child exit
are retained. Root changed only six scoped button styles, with no API/state,
rule, sandbox, timeout or assertion change. The
[final receipt](../../../receipts/integration/native-intake-browser-axe-20261019T0648Z.json)
pins4c5b2fdbf8e74a0224e1a7ec5a6a6ebfc2005c89: one actual case passes2.15seconds;
axe4.14.0 reports zero violations/incomplete,29passes. Source/bundle hashes,
actual binary7f414502 and direct child60163/SIGTERM/confirmed exit/PID absence
are bound to the proof. Dashboard and E2E typechecks pass. Independent final
source/proof review is clear. Existing123UI and68Linux suites are not repeated.

The new E2E source requires an exact binary SHA and never falls back or skips.
Three workflow lines export the hash of the actual newly built binary, preserving
all jobs, existing backend, runner labels, deadlines, sandbox and axe. Actionlint
passes with the actual existing custom-label catalog; its first plain invocation
reported only those undeclared custom labels. This is test registration for new
functionality, not an additional CI cleanup lane. The controlled capture deadline
from the broader36aa Rust run remains on the single backlog; archive/installer
groups on that source passed. Neither this private journey nor its strict axe
result qualifies a production intake grant, coding provider, general outbox,
reload recovery or any complete parent. Accepted2/32; native execution false.

### Merged browser proof and live rollout reconciliation — 06:53Z

PR113 merged06:51:34Z to5822aac041f41a559efa5ce3d79ee8447e766f1a after
actual neverhumanbot exacte3cc33ca32d868bcd037c2565eb01f59187eee88 approval
at06:50:29Z. The new intake browser/axe proof, scoped contrast repair and fresh
CI binary-pin registration are integrated. No open implementation PR was left.

Root's [06:52 reconciliation](../../../receipts/integration/rollout-reconciliation-20261019T0653Z.json)
verifies six transferred immutable receipts and archives the previous rollout.
It records actual PR110–113 component results without changing any parent state,
criterion, prior evidence, foundation packet or unrelated packet. It preserves
the original source observations in history; it does not refresh producer time
without an observation. Accepted parents remain P00/P01 only.

Commodore deployed6395 at06:36:14Z. Root's
[06:53 independent proof](../../../receipts/integration/native-6395f8c-independent-20261019T0655Z.json)
checks all15manifest/file/proc executable hashes, daemon3152134/start57722577,
observer3151000/start57722420, health/UI/feed200 and18891redirect307. Rust producer
reports0model turns and256history rows; no retired owned Python PID is present.
The actual Intake chunk matches the tested precontrast UI. Root does not infer
that the newer5822contrast repair has deployed, recount controls or qualify a
live browser. Root did not control primary services.

The [production-reference CAS](../../../receipts/integration/rollout-production-6395-20261019T0655Z.json)
then appends only P31 evidence and preserves every other packet byte-for-byte.
Current rollout SHA is e919843111da6a654ea11b6c4aebe5f09e9fc504747409a04ab8a9236cf9998d.
The sole Commodore receives the newer merged-source deploy request. General
coding-provider admission, production intake activation, model/capacity/budget
settlement, restart recovery, remaining work-order journeys and full final CI
remain separate gates. No fresh provider grant/fixture/run is created; SC133's
one route is consumed. The actual broader Rust capture-deadline failure remains
on the single backlog, with no extra cleanup lane or unchanged passing-suite rerun.

### 2026-10-19T07:25:37.472Z — native offline WorkOrder CLI component

Root integrated Luna candidate6727ac2 and registered the actual CLI on code9da94ab4d2d026d3fea78d61b8c51496a0c6202b. Five actual binary cases, strict CLI all-target Clippy and fmt pass. Independent review found private serde errors reflected a synthetic secret key; the failing-before case is retained and static diagnostics repair it. Read commands now refuse missing databases, and actual lock/stale-generation/zero-task-message-event/outbox assertions pass. Scope is preview/capture/explicit record/exact reads/all-state duplicate query in an exclusively owned offline Store; no provider, live issuer, task dispatch, full WO6/parent acceptance or deployment claimed. Evidence: [native-work-order-cli-20261019T0727Z.json](../../../receipts/integration/native-work-order-cli-20261019T0727Z.json). Exact-head forge approval and receiving-main merge are still pending in this source snapshot.

### 2026-10-19T07:28:26.861Z — durable budget admission and merged offline CLI

PR115 exact d225c8c was approved by neverhumanbot07:26:36Z and merged07:26:56Z to main03989ae76f93f38f6e3d2a63619c39b8fde74989. Root rebased the budget component to codee766ce6af29beb141568a7f07b86ad3b0e86153d. Actual reservation journal/atomic claim15 cases, lineage helper1, controller9 and composed CLI5 pass; Store/Server all-target strict Clippy and fmt pass. Separate source review cleared repaired bounds, immutable/reverse bindings, historical retry and final incident authorization. Original controller UnknownExposure fixture failure remains archived; known fixture budgets repaired it without relaxing the reducer. Evidence: [native-durable-admission-20261019T0730Z.json](../../../receipts/integration/native-durable-admission-20261019T0730Z.json). No production issuer/global slots/provider/settlement/recovery/general execution or full P03/P10/P14 acceptance.

Fresh actuala9e CI browser passed, contracts/fmt/strict Clippy passed; Rust unit run86pass/1controlled-capture-deadline failure/2ignored is retained in the single CI backlog. No additional cleanup lane/rerun. Read-only07:09Z primary observation remains6395 native binaries, healthok/executionfalse and no retired owned Python; both actual Gemini4Argon supervisor Screens show quota errors. No deploy handoff, service control, provider grant or new provider turn. Root source continues independently; soleCommodore owns latest-source deployment.

### 07:59Z — merged admission deployed, authority correction and failed diagnostic

PR116 exactd38187af was approved by actual neverhumanbot07:30:58Z and merged
07:31:17Z to `fcd35569ccfed7429e1bb4b22dfcb7dc142f9700`. Both115/116 are
merged with their component scopes intact. Root's07:33/07:36 rollout CAS receipts
archive prior bytes, verify12artifact hashes and preserve all32parent statuses,
criteria, existing evidence and producer/source/history clocks. The second CAS
corrects unsupported deployment wording only; it grants no authority.

The newer written06:47Z SC-DEC-155/156 order was discovered after the previous
snapshot. It permits one exact fresh diagnostic and delegates serialized backup
deployment to885617.adm_p18a_conflicts while2062267Commodore is quota blocked.
It supersedes the earlier no-handoff/no-new-route statements. Root does not
control primary services. The backup Screen actually shows Luna medium with
pending input/menu; root leaves written files without overwriting that input or
spamming quota-dead supervisors. No requested model is relabelled as observed.

Commodore deployedfcd at07:42:20Z. The [independent primary proof](../../../receipts/integration/native-fcd35569-independent-20261019T0800Z.json)
at07:59:37Z verifies all15manifest/file hashes, actual daemon3419362/start58119247
and observer3417759/start58119075 executable hashes, UI/feed/health200 and18891
redirect307. Rust producer reports0model turns;256history rows displayed/480
retained. Original first history time and new browser/control counts were not
measured. Same contrast-repaired intake/index bundle hashes are preserved.
Prior deployment PIDs and retired owned Python matches are absent. Native
execution remains false; root performed no service control.

SC155 was consumed exactly once on a fresh private fixture/request, unchanged
CPU0,1/512MiB/Tasks32/120s/concurrency1, actual CLI/auth metadata and fleetcap
gates. The [failure receipt](../../../receipts/integration/native-sc155-diagnostic-failure-20261019T0759Z.json)
pins6e binary63ff88d6 and request98dd59dc. Run exited17 at07:48:39Z: dispatch
not durably acknowledged; source unchanged. Read-only07:56 inspection finds
durable outbox unknown, execution deadline elapsed and recovery retained, no
receipt. Same systemd invocation timed out, MainPID0/recordedPID absent, no
current cgroup identity. Retained CPU/memory counters are advisory; provider
turn/model/usage/billing and resource closure remain unknown. No completion
observations were reached. A scratch reader's sysfs-size mistake is preserved
separately and repaired; it was not an actual fleet-gate breach. This consumed
grant/fixture is never retried. An isolated source lane adds diagnostics before
Held exits; it has no new provider authority.

Exactfcd browser39216521433 passes; Rust39216521388 instead exposes a real new
CLI preview directory-permission defect,2cases pass/3fail. Original raw failure
hash is retained on the single backlog. A disjoint Luna repair uses atomic0700
creation and child-umask002 cases; independent source review is clear, Mac5/5
and strict checks pass. Exact Linux qualification then passed all5actual binary
cases at08:02:45Z in one fresh owned fleetcap target with jobs1/private TMP,
unchanged assertions/overflow and child umask002. The [repair receipt](../../../receipts/integration/native-intake-cli-linux-repair-20261019T0804Z.json)
pins source7f203848/archive404b2a05 and complete logbeffc690; outside/inside/after
gates are retained. Mac checks are worker-captured output, not fabricated raw
log hashes or bot-executed checks. The older failed hosted run stays failed.
No Store guard, shared umask, deadline or runner setting is changed. OnlyP00/P01
are accepted; production issuers, global slots, provider/context/closure,
settlement/recovery and remaining work-order/release journeys remain open.

### 08:08Z — composed Held diagnostics and reviewed rollout correction

Root integrated the example candidate7cadfd49 and a narrow read-only
`NativeTaskLoop::pending_diagnostics` accessor on code22153c4b. The
[composition receipt](../../../receipts/integration/native-held-dispatch-diagnostics-20261019T0810Z.json)
records all21actual example cases passing11.21seconds, including6new cases;
strict Server all-target Clippy and workspace fmt pass. Worker full21 is a
separate overlapping check, not21additional independent cases. Source/artifact
red-team review is clear. Original compilation failures remain pinned.

Held/Idle/typed-error paths retain a bounded32KiB create-only0600 diagnostic
before exit: actual retained advisory samples, explicit missing reasons and
hashes of at most3exact protected journal files. Raw prompts/argv/tokens/output
are never copied; no new resource poll, process signal, reconciliation, model
identity, billing, receipt, closure or retry authority is manufactured. Tests
cover absent completion, numeric/missing resources, private canaries, symlinks,
oversize input, create-only collision and malformed digest denial. No new Linux
probe binary, provider turn or reused SC155 grant follows from these checks.

The [08:05 rollout correction](../../../receipts/integration/rollout-provider-fcd-20261019T0806Z.json)
pins exact6artifact names/digests/sizes and syncs archived prior bytes/directories
before sole-writer compare/rename. It appends onlyP08/P09/P31 evidence, preserving
every parent status/criterion, all original evidence and producer/source/history
objects. Current rollout SHA is
`7a40edc0858d544a90c4c692a9499ea33de70e5ba45ccc246ee651b84d0fc0b3`.
The current reference now distinguishes actual primaryfcd, SC155's failed
consumed diagnostic, explicitSC156 coverage and actual Linux5-case CLI repair.
Two pre-execution review findings (exact manifest inclusion and preimage sync)
were repaired and independently cleared before the mutation. No parent promoted.

Next ready source packet: compose the existing atomic budget/controller path
with explicit optional NativeTaskLoop admission and frozen retry state. Read
`controller::{planned_launch_intent,admit_one_reserved}`, Store
`admission_reservations` and current loop phase/custody tests first. Root owns
the loop file/exports; a separate lane may own new focused tests only after its
interface is frozen. Preserve ordinary compatibility mode, current writer
authority, command/source/CAS/effect bindings and unknown liabilities. Exact
recorded retries must reauthorize current budget authority; DispatchHeld must
never be reset to queued. Keep resource permit/plan reservation separate from
budget proof, and do not install production issuers/global capacity by adding
this composition seam. Validate actual nonzero atomic retry/revoke/collision,
rollback/registration custody and unknown-dispatch cases before any activation.

### 08:14Z — PR117 merged; latest browser failure retained

Actual neverhumanbot approved exact07c66f7db4d7f40b19259ceff1ab221b247e9a6d
at08:09:31Z, transparently registering Root/separate Codex findings. Matched-head
rebase mergedPR117 at08:09:40Z to749d9d299f23ef806f00c3893be484049616d9f5.
No open PRs remained. Root rebased the next clean source branch to that main and
wrote the exact-source result/deploy capsule in the existing coordination area.
The actual fresh Gemini4Argon supervisor hardcopies still show quota errors;
SC156 backup Luna has pending old input. Root leaves a written pointer without
overwriting input or adopting service control. Latest independent primary is
stillfcd at its recorded07:59 time;749 is requested, not independently deployed.

New749 browser workflow39220795727 finishes19pass/1fail: `atlas dark` has pinned
axe `label-content-name-mismatch` on hierarchy cards. This is a concrete semantic
failure, distinct from navigation/readiness/resource-pressure hypotheses. Complete
raw log/hash is retained in the single CI backlog. Rust39220795716 remains in
progress at this observation. No rerun, shared-runner change, weakened assertion
or extra CI cleanup lane is introduced. Current scoped CLI5/diagnostic21 checks
remain scoped evidence; neither this failure nor source merge changes parent
acceptance. Root's existing30-minute automation now contains the actual source,
deployed proof, SC155 consumed failure, SC156 coverage and next budget-loop task.

### 08:41Z — actual budget-backed loop component and Linux proof

Root claimed only the loop source/shared registration and exact documentation
at base0970118. The initial Luna test lane hit model capacity after drafting the
new sibling test module; a separate existing review agent finished that same
preserved test-only worktree. No extra Screen, runtime/provider fallback or paid
upgrade was introduced. Root retained ownership of implementation/fixture edits.

Code84cbf43e9d37c8b2fa4648a04de745fb05113126 adds explicit
`NativeBudgetAdmission` and `admit_reserved`, preserving ordinary compatibility.
It rejects source/real-host incarnation, actual reservation/scope/amount/
correlation and exact launch-digest mismatches before custody transfer. Retained
reserved retries reauthorize the original opaque proof/current writer and read
policy. Scoped dispatch replays the same already-claimed effect and receipt
without a second debit, then checks the current reservation again immediately
before executor invocation. Unknown dispatch never requeues or releases liability.

Independent review found three concrete issues before integration: the durable
budget version is the global ledger CAS successor rather than the resource
plan's version; incompatible producer/host incarnation must return preparation
before registration; and RequestStop can intervene at the second clock sample.
All three were repaired. The receipt distinguishes static review findings from
executed tests; no fabricated failing-before run is claimed. The new tests use
real Store transactions, actual Git/preparation and an unconfigured ResourceGate,
so an executor call is explicitly distinct from a child/provider launch.

The [component receipt](../../../receipts/integration/native-budget-task-loop-20261019T0841Z.json)
pins all16targeted cases passing on Root Mac8.70seconds and actual Linux15.28
seconds;8new cases are a subset, not an additional acceptance count. Linux ran
once08:36:57–08:39:04 under fleetcap with fresh outside/inside/after gates,
jobs1/testthreads1/private TMP/unique target and unchanged assertions/overflow.
Only an old owned build cache was copied; all four workspace packages were
cleaned in the new target before compilation. Exact archive1ff50f54 and raw
log1ff1d875 are retained. Helper strict Server all-target Clippy used identical
code hashes; workspace fmt/diff checks pass. Source/test/qualified-doc review is
clear. Original compile errors and a recovered test's wrong clock-count assertion
remain archived; only that expectation changed, with production guards intact.

A separate [read-only SC155 spool inspection](../../../receipts/integration/native-sc155-spool-inspection-20261019T0840Z.json)
at08:35 finds6contiguous frames and partial thread/turn-start plus one completed
agent-message item. It finds no completed turn, model/usage or final producer
watermark. Physical journal EOF cannot reconstruct producer EOF/closure, and
absence of fixed error markers is no cause/absence proof. No raw prompts, argv,
credential or provider text is emitted; no new run/grant/retry/service control.
The reason for the120-second wait remains unestablished.

Root's08:31 read still observes the samefcd primary process/start identities and
health200/executionfalse;08:39 finds no retired owned Python matches. The full
15hash proof retains its original07:59 time; no new source deployment/browser
qualification is inferred. Broader749 Rust CI later fails a different CLI case,
89pass/1fail: owned Node guard smoke exceeded deadline. Its raw hash and earlier
browser19pass/1axe failure remain in the single backlog, with no causal resource
claim, guard relaxation, rerun or new cleanup lane. Source implementation keeps
moving. No production issuer, global capacity, provider, settlement, recovery,
physical-start linearization or full P03/P10/P14/WO/release acceptance is claimed.

### 08:53Z — PR119 merged, rollout preserved, actual deploy owner advancing

Actual neverhumanbot approved exact530db6ebf7d4f3c29b9735262f8969a0fd882706
at08:47:13Z, transparently registering Root and separate Codex source/evidence
review. Matched-head rebase mergedPR119 at08:47:27Z to
18417d88def462e69d44bd6641d04109b373a67b. The actual postmerge08:50:55.821Z
forge snapshot has no open PRs. Root rebased onto receiving main; new work uses
an isolated branch. The [merge receipt](../../../receipts/integration/native-budget-loop-merge-20261019T0852Z.json)
and [final independent review](../../../receipts/integration/native-budget-loop-review-20261019T0844Z.json)
pin that scope; no bot test execution or whole-CI qualification is claimed.

Fresh097 CI results are preserved in the same single backlog: Rust39221684261
reached `native_codex_plan` and failed all5cases with `UnsafeSourceMetadata`
at229; browser39221684219 reported19pass/1Atlas dark axe mismatch. The old749
Node-guard deadline remains distinct. Both raw097 logs are archived with exact
hashes. These are failed workflows, not assumed resource issues or authorization
for weaker guards. No extra cleanup lane/rerun/cancellation/runner control.

Root's sole-writer rollout CAS at08:52:50.738Z verified eight exact artifact
name/size/hash pins, fsynced the immutable preimage/archive directories before
replace, and read back new SHA
c8bbfb619105d6692a2ce74a9eceece10ca05086d4126e62699fc9de7daec002.
The [proof](../../../receipts/integration/rollout-budget-loop-20261019T0853Z.json)
and [independent script review](../../../receipts/integration/rollout-budget-loop-review-20261019T0853Z.json)
preserve every32parent status/criterion/prior evidence and original producer
source/history objects. Only additiveP08/P14/P31 component notes and exact main
reconciliation change; P00/P01 alone remain accepted. The full primary proof
still retains its recorded07:59 time, distinct from cheap08:31/08:39 checks.

Fresh08:52:25 hardcopies show the Admiral/Commodore still on actual Gemini4Argon
with quota errors. The SC156 backup885617 session now actually shows
GPT-6.1-Sol xhigh and an active background build ofPR119, replacing the older
pending-input/Luna observation. Root read the written08:24 GB order and actual
08:47 blocked097 receipt: that candidate was not deployed because main advanced.
The receiving18417 build is active; no latest-source deployment is yet inferred.
Root's `CODEX_OBS_SC156_BUILD_18417D8_20261019T0853Z.md` leaves this owner in
control and keeps Root's next receipt merges off main during this specific
serialized build. There is no global merge freeze, second deployer or overwritten
Screen input. Result/deploy capsules name current18417 or a verified successor.

Next source review confirms model_selection already proposes linked slot/kernel
reservations and execution/settlement gates; its updated capacity is a cloned
snapshot and settlement explicitly has `durable_store_committed:false`. Do not
rebuild those components. The next bounded gap is a reserve-only durable inference
pool/slot journal joined to the exact claim/budget/outbox transaction. Root owns
migration/Store/shared registration, a future disjoint worker only new
`inference_slots.rs` and its tests after a frozen interface. Require independently
authorized managed-slot allotment/current policy; aliases or new generations
cannot reset held occupancy. Exact retries must make no second debit/hold;
unknown dispatch/stop/expiry/revocation never release a slot. Actual tests must
cover last-slot races across projects, current-policy denial, replay/upgrade
rejection, identity/capacity reduction, rollback, corruption and reopen. No worker
is reported running on that packet yet; no issuer/release/settlement/provider or
whole-parent qualification is implied by this next-step review.

### 09:08Z — repair the actual deployment-blocking smoke parser

SC156 owner885617's18417 release build completed, but its single isolated smoke
failed before cutover: the parser validated the unterminated stderr fragment
`Admiralty listening on http://`. The original09:01 blocked receipt and failed
smoke remain immutable; fcd primary stayed healthy and unchanged. Root read the
newest08:48 watcher note and actual blocked result, claimed only the harness and
inline tests, and sent a file pointer only after09:04 hardcopy showed blank ready
input. The deploy owner retains service control; Root performs no live cutover.

Source53bc56d8a6232225302f54127e5030d4e4530961 changes one parser iteration to
consume only LF-complete records. Pipe reads have no line-boundary guarantee;
an incomplete tail must await more bytes. The unchanged strict listener suffix,
literal loopback/protected-port validation,64KiB capture, HTTP/startup/whole
deadlines and owned-child cleanup remain. Five new inline methods cover every
byte split/CRLF, EOF tails, complete malformed/protected listeners, non-LF
controls and an actual staged pipe. Separate source review is clear.

The [framing receipt](../../../receipts/integration/canary-listener-framing-20261019T0908Z.json)
pins23Mac and23sameLinux self-tests, including the5new methods. A synthetic
current-script fixture restoring only the old predicate actually fails those
5methods with4assertion failures/111subtest errors; it is not an exact old Git
revision or115independent cases. The original source failure is also preserved.

Root's first gated Linux smoke fixture was incorrectly placed below the shared
0775receipt ancestor. The native private-config guard correctly denied it.
Root retained that typed failure, then created a completely fresh0700fixture
under existing protected `private-config`; shared permissions/guards stayed
unchanged and the passing23self-tests were not repeated. Exact immutable18417
binary hashes with fixed53bc harness then pass all15real startup/HTTP/auth/model-
proposal/demo/doctor checks in3.039seconds. Fresh outer/inside/after fleetcap
gates and wrapper/raw hashes are pinned. Owned smoke children were terminated;
no provider/grant/SC155retry, live ports, service control or parent acceptance.

This is the narrow required fix to the existing deployment-only Python test
harness. It adds no Python runtime/collector/ingestion/service. Root packages it
with the pending merge/rollout receipts rather than advancing main solely for
another receipt. The failed production-deploy attempt remains failed; the new
isolated pass is a separately sourced qualification for the serialized deployer.

### 09:45Z — durable inference holds and independently verified af6 primary

PR120 exact522442d23ee7f2ef155c00e078cf5be2acd8a5e4 was approved by actual
neverhumanbot09:10:30Z and matched-head rebase merged09:10:39Z, receiving
af6aeea9bb7353f80ef2ef9d573aba1790f0c593. Root's09:15:04.268Z sole-writer
CAS read back58e1fca8a5053875b9b334a0444e883791ef59d388e03709f501076c1d9359e6;
all32packet objects and original producer source/history remained unchanged.
These operational results are appended with the next actual source packet,
avoiding a receipt-only main advance during the serialized deploy attempt.

The newest read watcher0319 still records quota-dead actual Gemini4Argon
Admiral/Commodore and stale manager status. Its unrelated GPU/project directives
are outside this lane. SC156's explicit backup885617 authority remains the
deployment basis. Its09:17 resource hold was real:10.81GiB available, controller
frozen. Root neither thawed nor bypassed it. Fresh09:32resource clearance and
the owner's independent inside/outside gates allowed its one bounded build.
Root continued isolated code/test work and kept main unchanged during that
attempt. Fresh hardcopy showed the backup actually GPT-6.1-Sol xhigh building;
Root did not overwrite active input or claim a new ACK from a file-only note.

The deployer's fresh af6 binaries and React bundle passed15actual smoke checks
at09:40:08Z. It backed up the private database and cut over09:40:41.790291Z.
Root independently verified the14manifest file hashes, actual daemon2704251/
start58829578 and observer2703800/start58829547 executable hashes, plus health,
UI/feed200 and18891redirect307 at09:42:31.675Z. The
[independent proof](../../../receipts/integration/native-af6aeea9-independent-20261019T0942Z.json)
pins manifest15fb0619 and Rust producer0model turns,256displayed/528retained
history rows, no retired owned Python PIDs and no priorfcd daemon/observer PIDs.
Original first history time/browser controls were not newly measured. Config
and service control belong to the deploy owner; Root performs neither.
`native_execution_qualified:false` remains an actual health fact.

Root claimed only the next Store packet in
`CODEX_CLAIM_DURABLE_INFERENCE_SLOTS_20261019T0920Z.md` after reading current
coordination. A frozen API gave disjoint module and Luna test scopes; Root owns
migration0010/shared integration/docs/acceptance. Private worktrees and candidate
branches remain preserved. Source d9cd57464b01f08a59fef97159d5a040dddc4dbe now
joins task claim, budget reservation, managed inference hold and exact launch
intent in one sole-writer transaction. Exact current-authorized replay retains
original receipts and cannot upgrade/downgrade ordinary/budget/slotted modes.
Both association directions and original event/budget/intent bindings are
validated on open/rebuild/backup. A final slot denial sees the staged Claimed
projection and rolls every admission effect back. Outbox conflicts may commit
only the existing authorized integrity incident. Generation changes retain all
holds; immutable unique physical keys and below-held reduction guards prevent
alias/generation capacity resets. There is no release/reset/expiry eviction.

The [component receipt](../../../receipts/integration/native-inference-slots-20261019T0945Z.json)
pins RootMac219/219 and actualLinux219/219 full Store suites, the same cases on
two platforms. Six unit plus ten integration cases are16new cases within219,
not additive platform/worker counts. Linux used one freshly gated capped
invocation, jobs1/testthreads1/privateTMP/unique target, copied dependency cache
without modifying it and rebuilt workspace packages. Archive and post-run
source hashes match. Strict Store/server all-target Clippy and fmt pass.
The [separate static review](../../../receipts/integration/native-inference-slot-review-20261019T0938Z.json)
is CLEAR; no reviewer/bot test execution is claimed. Initial Clippy and two
9pass/1fail draft assertion results remain preserved, with corrected fixture
expectations and unchanged production guards.

Review identified a trust-contract limit: a final slot callback could mutate
budget permission after its prior sample. This is a static counterexample,
not an executed repro. Both traits/docs now require observational callbacks
that cannot directly/indirectly mutate any admission authority. Independent
policies are sampled sequentially; joint revocation through COMMIT requires a
common retained snapshot/epoch/lease fence. Repeated Boolean checks cannot
provide that fence. No production issuer/fence or physical pool mapping is
installed, so this source is an optional Store component, not global capacity
or provider activation. P00/P01 alone remain accepted; other30 parents stay open.

Fresh09:30forge state had zero open PRs; af6 browser39228085850 passed while
af6 checks39228085831 was cancelled by an unestablished external actor/reason.
Cancelled is not green. The earlier097/749 and18417 failed workflows remain
in the single CI backlog; no rerun/cancellation/runner control or inferred
pressure cause. New source is packaged for exact-head review/merge next.

Next: compose the durable slot receipt with actual model selection, prepared
plan/controller/NativeTaskLoop and current execution authority; independently
authenticate managed pool allotment and provide a common policy fence before
production activation. Provider routeSC155 remains consumed/unknown and is
never retried. Context delivery/closure/settlement/recovery and WO6/release
criteria remain separate ready implementation, not blocked by an old I1.

### 10:25Z — retained model-slot composition and a9d0 primary proof

PR121 exact `4d1eeef41f9dac47811d94150216ce45105e0069` received actual
neverhumanbot approval at 09:46:35Z and matched-head rebase merge at 09:46:47Z,
receiving main `a9d0a002628d1b4d433380aab7a414c328412f96`. Root's strict
09:49:52Z rollout CAS read back `43aee6ee09eaa43b443dbad68b97d9ff485a6fe9c01adef5b993c62ac5cc120a`;
all 32 packet objects and original producer source/history remained unchanged.
These results are recorded with actual source work rather than another
receipt-only main advance.

SC156's sole backup deployer built a9d0, passed 15 real isolated smoke checks,
and cut over at 09:57:56.840827Z. Root's read-only independent proof at
10:00:15.499Z verifies all 14 manifest files and actual daemon 51100/start
58933080 and observer 50478/start58933052 executable hashes. Health/UI/feed
return 200 and 18891 redirects 307. The Rust producer reports zero model turns,
256 displayed/537 retained historical rows, no retired owned Python and no
prior af6 processes. Original first-history timestamp and browser control count
were not newly measured. Manifest `f706b877...` and actual process/file hashes
are pinned in [the primary proof](../../../receipts/integration/native-a9d0a002-independent-20261019T1000Z.json).
Root performed no service control; health still says execution unqualified.

After reading current coordination, Root claimed controller/factory composition
in `CODEX_CLAIM_SLOTTED_CONTROLLER_FACTORY_20261019T1002Z.md` and native-loop
extension in `CODEX_CLAIM_SLOTTED_LOOP_AMENDMENT_20261019T1007Z.md`. Frozen APIs
gave disjoint factory-source, Luna controller-test and factory/loop-test scopes.
Root owns shared controller/loop integration, receipts and rollout; all worker
candidates/worktrees remain preserved. Watcher0348 still reports quota-dead
Gemini4Argon supervisors, not new active manager progress. No pending supervisor
input was overwritten and no unrelated project/GPU directive was acted on.

Source `a08d2e8d2bfc55121a0ce96532aa9c8fa822c65b` adds explicit
`admit_one_slotted`, private binding minted only by the original bound Codex
preparation, and `NativeTaskLoop::admit_slotted`. The private binding retains
the actual selected ModelSpec/prepared tuple, full kernel claims/reservation,
separate inference claim and both commands/proofs. Actual LaunchRequest.model
and plan.model_declared are registry IDs; provider-requested model is a
different tuple field. Full ModelSpec and routing-field digest domains are
computed independently. Neither becomes observed identity. Kernel scope and
Codex producer incarnation are independently bound rather than conflated.

Rejection returns the original preparation/plan and private slot capability.
Successful registration retains that custody and original CAS-successor
receipts across uncertain replies. Retry and dispatch require current writer
authority, exact unchanged budget/whole hold and current physical key/generation;
unrelated global-ledger advances do not invalidate original receipts. The final
RecordingRuntime clock guard repeats the protected check. Unknown/held effects
cannot downgrade, requeue, refund or release. Policies remain observational and
sequentially sampled; no common through-COMMIT/physical-start fence is installed.
Pure private-plan comparison does not attest live filesystem inode custody.

[The component receipt](../../../receipts/integration/native-slotted-loop-20261019T1025Z.json)
pins final Mac 18 integration + 16 existing loop cases and actual capped Linux
the same 34 cases, with 13 new cases within 34. Counts across workers, earlier
revisions and platforms are not additive. Final strict server all-target Clippy
and formatting pass. Linux rebuilt workspace packages in unique copied targets,
jobs1/testthreads1/private TMP, with fresh outside/inside/after resource gates,
archive and post-run source hashes. Two Linux invocations are preserved: c175
passed all 34, then a same-predicate test assertion spelling repair produced
a08 and all 34 were rerun on that final source. Both logs and the original
strict-Clippy bool-assert failure remain retained. No runtime guard was weakened.

Earlier draft compiler failures and the oversized rejection-result Clippy error
are retained; boxing the same original slot capability repaired the latter.
An unchanged original factory fixture was actually reproduced under an owned
shell umask002 (0 pass/1 fail/4 filtered, UnsafeSourceMetadata). Only its owned
Git child now uses argv-preserving umask077; process-global/shared settings and
production guards remain unchanged. The separate source review is static and
does not claim reviewer/bot test execution.

Main a9d0 browser workflow39232621504 passed. Rust workflow39232621495 actually
failed controlled capture at native_executor.rs2319: 95 server cases passed,
1 failed, 2 ignored. Raw SHA `c81df12a...` is archived in the single CI backlog.
No inferred resource cause, CI rerun/cancellation or shared-runner control.
P00/P01 alone remain accepted; 30 parents remain open. There is no startup
issuer, independently authenticated live pool/model policy, configured ResourceGate,
provider completion, new provider grant, settlement/release, restart recovery or
general execution qualification. SC155 remains consumed/unknown and is never
retried. Next ready work is those production authority/custody and provider/
context/closure/recovery gates plus remaining WO6 and release criteria.

### 11:05Z — common local policy lease and independently verified e71e primary

PR122 exact `60b53ff2854be6b44d730a48579d537ca2bc38c6` received actual
neverhumanbot approval at 10:30:55Z and matched-head rebase merge at 10:31:08Z,
receiving main `e71e12f709a13f27ddbb235c30ad0ee29e77d866`. Zero open PRs were
observed at 10:31:44Z. The source/test hashes match the qualified a08 payload.
Root's eight-pin 10:33:58Z rollout CAS/readback produced `ef6a5bcb...`, retaining
all 32 packet objects and producer clocks/history. These operational results
are recorded with the next real source packet rather than receipt-only churn.

SC156's sole backup deployer actually built e71e, passed 15 isolated smoke
checks and cut over at 10:41:26.512074Z. Independent Root inspection under fresh
fleet gates at 10:49:10.466Z verifies all 14 manifest files, daemon 3873790/start
59194048 and observer 3873358/start59194019 executable hashes. Manifest is
`88649297...`; health/UI/feed return 200 and 18891 redirects 307. Rust producer
model turns remain zero, history is 256 displayed/562 retained, and no retired
owned Python or prior a9d0 processes remain. Original first-history/browser
control count were not remeasured. [The independent proof](../../../receipts/integration/native-e71e12f7-independent-20261019T1050Z.json)
keeps execution unqualified. Root performs no service control.

The latest read watcher0410 still records actual Gemini4Argon quota failures
and a stale 07:57Z manager file; its parent counter does not measure later source
components. Other-project directives are outside this lane. Existing SC156
deploy authority remains with backup885617. Root claimed the common-fence
packet in `CODEX_CLAIM_COMMON_ADMISSION_FENCE_20261019T1048Z.md`, using a frozen
API and disjoint source-module/Luna child-test scopes. Root owns shared Store
integration/docs/rollout. No supervisor pending input was overwritten. Candidate
branches/worktrees and the superseded committed test draft are preserved.

Source `feee6086b40f6125b8e76c0e86b01413df0daaab` adds an optional startup
lease, acquired before BEGIN and authority samples and retained through every
protected success, replay, incident-only commit, rollback and error outcome.
Both original policies and all ledger/CAS/source/effect checks remain mandatory.
With this admission-only profile installed, every TaskClaimed event needs both
budget and slot inputs and standalone journal mutations deny. The absent
profile preserves ordinary/budget compatibility, without a weaker-path request
flag. The private paired cohort owns all participating rules/epoch under one
RwLock. Nonblocking read leases exclude successful exclusive rotation/revocation;
Busy changes nothing, Applied checks/advances the epoch and invalidates old proofs.
Distinct private proofs bind coordinator Arc identity, role and complete exact
source/event/time/effect/command/scope data. Bounded same-cohort try_read samples
are explicit; no blocking writer, Store reentry, external mutable callback or
thread-local context is introduced. Contract serialization is bounded first.

[The component receipt](../../../receipts/integration/native-admission-fence-20261019T1105Z.json)
pins the same 227 full Store cases on Mac and actual Linux, zero failed/ignored/
filtered. Eight new cases are a subset: four primitive units and four Store
journeys. Fifteen nonempty targets plus an empty doctest target are recorded;
worker/platform/revision repeats and nested scenarios are not additive. Linux
uses one fresh capped invocation 10:59:25–11:03:13Z, jobs1/testthreads1/private
TMP/unique copied target, workspace packages rebuilt and original cache intact.
Archive `415fb9d5...`, raw `ae8f84e5...` and post-run code hashes match. Strict
Store/server all-target Clippy and formatting pass. The real Store journey
pauses a delegated actual paired policy and observes Busy, then commit/Applied/
old-proof denial; current rotated replay retains original receipts. Other
journeys prove weaker-path denial, late CAS rollback and incident-only commit
with no admission effects and released lease. No provider is involved.

The source worker's four primitive cases have raw logs; the test worker did not
save its raw command/failure output. Its draft failures and focused success are
explicitly author-reported only. They are not called archived raw evidence;
canonical qualification is Root's and Linux's independently captured full suites.
No logs/digests are invented or old failures rerun for reconstruction. Separate
source/evidence reviews are transparently recorded, with no bot test-execution claim.

Root's three-pin operational CAS at 10:55Z/readback produces `64b0ceb2...`,
updating independently observed production to e71e at its original 10:49 time
while preserving all parents and producer source/history. Exact e71e browser
workflow39238035283 passed. Rust39238035073 was cancelled; annotation118011792853
explicitly reports the maximum execution time of 15m0s. Contracts/dashboard
tests/build and fmt/strict workspace Clippy passed, test compilation completed
and CLI unit execution began, but no Rust suite completed in the retained raw
output; later legacy/package/smoke steps were skipped. Raw SHA `8820a51d...`
and annotations are archived. Root owns the single later CI cleanup list; no
resource/actor/assertion cause is inferred and no CI rerun/cancel/runner control.

This is a process-local participating-cohort component. Its opaque catalog has
no public request/config constructor or authenticated loader/issuer; the default
remains None. There is no physical allotment/credential-expiry or later dispatch/
start fence, mutation/settlement route, provider completion or restart authority
recovery. SC155 remains consumed and unknown, never reused. P00/P01 alone remain
accepted; 30 parents stay open. Next ready work remains authentic installation
and external resource/custody composition, source-pinned provider diagnostics
with a separately fresh exact route authorization, context/closure/recovery and
the remaining WO6/onboarding/release criteria.

### 2026-10-19 11:20Z — PR123 merged, exact operational reconciliation

PR123 exact `5e480630...` received actual neverhumanbot approval at 11:10:15Z
and matched-head rebase merged11:10:35Z to `bcf5cce911e538de7c3f6d0b2e8e59f3185ebdd2`.
Root read zero open PRs at 11:12:06Z and all three qualified code/test hashes
match receiving main. [Merge evidence](../../../receipts/integration/native-pr123-merge-20261019T1112Z.json)
records local-CI-only eligibility and no bot test-execution claim. The sole
SC156 deployer actually read the file-pointer handoff and began the bcf5 build;
Root performs no service control and does not call merged source deployed.

[Eight-pin rollout CAS](../../../receipts/integration/rollout-admission-fence-pr123-20261019T1120Z.json)
at 11:19:00.944Z/readback11:22:01.677Z records sourcebcf5 versus independently
observed productione71e at its original10:49:10.466Z clock. New hash `18cf1203...`;
fsynced preimage/temp/dirs and final expected-hash recheck preserve all 32 packet
objects, source and source-history. P00/P01 alone accepted,30 parents open.
[Separate static CAS review](../../../receipts/integration/rollout-admission-fence-review-20261019T1120Z.json)
is clear after correcting the pinned CI observation clock.

[Exact PR123 browser evidence](../../../receipts/integration/ci-pr123-browser-20261019T1118Z.json)
records19pass1fail: Atlas dark pinnedaxe4.14.0 reports visible/accessibility-name
mismatch on13 hierarchy cards. Raw `20aaf626...` is hash-verified in the owned
receipt archive. Receivingbcf browser/checks were still in progress at the
pinned11:17:07.130Z snapshot. Root retains the single later CI cleanup backlog;
no new cleanup lane, assertion weakening, rerun/cancellation or runner control.

A fresh claimed read-capability repair follows the actual common-fence review:
NativeTaskLoop reused write proofs for ledger reads, which the paired cohort
correctly denies. Factory source, loop integration and role-strict regression
tests have disjoint owners. Explicit read capabilities must stay with original
preparation across every rejection/retry/unknown effect, with no write-token
fallback. This is not an authenticated production installer or provider grant.
The sole deployer may serially BUILD a source-stamped diagnostic example only
after primary deployment; no fixture/metadata/auth/grant/provider run is granted.
SC155 remains consumed and unknown, and no parent acceptance is promoted.

### 2026-10-19 11:35Z — explicit read-capability composition qualified

Source `9405ea948e48de4c01db1dc08b42872a9019a6d0` fixes the actual
factory/loop mismatch: paired Store write proofs intentionally cannot read
ledgers. A required custody-only read bundle and NativeBudgetAdmission read
proof remain distinct from unchanged write admission proofs. Observational
budget/slot read preflight precedes registration, without a global-ledger CAS
comparison or retained read lease. Both retained currentness checks and the
post-final-clock runtime wrapper use only those read roles. All factory
rejections return the original read bundle; all budget preflight/registration
rejections now return the original boxed command/write/read custody too.
Unknown effects retain plan, holds and liabilities with no refund/requeue.

[The component receipt](../../../receipts/integration/native-admission-read-proofs-20261019T1135Z.json)
pins actual Mac and actual capped Linux SAME32 cases:15 integration plus17
loop cases,82 unrelated lib cases filtered. Three new cases are a subset; the
38 factory-rejection variants remain one case. Strict server all-target Clippy
and workspace formatting pass. Source archive `6bb1d22d...`, Linux raw
`e6c1ccf4...`, all four post-source hashes match. One Linux invocation at
11:31:14–11:33:07Z follows completed deployment/diagnostic BUILD-only work,
fresh outside/inside/after gates, jobs1/testthreads1, privateTMP/unique copied
target, rebuilt workspace packages and original cache unchanged. Fixtures
independently preserve prior write revocation, test read-only revocation with
no executor attempt/retained hold, and wrong read rejection with original
plan/proof custody and no effects. They are explicit local role-strict policies,
not actual paired-cohort startup issuer qualification; ResourceGate remains
unconfigured in these loop journeys. Separate static source review is clear.

Independent primary proof at 11:26:51.124Z now verifies actualbcf5:all 14
manifest/files plus daemon3144998/start59452855 and observer3144488/start59452829
executable hashes, health/UI/feed200/18891redirect307, Rust0modelturns,
history256displayed581retained/no retired owned Python/prior e71PIDs.
[Proof](../../../receipts/integration/native-bcf5cce9-independent-20261019T1130Z.json)
does not remeasure original first-history or browser controls; execution stays
unqualified and Root controls no service. The sole owner separately built a
source-stamped diagnostic example, binary `15e885c9...`; Root read-only hashing
confirms the archived artifact. The example was NOT run and no metadata, fixture,
credentials or grant prepared. A fresh exact one-run route request is now pending;
SC155 stays consumed, never reused. Implementation proceeds independently.

Exact PR123 Rust39242527299 reports95serverpass/1controlled-capture-deadline
fail/2ignored. Mainbcf browser39242735139 later failed; API lists no failed
step and failed-log fetch returned log-not-found118027201976. Its exact cause
is unknown, separate from PR123 axe findings. Raw/API evidence is retained on
the single Root-owned later CI backlog. No CI rerun/cancel/runner control or
weakened guard follows. Production policy/catalog/physical-resource authority,
provider completion, settlement/recovery/WO6/release and30 parent acceptances
remain open. No source component activates them by itself.

The evidence audit verifies 109 referenced canonical/worker artifacts and
identical 32 case names; the owned remote archive verifies 69 unique payloads
under SHA `e58e7321...`. The archive verifier first failed to parse a generated
newline literal. The invalid verifier is preserved beside its corrected version
in the owned receipt directory; the repaired verifier checks the same unchanged
archive and every payload hash. No source test was rerun or guard weakened.


### 12:09Z — Native related-work lookup composed; 65c1 primary independently verified

Source013cd33680ce45c1fcaf6e7a4c80ab7cde0edceb adds the existing Rust
duplicate-query API to Intake through two disjoint client/UI workers and Root
integration. Lookup is explicit/read-only, independently scoped to the current
preview/root/credentials, with typed Rust query identity, bounded escaped
candidates and honest complete/partial/incomplete coverage. Pagination is
manual, stops repeated cursors and caps five pages/100rows. No source/command
identity is invented from the raw-capture form; no record/merge/dispatch action.

Actual Root41scoped dashboard cases (28client+4UI+9existing intake), dashboard
and E2E typechecks, actual Rust1golden integration case/twovectors, scoped strict
core-test Clippy and fmt pass. Worker copies of the same cases are nonadditive.
Actual final native browser1case passes in2.74s with immutable binary SHA
3dfcd4fb393b092ea5d47f61385506bfb06692bf4f9b0c05785b3f75d0052a81,
production source3f42f940/bundle1fd198d7, two empty/incomplete scoped queries,
identical committed-reply-loss capture replay/readback, busy-state assertion and
late ignored-abort reply discarded after credentials change. Tasks/messages stay
zero; pinned axe4.14 reports0violations/0incomplete/29passes. Direct child78800
SIGTERM/exit confirmed. Positive candidate/pagination views are unit coverage,
not native browser qualification. All159payloads independently rehashed in the
owned remote archive with fresh outside/inside fleetcap admission; no Linux
build/test/provider job. Draft failures and the prior browser variant retained.

Canonical receipt native-intake-related-work-20261019T1208Z.json and separate
source review/archive receipts bind exact source/raw proofs. This bounded WO6
journey does not complete WO6/P11/P13/P29/P30 or install live intake credentials,
production provider admission, physical-pool/start fence, context/closure, budget
settlement or restart recovery. P00/P01 alone accepted;30parents remain open.

SC156sole backup885617 deployed65c1 at11:54:03Z. Root independently verified
11:59:45.876Z all14manifest/file hashes and daemon1486257/start59629760 plus
observer1486006/start59629754 executable hashes; health/UI/feed200/18891
redirect307, Rust0turns/history256displayed598retained, oldbcf5PIDs absent and
no retired owned Python processes. Exact independent receipt
native-65c1bb11-independent-20261019T1202Z.json SHA
bf12452e80bb475650a4b6037d5800b765c03ecfded80eaa8bda57a0a7c8a6ac.
Original first-history/browser controls were not remeasured. Root no service
control. The newer related-work source is not yet deployed.

PR124exact992Rust39246319430 actually failed96serverpass/1controlled-capture
deadline/2ignored; main65c1 Rust39246533041 was cancelled with actual15minute
annotation118039708355. Browser runs for both passed. Red evidence remains in
the one Root CI backlog; no cause inference, rerun, cancellation or runner control.
Fresh exact bcf5 diagnostic question/request remains pending: no new provider
fixture, CLI metadata, authorization/grant or provider turn. Never reuse SC155.


### 12:54Z — Recovery diagnostic support, consumed coding probe and release gates

Source a6df0fb38314232a67290ec5bba86733d00dc327 adds bounded existing-run
journal metadata inspection and an eight-entry history of readable resource
samples. Descriptor/entry checks reject unsafe journal files; history keeps
individual samples/unknowns and preserves frozen closure behavior. Neither API
parses a journal into authority, retries an effect, refunds liability or supplies
a recovery controller. Read docs/architecture/native-recovery-diagnostics.md.

Actual Root8server journal +14runtime diagnostic cases pass; cappedLinux runs
the SAME22 names, with6new definitions a subset, in75.378s. Unique copiedtarget,
privateTMP, jobs1/testthreads1 and fresh outside/inside fleetcap gates recorded;
all3post-source hashes match. Strict Root runtime/server all-targetClippy passes.
Rootfmt was reported0 with empty log but its async exit metadata was lost;
worker fmt exits/hashes are retained, not invented. Source review is clear with
disclosed sampled-metadata/sameUID/advisory-history limitations. All9nonzero
worker draft runs remain in the evidence set; no test/guard weakened. See
native-recovery-diagnostics-20261019T1252Z.json and hash-bound source review.

The human's fresh ONE bcf5 diagnostic grant was consumed once12:31Z. Actual CLI
turn completed0/capture5events, but code-mode helper handshake/spawn EAGAIN
prevented editing/verifier; probe17 SourcePreflight rejects unchanged source.
Cause/model/effort/final accounting remain unknown. Provider-reported usage is
not billing. Initial counters readable, final counters missing; new history
cannot retroactively repair this older binary. Recorded systemd max380.317s
was permit remainder versus software executor120s, actualrun17.779s: no timeout
enforcement proof. A new source repair must bound launch/backend max before
any separately authorized provider diagnostic. No grant/fixture/nonce reuse.
Read native-human-bcf5-diagnostic-20261019T1245Z.json and independent review.
Read-only12:46unitstate is inactive/dead/MainPID0, emptyControlGroup/resultsuccess
and both recordedPIDs absent; no service control or unknown-effect refund.

SC156sole backup deployed e6e; Root independently verified12:23:52.714Z all14
manifest/file/process hashes, health/UI/feed200,18891redirect307,Rust0turns,
history256displayed610retained and no retiredownedPython/prior65PIDs.
Original first-history/browser controls not remeasured. Nativeexecutionfalse.
Main e6e browser39250210328 passed; checks39250210472 failed96serverpass/
1controlledcapturedeadline/2ignored. Exact rawSHA4888d93249a19aca019fdfa6606783012f64e444f1a1af07d4ea09476ef14b6a
retained/archived; no cause inference/rerun/cancel/runnercontrol.

P00/P01 alone accepted;2/32 is not simply absent bookkeeping during Geminiquota.
Fresh P02 static audit (bf8e233e694a6dffb41d603970edb4031c3e0442a9920402295c845b3f2c3cc3)
credits full coordination/scheduling/delivery contracts and identifies actual
independent outcome/taskrevisioncompat/executionrequirement/export gaps.
Provideractivation is not aP02criterion. Remaining groups are listed at top
of REMAINING_20261019.md; other parents are not accepted by this component.

The user explicitly selected MIT. RootLICENSE and all9Rust/3JavaScript package
declarations now agree in actual lockedoffline metadata. Contributor/security
guides added; Python is needed only for retained compatibility checks, not
native runtime/ingestion. Dependency/asset/support/install/release qualification
remains separate; no published GitHub release at the recorded API snapshot.
Fresh screen reads between actual12:45:23Z and12:51:13Z show Gemini4Argon Admiral
and Commodore resuming; nominal1253/1255 filenames are not observed timestamps.
Root notified both of the consumed grant and acceptance/soleCAS boundary via
file pointer. A resumption is not an inferred deploy-authority handback.

Remote evidence archive d9232d836c78933395f709000ebc8fb6a4ff81e04b50dfb32e376693464096af
was independently rehashed12:53:51Z underfresh cappedgates:126unique payloads,
manifest92941f7b8ab05fde2faa19f3b1185d108abc20ee09e38296eeaea396517d735b.
This is source/failure evidence, no additional test/provider run.

### 12:57Z — Kernel launch timer bound composed before opening the release PR

Worker3f19a292839006c065c7a72c8f5af1a1e4cd960c, Root0ab8214, fixes the
observed timer mismatch by requiring the existing trusted executor Duration at
AdmittedLaunch::launch. Both actual callers supply their original limit; backend
RuntimeMaxSec floors the positive minimum of that duration and original permit/
resource lease remainder. Zero/submillisecond/expired bounds fail before launch.
Claims/caps/digests/unknown-effect custody and software timer are unchanged.
Actual Root4wrapper cases pass (3newsubset), strict Mac runtime/server all-target
Clippy/fmt0 recorded; separate worker same cases nonadditive. Static review
9a23db98aa90a17c179fb982acd970b0542cfe1754e5682fe6f6bfb85381dc3c CLEAR.
Current Linux cfg-only caller compile and combined26cases follow below; no
provider, actual timeout or stop-grace qualification is claimed. The two earlier
draft compile/read-path failures remain preserved in the worker receipt.

### 13:04Z — Final combined source qualification and MIT metadata

Exact3a2135e384d36cc82e686211e5ebb0e38ff0fa5f actual cappedLinux142.426s
passes26distinctcases (8journal+14history+4wrapper,9newsubset) and strict
runtime/server all-targetClippy, including compilation of Linux-only lifecycle
example caller. Actual Mac names match26 exactly; wrapper code/metadata reuse
before3a docs commit is explicit, final journal/history run after3a. All4post
source hashes match. ActualMacstrictClippy/fmt0 exit files now preserved, not
the earlier lost metadata. No provider or timeout/stop-grace experiment.
Canonical native-kernel-deadline-final-20261019T1302Z.json SHA
6d6748c800287d4cbbcec7f56396e6c57f0717b1b812bb429f290bcec6a8a2f0.
Remote archive b0ca4b9fc97b2ed840b7c50fbc5d971542d6de55974f24426cdcde84fe47b0d0
independently rehashes78payloads13:04:09Z under fresh cappedgates. Initial
recovery22 and worker copies are nonadditive. Source/static/docs reviews retain
their exact separate scopes; all failed drafts preserved. OwnerMIT choice,
LICENSE, all9Rust/3JavaScript declarations and contributor/security guides
are ready. Public distribution/dependency/asset/support/installation/final
qualification and remaining production/product criteria are not accepted.

Independent final evidence review62235e1710d8880c61352f0a8b74a4219dc51e338c27aea2db18f6c08f8b5fa2
CLEAR:13direct+56nestedworker hashes,26namedcases,4source hashes and cb2d archive.
Source review remains9a23; docs/license review1816. No bot test-execution claim.

### 13:48Z — Observed model and visual access regressions repaired

Source19baae219c1780c43c22c3ceb13a813adb89fcae integrates observerd5834d8,
visualad4f9c7/cb6072e and Root browser/provenance repairs on main68bae1e.
Actual uppercase Codex/replacement-separator footer reproduction failed before
the parser fix;15observer cases (4newsubset) and strict all-target Clippy pass.
Actual process family/PID/start/TTL remain binding; requested models and Screen
names never establish model identity. Claude short labels and Grok retain bright
badges; genuinely unavailable identity remains unknown.

Private native view endpoints remain authenticated. On401/403, the client may
project only an explicitly public legacy producer endpoint, with original source
times, exact configured roles and explicit unavailable private assignments.
Access epochs discard old private/paused cuts and late replies. Actual combined
31dashboard cases, typechecks/build and2real isolated-native browser cases pass;
the latter use synthetic producer observations with a hash-pinned Rust binary.
One new public-to-owner-access journey and the existing9surface dark/light/mobile
axe case pass. These are2cases, not11tests or an additive platform/worker total.
Earlier accessible-name, fake-owner overlap, decorative paint and light contrast
failures are preserved; no axe assertions or authorization guards were weakened.
See `receipts/integration/native-model-public-views-20261019T1348Z.json`.

The Admiral actually acknowledged the owner's requests in SC-DEC-167, retaining
disjoint source scopes, Root sole rollout CAS and885617 sole deployment ownership.
Read-only audit maps15Screens/60worktrees/148branch refs;14older lanes require
owner release/recoverability/equivalence proof. Two dirty worktrees and shared
general runners remain protected. No retirement was performed.

Primary68bae1e was independently verified at13:42:48.155Z:14manifest/file hashes,
both PID/start/executable hashes, health/UI/feed and18891redirect, Rust0modelturns,
prior e6PIDs absent/no retired ownedPython. The repaired probe is BUILD-ONLY,
not a new authorization. These new model/UI fixes are separately packaged source,
not yet a live deployment in this receipt. MIT is merged; P00/P01 alone accepted.
Current68bae browser CI failed19pass1Atlasname mismatch; Rust hit its explicit
15minute limit with missing job logs. No full-green/resource-cause claim.

### 14:06Z — Full browser sequence repaired after exact Linux failures

PR127 merged13:50:07Z into6709bf51a39ab51938852da4719a5a9061a3329f after
exact9d5d189 neverhumanbot approval13:49:44Z;0openPRs observed13:50:34.677Z,
all14qualified hashes match. Remote custody independently verifies271payloads
13:53:01.434Z (8d4a5662), preserving the first rejected AppleDouble/PAX archive.
Sole deployer885617 ACKed exact6709 at13:54:16Z; merged is not deployed.

Actual6709 Linux browser39263549713 reports16pass/5fail: retained-access helpers,
old real-Git access label, owner contrast incomplete/partial overlap and new
fixture128MiB binary bound. These are separately retained, not the old68bae name
failure. Source22cb3ca336135841ca8ecda203730f27aed7d5ab repairs actual UI access
reuse, visible identity selectors, owned-card avatar layout and bounded streaming
fixture fingerprint; step-local dev debug metadata is disabled. Production
ArtifactStore/runtime128MiB guards, assertions, overflow and deadlines unchanged.

Actual full Mac browser suite now21pass/0fail/0skip across4files in29.70s,
including native temporary-Git, private/public access, nine dark/light/mobile
axe surfaces and mock command fixtures as declared. Earlier20pass1obsolete-selector
failure is retained. All11audited surfaces0violations/0incomplete are not11tests.
Typechecks/build/actionlint pass; exact Linux binary size/result and actual CI
owner occluder remain unestablished. See native-browser-session-20261019T1406Z.json.
No new case definitions, live-new-source proof, provider authority, settlement,
recovery or whole-parent acceptance follow. Prior reviewe110d8f6 called20c09...
a production-bundle hash; it is the pinned axe script hash. Actual UI provenance
uses source/dist/deployment hashes. Immutable prior review remains preserved.


### Native role communication and complete local qualification — 19 October 2026

Source `7bf86c4db676a4f35baa4baf85e0c3f1f6754157` adds explicit native
captain–admiral message routes and separately granted own-inbox application ACKs.
Protected startup installs the same credential registry and finite route policy
before Store open. The writer rechecks current contract membership; source and
message identities bind the authenticated principal and retained request UUID.
Credential read guards span the Store operation, current expiry is checked at the
writer, and changed-payload identities retain collision evidence. CLI/MCP expose
the typed commands and protected credential-file input. Messages do not claim,
execute, verify, accept or merge tasks.

Actual synthetic HTTP/Store qualification includes six scoped role-route cases,
both communication directions, context reads, lost replies and two complete
reopens. Complete serialized Mac workspace validation passes 1,702 test executions,
zero failures, five existing ignored helpers across 164 reported targets including
empty/doc targets. These are execution counts, not distinct acceptance criteria.
The pinned real-browser suite passes all 23 cases; dashboard units 183, client
tools five and actual isolated-binary smoke fifteen also pass. Full all-target
workspace Clippy, formatting, contracts, frontend checks, retained fixtures and
configured-label actionlint pass. Browser/build source `353fa969` differs only in
an integration fixture from the full workspace source; later source `4bd0a70b`
changes only the CI job budget. Exact commands, hashes, reuse scopes, raw logs and
all failed drafts are in [the receipt](../../../receipts/integration/native-coordination-20261019.json).

Qualification exposed and repaired real gaps: control-plane main runtimes now
use two async workers independently of CPU count (the existing primary daemon
was observed with 106 threads); browser retries/cleanup require confirmed owned
PID exit; known-revoked evidence is denied before probing its removed checkout,
with the final locked race check retained; the audit fixture now uses the current
policy version and explicitly rejects its old v2 control. The real 8 MiB capture
failure test has an explicit thirty-second storage-work budget for more than
1,000 durability flushes; production limits and failure assertions are unchanged.
The complete Rust CI job now allows thirty bounded minutes, retaining serialized
tests, compiler jobs2, assertions, overflow checks, toolchain and runner selectors.

Hosted PR155 browser failure (9 failed/14 passed) and receiving-main Rust capture
failure (101 server passed/1 failed/2 ignored) remain source-attributed failures.
New-source hosted qualification is pending at this receipt; no green result is
inferred from the local passes. The added Linux-only actual-binary thread bound
case has not run on this Mac. A read-only primary observation found source-path
stamp `ddcf09cf`, one native agent/task, zero configured routes/messages/ACKs and
`native_execution_qualified=false`; it was not a full binary/hash requalification.
No primary service control or provider invocation occurred. Actual supervisor
adoption, authenticated execution/pool policy, completion/settlement/recovery,
distribution and full P02–P31 criteria remain open. P00/P01 alone stay accepted.

PR156 first-head Linux qualification at `1760a782` passed contracts, all108server
cases (two existing ignored helpers), and the new actual-binary runtime-thread
bound case. Browser qualification passed23cases on retry; its first Chrome
version-probe timeout remains a separate failure with unknown cause. Rust then
exposed four admission-fence fixtures whose default temp directories were
group-writable under the runner's umask0002. An exact local umask0002 reproduction
failed the same four guards. Atomic0700 fixture creation repairs that setup.

The full local umask0002 sweep at `8c5d94f0` then passed1689executions and failed13
additional fixtures in three targets, with five existing helpers ignored. Source
`36de63448c45cc4de698958c37a935634e69b0eb` applies the same private-at-creation
rule to the reservation/atomic-claim and peer-inbox fixtures. All18affected target
cases pass under umask0002; strict full-workspace Clippy passes. Counts across
these runs are nonadditive. Production custody guards, authority/SQL/replay tests,
byte limits and assertions remain unchanged. The [follow-up receipt](../../../receipts/integration/native-coordination-umask-20261019.json)
preserves every failure and the exact profile/source. PR156 is held for fresh
exact-head Linux checks and renewed review; no second repair PR is opened.

### PR157 local CI: complete HTTP responses before TCP reset

The human requested full local CI rather than waiting on hosted runners. On
`3a8de44cb9233691e29ba7025c5be741f8d4ba46`, fmt and strict workspace Clippy
passed. The full Rust run then exposed one intermittent authorization fixture
failure: `read_to_string` waited for EOF and received `ConnectionReset`.
The remaining server result was107passed/two ignored. Twenty standalone
diagnostic repeats passed; the original failed response bytes were not captured.
An earlier attempt used an overly long Mac scratch path and failed a Unix-socket
fixture; Root corrected that invocation to short private scratch.

The test-only coordination HTTP reader now stops after one bounded declared
Content-Length body, preserving status checks and rejection of incomplete or
ambiguous frames. A deterministic complete-denial-plus-reset case proves the
old EOF read fails and the framed read succeeds. All seven coordination cases
pass, including truncated header/body, conflicting framing and length bounds.
Production code, authentication, limits and deadlines are unchanged. The
[receipt and original logs](../../../receipts/integration/native-test-http-framing-20261019.json)
preserve both failures and the scoped source qualification. Full local matrix
and renewed exact-head review follow this source commit; this entry does not
claim hosted CI, full native execution or release acceptance.

The complete19-stage macOS matrix at repaired source
`6d539218194282c6708cfa58921f4576235568f1` now passes:1,703Rust executions,
five existing ignored helpers,24real browser cases, package/canary checks,
actual binaries, contracts, unit tests, typechecks and build. Rust took150seconds
with jobs2/testthreads1 and Node24.19.0. The [receipt](../../../receipts/integration/local-ci-20261019.json)
preserves every command, exit and hashed compressed log. `scripts/check/all.sh`
now includes the previously omitted audit/package/canary/native smoke/E2E gates,
short private scratch, pinned dependencies and browser-native hashes; its full
invocation follows this source commit. Linux-only cases and hosted checks remain
separate. One actual production role cycle is now demonstrated by the
[sanitized adoption receipt](../../../receipts/integration/native-actual-role-cycle-20261019.json),
without publishing raw messages or granting native execution. Legacy wake-up,
production admission/provider/closure/recovery/distribution and full parent
acceptance remain open; P00/P01 alone remain accepted.

## History export platform and lock failure, 20 October 2026 UTC

Full local CI at `6cc64b7` caught the Linux cron wrapper treating missing
`flock` on macOS as successful lock contention. The wrapper now refuses
unsupported hosts and missing required tools before effects, and reserves
exit75 for actual lock contention; other lock errors fail. The original Linux
publication/custody test remains platform-specific; macOS executes a real
no-effects refusal test. All six policy tests pass on macOS, the old wrapper
fails the new control, and exact repaired wrapper bytes pass eight isolated
Linux publication/preservation/lock cases. The [receipt](../../../receipts/integration/history-export-platform-20261020.json)
preserves scope, hashes and original failures. Final whole local CI and hosted
checks still follow; no parent or release acceptance changes.

## Community source candidate and Store startup repair, 20 October 2026 UTC

The single integration PR162 at `5145621` passed hosted contracts, browser,
security and advisory audit checks. Its Rust job118428083900 failed both demo
signal tests at the existing five-second listener-readiness bound, before their
shutdown assertions. Original diagnostics remain preserved; resource contention
is not established as the cause. The security lane now provisions its required
pinned tools on a fresh runner and scans the full validated event range, including
merge-parent diffs; its original missing-Cargo and zero-byte false-pass controls
remain in the [security receipt](../../../receipts/integration/security-fresh-runner-20261020.json).

Source `d1659f423257f6a63dbc79cf770f3cbe53e49dc7` groups foundation DDL and
the final four non-nesting ledger initializers into separate immediate
transactions. Other transaction owners, WAL/FULL/FK/version gates, replay and
authority remain unchanged. Both new partial-schema fault controls fail against
the old initializer and pass after repair. This is not whole-startup atomicity.
Signal-test diagnostics now retain bounded child output; no five-second budget,
open-SSE, child custody, shutdown or cleanup assertion was weakened.

Actual clean-source full local20 passes 1,729 Rust executions (five existing
ignored helpers), 187 dashboard units and 30 browser/harness tests. Separate
bounded Linux qualification passes 238 Store and two actual signal cases;
listeners became ready in 2.5604s and 2.3752s. Independent source review found
no new issue within its declared scope. Three release-profile binaries and fresh
production assets pass a real 73-file macOS private bundle round trip, repeat
archive equality, all hashes/modes, tamper/no-overwrite refusal, deterministic
replay and both open-SSE shutdown journeys. The earlier 74-file test-marker
probe remains history. See [the source-bound receipt](../../../receipts/integration/community-release-startup-20261020.json).
Final documentation-head CI, hosted repair checks and actual main integration
follow this observation; no release or parent acceptance is inferred.

The [independent P03 audit](../../../receipts/integration/p03-source-gap-audit-20261020.json)
records six implementation/qualification gaps rather than an unrelated provider
blocker: actual writer telemetry, WAL checkpoint semantics, indexed filtered
pages, canonical migration families, backup version binding and abrupt-crash
coverage. Root retains shared Store/migration/wiring ownership. P02 is source
complete and conditionally ready for exact final checks/main/criterion evidence;
only P00/P01 are currently accepted. Native coding and full-release flags stay
false. One integration PR, local CI before push, exact-head bot review and
ordinary protected rebase merge remain required. Foreign dirty, ignored and
custody-held work remains preserved.

## Live trends, measurement formatting and development routing, 20 October 2026 UTC

The user reproduced excessive decimals and four blank temporal metrics. Source
history discarded known partial token counts whenever complete enumeration was
unavailable; the current Admiral status also omitted measured hygiene reports.
The observer now preserves an independently priced observed-sample cost index
and timestamped observed token/yield series while leaving incomplete full
aggregates unknown. The dashboard labels these bases separately, preserves
gaps and audit clocks, and never restores an older full cost as current after
an explicit unknown. Current observed counts render before history sampling.
No missing hygiene score is synthesized. Numeric cards, chart labels, balances
and source inspectors display at most two fraction digits without changing
stored observations or contract digests.

Hosted PR162 attempt2 failed one live160ms missing-label probe after its entire
CDP budget expired. The helper's default DOM-negative reason was unobserved.
The repair preserves actual DOM and transport evidence separately, uses a
monotonic deadline and an injected clock for deterministic budget controls.
The original helper fails the new negative control; repaired targeted real
browser cases pass without extending deadlines or weakening process custody.
Mock data covers complete/partial/missing/zero values, mixed series, stale
hygiene clocks and the user's decimal examples. Scoped checks pass63 observer
tests,197 dashboard units, both new launcher tests and the partial-trend
browser case. Complete exact-source local CI and hosted checks follow these
source observations; they do not establish deployment.

The live routing audit confirmed14 inherited Sol xhigh Codex worker settings.
All14 were switched to Luna medium at idle task boundaries and logged; no busy
worker was killed. Two subsequent disjoint source packets ran on Luna medium.
The [fresh development launcher](../../../tools/codex-luna.sh) explicitly pins
Luna medium and rejects override arguments without changing shared config.
Its mock-provider tests check exact argv, hostile defaults, refusal before
invocation and exit propagation. Existing sessions require observed-footer
preflight and a session-only switch; escalation requires objective failure
evidence and a small handoff. These are development routing controls, not
qualified native execution. P00/P01 remain the only accepted parents.

Clean `ce5504ac077f1826bd4ec0d086d90f8d76c7eafd` passes the complete20-stage
local matrix:1,735 Rust executions, five existing ignored helpers,197 dashboard
unit and32 browser/harness tests. The [source-bound receipt](../../../receipts/integration/live-trends-display-20261020.json)
retains every compressed-log hash and the original CDP negative control. The
full23-commit security range also passes at887c5ac; the later delta only improves
fixture diagnostics/formatting. The original887 Clippy and5f9 formatting failures
are preserved. Two independent Luna source reviews cover declared nonauthor
UI and observer/harness scopes at887; neither ran tests or granted acceptance.
The conditional balance-range observation preserves the preexisting reader's
string bounds; the native quota producer already rejects negative/nonfinite
balances. A later generic observed-sample label avoids calling a fully covered
sample partial; final-head local and hosted checks follow that small UI/doc delta.

### Browser qualification runtime follow-up — October20

Hosted75c073d passes Rust, contracts, security and the advisory audit, but browser
qualification fails30/32: the forced startup-stall control retains a direct child
in custody without an observed exit, and Atlas navigation exceeds both existing
CDP deadlines. Unchanged75 passes all32 locally on Linux with Node24.22.0,
Chromium145 and the actual AppArmor chrome transition. The hosted root cause
remains unproven. An earlier local attempt failed because its temporary path
exceeded Linux's socket limit; that separate failure is preserved.

The candidate pins Node24.22.0 and a SHA256-verified Chrome for Testing155 archive
in private hosted scratch, records runtime/resource diagnostics and asserts the
actual CDP version. It removes the synchronous `/bin/true` sandbox probe while
other owned browsers are live; the actual browser handshake still qualifies the
transition. Full sandboxing, deadlines, retry limits and process/group custody
remain required. The candidate passes all32 Linux browser tests and the actual
provisioning step selects the pinned browser despite a preexisting145 selection.
The [file-bound receipt](../../../receipts/integration/browser-runtime-qualification-20261020.json)
records exact source/log/archive hashes and the local-versus-hosted sandbox
difference. Exact-head local CI, independent review and hosted qualification
follow; these observations do not establish production deployment or acceptance.

### Atomic startup and replay validation — October20

Hosted dd4cf1c passes Rust, contracts, security and the advisory audit but fails
two browser cases. One assertion used single-line matching across multiline
retry diagnostics despite an actual final unsafe-port denial; it now retains
both diagnostics and still requires the final observed denial. The native
intake fixture again exceeds its unchanged eight-second readiness deadline.

Store startup previously made eleven independent durable schema commits.
Root's stronger last-schema rollback test fails that source with earlier
metadata left installed. An existing Luna lane supplied seven caller-owned
initializer helpers; Root integrated them with one Immediate transaction.
An additional actual negative shows that committing before replay could retain
a recreated table after invalid history was refused. Replay, questions,
subscriptions, pending inbox construction and workplan validation now finish
before the same commit. Standalone public APIs retain their guards; no SQL,
durability, authority or migration-order relaxation is applied. The initial
replay-corruption fixture failed its own FK insertion and is explicitly excluded
from negative proof; corrected v2 demonstrates the old rollback defect.

Exact70fb5e6 passes239 Store tests, strict Store Clippy and all32 Linux browser
cases. Two controlled fresh-Store measurements fall from2.76–3.17s to about0.11s;
fsync/fdatasync calls including orderly drop fall from19 to9. The
[file-bound receipt](../../../receipts/integration/atomic-store-startup-20261020.json)
retains source/log hashes, both valid negatives, the excluded fixture failure,
dependency equality and measurement/platform limits. Independent review and the
final whole matrix/hosted gates remain separate. Production stays on the last
qualified main until ordinary merge and the sole controller's serial cutover;
P03 and full native release acceptance remain false.

### Partial hygiene observations — October20

PR162 merged normally at05:54:56Z after exact-head bot approval and all five
hosted checks passed. Actual main6e974790 has the same tree as qualified source
d279166; the sole adopted watcher owns production cutover. There are no other
open PRs at that observation.

The real05:55:53Z producer still reports null final hygiene scores, but supplies
per-project possible_score_range, known_component_subtotal and an04:22:27Z audit
clock. The UI discarded that partial observation and labeled it not reported.
Cards and hierarchy now show the reported range/subtotal as a partial audit.
History retains only allowlisted numeric bounds/subtotal and the original
per-project audit time; separate minimum/maximum series do not become a final
score. Missing/invalid/out-of-range intervals remain unknown. Original source
fails the new partial-audit mock; the restored candidate passes15 temporal
tests,65 observer tests and the dashboard typecheck. Whole local/hosted gates
and actual deployment remain separate from these scoped checks. Root requested
the existing Admiral complete missing audit measurements; no score, grade,
source freshness or native acceptance is inferred.

The existing Luna medium independent reviewer found two source edge cases:
an incomplete range could retain a final grade in the hierarchy, and generated
range series IDs could collide with a reported project ID. Root suppresses the
grade for partial audits and allocates IDs against the complete occupied set.
Both new mocks fail0939eb5. A third negative catches a real production
inconsistency: a0.01% yield in trends rounded to0% in the token summary. Both
views now use at most two decimal places. The first new percentage assertion
incorrectly matched raw HTML against adjacent visible words; it was corrected
to inspect rendered text, and is excluded from regression proof. All18 temporal
tests pass after restoration. The earlier0939eb5 full20 matrix passed before
these changes; the corrected exact head requires its own full qualification.

The corrected e6763bb passes the whole20-stage local matrix and its independent
Luna follow-up reports both findings resolved with no remaining source finding.
During live verification, a fresh five-project audit expanded the valid Admiral
status file to258124bytes, beyond the observer's unchanged128KiB source limit;
the actual06:08:21Z producer therefore contained status:null. Root notified the
sole Admiral publisher to keep the detailed evidence immutable and publish a
compact summary with receipt references. Source now reports bounded-read/parse
errors explicitly; trend cards distinguish unavailable status from an absent
report and preserve any historical audit's original clock. New mocked overflow,
malformed-source, compact-recovery and UI diagnostics cover this real failure.
Both new negative controls fail the original e6763bb. The limit is retained;
no stale value is relabeled current and Root performs no role/status/config
writes. Final exact-head qualification follows this source change.

The sole Admiral publisher compacted the status to25582bytes and retained the
raw audit receipts. The actual06:14:54Z producer again contains all five partial
ranges and original06:00–06:02 audit clocks. Root's first04faee0 whole-matrix run
stopped at strict Clippy because the new fleet fixture used11 unwrap calls;
these now use descriptive expect failures, retaining every assertion and lint
policy. Runtime source is unchanged by that correction. Publisher guidance
records the existing128KiB bound, byte validation and immutable detail references
to prevent repeat overflow. Final exact-head gates remain required.

### Native capture-overflow qualification repair — October20

PR163 source6c9bee9 passes the whole local20 matrix and hosted browser,
contracts, security and advisory checks. Hosted Rust job118486084876 fails the
existing actual-producer capture-failure test at its30-second deadline with
Busy. Unchanged source passes on Root's reserved Linux slice in12.54s, so that
is retained as a non-reproduction. Tracing shows1022 output-journal fdatasync
calls consuming16.215625s; exact hosted storage latency remains unproven.

An existing Luna medium worker supplied a test-only artifact using a private,
validated Linux tmpfs fixture for this overflow case. The actual9,000,000-byte
producer request,8MiB runtime cap,1022 durable calls and30-second deadline remain;
the test adds real child-exit, retained-prefix, absent-watermark and exact typed
rejection assertions. Production code and other disk-backed tests are unchanged.
The first artifact's shell prerequisite checks were replaced with existing
Rustix APIs. Root's first Linux runner had a template SyntaxError before tests;
that setup failure is preserved and excluded from code/model failure evidence.

The candidate passes Linux115 server library tests (two existing ignored), strict
all-target Server Clippy, the actual Runtime capture-bound test and Mac's scoped
capture case. Tracing still records1022 real output syncs, now0.008469s on tmpfs.
Independent artifact review finds no production blocker and records the possible
empty-directory retention on failed prerequisites. The
[source-bound receipt](../../../receipts/integration/capture-overflow-fixture-20261020.json)
preserves scope, hashes, original failure/non-reproduction and platform limits.
Root released CPUs112,113 back to project standby; no Linux qualification job
remains. Final local/hosted gates and actual deployment remain separate; no
native/provider/parent/full-release acceptance is inferred.

### DOPE audit, project Git Flow chat and Git protector — October20

Source `91eae7345bd1bf0554a073d763ee9fa37f164706` in the isolated Mac lane
`codex/git-protector-chat` implements project-scoped Captain/Admiral chat,
immutable durable inbox/reply CLI transport, and a Rust Git health evaluator over
the existing observation. The [worker-ready plan](GIT_PROTECTOR_CHAT_PLAN.md)
defines ownership, rollout, stop conditions and sequential integration with the
independent linear-collapse lane. No generated Views contracts were changed.

The [DOPE audit](DOPE_GIT_AUDIT_2026-10-20.md) covers eleven existing worktrees and
four missing registrations. Old workplan content is patch-equivalent, the
captain bridge has main's exact tree, and the real pending worker delta remains
in existing DOPE PR290. Canonical main's43 staged paths remove6659 lines;
historical author/intent remain unknown. The Admiral acknowledged the notice
and preservation/custody decision. Captain queue acceptance is recorded without
claiming consumption. No DOPE source reconciliation or cleanup occurred.

The final clean-source local `pnpm check` passes20/20 stages:1759 Rust tests
(6 ignored, including the explicit benchmark),208 dashboard tests and37 browser
tests; strict Clippy, contracts, native startup canary, typechecks and build pass.
Actual final binaries additionally pass29 authenticated startup/chat/CLI checks.
The final-source unoptimized benchmark averages56.087021ms across100 evaluations
of64 checkouts/512 commits/65 refs with zero Git subprocesses. It is not a release
or fleet latency guarantee. Strict axe gates remain unchanged and pass after
bounding chat overlay layers. Earlier lint, canary readiness, inventory, native
mode and accessibility failures remain retained, with their specific repairs.

The [source-bound receipt](../../../receipts/integration/git-protector-chat-20261020.json)
records exact commands, source/tree/file hashes, logs, binaries, screenshots and
limits. Its evidence commit follows the tested source. Existing PR163's owner is
the receiving integration route; no duplicate PR or production controller is
created. Exact-head review/hosted gates, sequential combination with collapse,
deployment, protected role enrollment and a real role round trip remain separate.

Publication follow-up: branch `codex/git-protector-chat` was pushed with source
and evidence tip `b47fc42`; a read-only remote-ref check confirmed it. Final
handoff `GPC-SOURCE-20261020-0731Z` was durably appended/read back at07:32:44UTC
in the Admiral inbox, DOPE captain COMMS and Admiralty captain/integration-owner
COMMS. The single bounded07:33:37 check found no matching final-source reply;
the earlier Admiral audit/custody acknowledgment remains distinct. The
[delivery receipt](../../../receipts/integration/git-protector-chat-role-delivery-20261020.json)
preserves the exact handoff and evidence. No duplicate pointer or process effect
accompanied it. DOPE PR290 independently advanced to `5701ba33`, one repair
commit after the audited worker; its CPU run was still in progress at07:33:30.
Static inspection of that delta does not inherit the earlier source's tests or
its saved CPU failure. Review/merge/deployment remain with the receiving owners.


## GFI — unified Git Flow integration, 20 October 2026

Host Mac. The ready protector/chat and linear-collapse lanes are combined on
`codex/gitflow-unified` above actual receiving main
`35b497838c7e78bd366f2a0fabb6a455692fd93e` after PR163 merged with green checks
and exact-head review. Original published lane refs and foreign worktrees stay
preserved. The detailed integration plan and branch reconciliation inventory
record all428 refs/85 worktrees,20 dirty custody checkouts, historical aliases,
optional old composition variants and the unaccepted unwired authority proposal.
The deeper audit recovered the missing execution observation safety behavior
while preserving current identity, admission and settlement APIs. The causal
clock production guard was already present; this integration restores a distinct
negative regression for it.

Frozen code `ceb074e741ee384f8f0dc90eda4f1dcf3d0a8f69`, tree
`2ffc8c27b7f1bc6044f613d743bd70b4d4835d23`, passes all20 local Node24/pnpm11.15.1
matrix stages:1768 Rust tests (6 existing ignores),231 dashboard and47 browser
tests, strict workspace Clippy, contracts, build and actual isolated binary
canary. Logs remain `/private/tmp/adm.i7n4rR/logs`. Fresh final binaries also pass
29 protected startup/chat/CLI checks; owned daemons stop and ports close.
The local workflow/secret/locked dependency check passes across the full
receiving-base range. Hosted Linux and pinned Jankurai remain separate gates.

Eight controlled pre-fix browser assertions demonstrate the repaired anchor,
manual collapse/live growth, shared-OID camera and filtered-project intent bugs.
The historical91e screenshot paths had been overwritten by later runs; their
original receipt hashes remain unchanged, with the retention limitation disclosed.
Final-source PNGs are frozen in Git under
`receipts/integration/artifacts/gitflow-unified-20261020/`.

The [combined source receipt](../../../receipts/integration/gitflow-unified-20261020.json)
records measured source/log/artifact bindings, prior failures and supplemental
non-author source/document reviews. Its evidence-only commit follows the tested
code. The Admiral reserved the next single integration slot and same independent
reviewer. Final exact-head official review, hosted CI, eligible neverhumanbot
approval, ordinary matched rebase merge and receiving-main verification remain
pending at this record's time; they must be observed before reporting completion.
No production deployment, enrollment, provider authority or DOPE mutation is
inferred from these checks. Execution deadlines bound intentional polling/live
waits; blocking kernel/filesystem/lock calls remain an explicit limitation.


### GFI05 — explicit input and isolated browser qualification follow-up

First candidate fc47 passes four hosted checks, including Linux Rust/legacy,
contracts, security and pinned advisory Jankurai, but browser run39394318694
reports45 passed/2 failed at the idle-opacity assertion and its dependent unopened textarea.
The original log and role HOLD remain retained. Native touch emulation reproduces
the original0.86/1 mismatch; the original hosted media state was not captured and
is not invented. The repair opts this suite into explicit fine/hover input,
uses actual CDP mouse movement and touch emulation, asserts real CSS capability
and hover/focus states, and rebuilds every fixture after about:blank. Production
CSS and application source remain unchanged. All prior route, retry, privacy,
isolation and accessibility assertions stay in place.

New frozen code `3171c5374b633a5ce011382b7c08b89e938d703a`, tree
`148280b3d26b33449d0047304d721e08306a22fb`, passes all20 local stages again:
1768 Rust tests (6 existing ignores),231 dashboard and47 browser tests, with
strict Clippy/contracts/native canary/typechecks/build. Logs remain
`/private/tmp/adm.u083dm/logs`. The focused5-test suite separately observes idle
mouse0.86, real hover1, keyboard focus without hover1 and touch/no-hover idle1.
A non-author coverage audit closes the missing source paths, including the
one-line core module registration and shared test harness launch/custody seams.

The evidence follow-up also corrects two stale Python inventory summary totals
from11351/2293 to11370/2312; all40 inventory rows and exceptions stay unchanged.
The six current Python policy tests pass after that correction. Exact daemon/CLI
bytes match the prior29-check protected-chat smoke; no29 new executions are
claimed. The current matrix separately runs its actual native binary canary.
The pinned first-head audit remains advisory56 with80 findings/53 hard: its two
new JSX select hits are source-reviewed SQL-detector false positives, while
existing audit debt remains open. No strict compliance or new-head CI is inferred.

The [follow-up receipt](../../../receipts/integration/gitflow-unified-input-20261020.json)
records original/intermediate failures, repair scope, final source/log/image
hashes, metadata correction and review boundaries. All original receipts/images
remain preserved. Replacement exact-head official review, hosted CI, eligible
approval and normal matched merge remain pending at this dated record. DOPE
PR290 independently merged08:18:40 with its exact reviewed5701 tree; its earlier
pending audit observations remain historical. No DOPE custody work was changed.

### GFI07 — README follow-up and final source qualification

The user's overnight owner request adds a current README and a 30-minute
continuation check. Frozen code `2bb7f83b60422bb4d73dadb7c3fd57923d83669c`, tree
`3e9bed5e82b0b50c2aad732e045137601f829129`, changes only README.md above71fb.
All53 prior integration blobs remain identical at that tested commit. The README
describes reversible Git compression, protected project chat and cached Rust Git
health, links both operator guides, and preserves the historical demo and release
limits. A non-author review finds no blocker and verifies68 relative-link
occurrences/47 unique targets and fragments.

All20 local stages pass again on clean2bb7f83:1768 Rust tests (6 existing ignores),
231 dashboard tests and47 browser tests, strict Clippy, contracts, typechecks,
build and a fresh actual native canary. Logs remain `/private/tmp/adm.my6lrN/logs`.
The full receiving-base security check passes. The
[README follow-up receipt](../../../receipts/integration/gitflow-unified-readme-20261020.json)
binds source, logs, independent documentation review and the unchanged53 blobs.
The prior71fb hosted head passed all five checks; it does not qualify this new
head. The same official reviewer continues the existing source review and must
review the README/evidence delta and approve the exact final head. Fresh hosted
CI, actual eligible approval, normal matched merge, receiving-main checks and
safe cleanup remain pending at this dated record. The owner continuation stays
active until those scoped tasks are verified complete.

### GFI08 — short linear sequences after the live UI report

The user's renewed20October report is verified in actual Chrome: production
still loads the pre-PR164 feature chunk. That deployment remains the sole Fleet
Admiral's work. A separate concrete source gap is reproduced by executing the
merged geometry on the21:28:06UTC public DOPE producer:8ranges/198members fold,
but two/three-interior stretches stay individual under the four-interior rule.
Several neighboring screenshot commits are actual forks and remain protected.

The [short-range plan](GITFLOW_SHORT_RANGES_2026-10-20.md) registers this chat's
sole managed source lane. Three pre-fix assertions fail on the actual short-run
behavior. The repair folds at least two eligible interiors, sizes controls to
their omitted-column budget, stacks endpoint hashes for short runs and retains
full accessible metadata at extreme zoom. All eligibility, topology anchors,
project scope, explicit focus and manual-collapse semantics remain unchanged.
On the captured400-commit producer the repaired algorithm finds33ranges with258
members and retains every loaded ref target and the six screenshot fork points.

Focused qualification passes18compression unit tests, frontend and browser
typechecks, production build and18real isolated server/browser tests, including
the new two/three-interior cases, keyboard recollapse, global controls, focus,
inventory, actual hit-testing and mobile axe. The first browser run passes17/18:
the second new fixture observed the prior producer before the server's next
source cut. Waiting for its exact new commit/range count fixes synchronization
without changing an assertion or product code. Both logs and frozen PNGs remain
under `/Users/bentaylor/.codex/artifacts/hygiene-redeploy-20261020T141911Z/live-collapse-followup-20261020T212618Z/`.

These are focused working-tree checks. Full clean-source local CI, exact-source
independent review, hosted checks, eligible approval, ordinary matched merge,
resulting-main checks and actual rollout/Chrome verification remain pending.
README/operator instructions describe the new source behavior while retaining
the explicit production-preview limit. No private gate, pin, service, custody or
foreign source was changed. The controller was notified of this disjoint source
scope through both locked logs; no conflicting controller or PR was created.

The first clean source `fd740ed40d0897878f028884cb056b1881423610`, tree
`0d2451b47dc63f493b218ab1e469c439ed1298c3`, passes all20 local stages:
1768Rust tests/6existing ignores,232dashboard tests and49browser tests. Logs are
`/private/tmp/adm.wvKunK/logs`. Actual Chrome additionally proves pointer expansion
of the screenshot's40441b7→32cc61a pair, Enter recollapse, global33→0→33 range
counts and retained192c46f inspector, on an isolated finite native preview.
That preview is stopped and its port closed; it is not the production service.

The same real-history check finds another concrete gap before publication:
192c46f sits on lane12, but the direct camera scrolls only to the project's band
top, leaving that lane below the viewport. A20-branch browser regression actually
fails waiting for its adjacent short range. The existing focus effect now centers
the mark vertically, clamped at the project band top, retaining project-scoped
identity and first-lane behavior. All19focused browser tests pass after this
repair. The fd740 matrix does not qualify this successor; freeze the new source
and repeat the complete matrix before publication/review/merge. Both original
negatives, all historical passes and actual Chrome limits remain retained.

Clean successor `e92f81ed2994db0ab6beee0492405bdc6b702a16`, tree
`5be6994f845d6a4bd7bc887ddca3cd1855d455d2`, passes all20 local stages again:
1768Rust tests/6existing ignores,232dashboard and50browser tests, with strict
Clippy, contracts, typechecks, production build and actual native canary. Logs
are `/private/tmp/adm.BNeeTq/logs`. The complete b3→e92 local security range passes.
Actual Chrome on the captured DOPE producer now opens192c46f and its nearby
short controls on first load without manual scrolling; real pointer33→32 and
Enter32→33 pass with an unobstructed target and preserved inspector. The finite
preview stops, its port closes, and its private runtime is preserved outside
temporary storage with exact inode identity. This is source qualification, not
the still-pending production rollout.

The [source-bound short-range receipt](../../../receipts/integration/gitflow-short-ranges-20261020.json)
records all source/log/artifact hashes, original failures, measured counts and
limits. The following metadata-only commit adds this receipt and execution
record. Exact-source independent review, hosted checks, actual eligible approval,
ordinary matched merge and resulting-main/deployment verification remain pending.
The sole controller has the original source-qualified deployment handback and
new human escalation; no controller adoption or serialized handback is inferred.


## 2026-10-21 Git Flow v2 scope adoption (source in progress)

Adopted [the reconciled execution plan](GITFLOW_V2_2026-10-21.md), source SHA256 `52f8a4020a8e639d396ebf1015295566a9c990ac1b4cc6ac72a99a1ebc5ecd85`, after the actual user supplied all three engineering documents. Existing PR166 and `codex/git-flow-viewport` remain the one UI integration train. Current main `5d4e656bbe7438a25728aefbac78b2576e1732ed` was fetched and the feature rebase was a no-op. Published `217fd80` is held after two hosted browser failures; local repair `dfd2eff8` has targeted negative/positive occupied-scrollport proof but no transferred full qualification.

Root owns shared UI/backend registration, source integration, the one CI observer and merge. Disjoint helpers own only new pure backbone/camera and layout files; a separate own-checkout core facts packet may author only new `crates/admiralty-core/src/git_graph/{mod,history,integration}.rs` pending root API adoption. No helper is an independent approver. Full history, evidence, contracts, cache-only authenticated APIs, worker/client and actual deployment acceptance remain required gates. Source starts do not imply completion.

The separate urgent three-path delivery-audit patch was consumed, exact-preimage checked, and held pending actual maintainer Python exception/inventory and serialization disposition. It is not yet part of this source or its tests. Actual same-Admiral deployment of the audit clone remains separate from web rollout. Historical failures and source packets are preserved under the private `git-flow-viewport-20261021` artifact directory.


### 2026-10-21 — exact PR166 82 candidate and separately preserved GitUXv2 work

- Published candidate: `82eaa4e40e562e3571ba53e45ab5cad0a02810d0`, tree `432fbf964aa06a98b10472ccce5daec76aafb3cb`, base `5d4e656bbe7438a25728aefbac78b2576e1732ed`. This includes the classic scrollport repair and the actual Admiral-authorized retained-audit/helper exception and registration, not the broader uncommitted GitUXv2 source below. Clean detached qualification checkout `/Users/bentaylor/.codex/worktrees/audit-pr166-qualification/admiralty` passed all 21 local stages (1768 Rust / 6 existing ignored, 88 legacy + 110 retained audit + 25 native queue, 255 UI unit + 54 browser). Exact source/tool/log metadata is in `/Users/bentaylor/.codex/artifacts/git-flow-viewport-20261021/audit-handoff-03paths-RECEIVED-NOT-APPLIED/FULL_CHECK_82eaa4e_01.json` (historical directory name; repair is applied and published).
- The single GitHub observer completed at 04:08:43 UTC with all six current checks actual SUCCESS. Browser attempt 1 failed before tests on the pinned Chrome archive download (curl28); one diagnosed targeted retry on the same head passed. Both original failure and retry receipts are retained. No CI policy, test, pin, runner or timeout was relaxed. Current GitHub predicate is REVIEW_REQUIRED, formal reviews empty, not merged; normal exact-head independent `neverhumanbot` approval remains required. Sole Admiral owns coupled protected audit-clone/helper deployment; source or web deployment alone is insufficient.
- Corrected immutable private offline media/review packet02 manifest SHA256 `0bf69bd4147350c5929dbe108614c2aa5c1a82e7fd9a3501cccffa486b5eca01`, archive `ed2d6d92af75d3798b137d8e6e7948b24cf0c08c2f56050b93c64991879290e2`. The original capture caption was outside browser fullscreen; its visible-caption claim is explicitly superseded. Private corrected capture-only sidecar/source provenance and the original failure remain preserved. Production/user Chrome was not replaced by this offline evidence.
- Separately preserved, UNCOMMITTED GitUXv2 development adds one canonical worker layout, explicit first-parent display backbone, per-instance/access camera memory, real wheel/drag input, branch-without-checkout inspection and screen-space grouped labels. Source is excluded from published 82. Development checks: 337 UI unit tests and app+worker typecheck passed; six new private headless investigation cases passed on development run04, including exact first-OID expansion anchor, Ctrl/Command pointer zoom, drag suppression, history/camera restoration, access reset and mobile dock/inspector hit targets. Initial failures and narrow-screen zero-height/chat-clipping diagnostics are retained, not discarded. Root is running the existing 23-case browser regression next; no final broad qualification or acceptance claimed.
- Development receipts/logs remain under `/Users/bentaylor/.codex/artifacts/git-flow-viewport-20261021`. No broad candidate push while current 82 review/merge is pending. Backend domain V2 is still unregistered in its disjoint three-file worker scope; its 44-test private source-equivalent harness is not registered application/API qualification or final API adoption.

### 2026-10-21 — PR166 merged; separate trunk-first source candidate

PR166's independent reviewer found `PR166-82-AUDIT-01`: a raw terminal-tail fallback could revive historical busy text after a newer completed footer. Successor `4dab2454c011a8aaa1c6eb45960a39b2e5b0554f`, tree `942c69223cee60df0bf056afd25d6023e39bab30`, routes both production callers through the same current-status classifier and adds meaningful caller-level negative and positive regressions. The four-path correction, original failures, 127-member review packet and clean all21 local qualification remain under the private artifact root. Actual local counts are1768 Rust passes/6 existing ignores,88 legacy methods/1 existing skip,117 retained-audit passes,25 native-queue passes,15 contract passes,255 UI unit and54 browser passes. All six exact-head hosted checks succeeded on their first current-head attempt.

The designated non-author completed the full24-path source review and closed that finding. Genuine `neverhumanbot` approval was submitted at05:02:30UTC, bound to exact4dab. A fresh approval/head/check read preceded ordinary `gh pr merge --rebase --match-head-commit`. PR166 merged at05:03:25UTC into main `5916726f8c085f2687ec756508914c5a713fa5bc`; its receiving tree is exactly the reviewed tree. `MAIN_5916726/MERGE_PROOF.json` records that readback. The sole receiving-main observer is separate; the ARM workflow is PR-only, leaving five main checks. The same Admiral owns production web rollout and the paired protected audit-clone/helper update. Merge alone does not establish either deployment or user Chrome acceptance.

The broader source is preserved on unpublished `codex/git-flow-trunk-first-main`, based on actual5916726 by a normal branch switch carrying disjoint local UI changes. A prior `--ff-only` attempt correctly refused the changed GitHub-rebased ancestry; no reset, stash, force push or published rewrite was used. This cut adds the canonical worker layout, explicit first-parent backbone, actual incoming merge returns and ordered-parent inspector, grouped screen labels, scoped cameras, pointer zoom/pan and branch-without-checkout isolation. Fit and actual size remain distinct; closed chat returns width, while open chat/inspector occupy measured docks. The scope is loaded history. Full-history indexing, integration-record overlays and measured complete-history performance remain unfinished packets.

Development source checks pass app/worker typechecking,352 UI unit tests and all62 browser tests. The final development browser log `v2-composed-ui07-browser.log` has SHA256 `93f4f2581fc42f6332a1435f176475ebd95f2b8822ac5c96c9e4c1752a27358e`; isolated fixture images are under `v2-composed-ui07-browser-evidence/`. The eight new investigation journeys cover native wheel/drag and form guards, anchor preservation, mobile panels, fullscreen refusal/inert restoration, per-instance/access memory, missing explicit refs and actual ordered merge parents. Initial failed selectors, negative layout/hit tests and artifact-scope correction remain retained. These checks precede the final candidate freeze; they do not qualify a commit, hosted CI, deployment or real user acceptance.

Backend V3 remains a separate frozen three-file input with64 private path-import tests. Source/API review is pending; no shared registration, index, server endpoint, schema publication or production use is inferred. Root retains sole source integration and the one CI observer. The reconciled plan remains active beyond the incremental UI release.

Receiving main5916726 completed all five configured main checks at05:13:33UTC. The sole observer completed and was reaped; immutable terminal metadata is `MAIN_5916726/MAIN_CI_OBSERVER.json`, SHA256 `40537f84ec6b85b6d36b376b201baefc3efc69e19f40d7ba512b4e26723ebb7e`. The actual sole owner received the rollout-ready main proof through the existing coordinator. Production deployment and real user Chrome verification still require actual owner/readback evidence.

Private broader commit `8357e67289a7ba404fe4f5a3c1886a9702d7cbed`, tree `637724811598e4e98b13de555e3cbfc874a140c6`, passed a clean all21 local matrix05:08:02–05:12:31UTC. Counts include352 UI unit and62 browser tests; raw logs are `/private/tmp/adm.lhOFD1/logs`, with exact receipt under `V2_8357e67_FULL01/`. This commit was held before publication after visual review identified ref labels physically covered by range buttons. A passing matrix did not close that concrete visual defect.

The following narrow source repair reserves the very same range-control rectangles used by JSX before placing canvas labels. Six new unit cases actually fail before the repair (10 existing pass), then all16 label tests pass. They cover alternative placement, complete obstruction/grouped hidden counts, clipping/invalid rectangles, deterministic ordering and every control height. All eight investigation browser tests pass with added comparisons between painted-label boxes and actual DOM button bounds at Fit,100%, native pan and inspector resize. The new private fixture image was visually inspected: the incoming topic and remote ref labels remain readable above the controls. Original before/after logs and screenshots remain retained. This focused source pass does not transfer the8357 full qualification to its successor; freeze and run the complete clean matrix again before any push.

Private successor `7f6d53cb4e5a3126af73c2e83cc7ff9b56e9abaf`, tree `8b626b8aeb18cf3d6212a7abf41c0cb47f3b26c4`, passed clean all21 again05:18:12–05:22:43UTC:358 UI unit and62 browser tests, with unchanged Rust/retained-audit coverage. Logs remain `/private/tmp/adm.i91I8B/logs`. The separate full-base security command then reported one `generic-api-key` match in ordinary plan prose at line151. A first noun change in private467bf1b did not fix it; the redacted match identified assignment-like punctuation before the observed/integrated time fields. The publication copy splits that sentence while preserving every evidence field and exclusion of private access data. The frozen original plan artifact and its52f8 input hash remain unchanged; the linked publication copy is therefore a documented prose derivative.

The private candidate refs and their exact source/failures remain intact. Their complete source tree was imported through retained binary patches into new `codex/git-flow-workspace` from5916726, changing only that plan sentence and this execution note. No published history or old ref was rewritten, and no scanner rule, allowlist or gate was changed. This new publication source requires its own clean full21 and full-base security result before push. Prior local passes remain historical and no independent approval is transferred.


### 2026-10-21 — PR167 independent pointer-transition correction

Published PR167 head `28d0b90deb19e7dbdbdb2895219ced0289969fbb`, tree `9b7e132468d13a83e9d773f13aea8b3842ec5430`, passed its own clean all21 local matrix (358 UI unit and62 browser tests) and all five applicable hosted checks. The actual independent non-author then found `PR167-28-INPUT-01`: lifting the second finger after a pinch left the first finger's pre-pinch drag camera active, so its next movement restored the old zoom. The source and all original qualification remain preserved; no approval was granted or transferred.

Four new in-browser pointer-sequence regressions actually fail against the unchanged28 viewport: first-finger continuation restores scale1 instead of1.6667, second-finger continuation does not pan, cancellation leaves a stale pointer, and lost capture restores the old camera. Real React handlers and rendered cameras execute in isolated headless Chrome; only capture custody is adapted because synthetic PointerEvent IDs have no native capture owner. These are not physical touchscreen acceptance. An earlier CDP-touch sequencing attempt failed inconsistently, its raw log remains retained, and the unused harness change was reverted.

The scoped repair rebases the remaining finger on its current point and the latest pinch camera, including a pending animation frame. Pinch captures both fingers. Cancellation or unexpected lost capture ends all pointer custody, and intentional release notifications are idempotent. Repository identity changes reset click suppression. Six focused browser cases pass after repair, covering either remaining finger, cancel/lost-capture, post-pinch click suppression followed by a deliberate tap, and repository replacement during an active pinch. Original logs are `P2_TOUCH_POINTER_RED.log`, `P2_TOUCH_POINTER_GREEN.log` and `P2_TOUCH_POINTER_GREEN_02.log` under the private artifact root. Freeze this successor and repeat all21 local stages, full-base security, fresh hosted checks and the same independent exact-head review before ordinary matched merge. Production rollout and actual Chrome acceptance remain separate gates owned by the existing Admiral.


### 2026-10-21 — independent range-control clearance correction

The same independent reviewer completed the original28 complete38-path review with changes required and a second finding, `PR167-28-RANGE-02`. At scale0.69, adjacent lane centers are37.26px apart while narrow controls are52px tall. New actual DOM intersection tests fail at0.65,0.69 and0.9 against unchanged `dcb185b` range presentation. The negative raw log and exact rectangle observations remain retained.

The shared canonical lane spacing now feeds control presentation. Detailed narrow and expanded controls retain their existing asymmetric reaches only when the scaled lane clearance can hold their combined64px reach plus4px gap. Otherwise controls show an exact-count compact pill centered on the same canonical anchor, with height14 or26 according to clearance. Full OIDs/counts and keyboard labels remain available. World geometry, first-parent backbone and camera coordinates are unchanged. Actual DOM pair-intersection and five-axis hit checks pass at all three scales with collapsed, expanded and panned ranges. The first positive driver incorrectly sampled rounded-pill corners outside the painted hit shape; that failure is retained, and the corrected driver checks the center and four axes while preserving the intersection assertions. All17 focused investigation browser cases pass; the69% synthetic fullscreen image was visually inspected and its adjacent controls are distinct.

The preserved clean `dcb185b` full21 attempt passed stages1–20 and67 of68 browser cases; a mobile native chat toggle readfalse instead oftrue. All six pointer regressions passed. Its exact source/logs remain under `V2_dcb185b_FULL04/`. No product cause or repair is inferred from later focused passes of that unchanged mobile assertion. Event tracing and a failure screenshot were added to expose any recurrence. This new source needs its own clean full21/security, current hosted checks and the same actual independent exact-head review; no earlier approval/check result is transferred. Production remains the sole Admiral's separate rollout gate.


### 2026-10-21 — edge-clipping and browser fixture admission follow-up

An additional read-only self-review by the earlier layout author found a concrete remaining edge case in private `1d01acfc10025bf04963d74fd6f644c82d7b4703`: horizontal pinning could translate a partly offscreen range control over a same-lane neighbor. This is source assistance, not independent approval. Two actual DOM negatives reproduce30.31px overlap at the left and right edges of a canonical15-commit chain split by a midpoint ref. The source-bound local21 was deliberately stopped before edits at06:30:42UTC using SIGTERM only on the verified owned group46088; its wrapper reaped exit−15 with clean source. `V2_1d01acf_FULL05/` retains the source, security pass, partial matrix and stop identity. No hosted job was canceled and no product-test failure or full pass is inferred.

Fixed project controls now retain their exact projected horizontal positions; viewport clipping replaces edge pinning. The same rectangles remain label obstacles. Actual pair and five-axis visible-hit assertions cover collapsed and mixed expanded ranges at both edges. Source math gives154.56px collapsed and49.68px mixed gaps at0.69. The compatibility-band branch is unchanged.

The first composed positive run also demonstrates a test-admission race: the asynchronously polling native producer could still expose the preceding15-commit fixture when the next92-commit journey started, including missing refs and wrong range counts. Tests now give each initial fixture a distinct project marker and repository identity and wait for that source before interaction. No assertion or threshold was removed. All19 focused investigation cases pass after both corrections; original negative and failed-positive logs remain retained. This does not conclusively attribute the earlier dcb mobile-toggle failure. Freeze and repeat the complete clean21/security on the new source before push and same-reviewer exact-head acceptance.


### 2026-10-21 — preserve below-five-percent manual zoom before rollout

Frozen `7e1c9a2bafa9ad1c42370fd35b6fe08cbbdd8409`, tree `0476d4661f183156daaf02f787a8ccc9a1c10466`, passed its own clean all21 and all five current GitHub checks. It remains preserved and unmerged/uninstalled. Before rollout, the source owner identified that its fit-scale/2 manual lower bound could stop at14.6% for the captured DOPE fit0.29255, contrary to the latest explicit human requirement for5% and below. The earlier derived-floor plan does not override that acceptance requirement. Current live UI591's separately verified manual3.8495% is not evidence for this newer source.

Three new unit cases actually fail against unchanged7e at fitted scales0.29255,0.69 and1, returning minima0.146275,0.345 and0.5. An isolated native-server/headless browser journey with32 real mouse clicks on Zoom out also fails below5% at scale0.4679666. The narrow correction caps the ordinary manual minimum at1%, retaining fit-scale/2 when an exceptionally large scene needs an even smaller bound. Fit has no floor, the maximum remains4, and pointer anchoring is unchanged. The existing lower-bound expectation now asserts0.01; the maximum test uses an actual large zoom factor instead of depending on an artificially large fit input forcing zoom-in.

All21 focused camera tests pass. The new native-control browser journey passes at actual scale0.0421855432, resets to1, then restores the exact0.9359331476 fitted camera with all four padded edges. These are isolated fixture observations, not user Chrome acceptance. Original negative drivers, raw logs and numerical observations remain under `ZOOM_FLOOR_ACCEPTANCE_01/`. The new source requires its own clean21/security/current five checks and exact-head independent review; no7e pass or approval transfers.

The actual user-proxy coordinator assigned existing non-author `/root/git_health` as the sole final-source reviewer, with author exclusion for his separately preserved three Rust input files. Original28 findings/history stay retained, and the obsolete queued7e review request is superseded through the existing owner route. Pre-reading may proceed, but no final verdict, native UUID, bot approval or owner adoption is inferred. Root remains the sole source writer, CI observer and ordinary matched merger. Deployment instructions remain a separate frozen documentation packet for integration after this UI release.


### 2026-10-21 — owner-authorized PR review policy and consolidation candidate

The owner explicitly removed the mandatory second/independent PR reviewer requirement. The live main-branch protection approval count changed from1 to0; a before/after comparison verified every other protection field unchanged. Source instructions and the existing hourly maintenance prompt now reflect that instruction. This does not remove source review, current-head checks, linear history, no-force rules or deployment qualification.

The private integration candidate starts from PR171 `b3c1fa9f86a0f213837fa37515c5226ad123d4f4` and ports distinct changes from PR169 `50409822d7b0ebbbc17f70897ab4483520bc6120`, PR168 `40950f6dcbc2b0b0ba5fc0851d3d950d9f444095`, PR172 `1ee8a7d1e84bf0982a44c6356d33ab00378a38ba` and PR170 `8caa6caafcd154dc22d224101fcb0715b09c504f`. Original refs, source-bound failures and receipts remain preserved. This is a new composed source requiring its own complete local and hosted qualification before publication/merge; individual input checks do not transfer.

The unchanged PR170 retry on a USB-backed xbabe3 runner failed on both five-second demo listener startup assertions after its96-test CLI suite took399.26s. PR172's original startup fixtures failed protected-ancestor checks and its mobile health page had an axe incomplete result. These failures are retained. Consolidation uses the repaired private initializer, private-at-creation canonical startup fixtures and separate bounded-layout Agent brief styling; original assertions remain. No successful qualification, merge or deployment is claimed by this candidate note.

The owner also authorized owned artifact/worktree cleanup after integration and a subsequent xbabe2 redeployment. Deployment remains a separate serialized attempt with actual receiving-main qualification, current process/configuration ownership, existing locks and rollback evidence; this note does not assert that it happened.


The first composed freeze `9acf158f56e847d0c1bc8a849cf42c5e01918126` passed stages1–20 and87 of88 browser tests. The byte-bounded Agent brief table was scrollable but not keyboard-focusable on mobile. Adding focus exposed horizontally clipped text during the contrast audit. Original full and focused failures, geometry and images remain retained. The repaired view uses the same field renderer for its desktop table and narrow project cards; one actual browser journey now passes all eight390/768/1024/1280px dark/light audits without waived violations or incomplete results. Shared credential initialization also reports creation from the actual create-only open, rather than an earlier existence check. The repaired source requires its own complete matrix.

Candidate `7b809f99cb60152f7ebff46256369c86228283c8` temporarily selected GitHub-hosted Ubuntu24.04 for Rust. The owner rejected that routing and explicitly required general self-hosted runners on xbabe1, xbabe2, xbabe3 and xbabe4. Run39615089847 was canceled; Rust job119273870756 completed canceled with no assigned runner. No cloud execution or performance qualification occurred. The successor restores the shared `neverhuman-lan` selector, documents the no-cloud policy and retains compiler/test concurrency, all assertions and the30-minute deadline. Fleet storage and four-host capacity still require actual live qualification; no runner services have been changed.


The restored self-hosted candidate `42787e3d026ba79e3b097420da2ded38bfcc6f0f`, tree `1f1bf88f91b6b38ab958c2fa7c895d3c5f14b353`, passed all21 local stages (1869 Rust/7 existing ignores,392 UI,88 browser). Exact-source Rust CI passed on xbabe1 in9m28s with1882 Rust passes/9 Linux ignores. Contracts passed on xbabe3, audit on xbabe2, security on xbabe1 and the manually dispatched offline ARM64 qualification on xbabe4. This demonstrates four general hosts participating, not every workload qualified on every host.

Browser job119281424529 on xbabe3 reached its15-minute outer deadline. A read-only in-guest snapshot bound synthetic PR checkout `3f6670cad903471ccaba711b1b128d0df44a5313` to the exact candidate tree. I/O pressure was81.00% over10seconds (full80.24%); memory pressure was zero. The log records one failed18-case-suite test before termination, but Vitest never reached its final error summary. Original logs remain failed evidence, not a browser pass or proof that storage explains every assertion.

The general x64 launcher now adds reusable `storage-nvme` only for a direct, nonrotational NVMe-backed job-state filesystem. The script changed from SHA256 `e51ee6d1f10faf7e1b051e2cf0b803bfb6b7cf67135effd413af253412dcb996` to `9e6c28b7433376a85ea8a5540cfa9b346b0177cf16d15ad10a3dc99f0a9cdadb` on xbabe1/2/3, with actual bash syntax and source-aware ShellCheck passes, root-only original copies and receipts. No service or active job was restarted; future natural registrations advertise verified storage. xbabe1/2 qualify, xbabe3 does not. Image/isolation/group/existing labels/resource admission and cleanup remain unchanged; xbabe4 ARM64 is unchanged.

The successor selects this shared capability for Rust and browser CI and retains the broader pool for other jobs. Browser cases synchronously emit their completed result and original error/stack before an outer timeout can suppress the summary. A private intentionally failing two-case probe emitted its error0.897s before the later result and before the final summary, with expected exit1 retained. No application assertion or deadline changed. The successor needs its own full local and actual matching self-hosted qualification before merge.


The `bfa3b86fdf87db5f696c8cbb6385bc11a2525845` browser job119296851513 passed on xbabe1 in5m31s within the unchanged15-minute deadline. Security jobs119296851475 and119300235302 both failed because the repository actionlint allowlist omitted the new shared `storage-nvme` capability. The earlier standalone lint used a private configuration and did not qualify the repository configuration; both original failures remain retained. The successor adds the label to `.github/actionlint.yaml` and must pass the enforced security entry point with that actual configuration, the complete local matrix and its own hosted checks before merge.

The general launcher now also handles systemd bind-mount SOURCE suffixes while checking the real filesystem device identity. Actual running-unit mount-namespace checks qualify xbabe1/2 and reject xbabe3. Idle root-receipt-bound registrations on xbabe1/2 received only the shared capability label through the existing runner controller credential scope; no active job or service was restarted. Original controller scripts, metadata responses and root-only installation receipts remain retained. The xbabe4 general ARM64 controller is unchanged.


### 2026-10-21 — receiving-main integration and next Rust-only candidate

PR171 merged by ordinary SHA-matched rebase into `1bb3190a726f842d36c40469da52456f32d427ed`; its receiving tree `91a8321f0ddf256bd8aaf82fbf5a892969aaba48` equals the reviewed `ee772f9` tree. All five receiving-main jobs passed. PR168/169/170/172 are closed as superseded, with original branches and failed evidence retained. This records integration, not deployment or parent release acceptance.

New draft PR173 `a98668ae036f260eaeb93bf78f40951fbbb77c48` introduces the Rust tool migration, embedded console and chart-room design. Its original source remains preserved. The serial candidate adapts its three commits over receiving main without rewriting that branch. Shared observer/server entry points retain current telemetry, private startup, optional cycle formats, resource output and graceful-stop behavior. Native release evidence/qualification retain their existing stricter implementations. `admiralty release rollout` replaces shell logic while its fixed-path wrapper remains an exact delegation checked by language policy. A failed unit restart now also triggers the retained single rollback.

Original PR173 ARM64 fixtures failed11 cases; the expected resource document/logs and required empty sandbox directories were absent from Git. The original Python sampler, with external processes/network denied and probe inputs supplied solely from the committed fixtures, reproduces the existing table/history exactly and supplies the missing expected JSON. Actual original kick/classifier/screen-send/screen-snap scripts in a private sandbox reproduce all11 captured stdout/stderr/state/simulated-keystroke scenarios before their missing logs are adopted. No real screen or SSH command ran. Literal terminal/export oracle bytes keep their spaces and CRLF through narrowly scoped attributes; source whitespace checks remain enforced.

The five original security results are canonical synthetic scanner-negative inputs. Their exact runtime bytes are retained through concatenated literals in the adapted history; no scanner allowlist is added. Current source postimages scan clean. macOS umask-width and Linux-only import assumptions are repaired without changing limits. The general self-hosted selectors, NVMe capability, two compiler jobs, serialized tests and existing outer deadlines remain; ARM qualification retains its five-minute deadline.

Embedding now pins no-follow directory/file descriptors, rejects special/replaced/oversize inputs, bounds actual read bytes and directory traversal, and keeps encoding-specific strong validators separate. Focused controls verify these refusals and conditional-request behavior. All106 fleet-operator tests and three real-binary rollout tests pass locally. Dashboard and script typechecks and10 media tests pass. The narrow ordinary-window graph originally shrank to29px when chat opened; the dock now yields to a96px graph minimum. The unchanged repeated two-repository Fit/zoom/pan/inspector journey, including axe, passes. Original failures/geometry and raw proofs remain private. This composed source still requires its own complete local matrix and hosted current-head checks before publication/merge. Production is unchanged.


PR175 maintenance review found that the first64-line paragraph cache accepted later prose and omitted later valid trailers; an orphan indented line was also accepted. Three new raw-log regression cases failed on original head `c203e821c6490880a2a149e317ca37f32debc99a`. The repair validates every line of the already-bounded Git output with only three128-character retained values, rejects orphan continuations and unfolds valid continuations without extending the total value bound. All12 focused Git observer tests passed, including the real repository nonmutation fixtures. Original failures are retained outside source in the hourly-maintenance `pr175/negative-regressions.log`. Full local and exact-head self-hosted checks remain required; this note does not assert merge, deployment or release qualification.

The intermediate repair `c773d688e133cbdeb5c51aa19613bca467cec08d` passed local stages1–10, then its exclusively owned process group was stopped at stage11 for a further correctness repair; this interrupted run is not a complete matrix. A synthetic probe using the actual extracted parser and observation cleaner showed that joining continuations could assemble credential syntax absent from each line. Final values now pass through the same public-observation redaction after unfolding. All13 focused Git tests passed, including the new split-marker regression. Its original failing probe and interrupted-run process custody are retained alongside the hourly proof. The successor still requires its own clean complete matrix and current-head hosted checks.
