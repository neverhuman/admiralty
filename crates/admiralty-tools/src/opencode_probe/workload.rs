//! Remote qualification workload sent verbatim over SSH as data.
//!
//! The owner oracle and the remote harness live in `payload/` as data files,
//! byte-identical to the constants of the retired `tools/probe_opencode.py`.
//! They are not repository programs: the qualification task is a Python
//! exercise by design, so the harness is interpreted by the qualified node's
//! own `/usr/bin/python3`, and its starting stub (`raise NotImplementedError`)
//! is the state the agent under test must replace. The oracle's SHA-256 is
//! recorded in receipts and compared across qualifications.
//! Changing a single byte changes `oracle_owner_expected_sha256` and breaks
//! comparability with earlier receipts, so tests pin their digests.

/// Independent owner oracle (unittest program) for the `gaps.py` task.
pub const ORACLE: &str = include_str!("payload/owner_oracle.txt");

/// Exact task prompt given to the remote coding helper.
pub const PROMPT: &str = r#"Implement only gaps.py: uncovered_ranges(start, end, covered) returns sorted disjoint half-open integer intervals in [start,end) not covered by any supplied interval. Clip and union coverage, merging touching/overlapping ranges; ignore zero-width coverage; support negative endpoints and empty windows. Reject non-integer endpoints including bool, malformed pairs, and reversed intervals/windows with TypeError or ValueError. Accept pairs as lists or tuples. Do not mutate input. Python standard library only. All required contract and visible tests are inside this checkout: read README.md and test_gaps.py; run python3 test_gaps.py . after implementing. Do not access paths outside this candidate checkout. README.md and test_gaps.py are protected: do not modify them. Owner already pinned a baseline Git commit; do not create or alter commits. Do not install packages, use network, inspect credentials, or modify any other file or existing project. Stop with a concise result. No delegation or model switching."#;

/// Remote harness executed as `/usr/bin/python3 -c REMOTE` on the node.
pub const REMOTE: &str = include_str!("payload/remote_harness.txt");
