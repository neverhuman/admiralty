# Verification receipt prototype

A verification receipt is a bounded record of the immutable task contract,
candidate identity, protected oracle identity, exact declared checks, and
observed output digests and exits. The core parser accepts at most 1 MiB of
JSON and 256 checks. Unknown fields are rejected. Source commit/tree IDs must
be full lowercase Git object IDs (40 or 64 hexadecimal characters); content
digests use `sha256:` followed by 64 lowercase hexadecimal characters.

Validation first validates the original task contract and verifies that the
independently supplied expected project, root, task, and revision identify that
contract. It then requires every receipt input to equal the complete expected
input record, including the contract digest, source commit and tree, source
artifact, and protected oracle. The expected oracle digest is required and
cannot be replaced by a value supplied only by a provider claim.

Each task check must occur exactly once, in contract order, with the full
`CheckSpec` unchanged: ID, program, argument vector, working directory, and
timeout. An absent or nonzero exit means checks did not all pass. Output
digests are validated and included in the canonical serialized receipt digest,
so changing recorded output evidence changes the receipt digest. Reprocessing
the same receipt yields the same digest.

`ReceiptProducer::ProviderClaim` is rejected by independent receipt
validation. A receipt marked `IndependentVerifier` is still only a consistency
claim at this layer: hashing the data does not authenticate the verifier,
prove custody of outputs, or show that the named commands were executed. The
caller must authenticate the verifier principal and evidence custody
independently. This prototype never executes commands and makes no signature
or cryptographic provenance claim.

Even a fully consistent receipt with zero exits returns
`acceptance_granted: false`. Verification of the exact frozen candidate,
current policy and non-revoked dependencies, followed by integration or owner
approval, remains a separate decision. A receipt for one commit/tree cannot
approve another candidate head.
