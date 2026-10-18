# Admiralty execution ledger

Planning baseline: `1e8ed17eff27ed3ab80dc6860fc8eda301406aee`. Implementation started on 18 October 2026. PR #9 was merged by the owner at head `9debe5dd` on 18 October at 13:58 UTC. Subsequent main commits contain the offline workplan registry, controlled Codex harness and private scratch correction. The read-only dashboard still runs its separately pinned binary/assets; merging source does not deploy it. Existing coding projects retain their lanes and ownership. Native v2 execution remains disabled pending qualification.

Read [MASTER_PLAN.md](MASTER_PLAN.md) and [WORK_PACKETS.md](WORK_PACKETS.md). Integrator assigns one current base/interface version and exact write scope per packet. Update this table from evidence; do not infer completion from agent messages.

| Packet | State | Owner | Integrated commit or candidate | Evidence and next action |
| --- | --- | --- | --- | --- |
| P00 | review | evidence lane + integration review | integrated fixture baseline | 37-source/11-synthetic verifier and 74 Linux legacy checks pass at 9debe5d; full inventory/receipt closure remains pending |
| P01 | in_progress | integration + quality review | reviewed `9debe5d` | Six crates, pinned tools and independent Linux CI-equivalent gate pass; hosted runner jobs remain unassigned; full fault/simulator matrix pending |
| P02 | in_progress | domain + integration review | reviewed `03772f94` | Typed contracts, pure reducers and schema/TS checks are integrated; complete authority/policy negotiation remains pending |
| P03 | in_progress | local store agent + integration reviewer | reviewed `29acc013` | Atomic domain/outbox admission, immutable request/effect associations, replay and receive-time retry compatibility tested; durable executor admission loop pending |
| P04 | in_progress | artifact + custody lanes + strong review | reviewed `29acc013` | Transactional asset/pin catalog with full observation binding, fixed clock policy and receipt closure; trusted filesystem observation/startup activation pending |
| P05 | in_progress | local coordination team + strong review | reviewed `29acc013` | Durable scoped messages/ACK and question journal with canonical receipt/task-scope checks; delivery service/subscriptions pending |
| P06 | in_progress | local context team | PR prototype | Constraint-preserving capsules, epochs and bounded attention governor; durable governor/outbox activation and retrieval pending |
| P07 | in_progress | remote session lane + local reviewer | reviewed `e2b540fb` | Independent private Unix-socket host, reconnect continuity, compatible launch envelope, final capture watermark and native owned-group cleanup; restart adoption and admitted fleet activation unqualified |
| P08 | in_progress | provider + integration review | reviewed `455ee5f4` | Adds opt-in controlled exact-fixture Codex transport harness with offline CI tests and executable hashes; no real run or native qualification |
| P09 | in_progress | remote workspace lane + local reviewer | candidate `f0d15a82`, imported `d637bfe` | Read-only custody registration; real runner/resource isolation pending |
| P10 | in_progress | usage/reservation lanes + strong review | candidate `99f05b0`, imported `61e888b` | Segmented usage plus conservative scoped reservations reviewed; runtime reservation activation pending |
| P11 | in_progress | API + integration review | reviewed `17c7a76d` | Authenticated bounded read API/SSE and offline CLI are tested; live command envelopes, terminal input and agent tools pending |
| P12 | in_progress | frontend + integration review | live assets `9debe5dd` | Dense real-data console, 13 temporal metrics, observed manager selection and centered owner deployed; 75 React tests/build pass; full scale/accessibility qualification pending |
| P13 | in_progress | store + integration review | reviewed `455ee5f4` | Durable immutable JSON revisions, CAS/retry, reciprocal task identity, writer/CLI and backup/rebuild tests; TOML/Markdown, import UI and live admission pending |
| P14 | in_progress | scheduler lane + strong review | candidate `4aa3abe`, imported `2ef27f4` | Pure scheduler independently hardened at `03772f94`; durable scheduling and launches remain pending |
| P15 | in_progress | verifier lane + strong review | candidate `a4c358`, imported `1f8b32c` | Scoped evidence consistency hardened at `03772f94`; trusted independent verifier execution pending |
| P16 | in_progress | Luna audit lane + strong review | candidate `e8b9321`, imported `10fe48e` | Staged audit candidate under independent review; no Jankurai execution qualification |
| P17 | in_progress | Luna interface lane + strong review | candidate `fb366ab`, imported `3df778c` | Scoped interface reducer reviewed; peer tools/durable transport pending |
| P18 | in_progress | Luna conflict lane + strong review | candidate `54d57e0`, imported `03d42fe` | Bounded conservative conflict graph reviewed; merge train/runtime integration pending |
| P19 | in_progress | Luna publication lane + strong review | candidate `c3e3289`, imported `4a7eee0` | Frozen request/receipt publication fences reviewed; credential gateway/forge qualification pending |
| P20 | in_progress | Luna retirement lane + strong review | candidate `164aee7`, imported `94336b5` | Scope/pin/identity/replay retirement proposals reviewed; no destructive executor or permission |
| P21 | in_progress | provider candidate + strong review | imported `a16e41a` + `103c564`, unregistered | Four candidate tests pass but 14 offline counterexamples reproduce blockers; fix launch/session/usage/parser claims and registered tests before activation or qualification |
| P22 | in_progress | provider candidate + integration review | imported `84c7d15`, unregistered | Static review requires Cargo/fixture registration, conservative identity, strict framing and shared launch limits; help-only evidence is not live qualification |
| P23 | in_progress | Luna enrollment lane + strong review | candidate `e3a9125`, imported `a6637a6` | Scope/clock/revocation enrollment contracts reviewed; real enrollment/replay transport pending |
| P24 | in_progress | Luna placement lane + strong review | candidate `a791470`, imported `08c3a38` | Scoped fresh ownership/continuation placement proposals reviewed; real partition qualification pending |
| P25 | in_progress | Luna recovery lane + strong review | candidate `fa77ebd`, imported `4494af9` | Snapshot/fence/closure recovery bindings reviewed; executor restore/cutover qualification pending |
| P26 | in_progress | Luna economics lane + strong review | candidate `4aec329`, imported `78688d4` | Exact scoped usage/correction/coverage economics reviewed; explorer/service integration pending |
| P27 | in_progress | Luna excursions lane + strong review | candidate `c3a1895`, imported `1fe0bb2` | Canonical bounded scoped excursion state reviewed; durable proposal loop activation pending |
| P28 | in_progress | Luna trials lane + strong review | candidate `142bd2a`, imported `87b0a94` | Fixed cohort/seed/oracle trial analysis reviewed; independent live trials pending |
| P29 | in_progress | diagnostics + frontend review | reviewed `17c7a76d` | Bounded recorded diagnostic CLI tested; durable action/job, assets/fleet UX and named accessibility/E2E gates pending |
| P30 | in_progress | release + integration review | reviewed `17c7a76d` | Explicit-input archive builder passes 10 checks; clean-install, signed distribution, rights and complete operator paths pending |
| P31 | in_progress | integration + qualification review | reviewed `455ee5f4` | Isolated binary smoke and offline controlled provider harness are integrated; real provider, soak, savings, restore and fleet cutover gates remain open |

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
