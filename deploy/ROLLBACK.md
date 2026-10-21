# Roll back without losing state

Prepare rollback **before** publication. Use the same two deployment locks and
fresh process/source checks as the forward deployment.

## UI-only rollback

The asset procedure preserves the exact old index and all old hashed assets.

1. Verify both exclusive locks and the current runtime identity.
2. Verify the current entry matches the UI overlay being rolled back. If another
   operator has published a newer entry, stop and reconcile; do not overwrite it.
3. Verify the rollback index hash against the original switch receipt.
4. Write the original index to a new temporary file beside production index.html,
   fsync it, then atomically rename it over the current entry. Fsync the directory.
5. Verify production HTTP and reload the actual user Chrome tab. Confirm the
   restored lazy assets and feature behavior.
6. Record the rollback and notify the existing owner route.

Keep both generations' hashed assets. Removing them during rollback can break
open clients and erase evidence. Asset cleanup is a later, owned retention task.

## Full runtime rollback

Use the existing qualified controller and retained executable/configuration
manifest. Verify every PID by UID, start time and executable before a signal;
never kill by name or touch another project's process.

A database that accepted new writes cannot be restored blindly from an older
backup. Apply the recorded compatibility/recovery plan and preserve authoritative
state. Credentials and private configuration are not overwritten from source.

## After any failed or ambiguous attempt

Retain its logs, allocations, staged bytes and accounting state. Determine the
actual entry, executable and database state before deciding what to do next.
Never report a successful rollback from a process exit alone.
