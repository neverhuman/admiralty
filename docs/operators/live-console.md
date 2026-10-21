# Live fleet console

Start the React console against an existing producer snapshot:

```sh
pnpm install --frozen-lockfile --ignore-scripts
pnpm live --status /absolute/path/status.json
```

The producer feed renders in **Fleet detail** (`#/detail`). `--index
/absolute/path/index.html` is optional and only needed to keep serving a
retained HTML page at `/legacy/`; without it `/legacy/` redirects to Fleet
detail under the same access scope.

Open `http://127.0.0.1:18891/`. This reads real producer observations. The separate
`pnpm demo` launcher uses synthetic data and defaults to port18892. Neither
launcher starts coding agents or enrolls a machine. `ADMIRALTY_LIVE_LISTEN` can
select another loopback address/port; `ADMIRALTY_STATE_DIR` selects an owned0700
preview state directory. The access token's private file path is printed; its
value is not logged. Use Session access for native/model settings. The source
preview preserves public loopback reads of the existing dashboard data only.

The console opens in the dense black fleet view. Project chips select one
project or all projects; Admiralty uses the same project component. The chart
strip scrolls horizontally and each card expands on click or keyboard activation.
It includes cost index versus all-Luna, Luna failures/first-pass, ToDo burn-up,
in-progress work by project, agent tiers, escalation funnel, CPU load, root disk,
host memory headroom, integration CI history, unmerged Git lanes, Commodore hygiene, generated/integration/main tokens,
estimated token yield/coverage, and quota usage. Expanded cards retain the source
period, timestamps and accounting notes. Missing measurements stay unknown and
break graph lines.

The hierarchy keeps bright model-family badges, reported activity dots, host
badges, task IDs and hygiene. Its project columns come from source identities,
including additional rollout projects. The token panel shows source model mix,
cached-input/output usage, credit estimates, routing flags and yield coverage.
Reported process presence is distinct from task acceptance. Estimated credits,
attribution and measured tokens keep separate labels.

On a large display, project cards sit alongside hierarchy and efficiency panels.
On smaller displays, hierarchy and efficiency come first so they remain easy to
reach. Density can be changed without discarding detail. Logs, evidence and raw
source sections remain available through their disclosure controls. The original
complete dashboard is linked at the top and served under `/legacy/`.

Pass `--models /absolute/private/models.json` to enable model settings. Add a
self-hosted provider at a lower tier number than Luna, set measured concurrency,
and choose an explicit fallback. See [model policy](models.md) and
[OpenCode SSH](opencode-ssh.md). Routing configuration alone does not qualify or
launch a native provider. Existing Screen workers remain visible through the
read-only rollout bridge while native execution is built and qualified.

The React role labels are **Admiral** for fleet coordination and **Commodore**
for each project lead. Existing producer keys and Screen IDs retain their exact
identities in observations; the display mapping does not rename live sessions.

## Fleet health

`#/health` (**Fleet health** under Observe) answers one question: what is
degrading, and what should happen next. It reads the same producer snapshot as
the detailed view plus two service endpoints, and it never acts on its own.

- **Needs attention** ranks findings as Critical, Warning or Note. Severity is
  written out and carried by shape, not only color. Sources: `/readyz`, observer
  age (stale after six refresh intervals, minimum three minutes), unreachable
  hosts, host warnings (disk, memory and I/O pressure), Git health attention or
  unknown coverage, merge commits that reached the integration branch this week
  (the repository merges rebase-only), lanes that
  trail integration past the configured divergence threshold, merged-but-clean
  worktrees, reverts and fixups. Each finding names a next step; Git findings link
  to that project's Git Flow. The first eight are shown; the rest expand.
- **Service** shows readiness and the busiest routes with request counts, 4xx/5xx
  and the p95 latency bucket, parsed from `/metrics` (session access required).
- **Hosts** show load per core, memory, swap, PSI stall percentages, blocked
  I/O processes, GPUs and the largest processes. Disk meters fill solid for space
  used today; the hatched extension is where the measured growth rate puts usage in
  24 hours. Shrinking or unmeasured growth draws no projection. Unreachable hosts
  show no numbers rather than guessed ones.
- **Git hygiene** lists each project's state, unmerged and merged branches,
  maximum lag behind/ahead of integration, dirty and conflicted checkouts,
  cleanup candidates and integration activity for the last 24 hours and 7 days.
  `≥` marks a lower bound where the observed history window ended first.
- **Log digest** fetches `/v1/logs/digest` with a token budget and minimum
  level, then copies it for an agent. `W×5` means five warnings of one template;
  `{a,b,+2}` lists varying values; `3..803` is a range; a trailing `?` marks a
  field missing from some events in the group.

## Navigation and preferences

Views are grouped as **Observe** (Fleet detail, Fleet health, Git Flow, Fleet
Atlas), **Operate** (Overview, Workplans, Coordination, Commands, Intake,
Incidents) and **Configure** (Models). **Ctrl K** (**⌘ K** on macOS) or the
**Go to…** button opens a command palette: type to filter views and
preferences, use the arrow keys, Enter to open and Esc to close; focus returns to
where it was. The light or dark theme choice is stored in this browser only
(`admiralty.theme`); the default stays dark. Views that do not read the native
fleet, including Fleet health, do not hold the native event stream open.
