# Building a deterministic source package

`scripts/release/package.py` creates a `.tar.gz` from explicitly listed, already
built regular files. It performs no compilation, download, installation, signing,
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
python3 scripts/release/package.py \
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
`MANIFEST.json` path are rejected.

The closed path denylist rejects runtime/state/private/credential/secret/key/token
names, `.git`, `.ssh`, `.codex`, `.env` files, and common private-key extensions.
Do not list private configuration or credentials. The tool never searches for or
reads credentials outside the explicit file list.

The archive contains a sorted `MANIFEST.json` with schema version, supplied
version/target/source commit, and each entry's archive path, SHA-256, byte size,
and mode. Tar ownership and timestamps are fixed; gzip timestamp and filename are
empty/fixed. Source bytes are staged under the output parent with bounded streaming,
hashed while copied, and checked again for identity/content changes. Repeating a
build with the same bytes and metadata yields identical archive bytes in the same
Python/zlib environment, independent of source modification time and input-list
order.

The manifest checksum provides content consistency, not publisher identity or
forge authenticity. This utility creates no signature, SBOM, provenance statement,
installation procedure, or release/acceptance evidence. Clean-machine archive
installation, supported-platform compatibility, maintainer rights, and live
release qualification remain separate work.

## Self-tests

Run the inline standard-library tests with:

```sh
python3 scripts/release/package.py --self-test
```

They cover deterministic output, manifest content and tar metadata, path and
credential rejection, symlinks and hard links, size and metadata bounds, output
no-replace behavior under a creation race, and source mutation during capture.
