# Claude offline observation fixtures

The original help/version and JSON/JSONL vectors are retained byte-for-byte from
the imported P21 candidate. This hardening run invoked no provider and does not
independently attest how those original observations were collected.

Files prefixed `synthetic-` are constructed regression vectors, not live provider
receipts. They cover a session-bound denial, an explicit failed turn, opaque
child usage with missing parent terminal usage, and duplicate counters. Further
negative events are constructed directly in `tests/integration/provider_claude.rs`.
All fixture lookups resolve from checked-in repository paths.

The strict parser requires one leading init for fixture dialect 2.1.302 and a
session-bound parent result, with every frame LF-terminated. The original
`approval-denial.jsonl` lacks terminal session identity and is deliberately
retained as a rejection vector. `turn-boundary-resume.jsonl` lacks an explicit
error flag: its terminal outcome remains unknown. No parser return proves
process EOF, journal continuity, actual model/configuration binding, resume
execution, trusted task acceptance, or native managed execution.

Only parent terminal usage can populate the turn's usage projection. Missing
categories remain unknown. Parent assistant and opaque child observations retain
separate ordered digests and native counters; their inclusion/overlap is
unqualified and never inferred as settled billing. Malformed supplied counters
fail parsing rather than falling back to lower earlier values.

Launch argv is an UNADMITTED lexical proposal. Explicit supported permission
modes are acceptEdits, dontAsk, manual and plan. `default` omits the CLI flag;
`auto` is outside this proposal's supported subset, and captured `--restricted`
help explicitly excludes bypassPermissions. Fresh explicit session IDs must be
UUIDs; resume requires an explicit bounded selector. `--print` is emitted.

Input frames preserve LF, CR and tab through JSON escaping, reject other control
characters, and streaming-count the complete encoded frame before allocation.
The frame plus final LF must fit the actual session-host input limit (4096 bytes).
Larger task capsules require retrieval or separately qualified chunking; they
are rejected, never truncated. All runtime/configuration/resume/usage
qualification flags remain false. Immutable launch admission, journal/capture
binding, real continuity and native child accounting remain separate work.
