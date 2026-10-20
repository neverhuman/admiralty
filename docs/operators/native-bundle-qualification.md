# Real native bundle qualification

The native staging allowlist accepts the exact release binaries `admiralty`,
`admiraltyd` and `admiralty-observe`, plus an explicitly supplied root `LICENSE`.
The observer installs as `bin/admiralty-observe`; the license installs as
`share/doc/admiralty/LICENSE`. Executables require 0755 and documentation 0644.
Unknown binaries and arbitrary root files remain rejected. Supplying both
`LICENSE` and `docs/LICENSE` collides at the destination and fails before staging.
Older native bundles remain accepted under the existing policy.

## Preparing inputs

Build the reviewed source using the pinned development tools, then build the
static dashboard:

```sh
cargo build --release --locked -p admiralty-cli -p admiralty-server -p admiralty-observe
pnpm --dir apps/dashboard build
```

Prepare an owned 0700 input root. Copy the three binaries to its `target/release/`
directory as independent regular files. Copy the built dashboard to
`apps/dashboard/dist/`, the public schema to `schemas/domain-event-v2.schema.json`,
the root license to `LICENSE`, and reviewed operator documentation below `docs/`.
Include `packages/contracts/generated/validators.mjs` and every source-bound
generated sibling listed by the native staging policy; preserve the generated
dependency closure. Hash each selected input and supply that exact explicit
allowlist to `admiralty release stage`.

Build-output trees can contain hard links. Copy bytes into the private input
root rather than relaxing staging's single-link and retained-handle checks.
Exclude credentials, private configuration, runtime databases, transcripts and
observation history. Source/target labels and caller-supplied hashes establish
no publisher or source attestation.

Follow the [staging source](../../crates/admiralty-cli/src/native_package.rs),
[archive guide](native-archive.md) and [private installation guide](native-install.md)
for integrity pins, private parent permissions and uncertain-outcome handling.
No command activates a service or changes a shell profile. Preserve uncertain
outputs for reconciliation.

## Recorded exercise

The [source-bound receipt](../../receipts/integration/native-real-bundle-20261019.json)
records an actual macOS arm64 release-profile Stage→Archive→Install run with 42
explicit files, all three compiled binaries, built dashboard, public schema,
complete generated guards and the project license. The archive is 14,298,579 bytes.
Every installed file's digest and mode matched its manifest; the installed
license matched the root source bytes. Tampered input and an existing destination
were rejected.

The installed CLI's doctor and synthetic snapshot, daemon/observer help, and
loopback demo ran with an empty command PATH, isolated HOME and private temporary
state. No Node or Python executable was available to those children. The demo
served `/`, `/healthz`, `/v2/fleet` and `/v2/views`, exited 0 on SIGINT after ordinary
HTTP requests, removed its temporary state and released its listener. This did
not exercise an open SSE stream at shutdown. An earlier private probe used the
incorrect `/v2/health` URL; its 404 and scratch remain preserved separately.

These checks establish a real payload/private-version round trip on the current
host. Clean-machine setup, Linux and other targets, observer configuration/live
collection, service installation, active-job upgrades, signing, publisher/build
provenance, final SBOM/third-party notices and managed coding execution require
separate qualification. Runtime execution/acceptance/activation flags stay false;
this receipt does not accept the full major release.
