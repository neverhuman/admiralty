# Legacy regression evidence

`source-manifest.json` is a **historical inventory** of the legacy baseline at
`repository.base_sha` (`1e8ed17eff27ed3ab80dc6860fc8eda301406aee`). Its digests
describe immutable Git blobs at that commit. They are no longer re-verified
against the working tree: the pinned legacy dashboard collector stack
(`dashboard/collector.py`, `extras.py`, `make_snapshot.py`, `rollout.py`,
`serve.py`, `token_yield.py`, `run.sh`) has been retired in favor of the Rust
observer (`crates/admiralty-observe`), and `verify.py` was retired with it. Do
not refresh these hashes; an operator can still compare one entry by hand with
`git show <base_sha>:<path> | sha256sum`. JSONL/CSV mirrors are alternate
representations of the same facts. The older 449-row ledger belongs to a
different source pin and is explicitly unverified here; it must never be
unioned with this 503-row export as new events.

Every case under `cases/` is a **synthetic reconstruction**, using invented
identities, counters, paths and prose. Cases illustrate code-visible risks and
the wrong-checkout regression. They are not historical transcripts, independent
acceptance receipts or exact reconstructions of historical attempts. Expected
results specify the new system's acceptance contract; they do not certify the
legacy implementation or an unfinished replacement. `source_reference` fields
cite files and lines at the pinned base commit, some of which no longer exist in
the working tree.

## Checks

Both suites are offline and read-only; neither executes collector, exporter,
Screen, SSH, provider, cron or deployment commands.

```sh
rtk cargo test -p admiralty-observe
rtk cargo test -p admiralty-testkit --test legacy_import
```

`admiralty-observe` test `legacy_cases::tests::fixture_hygiene_replaces_retired_verify_py`
replaces the fixture half of `verify.py`: every fixture JSON file (cases and
`source-manifest.json`) parses, contains no credential-like text (a regex-free,
strictly broader re-expression of the exporter's `SECRET_RES` patterns) and no
absolute home path, and exactly 11 uniquely identified synthetic cases exist.
The Git-blob pin verification, working-tree drift report and ledger/history
mirror comparisons are retired with the pinned sources.

The observer runs these cases against its real code paths:

| Case | Rust observer test |
| --- | --- |
| `wrong_checkout_pass` | `projects::legacy_parity::legacy_case_wrong_checkout_pass` |
| `docs_only_implementation` | `projects::legacy_parity::legacy_case_docs_only_implementation` |
| `unknown_price_zero_fill` | `usage::legacy_parity::legacy_case_unknown_price_zero_fill` |
| `mixed_model_attribution` | `usage::legacy_parity::legacy_case_mixed_model_attribution` |
| `lifetime_window_leakage` | `usage::legacy_parity::legacy_case_lifetime_window_leakage` |
| `coverage_accumulator` | `usage::legacy_parity::legacy_case_coverage_accumulator` |
| `stale_capture` | `fleet::legacy_parity::legacy_case_stale_capture` |
| `quoted_approval`, `missing_scope`, `affected_test_omission`, `foreign_cleanup` | Not observer behavior (the observer has no approval classifier, input path, admission or cleanup authority); covered by `legacy_import` |

`admiralty-testkit --test legacy_import` independently checks every case's
expected contract and the manifest's pinned identity.

Additive future source pins need separate manifests and reviewed identities;
do not silently refresh these hashes to make a failing check pass.
