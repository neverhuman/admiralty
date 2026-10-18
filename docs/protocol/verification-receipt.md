# Verification receipt prototype

`admiralty_core::verification` parses bounded receipt claims and checks their
consistency against independently supplied task, candidate and verifier-policy
inputs. It never executes checks, reads a checkout or output artifact,
authenticates a verifier, or grants acceptance. It is not a qualified trusted
verifier and is not active fleet acceptance policy.

## Inputs and protected policy

Use `validate_receipt(&contract, &expected_inputs, &policy, &receipt)`.
`VerificationInputs` binds project, root, task, attempt, immutable candidate
reference, contract revision and digest, verification-policy digest, source
commit and tree, candidate artifact digest and protected-oracle digest.
The expected inputs must come from authoritative candidate/attempt state,
not be copied from a provider's receipt. The original task contract is validated
and hashed; its exact identity, revision and digest must agree with expected
inputs before any result is reported.

`VerificationPolicy` has schema version 1, a pinned verifier principal,
protected-oracle digest and 1 through 64 `RequiredNegativeCheck` entries.
Each entry has an exact `CheckSpec`, `expected_exit_code` and pinned negative
fixture digest. Expected inputs must pin the hash of the complete policy and
its protected-oracle digest. Changing a fixture, expected exit, verifier or
check arguments therefore invalidates that pin. The caller must load this
policy from protected custody outside the candidate's write authority and
validate the actual artifact closure; matching digest strings do not prove
custody or isolation.

`VerificationReceipt` contains its schema version, safe receipt ID, producer
label, verifier principal, complete inputs, ordinary `checks` and required
`negative_checks`. The receipt verifier must match the pinned policy identity.
A producer labelled `ProviderClaim` is rejected by this independent-receipt
interface. Merely changing that label to `IndependentVerifier` still proves
nothing about its source; this component cannot authenticate that claim.

## Exact checks and outputs

Every ordinary contract check must occur exactly once, in contract order,
with unchanged ID, program, argument vector, working directory and timeout.
Every required negative check must likewise appear once in protected policy
order with exact arguments. Ordinary and negative check IDs must be distinct
across the entire receipt; duplicated, omitted, reordered or renamed checks
are rejected rather than treated as partial success.

An ordinary check claims success only with exit zero. A negative check claims
success only with its independently pinned expected exit. An assertion test
that proves invalid input is rejected normally expects zero; a command that
must directly reject an invalid fixture may explicitly expect a nonzero exit.
The receipt cannot choose its own expected result. Missing exits do not pass;
portable exit codes are 0 through 255, with absent exit representing unknown
or unavailable completion. These are reported exit claims, not proof that
any process ran.

Each check records SHA-256 digests of stdout and stderr. Exact candidate
inputs, policy, argv, exits and output digest identities contribute to the
canonical receipt hash. Changing evidence changes that hash. Validating a
digest's shape does not prove that bytes exist or match it; the caller must
retrieve and verify immutable outputs through trusted artifact custody.

## Bounded validation

`parse_receipt` accepts at most 1 MiB of JSON, rejects unknown/duplicate fields
and invalid typed IDs, then validates receipt shape. Direct typed calls to
`validate_receipt` enforce the same serialized byte limit; bypassing parsing
does not bypass bounds. The complete task contract and verification policy
are independently limited to 1 MiB before hashing. Serialization counts JSON
escaping without allocating another full serialized copy.

There may be at most 256 ordinary-plus-negative results, with at most 64
negative results. Each check has a safe 1 through 128 byte ASCII ID, program
of at most 256 UTF-8 bytes, at most 128 arguments of at most 4 KiB each,
repository-relative working directory of at most 1 KiB, and timeout from
1 through 86,400 seconds. NUL arguments and control characters in program or
working-directory names are rejected. Arguments remain exact literal strings,
including permitted whitespace; no shell expansion or normalization occurs.

Receipt IDs use the same bounded safe ASCII identity alphabet. Candidate and
authority references contain at most 512 UTF-8 bytes and no whitespace or
control characters. Contract titles are at most 512 bytes; objectives 16 KiB;
non-goals and criterion text 4 KiB each. Non-goals have at most 64 entries;
path, dependency and criterion collections have at most 256 entries, and
paths at most 1 KiB each. The complete-byte limit still applies when individual
fields fit. Existing typed revision/attempt-budget bounds and positive-value
contract requirements also apply.

Source commit/tree IDs must be distinct full lowercase Git object IDs in one
hash format: both 40 or both 64 hexadecimal characters, excluding Git's null
object ID. Content and policy digests use `sha256:` plus 64 lowercase hexadecimal
characters. These checks do not establish Git object type or existence.
Parse errors report a bounded location rather than echoing arbitrary source
field names or purported output into logs.

## Replay, identity collisions and result authority

Stateless validation of identical inputs produces the same digest and result.
For receipt-ID collision detection use
`ReceiptRegistry::new(max_receipts)` and its
`validate_and_record(contract, expected, policy, receipt)` method. The bounded
registry retains up to 4,096 receipt IDs and digests. Exact replay is idempotent;
a changed digest under an existing ID is an integrity error. At capacity an
existing exact replay remains valid, but new input returns explicit
backpressure. No receipt identity is silently evicted. Failed-check claims
are retained as failed-check facts rather than converted to successful ones.

The registry is pure in-memory policy, not durable storage. The caller must
journal inputs and atomically persist registry decisions and any downstream
outbox before acknowledging input. Reconstruct that history for replay and
define safe retirement before long-lived activation. Resetting it to bypass
a bound destroys collision protection.

`ValidatedReceipt` is a serialize-only sealed result with getters, not an
untrusted deserialization format or public mutable trust record. Even if
`all_checks_passed()` is true, all of the following remain false:
`acceptance_granted()`, `verifier_authenticated()`, `execution_qualified()` and
`evidence_custody_verified()`. A compile-fail test prevents JSON from being
parsed directly into the validation result. Receipt inputs carrying those
trust flags are unknown fields and are rejected.

Current grant revocation, exact candidate/attempt custody, actual verifier
execution, artifact retrieval, protected negative fixtures, dependencies,
integration approval and acceptance must be checked by the eventual trusted
executor and authoritative decision path. This prototype qualifies none of
those effects. Its offline tests establish bounded consistency and replay
behavior only.
