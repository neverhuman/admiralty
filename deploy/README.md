# Deploy Admiralty

Start here. A merged PR is **not** a deployment.

The current production host is **xbabe2**. The operator URL is
http://localhost:18890/#/git through the existing SSH forward. Keep that forward
running while the user needs it.

## Choose one deployment path

| Change | Path |
| --- | --- |
| Dashboard assets only; deployed server and API remain compatible | [UI-only deployment](UI_ONLY.md) |
| Rust binaries, server behavior, contracts, observer, database or private configuration changes | [Full release](FULL_RELEASE.md) |
| New host, or migrating a host to systemd units (not used on xbabe2 today) | [Clean deployment with systemd](../docs/operators/deployment-systemd.md) |

Do not rebuild Rust just to publish a compatible dashboard change. The server's
`--assets` directory is read for each request; publishing a verified UI bundle
does not require restarting the daemon.

Use one operator and the existing deployment locks. Existing user/maintainer
authorization covers the ordinary deployment; do not introduce another approval
conversation. Git review and CI requirements still apply to the exact source.

## The six steps

1. **Freeze source:** record the reviewed head, receiving main, complete tree and
   applicable successful checks. Resolve blocking findings.
2. **Build:** build that source with the pinned toolchain and retain its log.
3. **Stage:** copy into a new private directory on xbabe2. Compare every file's
   size and SHA-256 with the local manifest.
4. **Switch:** acquire both locks, verify the actual running process and rollback
   material, then perform one atomic publication.
5. **Verify:** check served entry, lazy chunks, CSS and workers, then drive the
   actual user Chrome page with real repository data.
6. **Report:** say which UI and runtime revisions are live, what was visibly
   verified, where rollback lives, and what remains incomplete.

[Verification](VERIFY.md) is mandatory. [Rollback](ROLLBACK.md) must be prepared
before the switch. The [21 October incident](2026-10-21-ui591-rollout.md) shows the
actual component deployment and why the previous full-build route was unsuitable.

## Use precise status words

| State | Required evidence |
| --- | --- |
| Merged | Actual GitHub merge and resulting source/tree |
| Built | Successful build log and exact artifact manifest |
| Staged | Remote hash/size readback; production unchanged |
| Serving | Production HTTP bytes match the staged entry and all assets |
| Browser verified | Actual user Chrome loaded those assets and passed the journey |
| Deployed | Serving and browser verification complete for the stated component |

A running process, green CI, an offline replay or a source screenshot alone does
not establish deployment. A UI-only release can have a newer UI revision than
its unchanged runtime. Record both; never relabel the old executable.

## Rules that prevent another confusing rollout

- Preserve failed attempts and their logs. Diagnose before a new attempt.
- Never reuse a consumed one-attempt allocation or reset a tainted ledger.
- Never overwrite a private configuration, database or foreign checkout.
- Never delete old hashed assets during publication; existing tabs may use them.
- Never use a bot account to manufacture independent approval.
- Never claim a full native/provider/scientific release from a dashboard rollout.
- All shell commands use RTK. xbabe2 does not currently have RTK installed:
  remote commands are passed through the local `rtk proxy ssh` invocation.
