# Native task inspection result (I5c)

## Source and branch

- Implemented on `codex/native-task-inspection`, created from `origin/main` at `e8f11153f5d0432f6833eb3c52a432f7c15463e2`.
- Prior lane branch `codex/accept-p26-p31` remains at `e9c612599692d2e5f01dc897b0d0626c31b9b628`.
- Changed UI sources: `apps/dashboard/src/ui/App.tsx`, `apps/dashboard/src/ui/NativeTasks.tsx`, `apps/dashboard/src/ui/styles.css`.
- Focused UI tests: `apps/dashboard/src/native-tasks.test.tsx`.

## Data source and behavior

The task panel reads only the current `GET /v1/fleet` `Fleet` snapshot already consumed by the browser client in `apps/dashboard/src/api.ts` and `apps/dashboard/src/events.ts`. `Fleet.tasks` is the server projection of core `TaskView` (`contract`, `state`, `attempt_id`, `owner`, `candidate_ref`) from `crates/admiralty-core/src/reducer.rs`. The server requires bearer authentication for non-simulated fleet snapshots. `/v1/events` carries durable change notifications and causes a refreshed `/v1/fleet` snapshot; the panel does not reinterpret event history or legacy rollout receipts.

The panel supports project, state, and text filters; shows projected task state, active attempt/owner, and candidate reference only when present; distinguishes empty, unavailable, and refresh-error states; labels simulated snapshots as synthetic; and clearly leaves native execution unqualified. It has no mutation controls or invented command route. The existing Legacy detailed view, Models view, temporal plots, profile, and Admiral/Commodore UI were not modified.

## Checks

- `npm test -- --run src/native-tasks.test.tsx` — passed (4 tests).
- `npm run typecheck` — passed.
- `npm run build` — passed.
- No backend, contracts, or manifest changes were made.

## Limits

This is a read-only inspection panel over the existing snapshot route. It does not authorize, start, accept, or execute tasks; `native_execution_qualified` remains false. Simulated snapshots are explicitly labeled and are not represented as native acceptance.
