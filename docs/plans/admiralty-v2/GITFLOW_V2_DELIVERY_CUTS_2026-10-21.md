# Execution addendum: delivery cuts and actual integration-test registration

This supplements the frozen master plan; it does not alter its supplied inputs or overwrite a delivered packet. The same user authorization, ownership and evidence rules apply.

## Ship useful behavior without holding it for the entire index

The immediate viewport/audit cut landed through PR166 at receiving main `5916726f8c085f2687ec756508914c5a713fa5bc` on21October2026 at05:03:25UTC. Its exact-head independent approval and six PR checks were observed before the normal matched rebase merge. Its receiving-main tree is identical to the qualified `4dab245` tree. Main checks, paired protected audit-clone/helper deployment and actual browser acceptance remain separate evidence.

Packet A's broader loaded-history trunk-first repository investigation is the next independently useful delivery cut, on `codex/git-flow-trunk-first-main` based on that actual receiving main. PR166 is closed; it does not qualify this wider successor. Once this cut has its own clean full qualification, legitimate exact-head independent approval, current hosted gates and ordinary merge/main CI, the existing sole deployment owner may roll it out. Do not hold its visible improvement merely because the complete-history index is still being implemented.

The full history/domain/cache/server/worker-client packets remain authorized active work after that cut; a camera/backbone release is explicitly **not** full-spec completion. Their next integration train starts from the actual receiving main only after the previous active train closes. This preserves one active integration PR and permits genuine incremental delivery rather than one enormous mixed PR.

The G2-1 domain worker's input branch is not a second integration PR. It may finish pure types/tests while UI Packet A proceeds. The sole integrator decides whether to consume a genuinely ready domain foundation in the current cut or preserve its exact input commit for the next cut. Never transfer a test/approval result to a changed base or wider diff. No unused stub is added merely to reserve a filename.

Urgent audit repair is separate: only the actual existing owner can choose an explicit combined or serialized priority disposition under the Python exception policy. This addendum does not grant an exception, create a repair PR or supersede that owner decision.

## Register the real new integration test target

The backend source mapping verified that merely adding `tests/integration/git_history.rs` does not register it. Follow the current API-stream precedent in `crates/admiralty-testkit/Cargo.toml` (which uses `autotests = false`) or another explicitly inspected existing test owner. If using testkit, the integrator adds:

```toml
[[test]]
name = "git_history"
path = "../../tests/integration/git_history.rs"
```

Inspect actual crate dependencies and the neighboring registered target before applying this block. Add only the required test dependencies through the integrator. Then run the actual registered target:

```sh
rtk proxy cargo test -p admiralty-testkit --test git_history --locked
```

Do not cite this command as passing before the target exists and executes. The full final matrix must include the registered test; raw logs and the source receipt identify its actual assertion/test counts.
