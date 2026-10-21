# Git Flow viewport UX — 21 October 2026

User requests a snap-to-100% control, a stronger graph view, single-project drill-down, divided work with Admiral/Captain, and a feature branch merged through normal review.

Base: `5d4e656bbe7438a25728aefbac78b2576e1732ed`. Branch: `codex/git-flow-viewport`.
Checkout: `/Users/bentaylor/.codex/worktrees/git-flow-viewport/admiralty`.

## Ownership and boundaries

Root is sole integration writer. A scoped helper worker owns only new `git-viewport.ts` and `git-viewport.test.ts`; its nine unit cases passed before integration. Root owns `GitFlow.tsx`, `live-views.css`, focused browser tests, this record, operator documentation and additive execution/qualification receipts. Existing Git Flow owner confirmed previous source lanes are retired; no overlapping writer remains. Captain independent UX/source review and Admiral tracking/serialized rollout are being registered through existing locked coordination routes. Delivery does not imply adoption or approval.

Preserve original dirty checkout, foreign worktrees, generated contracts, Rust runtime, topology/compression rules, private feeds/session epochs, operator-chat authorization and sole deployment/configuration authority. No credentials, provider sessions, service controls, runner changes or foreign cleanup belong to this feature.

## Implementation and verification

1. Add distinct actual-size (100%) and fit-loaded-width controls. Zoom anchors an observed OID against newly measured layout rather than multiplying old scroll offsets. Clamp actual browser scroll after layout.
2. Make project focus an explicit route with a return action; retain per-scope camera state, scoped selection, inventory and manual expansion. Focus can hide the details gutter for working space while retaining access to metadata/health/checkouts.
3. Use one responsive gutter width for CSS and canvas math. Keep zoom controls readable independently of graph scale and add focused-canvas keyboard actions (0 fit, 1 actual size, +/- zoom), excluding form/chat controls.
4. Avoid overlapping branch labels, prioritizing selected/main labels; retain all source refs and hover/inspector/inventory evidence.
5. Test real zoom-dependent compression, center/selected anchors, lower project band shifts and fitting boundaries. Add built-browser journeys for actual-size, project focus/back, selection, keyboard, responsive layout and existing collapse/manual intent. Inspect screenshots.
6. Freeze source, run Node24/pnpm11.15.1 local `pnpm check` (all20 stages, compiler jobs2/test threads1). Preserve failures and logs. Independent review must cover exact final head; resolve findings without weakening checks.
7. One integration PR, exact-head eligible approval and hosted checks, ordinary matched rebase merge; separately verify receiving-main checks. Hand off to existing sole Admiral for actual rollout and verify served assets/browser behavior before calling deployment complete.

Stop affected publication on failing checks, ambiguous ownership, topology/selection regression, mismatched reviewed source or incomplete authorization boundaries. Continue independent source work. No bypass, force push, self-approval or inferred deployment.

## Current evidence

Implementation candidate complete. Final clean-source local qualification and genuine independent review are pending; no PR merge or new deployment claimed by this source record.

Built-browser preview passed one new161-commit/two-project journey, desktop/mobile screenshots and axe. Review identified legacy branch-only scope exit and stale per-scope remount camera; both repaired before final qualification. Fit is explicitly **Fit width**: existing range controls preserve54px lane spacing, so claiming whole-height fit would be inaccurate. Vertical branch position remains anchored. Initial native build failed because inherited Cargo target referenced unavailable `/Volumes/MOE`; explicit project-local target build passed. No foreign target was modified.

Integration feedback also found checkout HEAD anchoring; root now anchors checkout selections too. Dashboard unit suite241/241 passes. First complete21-case live-view draft had5 failures: four surfaces exposed the same accessible-name mismatch, and one new test expected zoom reduction before React applied expansion. The label was repaired; next draft20/21 passed, with the remaining harness sequencing failure retained. Waiting for the visible expanded state before fitting makes the focused real-server/browser journey pass. Assertions and axe rules remain intact. Full final suite runs on the committed candidate; its separate receipt/PR records exact results.

First clean candidate2785b072 completed19non-browser stages, then browser50/52 passed. One failure proved the new test could act on a prior producer cut; it now waits for the actual161-commit canvas before expanding/fitting (and the branch-only case waits for its4-commit cut). The other was axe incomplete contrast in the public project-focused view due to root's decorative background gradient; that gradient is removed. No axe rules or acceptance assertions changed. The complete failed raw logs remain `/private/tmp/adm.uLHG9y/logs`, copied/bound in the external qualification packet. A new clean-source full run follows these repairs. Manual browser-act synthetic preview verified focus,100% reset, unobstructed controls and details gutter restoration; it is separate from real deployment. Its owned session/server were closed/stopped.
