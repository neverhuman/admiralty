# I5c native task inspection — I1 interface follow-up

## Branch and scope

- Branch: `codex/native-task-inspection`, based on `origin/main` at `e8f11153f5d0432f6833eb3c52a432f7c15463e2`.
- The prior `codex/accept-p26-p31` branch remains preserved at `e9c612599692d2e5f01dc897b0d0626c31b9b628`.
- UI-only changes: `apps/dashboard/src/api.ts`, `apps/dashboard/src/validation.ts`, `apps/dashboard/src/ui/App.tsx`, `apps/dashboard/src/ui/NativeTasks.tsx`, `apps/dashboard/src/ui/styles.css`.
- Focused tests: `apps/dashboard/src/native-tasks.test.tsx`.

## I1 interface alignment

The interface pointer specifies `GET /v2/fleet` and `/v2/events`, with a schema-version-2 fleet, optional per-task lifecycle metadata, and terminal task states accepted/failed/cancelled. The dashboard now defines a typed `NativeFleet`/`NativeTaskView` boundary and `parseNativeFleet` validation for the v2 envelope, task contracts/states, identifiers, and optional lifecycle object. The task panel accepts this v2 projection as an injectable snapshot and never reuses v1 task data for lifecycle inspection.

The integrating branch `codex/i1-authoritative-lifecycle-contracts` was not fetchable from `origin` at this check. The current `origin/main` server and dashboard expose only `/v1/fleet` and `/v1/events`; no v2 route or v2 event schema implementation is available here to consume or verify. Therefore the panel explicitly reports the missing adapter and waits for a validated `NativeFleet` injection. The exact remaining UI adapter is an authenticated `/v2/fleet` client plus `/v2/events` refresh wiring after the I1 routes and generated event-v2 types are published. The detailed legacy route/path is untouched.

Lifecycle metadata remains opaque to this read-only panel beyond requiring a bounded JSON object. It is not interpreted as proof, acceptance evidence, or execution authority. There are no mutation controls, backend/contracts/manifests changes, or invented endpoints.

## Checks

- `cargo fmt --all -- --check` — passed.
- `npm test -- --run src/native-tasks.test.tsx src/validation.test.ts` — passed (11 tests).
- `npm run typecheck` — passed.
- `npm run build` — passed.
