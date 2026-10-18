# Recovery manifest reconciliation

This component checks supplied metadata and returns a pure proposal. It does
not restore or replace a database, restart/launch a service, rotate credentials,
change aliases, contact a remote system or authorize effects. The serialize-only
proposal keeps private `restore_authorized` and `effects_authorized` flags false;
getters expose that fact. A blocker-free `resume_proposed` is consistency of
claims, not trusted recovery qualification.
The proposal retains the exact manifest digest, snapshot watermark, proposed
generation and external fence-receipt digest; consumers must preserve these
pins rather than applying a proposal to a later snapshot or generation.

Create backups using SQLite's supported online backup API. Record the actual
snapshot watermark, schema version, recovery generation, exact database byte
digest and size, full immutable artifact closure and active bundle digests.
Never infer a snapshot watermark from a later current sequence or copy only
the live database file. Counters are canonical decimal u64 strings and preserve
values above JavaScript precision. Keep all pins for the backup's lifetime.

Recovery scope includes store ID, project/root/task, attempt, owner, contract
revision/digest and backup executor incarnation. The manifest hash covers the
complete scope, exact snapshot identity, watermark/generation, sorted artifact
closure and sorted active bundle list. Hash helpers enforce shared streaming
1 MiB input bounds before copying/sorting typed input. Each collection is
limited to 1,024 records; duplicate, conflicting or malformed identities fail.

The observation independently repeats exact manifest/scope/snapshot/watermark
and backup generation, records the actual current executor generation and
incarnation, and covers the complete active bundle list. The snapshot's exact
digest/size must exist as verified metadata in the required artifact closure;
every active bundle must also occur in that closure and in observed bundle
coverage. Missing/mismatched artifacts, incomplete compatibility coverage,
unknown session custody or unresolved/unknown remote effects block a proposal.
A `verified` metadata flag is a claim; actual blob custody remains external.

The external fence must pin that exact manifest and scope, the observation's
actual old generation/incarnation, and a distinct new executor incarnation.
Its new generation must exceed both old/current and backup generations. A
missing, unknown or false fencing assertion blocks resumption. A rolled-back
database counter cannot establish fencing; the external receipt's provenance
must be independently authenticated by the future executor.

Operator workflow:

1. Preserve the immutable snapshot and manifest, then gather exact artifact and
   active bundle observations without modifying aliases or pins.
2. Reconcile node spools, sessions and remote mutations. Record unknown results
   as unknown; never translate them to clear or completed.
3. Obtain independently authenticated external fencing for the actual previous
   credential-bearing executor and a fresh generation/incarnation.
4. Review the pure planner's complete blockers and preserve the input evidence.
5. Submit a consistent proposal to a separately qualified restore executor;
   require its authority and custody checks before any effects or resumption.

Supported backup production, signature/credential custody, authenticated fence
receipts, actual filesystem/blob verification, snapshot restore, durable replay
and restart continuity remain pending. Matching hashes or booleans here do not
qualify them. These offline tests establish bounded consistency only.
