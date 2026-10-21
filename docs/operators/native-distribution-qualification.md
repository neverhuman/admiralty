# Qualify a private native distribution

Build the Rust CLI and dashboard in the owned lane, using the host's capped
runner for heavy work. Rust outputs reuse the lane's `target/`; unset any
inherited `CARGO_TARGET_DIR` before building. Node is a build dependency for the
dashboard, not a runtime server or a requirement for the installed offline demo.

```sh
cargo build --locked -p admiralty-cli
pnpm install --frozen-lockfile
pnpm --filter @admiralty/dashboard build
target/debug/admiralty release qualify \
  --assets "$PWD/apps/dashboard/dist" --repository "$PWD" \
  --output /owned/private/new-install-proof \
  --source-commit FULL_SOURCE_COMMIT --target x86_64-unknown-linux-gnu
```

`admiralty release qualify` (Rust, `crates/admiralty-cli/src/release_ops.rs`)
replaced the retired `qualify_native.py` with the same steps and evidence;
`--binary` defaults to the running executable. The receipt records the
qualifier binary hash instead of a script hash.

The output must be a new directory beneath protected ancestors. Group-writable
checkout parents can be used to build, but the private installer rejects them
as installation ancestors. Choose a protected evidence directory instead of
changing shared permissions. The command keeps all inputs, archive bytes, command
captures and `qualification.json`; do not clear a previous run to reuse its path.

The qualification stages the actual CLI executable, built dashboard, generated
validator closure, v2 event schema, MIT license and operator/contributor/security
documentation. It creates two byte-identical archives, verifies their hashes,
installs to a new private directory and checks the installed executable hash.
It runs that executable's `doctor` and populated `demo --snapshot-only` with an
empty PATH and no inherited account, provider or project configuration. The demo
does not bind a socket. Existing destinations and a wrong archive pin must be
refused without replacement; temporary demo state must be removed on exit.

This is a current-host private installation journey, not a clean OS image,
global installation, service deployment or public release. A debug executable
is a real payload but does not qualify an optimized release build. The source
commit is a label retained alongside binary and archive hashes; the command does
not authenticate that source label, publisher identity or build provenance.

| Surface | Qualification boundary |
| --- | --- |
| Linux x86_64 private archive/install | Run this script on the target host; retain its actual result and hashes. |
| macOS/private installer | Existing component checks are separate; no result on one OS qualifies another. |
| Offline doctor and demo | No account, external tools on PATH, Node server or live provider needed. |
| Provider execution and fleet enrollment | Unqualified by installation/demo checks. |
| Upgrade, PTY/session survival and rollback | Separate [upgrade gates](upgrades.md). |
| Signing, SBOM, dependency/asset rights and public distribution | Separate release review; project MIT licensing alone does not clear dependencies or marks. |

The report preserves `signed:false`, `published:false`,
`service_activation_qualified:false` and `full_packet_acceptance:false`.
See [release status](release-status.md) for the complete remaining contract.
