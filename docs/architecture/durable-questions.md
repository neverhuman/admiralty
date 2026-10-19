# Durable question journal

`admiralty-store::questions` adds a caller-owned SQLite writer API around the pure `admiralty-core` question reducer. Call `initialize(&Connection)` before use, then call `append(&mut Connection, &JournalScope, &MessageId, &QuestionEvent)` for a command and `reconstruct(&Connection, &JournalScope)` to reopen a question. The scope binds `RootId`, question, project, task, and authority reference. Root is stored independently because the core reducer's `QuestionScope` does not contain it.

Each append runs in an SQLite `IMMEDIATE` transaction. The command ID is immutable and binds a digest over the complete scope and event. An exact retry returns its original receipt. Reusing a command ID with changed bytes or scope returns an identity collision. Reducer expected revisions provide compare-and-swap behavior; stale commands leave no event, projection, escalation proposal, or command receipt behind. Storage retains at most 1,024 questions per root and 66 command receipts per question, including duplicate receipts.

The journal stores bounded canonical JSON event bytes, a content digest, scope digest, sequence, previous digest, and chained digest. Event JSON is serialized through a bounded writer before it is materialized, and its unsigned 64-bit clock values remain decimal JSON integers. Receipt sequence and revision counters are canonical decimal strings. History is capped at the reducer's 33 event limit; capacity failures retain every existing event and never evict receipts.

On reopen, `reconstruct` uses one consistent read transaction to parse each bounded event, verify canonical reserialization, scope, identity, sequence, event digest and chain, then replay the pure reducer. It regenerates the canonical public projection and compares both its bytes and digest with SQLite. Persisted projection JSON is never deserialized into reducer state or trusted as authority. Deadline decisions use only the immutable `observed_utc` stored in `EvaluateDeadline`; replay never reads a clock. The shared core streaming JSON limit admits escaped bytes before allocating serialized copies, and stored text is bounded before copying it into owned strings.

Every command receipt, including a cached return, is checked against its actual canonical event and original command identity. A receipt's self-hash alone is insufficient. Event sequence, revision, duplicate flag, command digest and event-specific escalation reference must agree; every event must retain its original receipt. An exact command retry returns its original receipt. A new command replaying an old opening event after expiry receives a duplicate opening receipt with no fabricated expiry proposal.

The Store now exposes `question_command` and `question` on its existing sole
writer. Trusted startup installs `QuestionAuthority` through
`open_with_question_authority`, or combines independent policies with
`open_with_startup_authorities` and the non-deserializable `StoreAuthorities`.
Default construction installs none. Authorization runs inside every writer turn,
including exact retries and reads, against the writer's current task projection.
The public actor/source/policy fields are binding data, never grants.

Protected commands retain their complete canonical provenance and original
writer-supplied task contract in the same transaction as the existing journal.
Historical retries validate full provenance coverage and return the original
receipt when currently authorized. Legitimate contract revision preserves
historical evidence; fresh stale-contract actions and reads return
`StaleContract`. Startup, rebuild and backup verify original historical bindings
without requiring them to equal a later task contract.

Global storage is bounded at4,096questions,8,192commands and32MiB of stored text
across all roots, in addition to per-question bounds. A command is at most32KiB,
provenance8KiB and16evidence references; capacity refuses new writes without
eviction. The raw caller-owned Connection APIs remain compatibility components;
they do not acquire the protected Store authorization boundary.

An expiry creates one deterministic escalation proposal reference and its bounded proposal record in the same transaction as the event, projection, and command receipt. The proposal is data only. This module has no delivery, network, LLM, terminal, timer, or external-effect path. Callers remain responsible for actor/grant authorization before append; a valid reducer transition is not an authorization grant.

The proposal digest includes the journal root. Tests cover canonical forged receipts with recomputed hashes, missing historical receipts, real Store scope mismatches, two-connection revision races, and SQL receipt insertion failure rolling back the event, projection and escalation together. Capacity failures retain earlier receipts; an exact retry remains readable at capacity. These are storage integrity checks, not independent authentication against an owner who can rewrite the entire database and all hashes.

No production question issuer, HTTP route, provider delivery, effective decision
authority or auto-answer is installed by this registration.
