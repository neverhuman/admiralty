# Fleet operator tools (`admiralty fleet`)

`admiralty fleet ...` replaces the GNU screen operator scripts that used to
live in `tools/bin/`, `tools/*.sh` and `dashboard/globalres.py`. The
implementation is the `admiralty-fleetops` crate. It keeps the scripts' output
formats, state files, cooldowns, exit codes and safety guarantees. Hard-coded
host details (paths, session lists, captain screen ids, hosts) now live in one
typed config whose defaults equal the old constants.

## Safety guarantees

- **No attach, ever.** Every `screen` invocation is checked before it runs.
  Only `screen -ls` and `screen -S NAME [-p WIN] -X|-Q COMMAND ...` are
  allowed. `-r`, `-R`, `-x`, `-d`, `-D`, `-dr` and session names that start
  with `-` are refused and never reach the process runner. Tests check this
  on every recorded call.
- Child processes (`screen`, `ssh`, `bash`) get a null stdin, captured output
  and a hard timeout. On timeout the whole process group is killed. The
  timeouts are `screen_timeout_secs` (15 s) and `probe_timeout_secs` (25 s,
  same as `globalres.py`).
- `fleet resources` is read-only on the hosts. It refuses to run if `xbabe0`
  appears in the host list (the old `assert`).
- `fleet codex-luna` is the only command that hands the terminal over. It
  replaces the process with `codex` (`exec`), so the TUI inherits stdio.

## Old → new invocations

| Old invocation | New invocation |
| --- | --- |
| `tools/bin/screen_snap SESSION [OUT]` | `admiralty fleet snap SESSION [OUT]` |
| `tools/bin/screen_send SESSION --text 'msg'` | `admiralty fleet send SESSION --text 'msg'` |
| `tools/bin/screen_send SESSION --keys KEY` | `admiralty fleet send SESSION --keys KEY` |
| `python3 tools/bin/classify_screen.py SNAP [--session NAME]` | `admiralty fleet classify SNAP [--session NAME]` |
| `tools/bin/status` | `admiralty fleet status [--session a,b,...]` |
| `tools/bin/kick [--dry-run] [--session NAME] [--once] [--inject 'text']` | `admiralty fleet kick [--dry-run] [--session NAME] [--once] [--inject 'text']` |
| `tools/bin/kick --inject SESSION 'text'` | `admiralty fleet kick --inject SESSION 'text'` |
| `tools/owner_peek.sh SESSION [LINES]` | `admiralty fleet peek SESSION [LINES]` |
| `tools/peek_all.sh` | `admiralty fleet peek --all [--sessions a,b,...]` |
| `tools/owner_send.sh SCREEN FILE` | `admiralty fleet paste SCREEN FILE` |
| `tools/send_add1.sh`, `send_add2.sh`, `send_add2b.sh`, `send_compute.sh`, `send_notice2.sh`, `send_sc.sh` | `admiralty fleet deliver --plan PLAN.json` (see [Delivery plans](#delivery-plans)) |
| `tools/mr_probe.sh [OUTDIR]` | `admiralty fleet probe-models [OUTDIR]` |
| `tools/codex-luna.sh DIR` | `admiralty fleet codex-luna DIR` |
| `python3 dashboard/globalres.py [--quiet]` | `admiralty fleet resources [--quiet]` |
| `tools/sweep.sh` | `admiralty fleet sweep` (see [Host sweep](#host-sweep)) |

## Configuration

`FleetConfig::default()` reproduces the old constants. You can override them
in three places. Later sources win.

1. `admiralty fleet --config fleet.json <command>`. This is a partial JSON
   object; unknown keys are rejected.
2. Environment variables: `ADMIRALTY_KICKER_BASE` (kicker base directory) and
   `CDASH_RUNTIME`, which `globalres.py` already honored.
3. Flags: `admiralty fleet --kicker-base DIR <command>`, `status --session`
   and `peek --all --sessions`.

| Key | Default (old constant) |
| --- | --- |
| `kicker_base` | `/home/ubuntu/xbabe2_kicker` (`BASE` in `tools/bin/*`). State is in `state/`, snapshots in `state/snaps/`, the kick log in `logs/kick.log` and peek-all hardcopies in `hc_<s>.txt` |
| `monitored_sessions` | `grok_red jopedime_codex_cleanup jopeDime_cleanup_claude jopedime_codex_training jankurai` |
| `peek_all_sessions` | the 15-session list of `peek_all.sh` |
| `tmp_dir` | `/tmp` (`peek_<s>`, snapshot temp files) |
| `probe_models_outdir` / `probe_models_skip` | `/tmp/mr_probe` / `captains_dash`, `disk_janitor` |
| `dashboard_runtime` | `/home/ubuntu/xbabe2_kicker/captains-dashboard` (`CDASH_RUNTIME`) |
| `resource_hosts` / `resource_local_host` | `xbabe1 xbabe2 xbabe3` / `xbabe2` (probed with `bash -c`) |
| `ssh_user`, `ssh_connect_timeout_secs`, `ssh_server_alive_interval_secs` | `ubuntu`, `6`, `5` |
| `pr_cooldown_secs` | `900` |
| `codex_model` / `codex_reasoning_effort` | `gpt-6-luna` / `medium` |
| `captains` | `joil`, `dope`, `jopedime` and `jarambit`, with the screen ids, coordination logs and the joil lock from `send_add2.sh` |
| `proc_root` | `/proc` |

## Commands

### `snap`

`fleet snap` runs `hardcopy -h` on window 0 and falls back to a visible-only
hardcopy if that fails. It waits up to 10 × 0.1 s for the temp file to fill,
strips NUL bytes and applies the old `iconv -c || cp` cleanup: invalid UTF-8
is dropped, unless the file ends in a truncated sequence, in which case the
bytes are copied unchanged (glibc behavior). It then makes sure the file ends
with a newline and prints the path. Errors and exit code 1 are the same as
before.

### `send`

`fleet send --text` stuffs the text, waits 50 ms, then sends CR.

`fleet send --keys` accepts these case-insensitive names:

| Key name | Bytes sent |
| --- | --- |
| `enter`, `cr`, `return`, `\r` | CR |
| `esc`, `escape` | ESC |
| `y`, `a`, `n`, `2` | the letter or digit |
| `tab` | TAB |
| `shifttab`, `shift-tab`, `shift_tab`, `backtab`, `bypass` | `ESC [ Z` |
| `altup`, `alt-up`, `alt_up` | `ESC [ A` |
| `ctrlc`, `ctrlo`, `ctrlt`, `ctrlu` | `^C`, `^O`, `^T`, `^U` (the `ctrl-x` and `ctrl_x` spellings also work) |
| `always`, `yes` | the word, then 50 ms, then CR |

Any other value is expanded like bash `printf %b`, for example `'y\r'` or
`'\033[Z'`. The command prints `sent to SESSION (text|keys)`.

### `classify`

`fleet classify` prints the same JSON as `classify_screen.py`: the keys
`session`, `kind`, `state`, `confidence`, `signals`, `approval_hint`,
`pr_refs`, `summary` and `tail`, in that order, indented by 2, with non-ASCII
characters left unescaped. If the snapshot file is missing it prints
`{"error": "missing snap: PATH", "session": ...}` and exits 1. The test
fixtures in `crates/admiralty-fleetops/tests/fixtures/classify/` were
captured from the Python script and are checked byte for byte.

### `status` and `kick`

These print the same report text, `state/<session>.json` files (written with
`ensure_ascii` like Python), `logs/kick.log` lines, nudge texts, approval key
sequences and 15-minute cooldown as before. A live kick still exits 75 while
`state/DISK_HOLD` exists. `kick` keeps the script's own argument parser:
`--inject X Y` means SESSION TEXT when another argument follows and `X` has no
space and does not start with `Continue`.

The `(N chars)` count and the `[dry-run]` prefixes follow bash. In a UTF-8
locale they count characters; in the POSIX locale, for example under cron,
they count bytes. The kick parity fixtures in `tests/fixtures/kick/` were
captured from the original script against a fake `screen`.

### `peek` and `paste`

`fleet peek SESSION [LINES]` hardcopies the session to `/tmp/peek_SESSION`
and prints the last LINES (default 25) printable, non-blank lines.

`fleet peek --all` hardcopies each session to `<kicker_base>/hc_<s>.txt`,
waits 0.5 s, prints `=================== <s>` and then the last 9 lines,
cut to 180 columns.

`fleet paste SCREEN FILE` runs `readbuf FILE`, `paste .`, waits 1 s and sends
CR. As before, `screen` resolves a relative FILE against its own working
directory.

### `probe-models`

`fleet probe-models` prints the same `%-40s | TUI: %-28s | args: %s` table as
before. The TUI model is the last GPT, Gemini or Claude-family banner in the
hardcopy. The args column lists the unique, sorted `-m`, `--model`, `model=`,
`model_reasoning_effort=` and `--effort` flags found in the screen's process
tree. It reads `/proc/<pid>/stat` and `/proc/<pid>/cmdline` recursively
instead of running `pstree`, so arguments are no longer cut at 132 columns.

### `codex-luna`

`fleet codex-luna DIR` checks its argument in the same order as before. It
exits 64 for a wrong argument count, an argument that starts with `-`, a
missing directory or a directory that cannot be resolved. It exits 127 when
`codex` is not on `PATH`. Otherwise it execs
`codex -m gpt-6-luna -c model_reasoning_effort=medium -C <canonical DIR>`
from inside that directory. The exit status of codex comes through unchanged.

### `resources`

`fleet resources` runs the same `PROBE` script, locally on `xbabe2` and on the
other hosts over
`ssh -n -o BatchMode=yes -o ConnectTimeout=6 -o ServerAliveInterval=5 ubuntu@HOST`.
The parsing, the growth rate (samples 20 min to 3 h old, kept for 6 h), the
hours-to-full and level thresholds, the warning texts and the printed table
are unchanged. It writes `global_resources.json` (indent 1) and
`.res_history.json` (compact, original key order) under `CDASH_RUNTIME`
through a `.tmp` file and a rename. The outputs are checked byte for byte
against fixtures captured from `globalres.py`. `xbabe0` is never probed.

## Delivery plans

`admiralty fleet deliver --plan PLAN.json` replaces the one-off delivery
scripts. The six historical scripts ship as plans in
`crates/admiralty-fleetops/plans/` (`send_add1.json`, `send_add2.json`,
`send_add2b.json`, `send_compute.json`, `send_notice2.json`, `send_sc.json`).
The first five are checked against the output, screen-input sequence, sleeps
and resulting files of the original script run in a sandbox; `send_sc.json`
has unit tests for the idle and the busy outcome.

Steps run in this order:

1. `pre_log`: append a log entry for every target.
2. `prepare`: write message files.
3. `reset_sent_markers`.
4. Up to `passes` passes over the targets. Each pass skips targets that are
   already sent, runs the idle gate on the rest and pastes when idle. After a
   pass that still had pending targets, wait `interval_secs`.
5. `fallback`: append a log entry for every target that was never sent.
6. `unsent_echo`: print a line for every target that was never sent.
7. `list_sent_markers`.
8. `done_echo`.

| Field | Meaning (historical source) |
| --- | --- |
| `passes`, `interval_secs` | `for i in $(seq 1 N) ... sleep 15` |
| `idle.hardcopy_prefix` | hardcopy path = prefix + screen id (`/tmp/hc_a2_$SID`) |
| `idle.require_shortcuts_footer` | `true` (all five scripts) requires `? for shortcuts`. `false` uses the prompt-only check |
| `idle.fresh_hardcopy` | `true` = `rm -f` before the hardcopy and require `[ -s ]` (`send_compute.sh`, `send_notice2.sh`). `false` = reuse the possibly stale file (`send_add1/2/2b.sh`) |
| `idle.settle_ms` | `sleep 0.3` / `sleep 0.4` after the hardcopy |
| `paste_settle_ms` | `sleep 1` between `paste .` and CR |
| `message_file`, `sent_marker` | plan defaults, with `{key}` and `{screen}` placeholders. A target can override them |
| `sent_echo` | the line printed after a paste |
| `prepare[]` | `printf "%s" TEXT > f` (`{"text": ...}`) and `cat a > b` (`{"file": ...}`), concatenated |
| `pre_log`, `fallback` | `template` is appended plus a newline (`printf '%s\n'`). `message_file` is the source of `{message}` (default: the target's message file). `logged_marker` is touched afterwards. `echo` is printed. `lock_wait_secs` is the `flock -w` wait (60) |
| `targets[]` | `key`, `screen`, `message_file`, `sent_marker`, `coord_log`, `lock`. If `screen`, `coord_log` or `lock` is missing, the value comes from the config's `captains[key]`. `"lock": ""` appends without a lock |

Both idle checks also require that there is no `esc to cancel` and that the
last line starting with `>` is exactly `>` once spaces are removed. All five
historical scripts required the `? for shortcuts` footer. The variants they
actually differ in are the stale and fresh hardcopy checks.

Template placeholders: `{key}`, `{screen}`, `{message_file}`,
`{message_basename}`, `{coord_log}`, `{utc_iso}` (`date -u +%Y-%m-%dT%H:%M:%SZ`),
`{utc_hms}` (`date -u +%T`), `{denver_hms}` (`TZ=America/Denver date +%H:%M:%S`)
and, in log templates only, `{message}`. `{message}` is the file content with
NUL bytes and trailing newlines removed, like `$(cat f)`. Use `{{` and `}}`
for literal braces. Unknown placeholders and unknown fields are rejected
before anything runs.

Log appends take an exclusive `flock` on the target's `lock` file. The file
is created if it is missing, like `flock(1)`. The lock is polled every 100 ms
for up to `lock_wait_secs`. If the lock never comes free, the pointer is
**not** appended. A warning is printed, and the logged marker and echo still
happen, as in the scripts.

### Mapping of the historical scripts

| Script | Plan |
| --- | --- |
| `send_add1.sh` | `plans/send_add1.json`. The four `prepare` texts are written first, the screens are `captain_<key>`, there are 45 passes and no fallback, `list_sent_markers` is on, and the echo has no `MDT` suffix |
| `send_add2.sh` | `plans/send_add2.json` (below) |
| `send_add2b.sh` | `plans/send_add2b.json`: joil and jarambit, the same markers as add2, no fallback, `done_echo: DONE2` |
| `send_compute.sh` | `plans/send_compute.json`: a fresh hardcopy with 400 ms settle, 60 passes, a fallback with the `COMPUTE NOTICE` header, and `/tmp/logged_compute_<key>` |
| `send_notice2.sh` | `plans/send_notice2.json` (below) |
| `send_sc.sh` | `plans/send_sc.json`: the super captain only, a fresh hardcopy with 500 ms settle, 120 passes x 10 s, `reset_sent_markers` so every run pastes once, and `unsent_echo: SC busy 20 min; not pasted` |

### Example: `send_add2.sh`

```json
{
  "description": "Addendum 2 (2026-10-17 9:24 PM MDT): paste kick_add2_<key>.txt into the 4 captain screens only when the Claude prompt is idle; 40 passes x 15 s (~10 min); otherwise append a pointer to the captain's coordination log (joil under flock) (was tools/send_add2.sh).",
  "passes": 40,
  "interval_secs": 15,
  "idle": {
    "hardcopy_prefix": "/tmp/hc_a2_",
    "require_shortcuts_footer": true,
    "fresh_hardcopy": false,
    "settle_ms": 300
  },
  "paste_settle_ms": 1000,
  "message_file": "/home/ubuntu/xbabe2_kicker/captains-20261017/kick_add2_{key}.txt",
  "sent_marker": "/tmp/sent_add2_{key}",
  "sent_echo": "sent {screen} {message_basename} at {denver_hms} MDT",
  "fallback": {
    "template": "\n## {utc_iso} - Grok Bot to {screen}: ADDENDUM 2 (Jepson, 9:24 PM MDT) waiting for you\n\nCaptain: your prompt stayed busy for 10 minutes, so this pointer is going in the log instead. {message}\n",
    "echo": "busy 10m: pointer appended to {coord_log} for {key} at {denver_hms} MDT",
    "logged_marker": "/tmp/logged_add2_{key}",
    "lock_wait_secs": 60
  },
  "done_echo": "DONE",
  "targets": [
    {
      "key": "joil",
      "screen": "2708475.captain_joil",
      "coord_log": "/home/ubuntu/joil/COORD.md",
      "lock": "/home/ubuntu/joil/.private/v5-workers/coord.lock"
    },
    {
      "key": "dope",
      "screen": "2716151.captain_dope",
      "coord_log": "/home/ubuntu/dope/COMMS.md",
      "lock": ""
    },
    {
      "key": "jopedime",
      "screen": "2724936.captain_jopedime",
      "coord_log": "/home/ubuntu/JopeDime/COMMS.md",
      "lock": ""
    },
    {
      "key": "jarambit",
      "screen": "2740538.captain_jarambit",
      "coord_log": "/home/ubuntu/jarambit/COMMS.md",
      "lock": ""
    }
  ]
}
```

### Example: `send_notice2.sh`

`pre_log` writes the notice into every coordination log under its lock first.
`prepare` then composes the paste file from the notice and the compute
pointer, the old sent markers are removed, and the idle-gated paste is
retried for 25 minutes.

```json
{
  "description": "Proxy + hygiene notice (2026-10-17 10:01/10:02 PM MDT): append the notice to every coordination log now (under flock), then paste notice + compute pointer into each captain only when idle (fresh hardcopy); 100 passes x 15 s (~25 min) (was tools/send_notice2.sh).",
  "passes": 100,
  "interval_secs": 15,
  "idle": {
    "hardcopy_prefix": "/tmp/hc_n2_",
    "require_shortcuts_footer": true,
    "fresh_hardcopy": true,
    "settle_ms": 400
  },
  "paste_settle_ms": 1000,
  "message_file": "/home/ubuntu/xbabe2_kicker/captains-20261017/captain_notice2_paste.txt",
  "sent_marker": "/tmp/sent_n2_{key}",
  "sent_echo": "pasted {key} at {denver_hms} MDT",
  "pre_log": {
    "template": "\n## {utc_iso} - Grok Bot to {screen}: PROXY + HYGIENE ORDERS (Jepson 10:01/10:02 PM MDT)\n\n{message}\n",
    "message_file": "/home/ubuntu/xbabe2_kicker/captains-20261017/captain_notice2.txt",
    "echo": "logged {key} {utc_hms}Z",
    "lock_wait_secs": 60
  },
  "prepare": [
    {
      "path": "/home/ubuntu/xbabe2_kicker/captains-20261017/captain_notice2_paste.txt",
      "parts": [
        {
          "file": "/home/ubuntu/xbabe2_kicker/captains-20261017/captain_notice2.txt"
        },
        {
          "text": " (Also, if you have not yet: the COMPUTE notice in your coord log - xbabe1/xbabe2/xbabe3 available, never xbabe0 - see COMMON.md \"Compute\"; post COMPUTE ACK.)"
        }
      ]
    }
  ],
  "reset_sent_markers": true,
  "done_echo": "DONE",
  "targets": [
    {
      "key": "joil",
      "screen": "2708475.captain_joil",
      "coord_log": "/home/ubuntu/joil/COORD.md",
      "lock": "/home/ubuntu/joil/.private/v5-workers/coord.lock"
    },
    {
      "key": "dope",
      "screen": "2716151.captain_dope",
      "coord_log": "/home/ubuntu/dope/COMMS.md",
      "lock": "/home/ubuntu/dope/COMMS.md.lock"
    },
    {
      "key": "jopedime",
      "screen": "2724936.captain_jopedime",
      "coord_log": "/home/ubuntu/JopeDime/COMMS.md",
      "lock": "/home/ubuntu/JopeDime/COMMS.md.lock"
    },
    {
      "key": "jarambit",
      "screen": "2740538.captain_jarambit",
      "coord_log": "/home/ubuntu/jarambit/COMMS.md",
      "lock": "/home/ubuntu/jarambit/COMMS.md.lock"
    }
  ]
}
```

## Host sweep

`admiralty fleet sweep` replaces `tools/sweep.sh`, the super captain's one-shot
survey of the fleet host. It prints the same sections in the same order:

1. `== REPO URL branch=BRANCH` for each repository in `sweep.repos`.
2. For each repository's GitHub slug: `##### SLUG PRs`, `##### SLUG main runs`
   and `##### SLUG merged recent`, with the script's line formats
   (`#N author draft=… MERGESTATE REVIEW head8 upd=… title70`).
3. `##### GPUs`: one line per `sweep.gpu_hosts` host (GPU utilisation and
   memory, `avail X of Y` GB, compute processes) and one for local processes.
4. `##### dl_status`: size, modification time and the first 1,500 bytes.
5. `##### jsonl mtimes (codex/claude)`: the 8 newest transcripts written in
   the last 120 minutes under each `sweep.transcript_dirs` root.
6. `##### tmp/sibling worktrees`, then `git worktree list` without agent
   worktrees and `count jd wt: N`.
7. `##### kicker cron`, guard/storage timers, scratch disks and `uptime`.

Nothing is written. Every program runs directly with a bounded timeout and a
null stdin; there is no shell, `jq`, `awk` or `sed`. The `gh` JSON is parsed and
formatted in Rust, `free -g` is parsed in Rust, and an unreachable GPU host is
contacted once (ssh exit 255) instead of three times. The `sweep` object of the
config holds every host detail (`home`, `repos`, `gh_bin`/`gh_role`,
`gpu_hosts`, `dl_status_file`, `transcript_dirs`, `worktree_patterns`,
`cron_match`, `timer_matches`, `df_paths`, `command_timeout_secs`).

## Behavior differences from the scripts

- The usage and error messages name `admiralty fleet <command>` instead of
  the script name.
- Clap enforces `send --text|--keys`, so passing both is an error. Before,
  the last one won.
- `reset_sent_markers` removes only the plan's own markers. `send_notice2.sh`
  ran `rm -f /tmp/sent_n2_*`.
- `list_sent_markers` prints the plan's existing markers sorted bytewise.
  Before, `ls /tmp/sent_add1*` used locale collation.
- If the lock times out, a warning is now printed to stderr (`flock -w` was
  silent).
- `%Z` in local timestamps prints `UTC` when the offset is zero and a numeric
  zone otherwise. `{denver_hms}` uses the US DST rules for America/Denver.
- `resources` decodes probe output with lossy UTF-8. Python treated invalid
  UTF-8 as an unreachable host. History entries that are not `[ts, used]`
  lists are dropped instead of crashing the run.
- `probe-models` reads `/proc` instead of running `pstree`, so it is not cut
  at 132 columns, and it prints nothing when OUTDIR has no snapshots.
- `send_sc.json` hardcopies to `/tmp/hc_sc_<screen id>` instead of
  `/tmp/hc_sc_o` and touches `/tmp/sent_sc_orders` after pasting.
- `sweep` prints `dl_status` as `PATH SIZE bytes, modified HH:MM` instead of
  an `ls -la` line, sorts transcripts by modification time instead of by the
  `HH:MM` text (so a window spanning midnight is in order), queries each GPU
  host with three plain ssh commands instead of one remote shell pipeline, and
  stops after the first when the host is unreachable.
