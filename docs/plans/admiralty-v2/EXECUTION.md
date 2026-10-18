# Admiralty execution ledger

Planning baseline: `1e8ed17eff27ed3ab80dc6860fc8eda301406aee`. Implementation started on 18 October 2026. PR #9 was merged by the owner at head `9debe5dd` on 18 October at 13:58 UTC. Subsequent main commits contain the offline workplan registry, controlled Codex harness and private scratch correction. The read-only dashboard still runs its separately pinned binary/assets; merging source does not deploy it. Existing coding projects retain their lanes and ownership. Native v2 execution remains disabled pending qualification.

Read [MASTER_PLAN.md](MASTER_PLAN.md) and [WORK_PACKETS.md](WORK_PACKETS.md). Integrator assigns one current base/interface version and exact write scope per packet. Update this table from evidence; do not infer completion from agent messages.

| Packet | State | Owner | Integrated commit or candidate | Evidence and next action |
| --- | --- | --- | --- | --- |
| P00 | review | evidence lane + integration review | independently checked at `a9ba930e` | 37 pinned hashes, 11 synthetic cases, distinct 449/503 baselines and unknown-explicit inventory worksheet satisfy inspected artifact criteria; hashed current execution evidence recorded, authorized engineering receipt closeout pending |
| P01 | in_progress | integration + quality review | combined source `318ac713` | All 30 named targets registered; Mac 582 and clean Linux 584 Rust checks plus strict gates pass; hosted jobs queued without a runner; full fault/simulator matrix pending |
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
