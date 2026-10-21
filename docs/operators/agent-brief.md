# Agent brief and compact logs

Captains, watchers and other agents should not read a multi-megabyte
`status.json` to decide whether anything needs judgment. Admiralty now offers a
token-budgeted **brief** and compact **log events** built for that purpose. Both
are read-only, use no model turns and grant no execution or acceptance.

## The brief

The brief turns one observer snapshot into a few hundred bytes of stable,
line-oriented facts, most urgent first:

```text
# fleet gen=2026-10-21T07:12:00Z age=9s host=xbabe2 cycle_ms=1834 turns=0
! git dope attention diverged_branch,merge_conflicts fp=3f2a91c4
! res amber xbabe2 disk / 91.2% used, ~11.4 h to full at 2.10 GB/h
! host xbabe3 unreachable
proj dope done=61/65 ip=0 blk=4 pr=#212:open ci=failure hyg=C50 flags=1
proj joil done=160/160 ip=0 blk=0 hyg=30-80(partial)
git dope br=14 merged=9 ahead=3 div=2 old30d=1 co=6 dirty=2 unk=0 int24h=4 tip=3h
host xbabe2 load=55.1% mem_avail=61% root=91.2% full_in=11.4h gpu=43%
tok 15m=1.21M 60m=4.87M+ quota=41% burn=2.1%/h cost_idx=0.62
agents n=34 models=gpt-6-luna:12,unknown:12,gemini-4-argon:6
```

Read it as follows:

| Prefix | Meaning |
| --- | --- |
| `#` | Header: producer time, source age, host, observer cycle time and model turns. `STALE` is appended after three missed cycles (at least 90 s). |
| `!` | Something actionable or degraded: attention/unknown Git health (with issue codes and a short evidence fingerprint), red/amber resource warnings, unreachable hosts and producer errors. |
| `proj` | Delivery facts per project: done/total, in progress, blocked, the integration PR and its CI rollup, the reported hygiene grade (or partial interval) and source flag count. |
| `git` | Descriptive Git metrics: branches compared, merged, ahead only, diverged, unmerged tips older than 30 days, worktrees, dirty and unknown worktrees, integration first-parent commits in 24 h and the integration tip age. `partial` means the scan was incomplete. |
| `host`, `tok`, `agents` | Resource pressure, measured token windows and quota, and the observed model mix. |

Rules that keep it trustworthy:

- Values are copied or arithmetically summarized from the snapshot; nothing is
  inferred. A missing or null fact renders as `?`, never as zero. A count with
  `+` is a lower bound that may grow when unobserved history arrives.
- Lines are sorted by urgency, then by configured project order. When the byte
  budget runs out, the brief stops and ends with `… omitted=N budget=B`; it never
  drops an urgent line to keep a routine one.
- The same snapshot, options and clock always produce byte-identical output, so
  an unchanged fleet is cheap to diff or cache.

Get a brief from any of three places:

```sh
# CLI, from a status.json on disk (bounded 32 MiB regular file, no symlinks)
admiralty brief --status /path/to/status.json --budget 2000
admiralty brief --status /path/to/status.json --project dope --format json

# HTTP, from the running daemon (same read scope as /v1/legacy)
curl -fsS 'http://127.0.0.1:18890/v1/brief?budget=2000'
curl -fsS 'http://127.0.0.1:18890/v1/brief?project=dope&format=json'
```

The budget is UTF-8 bytes (about four bytes per token) between 160 and 65,536.
`format=json` returns `{text, lines, omitted_lines, bytes, source_generated_utc,
source_age_s, stale}`. The HTTP response also carries `x-admiralty-brief-omitted`.
The dashboard's **Fleet health** view shows the live brief with a copy button.

## Compact log events

Both long-running binaries now emit one event per line with a stable dotted
event name and short keys. The default `logfmt` form is readable in a terminal
and costs far fewer tokens than prose:

```text
2026-10-21T08:14:37Z info observe.cycle projects=5 ms=1834 fleet_ms=420 projects_ms=300 git_ms=900 git_health_ms=3 usage_ms=150 history_ms=20 git_attention=[] git_unknown=joil git_alerts=0 res_red=0 res_amber=1 source_errors=[]
2026-10-21T08:24:37Z info observe.cycle projects=5 ms=1790 ... repeat=40 since=2026-10-21T08:14:52Z
2026-10-21T08:25:02Z warn observe.cycle projects=5 ms=1811 ... git_attention=dope git_alerts=1 ...
2026-10-21T08:25:03.120Z warn http.request method=GET path=/v1/legacy/detail status=200 ms=1532 cache=miss
```

### Observer

`admiralty-observe` writes one `observe.cycle` event per cycle (stdout) or
`observe.cycle_failed` (stderr). The level is `warn` when a repository needs
attention, a resource is red or a source failed; otherwise `info`.

| Flag | Default | Effect |
| --- | --- | --- |
| `--log-format logfmt\|json` | `logfmt` | `json` writes the same fields as NDJSON (`ts`, `lvl`, `ev`, …). |
| `--log-rollup-seconds N` | `600` | Identical consecutive outcomes (durations excluded) are written once, then summarized every `N` seconds as `repeat=N since=…`. Any change is written immediately. `0` writes every cycle. Repeats are counted, never dropped. |

Stage durations are also published in `status.json` as
`collector.stages_ms` (`fleet`, `projects`, `git`, `git_health`, `usage`,
`history`). Fleet/project collection and Git scans run concurrently, so stage
durations overlap and can add up to more than `duration_ms`.

### Daemon

`admiraltyd` logs `http.request` events on stderr according to
`ADMIRALTY_ACCESS_LOG`:

| Value | Logged |
| --- | --- |
| `problems` (default) | 5xx responses (`error`) and requests taking 1 s or more (`warn`). |
| `all` | Every request (`info` unless slow or failed). |
| `off` | Nothing. |

Only method, path, status, duration and the snapshot cache state (`hit`/`miss`)
are recorded — never query strings, headers, bodies or tokens. Values longer
than 240 characters are cut with `…`, and control characters are escaped, so an
event is always exactly one line.

### Reading logs as an agent

```sh
journalctl --user -u admiralty-observe -o cat --since -1h | grep -v ' info '
journalctl --user -u admiraltyd -o cat --since today
```

Filter on `warn`/`error` first; a quiet healthy fleet produces one observer line
per rollup window. Use the brief for current state and logs for what changed.
