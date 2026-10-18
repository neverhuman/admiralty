# Conflict graph candidate component

`admiralty-core::conflicts` provides bounded pure analysis for proposed candidate
sets within one project/root scope; mixed scopes fail closed. It validates
candidate identities, Git commit/tree object IDs, SHA-256 ancestry references,
relative component paths, file counts and ordered hunk ranges, then returns a
deterministically ordered risk graph and a proposed frozen generation.
Git IDs may be native 40-hex SHA-1 or 64-hex SHA-256 strings, optionally prefixed
`sha1:` or `sha256:`. Algorithm and case are normalized for base comparison;
the original declared identities remain bound into the source digest. An
object-shaped ID is not evidence that Git contains that object.

The graph marks overlapping hunks, same-path changes with unknown/whole-file
hunks, ancestor-directory ownership, case aliases, missing ancestry and differing base
identities. A missing **or empty** hunk list is unknown, not an empty change.
Supplied closed line ranges must be sorted, nonoverlapping and nonzero;
touching endpoints overlap. Cross-candidate interval comparisons use a linear
walk instead of multiplying hunk counts. Sibling names
such as `src` and `src2` have no ownership relationship. Identical repeated
candidate content is idempotent for the graph and cache; reusing a candidate ID
with different typed content is a collision. Candidate delivery order and
exact repeated records do not change the resulting graph or cache identity.
The probe cache key and graph `source_digest` bind the full candidate records (including
base, head, tree and ancestry) and policy revision; equal trees with different
ancestry therefore have different identities.
The digest also binds `ANALYSIS_VERSION`; analysis/cache semantics changes must
bump it independently of caller-supplied policy labels. The graph reports this
version, but neither the version nor a cache hit authenticates input facts.

File paths use an initial portable ASCII subset: at most 4096 bytes and 128
components, with no roots, empty/dot/traversal components, backslashes,
controls, wildcard characters, Windows device aliases or trailing dots/spaces.
Case aliases are rejected within one candidate and conservatively flagged
between candidates, even on a case-sensitive host. The root scope `.` is permitted
only as explicit owned/denied scope meaning the entire registered root, never
as a changed file. This lexical analysis neither resolves symlinks nor establishes
filesystem custody.

Optional `CandidateConstraints` records a declared immutable contract
revision/digest, owned paths, denied paths and named interface revisions.
Changes outside declared ownership or intersecting a declared denial are
rejected, including an ancestor directory change that could contain a denied
child. Cross-candidate overlapping ownership and writes against another
candidate's denial are graph risks even if the currently listed files are
disjoint. Different revisions of one named interface are a risk. Distinct
candidates for the same task are competing alternatives, not independent
work; inconsistent declared contract identities/scopes for one task ID fail
closed. A changed interface/contract declaration changes the cache identity.
Declarations remain caller assertions: the runtime must retrieve and compare
the exact canonical task contract before trusting their scope or digest.

Missing constraints add `unknown_write_scope` edges, and missing constraints
or ancestry also populate `unresolved_candidates` so a single candidate cannot
hide the absence behind an empty pairwise graph. Ancestry references are not
verified here. No empty-edge result means accepted, conflict-free or safe to
run.

Source input and already-typed records are bounded to 1 MiB without allocating
a complete JSON copy during validation. `parse_candidates` rejects duplicate
JSON fields and unknown nested fields; its caller must bound file/body reads
before making the byte slice. Bounds also include 128 input records, 1024 files
and owned/denied paths per candidate, 128 interface records per candidate,
8192 total supplied hunks, and a checked one-million path/interface comparison
budget. Oversized or too-dense inputs return errors rather than a partial or
empty graph. Input records are not mutated. These input limits count transport
records, so excessive repeated deliveries can still exceed a bound.

An empty edge list does not establish mergeability. Pairwise checks cannot
capture all three-way effects, so every multi-candidate generation explicitly
requires combined-generation verification. This component does not invoke Git,
run merge probes, rewrite changes, choose a target writer, publish, or accept a
generation. Real merge probes, one-writer runtime behavior, semantic/build
impact checks, frozen review flow and external acceptance remain pending.
The proposed generation and source digest are data, not a freeze lock, lease,
resource permit, target-writer assignment or merge/acceptance capability. This
component is not wired into a worker scheduler or integration train and does
not enable native fleet execution. Cross-root alias relationships, trusted
diff extraction (including both rename endpoints), real merge/build probes,
authoritative interface/contract lookup and exact-generation verification
still require runtime integration.
