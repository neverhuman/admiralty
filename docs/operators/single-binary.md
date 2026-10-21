# Single-binary deployment

`admiralty` can be the whole deployment: one executable that serves the console
it was built with, runs the observer and carries every operator tool. Nothing on
the host needs Node, pnpm or Python at runtime.

## Build

```sh
pnpm install --frozen-lockfile --ignore-scripts
cargo xtask dist            # or: cargo xtask dist --out /srv/admiralty/build/REV
```

`cargo xtask dist`:

1. builds the dashboard (`NODE_ENV=production pnpm --dir apps/dashboard build`);
2. builds `admiralty-cli`, `admiralty-server` and `admiralty-observe` in release
   mode with `ADMIRALTY_EMBED_DASHBOARD=<absolute dist>`;
3. copies `bin/admiralty`, `bin/admiraltyd` and `bin/admiralty-observe` into a
   new output directory (default `target/dist/admiralty-<version>-<os>-<arch>`)
   with `SHA256SUMS` and `BUILD.json` (source commit, dirty flag, rustc,
   embedded dashboard file count and tree SHA-256).

It never installs, activates, signs or publishes. Use the existing
[`admiralty release stage|archive|install`](native-archive.md) commands for a
private version directory, and the runbooks in [`deploy/`](../../deploy/README.md)
for the production host.

To embed by hand: `ADMIRALTY_EMBED_DASHBOARD=$PWD/apps/dashboard/dist cargo build --release -p admiralty-cli`.
The build script (`crates/admiralty-server/build.rs`) refuses symlinks, special
files and non-portable names, skips dot-files (such as the e2e harness's build
identity), and bounds the payload to 1,024 files, 16 MiB per file and 64 MiB in
total. Ordinary `cargo build`/`cargo test` without the variable embed nothing and
behave exactly as before.

## Run

```sh
admiralty serve --database /srv/admiralty/native.sqlite \
  --token-file /srv/admiralty/private/token \
  --legacy-status-file /srv/admiralty/observe/status.json \
  --listen 127.0.0.1:18891

admiralty observe --config /srv/admiralty/private/observe.json
```

`admiralty serve` accepts exactly the `admiraltyd` flags; `admiralty observe`
accepts the `admiralty-observe` flags. The standalone executables remain for
existing systemd units and the e2e harness.

At startup an embedded build prints one extra line before the usual listener
line. For example (a 21 October 2026 build; the numbers change with the
dashboard):

```text
Admiralty dashboard: embedded 18 files, 3018300 bytes, tree sha256 444bd7210d929458ff8f3944448cbf02fa6d917905d28e56a91c8ad6221341e9
Admiralty listening on http://127.0.0.1:18891 (read-only; native execution disabled)
```

On Linux x86_64 with two build jobs, `cargo xtask dist` took 8 min 46 s for a cold
release build (sources already downloaded). The stripped, thin-LTO `admiralty`
was 31.9 MB with the console embedded (`admiraltyd` 16.5 MB, `admiralty-observe` 3.6 MB).

`admiralty doctor` reports the same summary under `embedded_dashboard` (or
`null` when nothing is embedded).

### Which assets are served

| Situation | Served UI |
| --- | --- |
| `--assets DIR` given | `DIR`, read per request (the UI-only overlay procedure in `deploy/UI_ONLY.md` is unchanged) |
| no `--assets`, embedded build | the dashboard compiled into the binary |
| no `--assets`, plain build | the API landing page, as before |

Embedded responses come from memory: text assets are gzip-precompressed at build
time (level 9, mtime 0), every response has a strong `ETag` derived from the
content SHA-256, and `If-None-Match` returns `304`. Content-hashed Vite assets
(`assets/name-XXXXXXXX.{js,css,webp,woff2}`) are `immutable`; `index.html` is
`no-cache`, so a binary upgrade or rollback is picked up on the next load.

### `/legacy/`

The retained static HTML page (`dashboard/index.html`) and its Python server are
gone; the React **Fleet detail** view renders the same producer snapshot.
`--legacy-index-file` is now optional. Without it, `/legacy/` answers
`307 Temporary Redirect` to `/#/detail` under the same access scope (it still
requires the bearer token unless `--legacy-public-read` is set). Hosts that
still pass `--legacy-index-file` keep serving that file unchanged.

## Rollback

Because the console is part of the executable, switching the binary (or the
`current` symlink of a private version directory) switches the UI with it; there
is no separate asset generation to keep in step. Already-open tabs keep working
until they request a lazy chunk the new binary does not have; a reload fixes
that, exactly as with a UI-only overlay.
