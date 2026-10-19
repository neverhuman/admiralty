# Admiralty execution ledger

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

Rollout reconciliation at01:32:53Z pins merged source779a008 and the real fixed-toy/factory/capture receipts. Before/after hashes are `1edbdda027b9942b1a007e8e3e9baa63c5d21f199bc44e1e815e12d8933d253e` and `91a647293f72eeb6af54a0c1450b8672ee4edb610077bc620bf6b7405f170307`. P00/P01 original criterion receipts were independently hash-verified; only their recorded reference status is normalized from verified_packet_criteria to verified with explicit criterion kind/scope, archived original reference and original verification timestamp. This repairs the existing sc_audit enum mismatch without changing parent acceptance or claiming a new check. Every parent status and unrelated packet stays intact. P00/P01 alone accepted;30parents and general coding execution remain open.

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
[single deferred cleanup backlog](../../../../CI_CLEANUP_BACKLOG.md). Jepson's
02:12Z code-first ruling authorizes this local-CI-only cutover merge; no red check
is green. Commodore alone owns the new Git/Fleet deployment.

The registered Store attention journal persists observations and delivery
decisions, atomically creates new wake intents, preserves immutable historical
receipts and unknown effects, and validates history on open/rebuild/backup.
Nine actual registered integration tests pass; the frozen worker's affected
Store129-test checks and independent source/registration review are preserved in
[the component receipt](../../../receipts/integration/durable-attention-20261019T0224Z.json).
Current authenticated scope/clock authority and safe-turn transport remain
separate. Accepted parents remain P00/P01 alone,2/32;30parents remain open.

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

Root independently checked the live f590837a deployment at03:10:25Z: all14
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
