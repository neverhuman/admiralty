# Recovery manifest reconciliation

This component validates supplied recovery metadata and produces a proposal. It does not copy or restore a database, restart a service, read keys, contact a remote system, or authorize an external effect. A complete proposal still has `restore_authorized: false` and `effects_authorized: false`; an independently qualified executor must supply that authority.

## Supported backup requirement

Create backups with SQLite's supported online backup API. The backup manifest must record the actual snapshot watermark returned by that operation, its schema version and recovery generation, the complete immutable artifact closure, and every active bundle digest. Keep those artifact pins for at least as long as the backup can be used. Never infer a watermark from the current store sequence or copy only the live database file. A decimal string preserves the exact counter even above JavaScript's safe integer limit.

Each closure record binds a SHA-256 digest, canonical decimal byte size, verified state, and a deterministic content-derived closure ID. The manifest digest binds scope, watermark, recovery generation, closure, and active bundle digests. Reordering closure records does not change that digest. A duplicate, conflicting record, changed manifest digest, missing blob, or mismatched blob is a blocker or input error.

## Metadata-only operator workflow

1. Preserve the original backup and its manifest. Gather artifact availability and exact compatibility evidence for every active bundle, without changing aliases or pins.
2. Reconcile every node spool, session custody state, and remote mutation. Record unknown outcomes as unknown; do not translate them into clear or completed.
3. Obtain an external recovery receipt for a fresh generation greater than both the backup generation and the prior executor generation. The receipt must establish that the old credential-bearing executor is fenced. A rolled-back database counter cannot establish this.
4. Pass the immutable manifest, observed artifact closure, compatibility and reconciliation observations, and external fence to the pure recovery planner. Review every blocker and preserve the evidence used to resolve it.
5. Route a blocker-free proposal to the separately qualified restore executor. Resume only after that executor validates its authority and the required compatibility and custody checks.

Unknown or false old-executor fencing, incompatible active bundles, unresolved or unknown remote mutations, and unknown session custody block a resume proposal. New-work aliases never rewrite active attempts or their pinned versions. This metadata workflow does not claim restart continuity or successful restoration.
