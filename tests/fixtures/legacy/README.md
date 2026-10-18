# Legacy regression evidence

`source-manifest.json` pins the actual repository baseline and immutable source
fingerprints. It inventories existing sanitized exports; it does not copy or
rewrite them. JSONL/CSV mirrors are alternate representations of the same facts.
The older 449-row ledger belongs to a different source pin and is explicitly
unverified here; it must never be unioned with this 503-row export as new events.

Every case under `cases/` is a **synthetic reconstruction**, using invented
identities, counters, paths and prose. Cases illustrate code-visible risks and
the wrong-checkout regression. They are not historical transcripts, independent
acceptance receipts or exact reconstructions of historical attempts. Expected
results specify the new system's acceptance contract; they do not certify the
legacy implementation or an unfinished replacement.

Run the read-only source, row-count, mirror and fixture secret-pattern check:

```sh
rtk proxy python3 tests/fixtures/legacy/verify.py
```

Once registered by the integration owner, run the independent Rust fixture
checks:

```sh
rtk cargo test -p admiralty-testkit --test legacy_import
```

`verify.py` verifies immutable Git blobs at the source pin, so implementation
changes cannot silently update the baseline. It reports worktree drift; use
`--working-tree` to require those files to remain byte-identical before a current
worktree import. Pinned-source mismatch or unavailable objects always fail.
It never executes collector,
exporter, Screen, SSH, provider, cron or deployment commands. The secret scan
uses the exporter's literal patterns plus an absolute-home-path guard. A finding
or source mismatch fails the check; review and resolve drift before importing.

Additive future source pins need separate manifests and reviewed identities;
do not silently refresh these hashes to make a failing check pass.
