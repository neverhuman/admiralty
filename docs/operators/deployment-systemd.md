# Clean deployment with versions, systemd and readiness

This guide describes a reversible, single-writer deployment of `admiraltyd` and
`admiralty-observe` without Python or ad hoc copies. It builds on the existing
[release staging and archive](native-archive.md) and
[private install](native-install.md) commands. Only the assigned deployment
owner should run the activation steps on a production host; these tools never
start, stop or signal services on their own.

## Layout

```text
$ADMIRALTY_ROOT/                       owned, no group/other write
  versions/<name>/bin/{admiralty,admiraltyd,admiralty-observe}
  versions/<name>/share/admiralty/dashboard/
  current  -> versions/<name>          atomic relative symlink
  previous -> versions/<older>         recorded by each activation
$ADMIRALTY_STATE/                      private 0700
  native.sqlite, dashboard-token (0600)
~/.config/admiralty/admiralty.env      private 0600 unit environment
~/.config/admiralty/observe.json       private 0600 observer configuration
```

Service units only reference `$ADMIRALTY_ROOT/current`, so a version switch is
one atomic rename followed by a restart, and a rollback is the same operation in
reverse.

## One-time setup

```sh
# Private state directory and bearer token (never printed).
admiralty state init --dir "$HOME/.local/state/admiralty"

# Release root.
install -d -m 0700 "$HOME/.local/share/admiralty/versions"

# Units and environment.
install -d -m 0700 "$HOME/.config/admiralty" "$HOME/.config/systemd/user"
install -m 0600 packaging/systemd/admiralty.env.example "$HOME/.config/admiralty/admiralty.env"
install -m 0644 packaging/systemd/admiraltyd.service packaging/systemd/admiralty-observe.service \
  "$HOME/.config/systemd/user/"
"$EDITOR" "$HOME/.config/admiralty/admiralty.env"   # replace every placeholder path
systemctl --user daemon-reload
```

On a server without an interactive login, enable lingering once
(`loginctl enable-linger "$USER"`) so user services run at boot.

## Each release

1. **Build and package** the reviewed revision with the existing
   [stage and archive](native-archive.md) commands.
2. **Install** it into a new version directory; installation never overwrites:

   ```sh
   admiralty release install --archive /abs/native.tar.gz \
     --expected-archive-sha256 ARCHIVE_SHA256 --expected-manifest-sha256 MANIFEST_SHA256 \
     --destination "$ADMIRALTY_ROOT/versions/<name>"
   ```

3. **Optional evidence preflight** for release candidates (exit 0 ready for
   independent review, 1 incomplete, 2 invalid):

   ```sh
   admiralty release evidence --manifest /abs/evidence/release.json \
     --evidence-root /abs/evidence --expected-revision FULL_SOURCE_SHA
   ```

4. **Activate** and restart:

   ```sh
   admiralty release activate --root "$ADMIRALTY_ROOT" --version <name>
   systemctl --user restart admiralty-observe.service admiraltyd.service
   ```

   Activation refuses a version that is missing `bin/admiraltyd`,
   `bin/admiralty-observe` or `bin/admiralty`, is a symlink, has an unsafe name,
   or sits under a group/other-writable root. It never replaces a `current` that
   is not a symlink into `versions/`.

5. **Check readiness**, not just liveness:

   ```sh
   curl -fsS http://127.0.0.1:18890/readyz
   ```

   `/healthz` only says the process answers. `/readyz` returns 200 when the
   configured producer feed is a bounded regular file published within the last
   180 seconds and the configured dashboard assets contain `index.html`;
   otherwise 503 with the failing check names. It never reveals paths or
   contents. A ready daemon still does not prove a browser rendered the console.

6. **Roll back** if readiness or the rendered console fails:

   ```sh
   admiralty release rollback --root "$ADMIRALTY_ROOT"
   systemctl --user restart admiralty-observe.service admiraltyd.service
   admiralty release status --root "$ADMIRALTY_ROOT"
   ```

Old versions are never deleted by these commands. Remove a version directory
only after confirming it is neither `current` nor `previous`.

## Shutdown behavior

Both units stop with SIGTERM. `admiraltyd` drains open event streams; the
observer finishes and atomically publishes the cycle in progress before exiting.
`TimeoutStopSec` bounds both.

## Logs

The units send compact `logfmt` events to the journal. See
[agent brief and compact logs](agent-brief.md) for the formats, the rollup of
unchanged observer cycles and the `ADMIRALTY_ACCESS_LOG` levels.

## Limits

These commands and units are qualified by unit and integration tests on Linux;
macOS (launchd) units, container images, signing, SBOM and clean-machine
qualification remain separate release gates. See [release status](release-status.md).
