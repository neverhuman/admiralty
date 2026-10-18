# Artifact file lifecycle prototype

`admiralty_store::artifacts` is a bounded filesystem primitive for immutable
content-addressed objects. `ArtifactStore::open` accepts an existing absolute
directory only when its path components are real directories without symlinks
and the root belongs to the current user with owner-only permissions. The
configured maximum is checked before publication; reads check the reference,
metadata size, configured bound and SHA-256 before returning bytes.

Publication writes a uniquely named temporary file with exclusive creation,
syncs its contents, then creates the digest name with a same-directory hard
link so an existing name is never overwritten. The parent directory is synced
after publication. Concurrent publishers converge on the same immutable name;
each verifies any object that already exists. A crash can leave an unreferenced
temporary file or object. This layer does not make an object available in a
database, and callers must not acknowledge availability until their own durable
reference transaction succeeds.

This prototype owns file publication and integrity checks only. It has no
database availability records, pins, retention policy, garbage collection,
deletion, network transfer, authorization, or execution authority. In
particular, it does not decide whether an object is safe to retire. Those
responsibilities belong to higher layers and their separately reviewed
lifecycle transactions.
