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

Raw failures and computed hashes are retained in
[the failure receipt](receipts/integration/ci-cleanup-evidence-20261019T0224Z.json).
Test-only repair WIP is preserved at `/private/tmp/admiralty-linux-fixtures-32dea09`;
readiness WIP at `/private/tmp/admiralty-native-readiness-32dea`. Neither unfinished
candidate is installed or qualified. No runner, GPU job or unrelated service was
stopped.
