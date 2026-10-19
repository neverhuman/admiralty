# Rust native package archives

The Rust CLI can archive an already explicitly staged native package. It does no build, discovery, installation, signing, publication, provider execution, native activation or acceptance. Keep scripts/release/package.py as historical reference until separate parity, supported-host and installation evidence exists.

The staged directory and output parent must already exist, be owned/private and contain no symlink path components. The output itself must not exist. Supply a reviewed expected PackageManifest JSON file outside the staged tree, owned by this user with mode0600. It can contain receipt.manifest from an actual release stage response; formatting may differ from the staged file. Merely copying an unreviewed manifest/checksum does not authenticate source or publisher identity.

```
admiralty release archive \
  --staged-dir /absolute/reviewed-stage \
  --manifest /absolute/private/expected-manifest.json \
  --expected-manifest-sha256 <64-lowercase-hex-raw-MANIFEST.json-SHA256> \
  --output /absolute/private/admiralty-2.0.0-fixture-unix.tar.gz
```

The expected raw SHA256 covers the complete canonical staged MANIFEST.json bytes, including its trailing newline. It is distinct from the existing manifest.manifest_sha256 field, which hashes the unsigned structured manifest and retains sha256:<hex>. It is also distinct from hashing the separately formatted private expected JSON file. Preserve the independently reviewed raw digest with the staging review.

Before output creation the command verifies native staged metadata/path/kind/mode/size/hash policy and the same source-bound generated guard dependency closure used by release stage. Unexpected directory entries are rejected through bounded metadata-only inventory; they never become archive inputs. Known generated module filenames conservatively retain reserved source-bound meaning because the staged manifest does not retain original aliases.

Archive paths currently use the native portable ASCII subset: depth32, component255/path4096 bytes; manifest<=1MiB, files<=1024, per-file512MiB/total1GiB and projected nodes<=8192. Files are read through retained no-follow handles, streamed and rehashed; tar/gzip metadata is fixed. Reproducibility applies to identical bytes/metadata with the same pinned compressor/backend versions, not byte equality with historical Python/zlib output.

A successful JSON response has recorded_only=true and receipt.archive_created=true, with full raw manifest hash, existing unsigned manifest hash, complete archive SHA256/size/count. installed, signed, published, execution_granted, acceptance_granted, native_execution_qualified, activated and source_commit_verified remain false. The archive's source_commit/target are supplied labels, not build/host qualification. The archive is atomically published without replacement. An uncertain commit error can leave the completed artifact present: preserve and reconcile that exact artifact; do not automatically retry or delete an unrelated destination.

Current proof is local macOS actual Rust CLI staging/archive/extraction with owned synthetic payloads, plus module tests. It is not a clean-machine install, real release payload, Linux archive run, signature, rights decision or full P30 qualification. No account, network request, Python process or provider turn is required by this archive path.
