# Offline publication regression coverage

The `codex_worktree_publication` and `publication_gate` integration targets test
pure publication proposals and a deterministic fake forge. They launch no Codex
or Claude provider, send no network request, publish no branch and grant no task
acceptance. Their names describe retained fixture data, not native P08/P19/P21
qualification. Full publication gateway/admission/runtime and real transport
gates in WORK_PACKETS remain open.

The local Git fixture creates a private repository and worktree, commits known
bytes with recorded timestamps, and preserves the original target ref. Its
publication snapshot binds approval/check evidence to that exact candidate head
and its base. A changed valid candidate must reject the prior approval and check
evidence. Only the synthetic forge applies the resulting fake CAS; the fixture
maps the admitted expected base to the simulated target and admitted candidate
head to the simulated new ref. No arbitrary post-admission candidate is supplied.

The lost-reply test records a single simulated mutation, observes uncertainty,
reconciles its recorded outcome and proves exact replay does not apply twice.
It does not qualify a real forge API, authenticate a reviewer or prove a real
remote mutation occurred.

Fixture custody and process limits:

- Temporary roots use the canonical temporary base and mode0700 at creation.
  Artifact directories also use mode0700 at creation, including under umask0002.
- Git is the installed `/usr/bin/git` on the declared Unix test platforms. Each
  invocation clears inherited environment, disables global/system configuration,
  hooks, signing and interactive prompts, and disables template copying at init.
- Each owned child has a10second deadline and64KiB cap per output stream, with
  one overflow byte examined before allocation. Safe nonblocking socket reads
  have bounded syscall batches; there are no blocking capture-reader threads.
- Failure kills only the direct owned unreaped child, then polls for at most
  two seconds. Rare incomplete OS cleanup is reported explicitly and transfers
  only the owned handle to an eventual reaper. A child already observed/reaped
  is never signaled; its success-path wait reads the cached exit status.
- Offline child controls prove deadline/output-cap handling and inject poisoned
  Git directory/index/config/hooks into a child test process. A private positive
  hook detector first fires; the protected invocation then leaves the other
  disposable repository and detector unchanged. No global environment/umask is
  mutated by those Rust tests.

Validation:

```text
rtk proxy env CARGO_TARGET_DIR=/absolute/owned/target CARGO_BUILD_JOBS=2 cargo test -p admiralty-testkit --locked --test codex_worktree_publication --test publication_gate
rtk proxy env CARGO_TARGET_DIR=/absolute/owned/target CARGO_BUILD_JOBS=2 cargo clippy -p admiralty-testkit --locked --test codex_worktree_publication --test publication_gate -- -D warnings
```

The ignored fixture-child test is explicitly spawned and reaped by its parent
controls. The named targets and their source receipt establish these component
regressions only; they do not establish full-workspace, provider, fleet, release,
economic or parent-packet acceptance.
