# Release evidence and deployment handoff

Only the currently assigned deployment owner controls the primary services.
For the 19 October cutover that owner is `adm_p18a`; qualification lanes use
isolated loopback port `0` and private disposable data. A merged source revision,
a local smoke result and an installed/running revision are separate facts.

Run the read-only release evidence preflight from the source checkout:

```sh
admiralty release evidence \
  --manifest /absolute/evidence/release.json \
  --evidence-root /absolute/evidence \
  --expected-revision FULL_SOURCE_GIT_SHA
```

The Rust command replaces the retired `tests/qualification/release_evidence.py`
with the same gates, bounds and exit codes: 0 ready for independent review,
1 incomplete, 2 invalid. For installing, activating and rolling back a version
see [clean deployment](deployment-systemd.md).

The bounded JSON manifest contains `schema_version: 1`, `source_revision`, and
up to 64 `checks`. Each check names one gate below and has `status` (`passed`,
`failed`, or `unavailable`). Passed checks additionally require the exact
`revision`, `kind` (`actual` or `synthetic`), UTC `started_utc` and `finished_utc`
ending in `Z`, a relative `evidence` file and its lowercase `sha256`.
The complete-cost trial must explicitly retain `savings_claim: false` here;
economic claims require separate review of coverage, units, cohort and method.
Artifacts are regular files up to 64 MiB under the evidence root, opened without
following symlinks. Manifests are limited to 1 MiB and reject duplicate JSON keys.

Required gates are `invariant_matrix`, `load_query`, `synthetic_soak`,
`clean_install`, `provider_transport`, `restart_continuity`, `backup_restore`,
`rollback`, `staged_canary`, `fleet_handoff`, `context_fidelity` and
`complete_cost_trial`. The soak requires a recorded interval of at least 24 hours;
the final nine gates require actual journeys. A short run cannot stand in for
the soak, and a synthetic provider cannot stand in for actual transport.

Exit 0 means the declared source/time/hash metadata is ready for independent
review. It does not authenticate measurements or establish full packet
acceptance, deployment authority or savings. Exit 1 lists failed, missing or
incomplete gates; exit 2 indicates an invalid manifest/artifact. Missing gates
remain visible rather than inheriting a blanket PASS from another packet.
The reviewer must inspect evidence contents, supported limits, negative results,
current ownership and the source/build/installed/running identities.

For the local HTTP subset, build the explicit `admiraltyd` and `admiralty`
binaries in the lane's reusable `target/`, then use the
[isolated canary smoke harness](canary-smoke.md) (`admiralty-canary`). It never attaches to the
primary endpoint or launches inference. It cannot qualify the provider,
restart/fleet/restore/rollback, full invariant/load matrix, 24-hour soak or
prospective cost/context study. Those gates remain open until their own actual
evidence is produced by the authorized operators.

The private scratch root must also have protected ancestry. In particular,
creating a `0700` child under a group-writable checkout is insufficient for the
server's private configuration reader. Use a new owned fixture directory under
an already protected runtime parent; preserve any refused run as evidence and
do not change shared checkout/ancestor permissions to make a smoke test pass.

The final deployer handoff must pin the source and package hashes, installed
and running identities, enrolled nodes, registered roots, effective policies,
service units, endpoints, backup watermark, old executor fence, rollback result
and limitations. Preserve unresolved effects and active bundle pins across
restart/restore. Never infer rollout completion from source integration alone.
