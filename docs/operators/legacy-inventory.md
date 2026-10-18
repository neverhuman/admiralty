# Legacy deployment inventory worksheet

Status: source-backed worksheet, **live deployment unverified**. Created from
local source pin `1e8ed17eff27ed3ab80dc6860fc8eda301406aee`; no SSH,
Screen input, provider call, service change, cron change or live write performed.
Source statements below are leads for an operator to verify; they are not a
deployment manifest or new authority grants. Record private addresses, resolved
paths, environment and credentials only in private inventory outside Git.

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
| History export | `history_export_cron.sh` declares own clone `~/.admiralty-history`, own lock, data-only commit path | UNKNOWN actual crontab entry, user, last run, clone HEAD, lock owner and log digest; do not execute cron script for inventory |
| Export schedule | `data/history/README.md` declares hourly minute 23 | UNKNOWN current scheduling mechanism and exact timezone/owner |
| Super captain ticker | README describes `super_captain_tick` and 30-minute checks | UNKNOWN active process, actual interval, pending input, delivery owner and rule revision |
| Other input writers | `tools/bin/kick`, `send_*.sh`, owner tools and captain prompts can send Screen input | UNKNOWN complete cron/timer/manual writer roster and overlapping scopes; disable corresponding old mutation scope only at authorized takeover |
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
