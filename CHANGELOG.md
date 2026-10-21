# Changelog

## Unreleased

- Finish the Rust/TypeScript migration: the last 42 Python files and 24 shell
  scripts are ported. 23 scripts are deleted; `tools/security-lane.sh`, the
  path Jankurai looks for, is now a one-line `exec cargo xtask security`. The super-captain audit,
  ticker and queue health (`admiralty sc-audit|sc-tick|queue-health`), screen
  operator tools (`admiralty fleet …`, including idle-gated delivery plans and
  the host sweep), rollout observation and packets, history export/publish,
  OpenCode qualification, source packaging, canary/excursion smokes and release
  evidence are Rust with parity tests against the originals; repository
  automation is `cargo xtask check|security|demo|live|dist`; demo capture and
  contract generation are TypeScript. `python_policy` now rejects any Python
  file or new shell script, and the exception inventory is retired.

- Ship one binary: `cargo xtask dist` embeds the production dashboard in
  `admiralty`/`admiraltyd` (gzip-precompressed, strong ETags, immutable hashed
  assets) and records checksums and build provenance. `admiralty serve` and
  `admiralty observe` run the server and observer. Without `--legacy-index-file`,
  `/legacy/` redirects to the React Fleet detail view; the static
  `dashboard/index.html` page is retired.

- Redesign the console around one "chart room" design system: a single grouped
  navigation rail, a 48-pixel watch bar, nautical-chart tokens for both themes,
  magenta reserved for interaction, self-hosted Atkinson Hyperlegible type and
  sentence-case labels. Contract validators load on demand, cutting the entry
  chunk from 522 KB to 348 KB (122.6 to 106.7 KB gzip).

- Observe Jankurai standing: the observer reads each project's
  `.jankurai/repo-score.json` (score, decision, findings by severity, caps,
  weakest dimension, audited head) and Fleet detail shows it per project.

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
