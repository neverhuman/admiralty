# Integration and quality evidence

Open **Commands → Integration & quality** to search visible tasks by task ID,
title, candidate reference or blocker. Project selection limits the result list;
each page renders at most 50 tasks. The snapshot event sequence identifies the
displayed observation. Connection errors remain visible above the journey.

Each row shows the recorded state, unmet dependencies, missing check definitions
and available lifecycle reason. Expand the decision for its exact source commit,
reviewer, receipt and verification policy. Configured checks are displayed as
configuration; they do not imply a passing result. Missing decisions and blocker
details remain explicitly unavailable. Simulated snapshots are labeled.

Expand **Checks & owned assets** for check IDs and owned/protected paths. Open a
task's title to reach its evidence timeline and available authorized pending-task
actions. Existing server scope, request identity and CAS checks apply. The fleet
inventory link opens node observations; it does not enroll, drain, update or
delete a node. Cleanup and deployment still require their controlling boundary.

Keyboard users can Tab to filters, expandable evidence, task buttons and page
controls, then use Enter to activate. Tables scroll within their labeled region
on narrow screens. The view adds no animation. The named
`e2e/integration-quality.e2e.ts` check exercises filtering, keyboard task opening,
390-pixel layouts, reduced motion and axe in both themes against the built app.
