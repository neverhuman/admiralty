# Prepare Admiralty implementation missions

`tools/rollout_packets.py` converts the installed P00–P31 plan into one bounded
worker capsule per packet and a dashboard manifest. It uses Python's standard
library. It prepares local artifacts; it does not open SSH connections, launch
providers, paste into Screen sessions, create worktrees, publish code or merge.

Run from the Admiralty checkout. Choose a dedicated directory outside that
checkout, owned by the current user. The examples use `/tmp/admiralty-rollout`;
operators should use durable private runtime storage for actual handoffs.

```sh
rtk proxy python3 tools/rollout_packets.py export \
  --out /tmp/admiralty-rollout --base-ref HEAD
rtk proxy python3 tools/rollout_packets.py check --out /tmp/admiralty-rollout
```

Export resolves `HEAD` to its full actual commit ID. It reads only the local plan
and Git revision; the repository is not changed. Source-plan SHA-256 digests,
explicit write allowlists, dependency edges, owners, release targets, acceptance,
validation and receipt requirements travel with each packet. Source scopes are
pinned: a changed scope requires review of the exporter's explicit allowlist.
Additional paths described only vaguely by the plan require an integration-owner
request; the exporter grants no guessed broad permissions.

`rollout.json` is the public runtime entrypoint. It names immutable capsule files
under `generations/<id>/capsules/P00.md` through `P31.md`. Publication replaces the
manifest atomically after all capsules exist and are synced. Prior generations
remain intact for readers holding old references; retention is an explicit
operator task after claims and evidence references have drained. Existing
nonempty directories without this tool's ownership marker and symlink output
paths are refused. Concurrent commands serialize under an advisory local lock.
The output directory is private runtime state, not a network-shared database.

The default project ID is `admiralty-v2`; `--project-id` and `--repository-url`
set deployment metadata. No node names or fleet size are built into the tool.
`--repo /absolute/checkout` before the subcommand selects another checkout.

## Dashboard connection

The existing dashboard reads the same manifest when configured with:

```sh
export ADMIRALTY_ROLLOUT_MANIFEST=/tmp/admiralty-rollout/rollout.json
```

Set this in the collector's environment using the deployment's existing launch
mechanism. The collector also checks `<dashboard-runtime>/rollout.json`. Keep
this manifest outside the Git checkout. A remote dashboard needs the manifest
and referenced evidence transferred through the existing approved deployment
path; this tool performs no remote transfer.

The version-one JSON envelope has `schema_version`, `producer`, `updated_utc`,
`project {id,name,repository_url}`, `base_ref`, source hashes, `packets`, `lanes`
and `pr {url,head}`. Each packet has `id`, `title`, `status`, `lane`, `depends_on`,
`claim`, `evidence`, `verification`, `integration`, `capsule_path` and
`capsule_sha256`, plus its bounded execution contract. Empty proofs are `null`.
The dashboard is an observational view, not an acceptance authority.

## Delegate through the existing system

At first export only P00 and P01 are `ready`. Both have no dependencies and their
write scopes are disjoint. Other packets remain `planned`; full-packet readiness
requires every dependency to be `integrated`. Conditional later-release
dependencies in P19 and P30 are conservatively retained; an integrator can split
those packets into separately reviewed contracts before starting partial work.

Assign a ready packet locally before handing its capsule to an existing captain:

```sh
rtk proxy python3 tools/rollout_packets.py claim \
  --out /tmp/admiralty-rollout --packet P00 \
  --owner evidence-worker --lane evidence-1 --node enrolled-node \
  --branch codex/admiralty-p00
```

This records intent and lane identity. It does not inspect capacity, create the
branch, acquire a remote lease, or start work. The operator/captain must verify
the current base, actual owned worktree, local private policies, integrated
dependency receipts and shared-file ownership before dispatch. A lane can hold
one active packet. Cross-host leases and atomic admission belong to later Rust
runtime packets, not this bootstrap tool.

Give the assigned worker its named capsule and scoped references. Avoid pasting
all plans, unrelated projects' prompts or repeated dashboard snapshots. Captains
receive changed evidence, blockers and decisions; workers perform implementation
and return a structured candidate receipt. Existing `owner_send.sh` immediately
pastes and submits input, so invoking it is a separate live operation requiring
the session's established authority, ownership and safe-input checks. No such
command is generated or invoked here.

## Candidate receipts and progress

Supported statuses are `planned`, `ready`, `claimed`, `in_progress`, `blocked`,
`review`, `verified`, `integrated`. A candidate receipt requires an explicit active
claim. It can advance that packet only to `review`; it cannot establish a claim,
verify code, integrate code or unlock dependent packets.

A worker receipt is a JSON object with these required fields:

```json
{
  "schema_version": 1,
  "packet_id": "P00",
  "owner": "evidence-worker",
  "base_sha": "FULL_ACTUAL_BASE_COMMIT_ID",
  "head_sha": "FULL_CANDIDATE_COMMIT_ID",
  "interface_version": "unimplemented",
  "changed_paths": ["tests/fixtures/legacy/example.json"],
  "criteria": [{"id": "fixture-count", "status": "passed", "evidence": "path/range plus actual count"}],
  "commands": [{"argv": ["python3", "check_fixtures.py"], "cwd": "/actual/owned/worktree", "exit_code": 0}],
  "environment": {"python": "ACTUAL_VERSION", "platform": "ACTUAL_PLATFORM"},
  "artifacts": [],
  "risks": [],
  "blockers": [],
  "next_action": "Independent exact-candidate verification"
}
```

Replace placeholders with observed values. Git IDs must contain 40 or 64 lower
case hexadecimal digits. Criterion status is `passed`, `failed` or `unavailable`.
Artifact entries use `path` and a computed 64-digit `sha256`. Preserve failed
commands as well as repairs. An empty artifact list explicitly means no external
artifact evidence was supplied; it never proves acceptance.

```sh
rtk proxy python3 tools/rollout_packets.py reconcile \
  --out /tmp/admiralty-rollout --receipt /path/to/candidate-receipt.json
```

Import validates scope, owner, pinned base, candidate ID and receipt structure;
it does not execute receipt commands or trust reported test results. The copied
receipt is content-addressed and marked `worker_report_unverified`. Duplicate
imports are idempotent. Candidate JSON fields claiming PASS, verified or
integrated grant no authority. The integration owner independently checks the
exact candidate, files accepted receipts, and records separate verification and
integration provenance through the implementation ledger's existing process.
The bootstrap tool offers no command to invent those trusted states.

Verification provenance fields are `receipt`, `revision`, `verifier`,
`observed_utc`; integration provenance fields are `receipt`, `revision`,
`target_ref`, `observed_utc`. Presence is structural validation, not cryptographic
or independent proof. After an integrator updates authoritative runtime state,
repeat export with the same source/base to preserve claims and refresh readiness.
Changed plan hashes, base or project identity require a fresh output directory
and explicit reconciliation of outstanding claims; they never silently reset work.

## Validation and failure handling

```sh
rtk proxy python3 -m unittest discover -s tests/legacy -p test_rollout_packets.py -v
rtk proxy python3 -m py_compile tools/rollout_packets.py
```

Exit 0 prints a compact JSON summary. Exit 2 reports invalid plans, DAG cycles,
changed scope, stale base, invalid receipts, ownership conflicts or damaged
capsules. Preserve previous manifest/evidence and repair the cause. An interrupted
export may leave an unreferenced complete generation; the current manifest still
references its previous intact capsules. This preparation step is not a claim
that any Rust implementation packet or fleet rollout has completed.
