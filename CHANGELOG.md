# Changelog

## Unreleased

- Add a token-budgeted agent brief (`admiralty brief`, `GET /v1/brief`, and the
  new **Fleet health** view) and compact `logfmt`/NDJSON log events: one
  observer event per changed cycle with counted rollups, plus a problem-request
  access log in `admiraltyd`. Unknown facts render as `?`, never zero.
- Publish descriptive `git_metrics` (branch relations and merged candidates,
  unintegrated commits, unmerged tip ages, worktree state, integration
  activity) next to `git_health` without changing health fingerprints.
- Observer: run Git scans concurrently with fleet collection, read the previous
  snapshot once per cycle, publish stage timings, stop cleanly on SIGTERM,
  estimate disk growth/time to full natively, and emit the per-project
  `credits_per_done_todo` that history already expected.
- Server: cache encoded snapshot projections per source revision with strong
  ETags and 304 revalidation; skip re-parsing unchanged producer files in the
  views and Git health loops; align the producer read bound with the 32 MiB
  writer bound (snapshots over 8 MiB previously returned 502); add `/readyz`;
  compare bearer tokens in constant time.
- Clean deployment: `admiralty release activate|rollback|status` atomic version
  links, systemd user units, `admiralty state init`, and the Rust
  `admiralty release evidence` preflight. Four Python files are retired
  (inventory 42 → 38) and `pnpm live` no longer needs Python.
- Dashboard: grouped navigation, Ctrl/Cmd+K palette, remembered theme, hidden-tab
  polling pause, conditional snapshot revalidation and memoized trend metrics.

- Compress linear Git history into expandable ranges while preserving topology
  anchors and the complete commit inventory. Add project Captain/Admiral chat
  with durable inbox delivery/replies and Rust Git health findings over cached
  observations. Protected role enrollment is required for live chat and alerts.

- Bound admitted execution observation with caller deadlines, retain ambiguous
  launch identities and reject resubmission of a submitted run token. Preserve
  current identity/admission contracts and reject a causal child timestamp that
  predates its recorded parent.

- Display reported partial hygiene audit ranges and known subtotals in live
  cards, hierarchy and history, with original audit times. Keep final scores
  unknown until the missing measurements are available. Use the same two-decimal
  percentage limit in token summaries and trend cards, preserving small yields.
  Explain unreadable or oversized Admiral status sources while retaining the
  original clock on any available historical audit.

- Refresh community onboarding with a directly usable synthetic-demo link,
  current local CI/tool prerequisites, verified private vulnerability reporting,
  platform limits and evidence-based major-release gates. Production recordings
  and the lossless WebP/GIF distinction remain explicit.

- Preserve the exact floating-point values of source decimals through JSON
  parsing and detail projection; independent bit-level regression retains the
  original one-ULP failure. No dependency versions or schemas change.

- Replace megabyte UI mascot PNG transfers with 192-pixel lossless WebP icons
  (42.5 KB combined), lazy-load watcher images and cache content-hashed images.
  Vite hashes containing hyphens retain their immutable cache policy.

- The detail view loads current project facts before history and complete
  records, with explicit enrichment state and the original full Git source
  preserved for Git Flow. Failed enrichment retains the initial facts and age.

- Negotiate lossless gzip for live snapshots and text assets, cache Vite's
  versioned JavaScript/CSS, and explain timed-out snapshot requests clearly.
  Existing source bounds, authentication and request deadlines remain enforced.

- Rust serves the real production observation feed, native HTTP service and
  React dashboard. Owned legacy Python production services are retired.
- Scoped native Admiral/captain messages and separate application receipts are
  durable. One actual production role round trip is verified; general managed
  coding execution remains unqualified.
- Real production console, Git and flow captures have a reproducible vivid,
  lossless WebP path with optimized GIF fallbacks.
- Full local application CI now checks native binaries, contracts, audit and
  package harnesses, frontend type/tests/build and real browser accessibility.
- Pinned read-only Jankurai auditing exposes conformance findings for review;
  temporary Python compatibility/port inventories preserve retirement criteria.
- Onboarding reports the retained Jankurai baseline under SC-DEC-186J; Rust CI
  enforces the 40-file Python ceiling and complete exception/migration records.
  Session qualification retries only read-only Busy observations under the
  original deadline, and browser provisioning qualifies each candidate.

The complete native major release is not yet accepted or published. See
[release status](docs/operators/release-status.md) for remaining gates.

## 21 October 2026 — PR consolidation

- Retain the integrated Git inspector-fit correction, service/host health and ALOG telemetry, source-bound cached projections, optional logfmt/JSON observer output, producer briefs and Rust live startup.
- Preserve Fleet operations and expose the byte-bounded producer view separately as Agent brief, with isolated styles and original observation clocks.
- Use one protected credential initializer and one activation journal/layout. Readiness checks Store/draining state, producer freshness and configured assets together.
- Remove mandatory independent PR approval under the owner's instruction; keep source review, current-head checks and matched ordinary rebase merges.
