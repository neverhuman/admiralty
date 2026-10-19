# Deferred CI cleanup

Jepson's2026-10-19T02:12Z ruling prioritizes cutover implementation. Locally
built steps with passing step smoke checks may merge after exact-head
neverhumanbot approval and matched-head rebase. Red/queued CI is never green.
One cleanup lane takes this list after remaining cutover implementation lands;
build failures remain immediate work.

Latest749d9d29 browser workflow39220795727 completed with19passed/1failed:
`atlas dark` reports actual pinned-axe `label-content-name-mismatch` on hierarchy
cards, matching item8's rule but separately observed on this source. It is not
a readiness timeout or proof of resource pressure. Raw failed log
`/private/tmp/adm-ci-749-browser-39220795727.log` SHA-256
`5eb101d06eb5e6473c9ad04557e4be696691a7a03931dec1c2833f04177594a2`
is retained.749 Rust workflow39220795716 was still in progress at08:14Z.
It later failed a different concrete case: CLI89pass/1fail,
`staged_real_generated_closure_imports_and_validates_all_public_roots` reports
`owned Node guard smoke exceeded deadline`. Raw
`/private/tmp/adm-ci-749-rust-39220795716.log` SHA-256
`6c212f5a000076b88e8f077a33a6b618ed1b16c28258302017d6d648274faee9`
is preserved. This is neither the old capture deadline nor the repaired intake
directory error; no cause or resource-pressure attribution is established.
Preserve actual visible-name/accessible-name semantics and pinned axe assertions;
no additional cleanup lane, rerun, cancellation or shared-runner control.

Latest observation: mainfcd35569 Rust workflow39216521388 failed the new offline
intake CLI suite:2passed/3failed with `database parent must not be group/world
writable`. The preview path used an unrestricted tempfile directory under Linux
umask002. Actual failure log `/private/tmp/adm-ci-fcd-rust-39216521388.log`
SHA-256 `4dc84f161beadee0617c127b4f06d03990891299f3052f870ce207cfdf391993`
is retained. This production feature bug is being repaired immediately with
atomic0700 directory creation and actual child-umask002 regression cases;
Store guards, global umask and shared service settings remain unchanged. Exact
fcd browser/accessibility workflow39216521433 passed. Exact repair7f203848 now
passes all5binary cases under child umask002 on actual Linux, one capped fresh
build07:57:33–08:02:45Z, jobs1/private TMP/unique target. Raw log SHA-256
`beffc690244c001a5d85048d13e296ebe1cda6898db15df64dce9d7d9cfdf7b1`;
see [repair receipt](receipts/integration/native-intake-cli-linux-repair-20261019T0804Z.json).
Independent source review is clear; Mac5cases/strict Clippy/fmt passed separately.
Later hosted outcomes remain separate; the failed workflow is not relabelled green.

Historical later observation: maina9e0f557 workflow39212711288/job117933239732
passed contracts, formatting and strict workspace Clippy, then failed
`actual_producer_capture_failure_cannot_yield_typed_transcript` at
`native_executor.rs:2319` with the controlled child capture deadline:
86server cases passed,1failed,2ignored. Raw failed-job log
`/private/tmp/adm-ci-a9e-rust-39212711288.log` SHA-256
`6f7ff35162a4b735ce97f7b4363c5d10803d7d37912f06435f0facfc1e765118`.
The exacta9e browser/accessibility workflow39212711302 passed. No cause is
inferred, no guard/deadline changed and no extra cleanup lane or rerun started.

Historical source-specific evidence: main36aa0eb workflow39208749813/job117917214621
passed contracts, formatting and strict workspace Clippy. Its CLI90unit cases,
5archive CLI and7installer CLI cases passed, so item10's archive-directory
failure is repaired on this source. The workflow still failed the controlled
capture deadline from item2:86server cases passed,1failed,2ignored, at
`native_executor.rs:2319`. Complete failed-job log is retained at
`/private/tmp/adm-ci-36aa-rust-39208749813.log`, SHA-256
`d7047ae6d5aa85ffb1507a6171efa5c71e929399b64eee689d3b0c58942ecbed`.
The exact36aa browser/accessibility workflow39208749754 passed. These results
do not qualify later6395 intake UI, prove the capture cause or make CI fully
green. The historical failures below remain preserved; no rerun/runner control
or additional cleanup lane was started.

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
9. Receiving main6e1c8f5 run39199569102/job117889538550 again failed the
   controlled capture deadline from item2:79server cases passed,1failed,2ignored.
   The contract job passed. Log `/private/tmp/adm-ci-6e-checks-39199569102.log`
   hashes `64fd88276458ea1b4cadbd50260142959fcd7c63ef536e8c3408c8edad62485d`.
   Its browser run39199569058/job117889538488 instead records8passed,5failed,
   6skipped: CDP/navigation and isolated-native readiness timeouts plus a
   navigation assertion. Those failures are distinct from item8's axe result.
   Browser log `/private/tmp/adm-ci-6e-browser-39199569058.log` hashes
   `87bb2bac58203ed52d4250d39bb2def603dda8e50395a4a5a71c4b7ca1e1c591`.
   Shared resource pressure is not established as their cause. Preserve original
   deadlines, real backend and pinned axe; final cleanup owns these recurring
   failures after implementation. Current b864 workflows were queued at the
   subsequent snapshot, not passing. No rerun/cancellation/runner control here.

10. Main7e282c3 run39203091433 passed formatting/strict workspace Clippy and
    contracts, then its CLI library suite reported56passed/8failed. Positive
    archive preflights fail `directory ownership or permissions`; later fault
    assertions therefore do not reach their intended seam. Original log
    `/private/tmp/adm-ci-7e-rust-39203091433.log` SHA-256
    `875ace92d1f723a1153c8bd833ca855497e9c93645eac3626d115cd393757876`
    remains preserved. This is separate from the restrictive077 manifest-mode
    defect and actual fixed619 Linux35-case pass; no cause is inferred from
    permission wording alone. Do not weaken production directory guards.
    The7e browser workflow39203091407 passed historically. New066 browser
    run39204416792 reports11passed/2failed/6skipped plus a failed live-views
    setup: isolated-native readiness and an assertion that the fatal-frame
    browser PID was already dead. This is neither axe failure nor permission
    failure. Raw log `/private/tmp/adm-ci-066-browser-39204416792.log` SHA-256
    `b1800e3b2cd835faea57afeed19c065a07c65892705256097dd9cce1792ab4e5`.
    Preserve actual child diagnostics and deadlines. Main066 checks were still
    running at the05:37 snapshot. Final cleanup owns follow-up after current
    implementation; no new cleanup lane, rerun, cancellation or runner control.
    Later main066 run39204416898 also passed contracts/fmt/strict Clippy, then
    its CLI suite again reported56passed/8failed with positive archive preflight
    directory-permission errors. Exact log
    `/private/tmp/adm-ci-066-rust-39204416898.log` SHA-256
    `d47fcab746003e2ef68f6aec4dcba236131cc31c5c221cf250d4f74ffdc8d453`.
    This is a failed workflow; the separate Linux35-case pass used its recorded
    owned077 environment and does not turn this different run green.

Raw failures and computed hashes are retained in
[the failure receipt](receipts/integration/ci-cleanup-evidence-20261019T0224Z.json).
Test-only repair WIP is preserved at `/private/tmp/admiralty-linux-fixtures-32dea09`;
readiness WIP at `/private/tmp/admiralty-native-readiness-32dea`. Neither unfinished
candidate is installed or qualified. No runner, GPU job or unrelated service was
stopped.
