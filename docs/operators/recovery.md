# Offline backup and recovery preflight

Run recovery commands against an exclusively owned, offline database. Keep the
source, backup, receipt and artifact directory in owned private directories.
Every path is absolute; outputs must be new. These commands never replace live
state, adopt sessions, signal processes or enable provider effects.

```sh
admiralty recover backup --database /owned/source.sqlite \
  --output /owned/backup.sqlite --receipt-output /owned/backup-receipt.json \
  --objects /owned/objects --artifacts /owned/artifacts.json
admiralty recover verify --backup /owned/backup.sqlite \
  --receipt /owned/backup-receipt.json --objects /owned/objects
admiralty recover rehearse --backup /owned/backup.sqlite \
  --receipt /owned/backup-receipt.json --objects /owned/objects \
  --scratch /owned/empty-rehearsal
```

The artifact input is an explicit JSON array of `{sha256,size_bytes}` objects.
An empty array declares no additional objects; it does not discover references
or prove a complete production inventory. The SQLite online backup captures its
actual event watermark and opaque reader generation. Rehearsal verifies bytes,
replays the projection, rotates the reader generation and invalidates old reader
cursors in a new scratch database. Preserve the original backup and receipt.

Before reviewing a recovery manifest, bind its claims to the verified bytes:

```sh
admiralty recover preflight --backup /owned/backup.sqlite \
  --receipt /owned/backup-receipt.json --objects /owned/objects \
  --manifest /owned/recovery-manifest.json \
  --observation /owned/recovery-observation.json --fence /owned/fence.json
```

Preflight requires the exact schema, database SHA-256/size, event watermark and
artifact closure, including the database object. It rejects missing or duplicate
closure members and invalid immutable artifact identities. A `verified` JSON
flag does not replace reading and hashing the actual objects. The backup receipt
also binds the semantic projection and the Store's opaque reader generation.
The external executor generation is a separate monotonic counter; do not convert
the reader generation to that counter or treat either as fencing authority.

The response distinguishes `backup_verified` from the planner's compatibility,
session and external-fence blockers. The CLI has no authenticated external fence
adapter or authoritative session inventory: admission remains held and restore
and effects authority remain false even for complete-looking input. A failed
preflight exits unsuccessfully. Preserve unknown outcomes and original evidence.

See [manifest reconciliation](recovery-manifest.md) and
[upgrade limits](upgrades.md) for the remaining production recovery contract.
