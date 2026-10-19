# Deferred CI cleanup

Jepson's2026-10-19T02:12Z ruling prioritizes cutover implementation. Locally
built steps with passing step smoke checks may merge after exact-head
neverhumanbot approval and matched-head rebase. Red/queued CI is never green.
One cleanup lane takes this list after remaining cutover implementation lands;
build failures remain immediate work.

1. PR95 Rust job117853119416: four `views` tests create scratch parents with
   inherited group/world-writable permissions. Store correctly refuses them.
   PR96 job117859845225 exposes six affected view fixtures. All six reproduce
   under umask `0002`. The owned follow-up at `f4432a2` creates private `0700`
   fixture directories atomically and asserts the mode; Store guards and
   production/shared settings remain unchanged. Current-main targeted proof and
   original failures are in the [fixture receipt](docs/plans/admiralty-v2/receipts/LV/f4432a204204d2848052580d6f4f0355a8e9ed7d.json).
   Source integration and a new hosted result remain separate gates.
2. Main779 job117850259638 and PR95 job117853119416:
   `native_executor::tests::actual_producer_capture_failure_cannot_yield_typed_transcript`
   hits its controlled-child deadline. It sends nine megabytes through
   synchronized output records under five seconds. Historical cause is unproven.
   Prefer a real controlled EOF-drain failure over throughput assumptions;
   preserve capture fencing and byte-limit coverage.
3. Browser readiness: fetch has no request deadline inside the nominal ten-second
   startup loop. Preserved follow-up keeps that outer deadline and the real Rust
   backend, adds bounded HTTP/child diagnostics. Six helper tests and TypeScript
   pass; actual browser qualification and independent review remain outstanding.
   This is not a proven explanation for old navigation failures.
4. PR94's reviewed job-local debug-symbol reduction preserves assertions,
   overflow checks, all checks, jobs2 and fifteen minutes. No measured timeout
   improvement is established; preserve previous failures and obtain actual
   final-source results.
5. Receiving main f590837 browser run39192222037/job117867775022 failed one
   negative diagnostic assertion (18passed/1failed): the160ms missing-label
   probe timed out in CDP before label inspection, so its retained bounded
   error differs from the asserted label-absence reason. This is distinct from
   the readiness issue above. Original log SHA-256
   `2f1421b5212ac5310dabb860f459c9df0c28147e39d9ee4d70340208d620fd93`
   is retained at `/private/tmp/adm-ci-f590-browser-117867775022.log`.
   PR99 head1542e212 and PR98 headf02ccfda browser runs passed historically;
   neither proves receiving-main green. Owner: the final cleanup lane after
   implementation; preserve negative timeout coverage and actual diagnostics.
6. PR100 run39193205311/job117870271726 stopped at strict Clippy on the
   native-attention test policy's nonminimal boolean. The equivalent expression
   repair at7c502ae builds through strict server all-target Clippy and passes
   all5attention tests (74unrelated cases filtered), with independent review.
   Original failure log SHA-256
   `7b1c3270f4b1fecadc7a279addb7da7cce5b0b4e62c76fcaadc371359f6819a6`
   is retained at `/private/tmp/adm-ci-pr100-clippy-117870271726.log`.
   New receiving-main hosted validation remains separate from this local result.
7. Receiving main8e99eaac run39195765992/job117878811711 passed formatting and
   strict workspace Clippy, then failed all5 `native_codex_plan` integration
   fixtures at line229 with `UnsafeSourceMetadata`. This is a test failure,
   separate from a workflow timeout or an actual provider run. Original log
   `/private/tmp/adm-ci-8e-checks-39195765992.log` has SHA-256
   `90c88ef5349787b70b6148490d7036cbd9758d9b577f55582bf591647e4fdf2e`.
   Receiving main9c4ba020 run39197760130/job117884103627 instead reached the
   controlled capture deadline in item2 (76server tests passed,1failed,2ignored).
   Its log `/private/tmp/adm-ci-9c4-checks-39197760130.log` hashes
   `4eaf60f5f945940338e943f5a1378c17ecae11b25283fed192bd477d69a02396`.
   Both contract jobs passed; neither full workflow passed. Owner: final cleanup
   lane after implementation. Preserve workspace and capture guards.
8. Receiving main9c4ba020 browser run39197760190/job117884103872 passed18cases
   and failed the Atlas dark accessibility case: pinned axe reported
   `label-content-name-mismatch` on hierarchy cards. The source-visible name
   must be included in the accessible name. This is distinct from the negative
   diagnostic timeout in item5; do not weaken axe, remove the assertion or call
   it transient without evidence. Original log
   `/private/tmp/adm-ci-9c4-browser-39197760190.log` hashes
   `9d12d61fd694656071641321f7ab32e5af2877496581f25a596ca8ac538c77f3`.
   Owner: final P12 cleanup after implementation; existing S3 coordination owns
   any already claimed UI repair. No additional CI cleanup worker is assigned.

Raw failures and computed hashes are retained in
[the failure receipt](receipts/integration/ci-cleanup-evidence-20261019T0224Z.json).
Test-only repair WIP is preserved at `/private/tmp/admiralty-linux-fixtures-32dea09`;
readiness WIP at `/private/tmp/admiralty-native-readiness-32dea`. Neither unfinished
candidate is installed or qualified. No runner, GPU job or unrelated service was
stopped.
