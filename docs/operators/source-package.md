# Building a deterministic source package

This is the retained compatibility packager. New native releases use the
[Rust stage/archive commands](native-archive.md). The two formats and
qualification scopes are distinct; this packager is not native ingestion or the
primary archive path.

`admiralty release source-package` (library: `admiralty_qualify::source_package`;
also available as `admiralty-canary package` from the `admiralty-qualify` crate)
creates a `.tar.gz` from explicitly listed, already built regular files. It is the
Rust port of the retired `scripts/release/package.py`:

| Retired invocation | Replacement |
| --- | --- |
| `python3 scripts/release/package.py --input-root … --output …` | `admiralty release source-package --input-root … --output …` |
| `python3 scripts/release/package.py --self-test` | `cargo test -p admiralty-qualify source_package` (or `admiralty release source-package --self-test`) |

The flags, validation rules, bounds, manifest and exit codes are unchanged: the
manifest is printed as sorted compact JSON and refusals exit 2.

The packager performs no compilation, download, installation, signing,
publishing, deployment, or platform qualification. A target value labels the
package; it does not prove that the contents were cross-compiled or tested on that
platform.

## Inputs and command

Create an input JSON file with exactly one `files` property. Each item contains a
relative source path beneath the input root, a POSIX archive path, and a mode. Use
mode `420` for `0644` or `493` for `0755`.

```json
{
  "files": [
    {"source": "bin/admiralty", "archive_path": "bin/admiralty", "mode": 493},
    {"source": "assets/index.html", "archive_path": "share/admiralty/index.html", "mode": 420}
  ]
}
```

Run:

```sh
admiralty release source-package \
  --input-root ./prepared-output \
  --inputs ./package-inputs.json \
  --version 0.1.0 \
  --target linux-x86_64 \
  --source-commit 0123456789abcdef0123456789abcdef01234567 \
  --output ./admiralty-0.1.0-linux-x86_64.tar.gz
```

The output must not already exist; it is atomically created without replacement.
The output parent must already exist and contain no symlink path component. The
input root is explicit. The packager follows no directory trees or globs. Assets
must be named one by one in the input JSON.

## Bounds and deterministic contents

The JSON input is capped at 1 MiB, the file list at 1,024 entries, each source
file at 512 MiB, all source files together at 1 GiB, and each input string at
4,096 UTF-8 bytes. Inputs must be regular files with one hard link and no
symlink in their path. Absolute paths, traversal, ambiguous normalized archive
names, control characters, backslashes, duplicate paths, and the reserved
`MANIFEST.json` path are rejected. Archive paths must be ASCII: the retired
Python accepted any NFC-normalized name, but without a Unicode normalization
table the Rust port refuses every non-ASCII archive path rather than risk a
non-NFC name (a stricter, safe difference).

The closed path denylist rejects runtime/state/private/credential/secret/key/token
names, `.git`, `.ssh`, `.codex`, `.env` files, and common private-key extensions.
Do not list private configuration or credentials. The tool never searches for or
reads credentials outside the explicit file list.

The archive contains a sorted `MANIFEST.json` with schema version, supplied
version/target/source commit, and each entry's archive path, SHA-256, byte size,
and mode. Tar ownership and timestamps are fixed; gzip timestamp and filename are
empty/fixed. Source bytes are staged under the output parent with bounded streaming,
hashed while copied, and checked again for identity/content changes. Repeating a
build with the same bytes and metadata yields identical archive bytes with the
same `admiralty` build, independent of source modification time and input-list
order. The uncompressed PAX/ustar stream (headers, padding, 20-block records),
the gzip header/trailer and the manifest bytes are identical to the retired
Python packager's output; only the deflate encoding differs (the Rust build uses
`miniz_oxide` instead of zlib), so compressed bytes are not comparable across
the two implementations.

The manifest checksum provides content consistency, not publisher identity or
forge authenticity. This utility creates no signature, SBOM, provenance statement,
installation procedure, or release/acceptance evidence. Clean-machine archive
installation, supported-platform compatibility, maintainer rights, and live
release qualification remain separate work.

## Self-tests

Run the ten built-in scenarios with:

```sh
cargo test -p admiralty-qualify source_package
# or, from a built CLI, in a private temporary directory:
admiralty release source-package --self-test
```

`--self-test` prints `Ran 10 tests` and `OK` on stderr. Set
`ADM_PACKAGE_TEST_SCRATCH` to choose the scratch parent (default: the system
temporary directory). The scenarios cover deterministic output, manifest content and tar metadata, path and
credential rejection, symlinks and hard links, size and metadata bounds, output
no-replace behavior under a creation race, and source mutation during capture.

Review hardening: archive names are portable NFC paths with case-insensitive and
file/ancestor collision checks; manifest/device names are reserved. Duplicate
JSON keys fail. Source and manifest reads walk directories through no-follow
handles, so swapping an ancestor cannot redirect a capture into a foreign tree.
The explicit file allowlist and filename exclusions do not detect secrets inside
arbitrary file contents; release inputs must already be approved build outputs.
