# Legacy deployment inventory worksheet

Status: source-backed worksheet, **live deployment unverified**. Created from
local source pin `1e8ed17eff27ed3ab80dc6860fc8eda301406aee`; no SSH,
Screen input, provider call, service change, cron change or live write performed.
Source statements below are leads for an operator to verify; they are not a
deployment manifest or new authority grants. Record private addresses, resolved
paths, environment and credentials only in private inventory outside Git.

Rows citing `run.sh`, the source server, collectors or `token_yield.py` describe
that pinned baseline. Those Python/shell sources are now retired from this
repository (see [Retired dashboard collector stack](#retired-dashboard-collector-stack));
a deployed host may still run old copies, so the actual-value column still
needs verification.

| Item | Source-backed lead | Actual value / evidence still required |
| --- | --- | --- |
| Source checkout | README declares `~/admiralty` on deployment host | UNKNOWN deployed Git SHA, branch, dirty status and Git common-dir identity |
| Dashboard legacy path | README declares `~/xbabe2_kicker/captains-dashboard/` with symlinks into source | UNKNOWN resolved path, symlink owner/device/inode and file-by-file mapping; do not atomic-replace a live symlink |
| Captain legacy path | README declares `~/xbabe2_kicker/captains-20261017/` | UNKNOWN symlink mapping versus runtime files and effective rule owners |
| Runtime override | Collectors read `CDASH_RUNTIME`; default is dashboard legacy path | UNKNOWN effective environment, runtime owner, permissions and actual directory |
| Dashboard endpoint | `run.sh` defaults loopback port 18890; source server allowlists `/`, `/index.html`, `/status.json` | UNKNOWN current listener PID/start time/executable/user and actual tunnel chain |
| Dashboard session | README/run.sh name `captains_dash`; run.sh starts static server and restarts collector child | UNKNOWN exact Screen PID/socket identity and parent/child process graph; process names alone are insufficient |
| Collector | Source defaults `CDASH_INTERVAL=45`, worker hardcopy every third cycle, forge cache TTL 120 s | UNKNOWN running executable digest, configuration, capture freshness and writer count |
| Browser refresh | Existing page polls status every 15 s | UNKNOWN deployed HTML hash and active client versions |
| Snapshot/cache files | Source writes `status.json`, `.hc`, `.gh_cache.json`, `.history.json`, `.git_cache.json` | UNKNOWN actual owners, symlinks, sizes, timestamps, access and retention |
| Economics storage | `token_yield.py` opens `tokens.db`, outputs `token_yield.json`; default window 48 h | UNKNOWN actual SQLite engine/source ID, writers, backup/restore status and effective window |
| Native usage inputs | Source reads operator Codex session logs and Claude transcripts | UNKNOWN granted read scope and native version/semantics; never copy raw logs into public fixtures |
| Resource sampler | `globalres.py` contains three legacy host names and SSH probes | UNKNOWN enrolled identities, connectivity, actual capacity and current pressure; names are migration data, not scheduler constants |
| History export | `admiralty history publish` (formerly `history_export_cron.sh`) declares own clone `~/.admiralty-history`, own lock, data-only commit path | UNKNOWN actual crontab entry, user, last run, clone HEAD, lock owner and log digest; do not run the publisher for inventory |
| Export schedule | `data/history/README.md` declares hourly minute 23 | UNKNOWN current scheduling mechanism and exact timezone/owner |
| Super captain ticker | `admiralty sc-tick` (formerly `captains/super_captain_tick.sh`) runs in screen `super_captain_tick` with 30-minute checks | UNKNOWN active process, actual interval, pending input, delivery owner and rule revision |
| Other input writers | `admiralty fleet kick`, `admiralty fleet deliver` plans (formerly `tools/bin/kick`, `send_*.sh`), owner tools and captain prompts can send Screen input | UNKNOWN complete cron/timer/manual writer roster and overlapping scopes; disable corresponding old mutation scope only at authorized takeover |
| Provider versions | Playbook claims agy 1.4.2 and historical aliases/model flags | UNKNOWN currently installed executable path/hash/version/help/capability and current process configuration |
| Toolchain versions | Legacy scripts invoke Python, Screen, Git, gh, SSH and provider CLIs | UNKNOWN actual versions/platform/kernel/architecture; each managed adapter needs qualified capability receipt |
| Task sources | SOURCE_REVIEW lists four heterogeneous legacy ledger/worktree seams | UNKNOWN actual selected worktree, plan revisions, immutable task IDs, active claims and owner rules per project |
| Approvals | README/COMMON require exact-head bot approval and rebase merge; one legacy PR is owner-only | UNKNOWN effective current rule files, revision digests, grants and existing unresolved forge effects; preserve restrictions until proper owner revises them |
| Protected resources | COMMON/CPU_SLICES specify excluded hosts/services and reserved CPU/lease ranges | UNKNOWN current effective private policy and reservations; import them without probing excluded infrastructure |
| Asset custody | COMMON forbids foreign/dirty/private/ignored/unmerged cleanup | UNKNOWN registration, lifetime pins, active process references and unique data; discovery never grants custody |
| Recovery | No complete active-session recovery manifest proven by inspected sources | UNKNOWN backup watermark, artifact closure, fenced publisher, tested restore and rollback owner |

For each future verified row, record UTC observation time, operator identity,
read-only command/API, exit status, sanitized evidence digest, exact target
identity and confidence. An error remains unknown; do not replace an unknown
with zero or copy a source-declared value into the actual-value column.

Safe collection sequence, once explicitly scoped by the deployment owner:

1. Identify intended host through existing trusted inventory; compare host UUID
   and host key. Do not infer identity from SSH aliases or probe excluded hosts.
2. Read checkout SHA/status and canonical paths; collect symlink metadata without
   following unknown links. Verify every observed source file against the
   intended release manifest, accounting for legitimate live dirty changes.
3. Enumerate exact service/Screen/process identities with start times, parentage,
   executable digests and effective working directories. Redact secrets from
   command lines and environment; avoid recording full environments.
4. Read the relevant user's cron/systemd/launchd entries and declared input
   writers. Resolve each scope to an owner and note queued/ambiguous deliveries.
5. Record provider/tool capabilities from read-only version/help inspection;
   perform no login, enrollment, test inference or session change during inventory.
6. Validate consistent backup/restore and artifact closure separately before
   shadow migration. A copied live SQLite main file alone is not a backup.

The new dashboard should initially consume sanitized observations through a
separate read-only bridge and data root. Existing task ownership, mutation
writers and active sessions remain controlled by their current deployment
policy until a scoped migration receipt proves takeover and rollback.

## Retired dashboard collector stack

Production observation runs on the Rust observer
(`admiralty-observe --config <private-config.json>`, one cycle with `--once`)
and the `admiralty-server` read-only legacy bridge. The legacy Python/shell
collector stack and its Python regression suites were deleted after their
cases were re-established as Rust tests (`cargo test -p admiralty-observe`).
Test names below are `admiralty-observe` module paths.

| Retired file | Rust replacement | Parity tests |
| --- | --- | --- |
| `dashboard/collector.py` | `lib::cycle` (status snapshot), `projects.rs` (configured ledger adapters, major-release receipt verification, audit verdicts), `fleet.rs` (screens, process identity, footer capture), `history.rs` | `projects::legacy_parity::*`, `lib::legacy_parity_tests::*`, `fleet::legacy_parity::*` |
| `dashboard/extras.py` | `lib::attach_hierarchy`, `fleet.rs` (`global_resources`, `super_captain`), `usage.rs` (`token_efficiency`, model-usage ledger stats) | `lib::legacy_parity_tests::extras_merge_keeps_producer_records_and_actual_worker_state`, `usage::legacy_parity::*` |
| `dashboard/rollout.py` | `rollout.rs` (bounded manifest loader and live screen join) | `rollout::legacy_parity::*` |
| `dashboard/token_yield.py` | `usage.rs` (`token_yield`; private JSON state `usage-state-v1.json`, no SQLite `tokens.db`) | `usage::legacy_parity::sc186o_sql_metacharacters_in_session_identity_are_stored_verbatim`, `usage::legacy_parity::legacy_case_*` |
| `dashboard/serve.py` | `admiralty-server` legacy bridge (`/legacy/` page and bounded status JSON) | `admiralty-server` tests |
| `dashboard/run.sh` | The `admiralty-observe` loop (single-writer `collector.lock`) plus `admiraltyd`; no Screen wrapper | none (launcher only) |
| `dashboard/make_snapshot.py` | None; the React detailed view and `/legacy/` read the live snapshot | none (no retained behavior) |
| `tests/legacy/test_rollout.py`, `tests/legacy/test_rollout_live_workers.py` | `rollout::legacy_parity`, `lib::legacy_parity_tests`, `fleet::legacy_parity` | One Rust test per Python case, named after it |
| `tests/legacy/test_stage_evidence_regressions.py` | `projects::legacy_parity` | One Rust test per Python case, named after it |
| `tests/fixtures/legacy/verify.py` | `legacy_cases::tests::fixture_hygiene_replaces_retired_verify_py`, `admiralty-testkit --test legacy_import` | Pinned-source and mirror checks retired with the pinned sources |

`dashboard/globalres.py` was ported to `admiralty fleet resources`
(`crates/admiralty-fleetops`, byte-for-byte fixture tests against the original
output), and the static `dashboard/index.html` page was retired in favour of the
React **Fleet detail** view; `/legacy/` now redirects there unless an operator
still passes `--legacy-index-file`.

Old to new invocations:

| Old invocation | New invocation |
| --- | --- |
| `screen -S captains_dash dashboard/run.sh` | `admiralty-observe --config <private-config.json>` plus `admiraltyd` |
| `python3 dashboard/collector.py` (one cycle) | `admiralty-observe --config <private-config.json> --once` |
| `ADMIRALTY_ROLLOUT_MANIFEST=<path>` | `"rollout_manifest": "<absolute path>"` in the observer configuration |
| `python3 dashboard/token_yield.py` | Included in every observer cycle (`token_yield` in the snapshot) |
| `python3 -m unittest discover -s tests/legacy -p test_rollout.py` (and `test_rollout_live_workers.py`) | `cargo test -p admiralty-observe rollout::legacy_parity` |
| `python3 -m unittest discover -s tests/legacy -p test_stage_evidence_regressions.py` | `cargo test -p admiralty-observe projects::legacy_parity` |
| `python3 tests/fixtures/legacy/verify.py` | `cargo test -p admiralty-observe legacy_cases` and `cargo test -p admiralty-testkit --test legacy_import` |
