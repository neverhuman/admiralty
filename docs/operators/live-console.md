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
integration CI history, Commodore hygiene, generated/integration/main tokens,
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
