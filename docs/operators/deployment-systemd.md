# Clean deployment with systemd

One path from source to a running, health-checked service, with automatic
rollback. Everything is Rust plus the built dashboard: no Python, no Node at
runtime, no Screen sessions and no hand-copied files.

```text
build ──► stage ──► archive ──► install ──► activate ──► restart ──► ready?
 cargo     admiralty release …            current → V    systemd     /readyz
                                                            │ no
                                                            ▼
                                                    rollback ──► restart
```

This page covers the read-only fleet console (`admiraltyd`) and the observer
(`admiralty-observe`). It does not qualify native coding execution, whose
fencing and drain rules remain in [upgrades](upgrades.md). Only the assigned
deployment owner runs these steps; see [deployment handoff](deployment.md).

**Which guide applies.** The current production host is deployed with the
runbooks in [`deploy/`](../../deploy/README.md): its existing controller, the
two deployment locks, and asset-only publication for compatible UI changes.
This page is the path for a new host, or for migrating a host to systemd
units once its owner decides to. `rollout.sh` does not take the production
deployment locks and restarts services, so do not run it on a host that the
`deploy/` runbooks still govern. The status words (merged, built, staged,
serving, browser verified, deployed) and the verification steps in
[`deploy/VERIFY.md`](../../deploy/VERIFY.md) apply to both paths.

## Layout

| Path | Owner / mode | Contents |
| --- | --- | --- |
| `/srv/admiralty/releases/<version>/` | `admiralty`, 0700 install | `bin/admiraltyd`, `bin/admiralty`, `bin/admiralty-observe`, `share/admiralty/dashboard/`, `MANIFEST.json` |
| `/srv/admiralty/releases/current` | symlink | Active version, switched atomically by `admiralty release activate` |
| `/srv/admiralty/releases/activation.jsonl` | 0600 | Append-only activation and rollback journal |
| `/srv/admiralty/private/` | `admiralty`, 0700 | `native.sqlite`, `dashboard-token` (0600) and private configuration |
| `/srv/admiralty/observe/` | `admiralty`, 0700 | Observer `config.json`, `status.json`, `history.jsonl`, samples |

`admiraltyd` serves `share/admiralty/dashboard` from its own bundle when
`--assets` is omitted, so the units below need no asset flag and a rollback
restores the matching dashboard automatically.

## 1. Build and install a version

```sh
cargo build --release --locked -p admiralty-cli -p admiralty-server -p admiralty-observe
pnpm install --frozen-lockfile && pnpm --dir apps/dashboard build
# Stage, archive and install exactly as in native-archive.md / native-install.md:
admiralty release stage   --input-root … --output … --allowlist … --version 0.2.0 …
admiralty release archive --staged-dir … --manifest … --expected-manifest-sha256 … --output …
admiralty release install --archive … --expected-archive-sha256 … \
  --expected-manifest-sha256 … --destination /srv/admiralty/releases/0.2.0
```

Optional rehearsal of the same flow in a private evidence directory:
`admiralty release qualify --assets "$PWD/apps/dashboard/dist" --repository "$PWD" --output /owned/evidence/NEW --source-commit SHA --target TRIPLE`.

## 2. First-time host setup

```sh
useradd --system --home /srv/admiralty --shell /usr/sbin/nologin admiralty
install -d -o admiralty -g admiralty -m 0700 /srv/admiralty/private /srv/admiralty/observe
sudo -u admiralty /srv/admiralty/releases/0.2.0/bin/admiralty preview-state --dir /srv/admiralty/private
cp deploy/systemd/admiraltyd.service deploy/systemd/admiralty-observe.service /etc/systemd/system/
systemctl daemon-reload
```

`admiralty preview-state` creates the 0700 directory and a random 0600
`dashboard-token` (the value is never printed). Write the observer's
`/srv/admiralty/observe/config.json` from `configs/examples/observe.json`.
Put optional `admiraltyd` flags (`--legacy-status-file`, `--native-config`,
`--operator-chat-config`, …) in a drop-in with `systemctl edit admiraltyd`.

Both units run as an unprivileged user with an empty capability set, a
read-only root filesystem (`ProtectSystem=strict`) and write access only to
their own state directory. They pass `systemd-analyze verify`.

## 3. Roll out

```sh
scripts/deploy/rollout.sh --releases-root /srv/admiralty/releases --version 0.2.0
```

The script activates the version, restarts both units and waits for
`GET /readyz` (`admiralty ready`, 60 s by default). If the new version is not
ready, it rolls back to the previous version, restarts again and exits 1.
`--dry-run` prints the plan; `--user` targets user units; `--units` and
`--listen` select other units or listeners.

The same steps by hand:

```sh
admiralty release activate --releases-root /srv/admiralty/releases --version 0.2.0
systemctl restart admiraltyd admiralty-observe
admiralty ready --listen 127.0.0.1:18891 --timeout-seconds 60
admiralty release rollback --releases-root /srv/admiralty/releases   # undo the last switch
```

`activate` refuses roots writable by group/other, symlinked or incomplete
version directories, and a `current` path that is not a symlink. Each switch
is one atomic `rename(2)` and one journal line; `rollback` undoes the latest
switch. Neither command starts or stops services.

## 4. Observe

| Endpoint | Auth | Purpose |
| --- | --- | --- |
| `GET /healthz` | none | Liveness: the process answers |
| `GET /readyz` | none | Readiness: Store writer answers a bounded query within 2 s and the service is not draining (503 otherwise). No task data |
| `GET /metrics` | bearer (demo: none) | Prometheus text: HTTP counters and latency histograms by route template, SSE streams, read slots, log volume, uptime |
| `GET /v1/logs/digest?budget_tokens=600&min_level=warn&since_s=3600` | bearer | Token-budgeted digest of recent events for agents |

Logs go to the journal in compact `ALOG/1` lines (`ADMIRALTY_LOG_FORMAT=json`
for shippers). Successful requests are summarized once a minute
(`http.summary`); failures, denials, throttling and slow responses are logged
individually and identical ones coalesce into `x=N` lines. To hand an agent the
last hour without flooding its context:

```sh
journalctl -u admiraltyd -u admiralty-observe --since -1h -o cat | admiralty logs digest --budget-tokens 600
```

`ADMIRALTY_LOG=warn,srv.http=debug` raises one target without the rest;
`ADMIRALTY_LOG=off` silences output but keeps the in-memory digest.

## Limits

Each version serves only its own dashboard assets. A tab opened before a
switch keeps the old entry, and its next lazily loaded view (for example Fleet
health or Git Flow) can fail to load until the tab is reloaded. Where open tabs
must survive a switch, keep one shared, append-only asset directory, pass it
with `--assets` in a unit drop-in, and publish into it as
[`deploy/UI_ONLY.md`](../../deploy/UI_ONLY.md) describes, never deleting old
hashed files.

Activation switches a link and restarting reloads binaries; it is not the
native-execution upgrade protocol (admission fencing, session drain and effect
reconciliation), which stays unqualified. Signing, publication, clean-machine
installation and multi-host rollout remain release gates in
[release status](release-status.md).
