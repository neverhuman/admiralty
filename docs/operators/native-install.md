# Install a private native version

`admiralty release install` verifies an explicitly pinned native Rust archive
and creates one new private version directory. It executes no package bytes and
does not change a current link, service, autostart, shell profile or PATH.

Build the reviewed CLI, prepare an existing owned0700 destination parent, then
supply absolute paths and independently reviewed integrity pins:

```sh
admiralty release install \
  --archive /absolute/native.tar.gz \
  --expected-archive-sha256 ARCHIVE_64_LOWERCASE_HEX \
  --expected-manifest-sha256 RAW_MANIFEST_64_LOWERCASE_HEX \
  --destination /owned/private/admiralty-version
```

The final destination must not exist. Its parent must belong to the calling UID
and have mode0700. Every source/destination ancestor must be root/current-UID
owned without group/other write permission; trusted sticky temporary directories
are allowed. Symlinks are not followed. Do not relax shared directory permissions
to make a path pass; choose an appropriately protected owned directory.

The archive pin covers the entire `.tar.gz`; the manifest pin covers complete
canonical `MANIFEST.json` bytes **including the trailing newline**. The latter
differs from the structured unsigned `manifest_sha256` field. Pins establish
integrity against the supplied values, not publisher authentication, source/build
provenance, signing or platform compatibility. Version/target/source labels do
not attest the running machine or its processes.

Only the existing native Rust staging allowlist, canonical tar/gzip format and
exact generated-validator dependency closure are accepted. Compressed/expanded
bytes, manifest entries, paths, file sizes and tree nodes are bounded. Unsafe
paths, links/devices/FIFOs, extra/missing entries, noncanonical metadata, bad
digests/modes/CRC, concatenated streams and trailing data fail closed. Package
guards and executables are inspected as bytes and are never evaluated.

The installer writes through retained descriptors in an owned0700 temporary
directory, checks identities/content again, syncs files/directories, publishes
with no-replace rename and syncs the parent. It never overwrites a destination.
The successful receipt records installed bytes/device/inode and
`installation_scope:private_version_directory_only`; execution, acceptance,
activation, publisher and complete-installer qualification remain false.
The owning UID or OS can modify its files later; this is not hostile same-UID
isolation or a permanent pathname guarantee.

An uncertain error returns `installation_outcome:unknown`,
`installed_bytes:null` and bounded reconciliation metadata. Preserve the original
archive, reported destination and temporary name; inspect their recorded device/
inode identities. Do not blindly retry or delete an uncertain tree. An unexpected
rename return or later sync/acknowledgment failure may have installed the bytes.
Known no-replace collisions remain ordinary refusals. Other precommit failures
clean only scratch whose created identities and complete inventory are proven;
unexpected rename outcomes preserve both names for reconciliation.
Actual disk-EIO behavior is not qualified by the simulated lost-return tests.

Current component checks exercise synthetic filesystem inputs and actual Rust
CLI Stage→Archive→Install children. Complete clean-machine setup, global/service
installation, activation, upgrade/drain, signing and public release remain separate
qualification gates. See [release status](release-status.md).
