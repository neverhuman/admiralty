# Exclusive publication gateway

`admiralty_runtime::integration::publisher::PublicationGateway` is an explicit,
supervisor-owned publication capability. Worker output and the pure core
`propose_publication` result cannot execute a merge. API/UI callers can use
`assess`; `publish` applies the same core predicate to fresh forge evidence.
This library does not enable publication in the running native task loop.

Configure one private journal directory for each repository/target, shared by
all publisher instances for that target. The directory must belong to the
publisher account with mode 0700, outside worker workspaces. Opening it takes an
exclusive lifetime lock; a concurrent owner fails. Keep the directory and its
ancestors under publisher/operator custody. Never delete or recreate it to
recover a blocked slot, and never give the same target two journal directories.

Construct `GatewayPolicy` from operator configuration, including project,
repository, target ref, authority generation, required reviewers/checks and
owner restrictions. Pass caller identity from an authenticated supervisor
session. Request bodies cannot override these policies. Rebase is the sole
supported publication method. There is no admin, force, squash or auto-merge
mode. Every request binds its key, PR number, scope, head and base; a key cannot
be reused for another PR or caller.

`GitHubForge::new` requires absolute paths to the installed `gh` executable and
a dedicated private credential home. Provision its GitHub credential under a
separate publisher service account. Workers and verifiers must run as different
accounts without access to that credential home or the gateway journal. File
permissions do not isolate mutually hostile processes sharing an OS account.
The adapter clears the inherited environment, fixes `GH_HOST=github.com` and
`GH_CONFIG_DIR`, and executes argument arrays without a shell. It never changes
the global gh login and never includes command stderr or tokens in errors.

The adapter reads the PR, latest decisive review per reviewer, check runs and
commit statuses. It rejects incomplete evidence pages. Configure checks using
`check-run:<GitHub-app-id>:<name>` or `status:<creator-user-id>:<context>` so a
different issuer cannot satisfy the configured check by copying its name.
Only completed successful check runs and successful statuses pass. Dismissed
reviews, changes requested, changed head/base, unknown/pending checks, foreign
scope, owner-only calls and stale generations fail admission. Evidence is read
again immediately before dispatch. GitHub also atomically enforces the expected
PR head through `gh pr merge --rebase --match-head-commit <head>` and its own
branch protection. GitHub does not offer an atomic expected-base precondition
for this command; base freshness is checked before dispatch, and remote base
movement during the request remains subject to forge protection.

Before any write, the gateway persists an ambiguous intent by syncing a private
temporary journal, renaming it and syncing its parent directory. It holds one
unresolved slot across every PR/task/generation on that target. A failed sync
poisons the running service; restart and inspect the retained journal. Exact
retry returns the original record and never resends the merge. `rotate_policy`
requires an increasing generation on the same project/repository/target and
preserves unresolved occupancy. Reopen with the last persisted policy.

Use `reconcile(key)` after a timeout or restart. A matching GitHub merged-PR
observation records a scope-bound receipt with the actual merge commit. An open
PR or unchanged branch is outcome-unknown and retains the slot: a delayed
request might still apply. The GitHub adapter deliberately does not infer
definitive no-effect from a failed command, closed PR or missing response. Other
trusted adapters may report no-effect only after independently proving the
original request is quiescent and never applied. Repair unresolved GitHub cases
under operator custody; do not retry through another publisher.

The ordinary P19 test target exercises real durable file/lock operations,
restart, revocation, stale evidence, collision and timeout boundaries. An
isolated executable fixture checks the GitHub adapter's exact arguments,
credential environment and lost-reply reconciliation without network access:

```sh
cargo build --workspace --locked
cargo test -p admiralty-testkit --test publication_gate --locked
```

Live forge qualification requires an explicitly configured disposable repository
and separately provisioned publisher identity. The local test receipt does not
claim a live GitHub mutation or activate publication in production.
