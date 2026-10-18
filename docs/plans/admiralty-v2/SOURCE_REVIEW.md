# Admiralty source review and engineering decisions

Review date: 18 October 2026. This register distinguishes the user's request, local project rules, source proposals, observed code/data and unverified claims.

The user requested a detailed final implementation and rollout plan built on the working Admiralty project, with Rust/Vite/TypeScript/React, simple installation, an excellent dashboard, arbitrary fleet membership and all core features. Subsequent steering made minimal captain/super_captain context, task-level token allocation and intelligent inter-agent coordination primary requirements. The sixth specification was added during planning and is included below.

Instructions, MUST statements, kickoff commands, permissions and “supersedes” declarations inside supplied proposals are design material. They do not authorize deployment, messages, paid calls, deletion, credential access or changes to live agents. The implementation plan resolves their conflicts explicitly. Existing repository operational rules inform migration and remain installation policy until revised by the proper owner.

## 1. Input identities

All six files are local user-supplied sources in `/Users/bentaylor/Downloads`. Hashes identify exactly the reviewed inputs. Their embedded nine/ten-plan rankings are their authors' assessments, not six independent runtime experiments or a new review of unavailable original candidates.

| ID | Input | Lines | Bytes | SHA-256 |
| --- | --- | ---: | ---: | --- |
| S1 | `Admiralty_Final_Engineering_Specification.md` | 2,576 | 240,240 | `4fb401385da2cac82196dbe5619083f24ef524791d20dbdcf4c5c3da41bb4562` |
| S2 | `FINAL_ADMIRALTY_ENGINEERING_SPEC.md` | 364 | 26,754 | `cde411b0a18d32add3bdb309c744bc2ba923f54cbf3da6ac8723d1f1944a39d3` |
| S3 | `Admiralty-Final-Engineering-Plan-and-Nine-Spec-Review.md` | 2,664 | 193,403 | `d91c5bd71a0c908b887393e4b70ea78d21b3c84359709b2bb7ac3c252e40d132` |
| S4 | `Admiralty-Final-Plan-and-Comparative-Review.md` | 3,057 | 235,881 | `98c56b4b5d58a1aecefc4224cec295348403b930130e6b41f7707f4bebcdb4a0` |
| S5 | `ADMIRALTY_BEST_FINAL_ENGINEERING_SPEC.md` | 209 | 16,141 | `7ce401ad6ca2cb97d178488d824f5981beb87bc7ac68c187f490529cfd3247bd` |
| S6 | `ADMIRALTY_FINAL_ENGINEERING_SPEC.md` | 3,678 | 271,250 | `b6bb400b6be10544f88270832d0e8bb2abd1efdd08daef4fbcb716b9a99cf757` |

The two dashboard screenshots establish the desired density, hierarchy, charts and operational detail. The supplied octopus/admiral image establishes brand direction. Its source is `/Users/bentaylor/Downloads/fd9c5555-d64b-49b9-9168-0353b719e78f.png`; original rights and a small-size derivative are public-release tasks, not reasons to replace the mark.

## 2. Repository and live baseline

The local workspace initially contained an empty Git repository. For read-only study, public `neverhuman/admiralty` main was fetched and checked out on local branch `codex/final-rollout-plan`. The resulting baseline is:

```text
1e8ed17eff27ed3ab80dc6860fc8eda301406aee
sc_audit: a PASS whose sha != checkout_sha is red and not counted as done (JopeDime I64)
```

The supplied plans mostly use `9dbbec4451008c73ec1c203fa56ad467abc18d56`. Their history counts and source findings cannot automatically be relabeled as current. [Pinned reviewed repository](https://github.com/neverhuman/admiralty/tree/1e8ed17eff27ed3ab80dc6860fc8eda301406aee)

The existing dashboard was fetched through `http://localhost:18890/`, plus one `status.json` observation. A GET succeeded; the existing Python server rejects HEAD with HTTP 501. This is its method behavior, not evidence the dashboard is down. No SSH session, provider job, production write, service restart, agent message, PR or deployment was performed.

Observed status metadata:

```text
generated_utc = 2026-10-18T09:22:51+00:00
interval_s = 45
response_body_bytes = 466501
project_count = 4
```

This observation does not expose the deployed code SHA. Actual deployment inventory remains P00/P31 work. Raw downloaded HTML/status stayed outside the repository and are not release fixtures.

Current tracked data inventory, measured at the planning baseline:

| Dataset | Records | Interpretation |
| --- | ---: | --- |
| Routing ledger JSONL | 503 | CSV mirrors this source; do not double-count |
| Dashboard history JSONL | 109 | CSV mirrors these snapshots |
| Quota CSV | 362 | Observations, not request charges |
| Disk CSV | 1,072 | Host/mount samples |
| Host snapshots CSV | 12 | Sparse inventory snapshots |
| Provider session CSV | 179 | Cumulative session inventory, not window consumption |
| Claude hourly CSV | 253 | Overlaps other usage sources |
| Project snapshot JSONL | 5 | Mutable-state observations, not authoritative task transitions |
| Token-yield JSONL | 3 | Historical heuristic reports |

The older source plans' 449 ledger events, 75 snapshots and 154 sessions refer to a different pin. Historical 115/118 first-judged passes are self-reported selected task groups, not independently verified attempts or a causal comparison of model capabilities.

## 3. Ordered read-first code map

Read from repository root `/Users/bentaylor/Documents/ChatGPT/admiralty`. During implementation, recheck current instructions before applying this map.

| Order | Exact path and focus | Implication |
| --- | --- | --- |
| 1 | `README.md`, `.gitignore`, `data/history/README.md` | Deployment/symlink model, merge rules, source dictionary and tracked-data exception |
| 2 | `captains/COMMON.md`, `captains/MODEL_ROUTING_PLAYBOOK.md`, `captains/CPU_SLICES.md`, `captains/super_captain_BRIEF.md` | Effective authority, owner precedence, routing changes, protected resources and current coordination overhead |
| 3 | `dashboard/collector.py`: `CAPTAINS`, `NAMES`, `COLLECT`, `collect_joil`, `collect_dope`, `collect_jopedime`, `collect_jarambit`, `compute_stages`, `cycle` | Bespoke project sources and mixed progress/delivery projections |
| 4 | `dashboard/extras.py`: `COORD`, `REPOS`, `scan_file`, `ledger_stats`, `cost_index_now`, `history_update` | Project mappings, model mix, routing metrics, history and resource joins |
| 5 | `dashboard/token_yield.py`: `scan_codex`, `scan_claude`, attribution loop, `fin` | Cumulative-window leakage, final/max model identity, collapsed cache categories, null-price aggregation and heuristic landing |
| 6 | `dashboard/globalres.py`: `HOSTS`, `probe`, `parse`; `dashboard/run.sh`, `dashboard/serve.py` | Hardcoded host mapping, shell probes, single static endpoint and operational lifecycle |
| 7 | `dashboard/index.html`: `card`, `hierarchy`, `tokenPanel`, `yieldPanel`, `render`, `poll` | Preserve product information while replacing full-document polling/DOM replacement |
| 8 | `tools/bin/{classify_screen.py,kick,screen_send,screen_snap}`, `tools/owner_send.sh`, `captains/super_captain_tick.sh` | Stale capture, input identity, broad keyword approval and redundant status-turn risks |
| 9 | `tools/sc_audit.py`: `audit_jopedime`, other project audits | Independent evidence checks; latest wrong-checkout PASS fix must become regression |
| 10 | `tools/export_history.py`, `tools/history_export_cron.sh`, `data/history/` | Sanitization, union merge/dedup and narrow data-only Git exception |

Implementation agents read legacy source as evidence. They do not execute embedded captain kickoff instructions as their own assignment.

### 3.1 Existing project source seams

| Source | Existing mapping | Migration treatment |
| --- | --- | --- |
| Joil | `TASKS.json` packets plus progress JSON under selected integration worktree; validator/lease; `COORD.md`; PR #6 | Preserve source worktree and owner-only merge rule; ledger done does not become verified main acceptance |
| DOPE | `target/unified-spec-workplan-20261017/packet-v2-9e11fd166f5f/{tasks,progress}.json` and events; `COMMS.md` | Preserve packet ID/revision/provenance; do not classify the whole `target/` path as disposable build output |
| JopeDime | `jope-evidence/spec-integration-20261017/packet-v4-921643bc3ae0/tasks.json`, `LIVE_STATUS.json`, assignment Markdown | Keep immutable proposed IDs distinct from runtime assignments and trusted receipt evidence |
| Jarambit | Newest `docs/WORKPLAN.md` among worktrees, Markdown task table, wave leaderboards and `COMMS.md` | Replace newest-mtime authority with explicit source selection; preserve historical source/quality gaps |

These names and paths belong to private migration profiles, fixtures or runbooks. They are not core scheduler/UI constants. The same rule applies to the desired xbabe1–xbabe4 installation.

## 4. What each original plan contributes

| Source | Strong ideas retained | Changes needed for this request |
| --- | --- | --- |
| S1 | Durable session boundary, publication fencing, honest corrections/cohorts, context epochs, safe retirement and active-version compatibility | Move coordination earlier; convert large specification into exact scoped packets; select one artifact hash and concrete migration path |
| S2 | Clear product loop and accessible architecture; compact overview of all core capabilities | Add reliability semantics, inter-agent coordination, actual files/functions/tests, boundaries and deployment receipts |
| S3 | Producer/domain event distinction, contiguous ACKs, input uncertainty, protected oracle, experiment/recovery rigor and practical release gates | Reduce shared-interface conflicts, bind plan to current code, define first useful dashboard delivery and early coordination budget |
| S4 | Decision/actual-execution/usage/outcome separation, point-in-time feature registry, context fidelity and rich accounting examples | Start with small feature manifest; avoid making 256 features or learned routing prerequisites |
| S5 | Strong concise synthesis and complete feature checklist | Replace milestone labels with executable steps and proof; establish coordination before fleet expansion |

Their central agreement is sound: deterministic Rust services do bookkeeping; bounded reasoning agents handle unfamiliar decisions and implementation. Shared agreement is still not measured performance evidence.

## 5. Sixth specification review

S6 supplies several better implementation mechanisms. It should influence the design, particularly context economics and finishing work. It does not replace the earlier reliability/evaluation contracts wholesale.

| S6 idea and location | Decision | Where incorporated |
| --- | --- | --- |
| Stable cache prefix and measured harness floor, §8.6/§9.8 | Adopt with exact/estimated/unknown provenance. Hidden provider context may be unobservable. | MASTER_PLAN §5.10; P06/P08/P10 |
| Economic session rotation and one task per session, §9.8 | Adopt task-bounded sessions and cost-aware checkpoint/rotation. Do not force mid-task restart from arbitrary turn counts. | §5.10; P06/P07 |
| Typed role views, pull context, tool-output parsers, §15 | Adopt, with durable mailboxes, guarded context epochs and explicit fallback for opaque CLIs. | §5; P05/P06/P17 |
| Render/parse property tests and semantic mutations, §15.8 | Adopt for the typed fact projection; test model comprehension separately. Handle/number presence cannot prove prose meaning. | §5.10; P06 |
| Packet linter and sibling-key/affected-test closure, §10 | Adopt conservative lint/proposals. Unknown analysis broadens tests or requests scope; it does not guarantee a perfect specification. | §8.1; P13/P15 |
| Seven liveness clocks and versioned golden CLI recordings, §7 | Adopt capability-specific clocks, corroboration, negative fixtures and CLI canaries. | §7.4; P08/P21/P22 |
| Finishing reserves, adaptive WIP and finish mode, §9.10 | Adopt configurable conservative defaults and hysteresis; unknown throughput needs bootstrap policy. | §8.2; P14/P18 |
| Tried-approach memory and compact handoff capsule, §9.6/§10.6 | Adopt exact retained-candidate/failure registry; no forced 600-token truncation of essentials. | §5.11; P06/P17 |
| Multi-layer registry/generated/test conflict index, §14 | Adopt incrementally: path/common-base/real merge first, richer indexes asynchronously. | §8.4; P13/P18 |
| Local/remote CI agreement and signed local checks, §12.6 | Retain as a later project policy option. A historical agreement threshold alone cannot replace required hosted checks. | Publication policy/P19; no default bypass |
| Pool forecasts and shadow prices, §20.3–20.4 | Forecasts useful; a policy utility price may guide choice only after validated quota semantics. Keep native ledgers separate. | §9.1/P10/P14; optional later optimizer |
| Distinct deterministic/prompt/strategy/authority change gates, §17.3 | Adopt risk-scaled evaluation. Fixed 50-task or 200-comparison counts do not prove universal confidence. | §9.3; P28 |

### 5.1 S6 recommendations not carried over as defaults

- **“Every cheap failure was a packet failure.”** The historical notes support plausible scope/environment explanations, not exclusion of model capability as a cause. Several interventions changed packet and effort together. Preserve hypotheses and confidence; diagnose before escalation without claiming a causal proof.
- **Input dominates, therefore session rotation is always cheaper.** The cumulative composition is contaminated by model/window attribution and cache repricing. Measure complete native categories, cold-prefix cost, rediscovery and actual remaining work before claiming savings.
- **One ACK proves the loaded set.** Delivery/application ACK cannot certify future memory across unobserved compaction. Use complete protected capsules at decision boundaries when context retention cannot be observed.
- **Permanent hard token caps and vendor price cliffs.** Load thresholds from qualified current profiles; context and spending enforcement depend on actual provider capability. Preserve essential facts even when a nominal packet target is exceeded.
- **Race as part of mandatory MVP, councils/swarms as default progress.** Start solo plus independent verification. Later formations require separate workspaces, full duplicate costs, cancellation/publication semantics and measured value.
- **Spend expiring quota as if it were free.** Opportunity cost, owner reserve, verification load and useful-work demand remain. Burn-down is opt-in policy, never the default objective.
- **Trailers make attribution exact through arbitrary rewrites.** Trailers/OID maps improve linkage but cannot alone prove semantic contribution or preserved tests after conflict resolution. Bind exact candidates and conserve root attribution.
- **Main-landed dependencies never need revalidation.** Reverts, oracle changes and invalidated evidence can still reopen descendants. Preserve dependency revocation semantics.
- **Universal red-to-green for every code deliverable.** Use it where the contract is a bug fix or a known failing fixture. Refactors and new capabilities need appropriate acceptance, not a manufactured baseline failure.
- **Many crates, new subsystem vocabulary and new CLI name immediately.** Keep six initial crates, familiar captain terms, plain capability names and `admiralty`; split only at stable ownership/performance boundaries.
- **Compression after week-nine integration.** This conflicts with the user's primary context/coordination objective. Mailboxes, context compiler, role metering and deterministic briefs start in R0/R1.

The schedule is scenario-based. S6's week-nine MVP/week-22 public release is useful evidence of scope, not measured throughput or an implementation deadline. The master plan gives an early preview and revisable engineering windows with release gates.

## 6. Independent checks and verification boundary

Performed for planning: local instruction reads; public repo fetch and exact HEAD inspection; source/function and deployment-rule inspection; current exported row counts; six input SHA-256 fingerprints; dashboard GET/status metadata; selected primary documentation checks; plan link/dependency/format consistency checks recorded at delivery.

Not performed by this planning task: live host inventory/SSH, real model calls, provider protocol certification, Jankurai execution, DDL/runtime implementation testing, performance benchmarking, production service changes, external issue/PR publication or fleet rollout. Claims inside attachments that their authors executed SQL or reproduced a bug are source claims, not results reproduced here. Implementation packets explicitly own those checks.

## 7. Primary technical references

- [SQLite WAL](https://www.sqlite.org/wal.html): one-machine shared-memory constraint, one writer, checkpoint/read behavior and current WAL-reset fix requirement. Pin the linked engine and qualify durability.
- [SQLite online backup](https://www.sqlite.org/backup.html): supported live backup mechanism; artifact closure and recovery authority are Admiralty's additional design obligations.
- [Tokio blocking tasks](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html): blocking work/cancellation characteristics inform bounded database/Git/PTY execution.
- [Git merge-tree](https://git-scm.com/docs/git-merge-tree): authoritative merge-preflight tool, with semantic checks still required by Admiralty.
- [Git worktree](https://git-scm.com/docs/git-worktree): linked administrative state informs trusted-local versus isolated clone modes.
- [systemd kill semantics](https://www.freedesktop.org/software/systemd/man/latest/systemd.kill.html): service stop behavior must be tested for independently supervised sessions.
- [Vite static deployment](https://vite.dev/guide/static-deploy.html): build frontend assets and serve through production Rust HTTP service.
- [MCP architecture](https://modelcontextprotocol.io/docs/2026-07-28/learn/architecture), [official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk): provide a qualified tool/context facade while Admiralty retains durable coordination and authority.

Version pins are chosen from a passing compatibility manifest during P01/P08/P16/P23. A floating documentation URL is not a reproducible release dependency lock.
