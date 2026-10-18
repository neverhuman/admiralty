# Conflict graph candidate component

`admiralty-core::conflicts` provides bounded pure analysis for proposed candidate
sets. It validates candidate identities, SHA-256 commit/tree/ancestry digests,
relative component paths, file counts and ordered hunk ranges, then returns a
deterministically ordered risk graph and a proposed frozen generation.

The graph marks overlapping hunks, same-path changes with unknown/whole-file
hunks, ancestor-directory ownership, missing ancestry and differing base
identities. A missing hunk list is unknown, not an empty change. Sibling names
such as `src` and `src2` have no ownership relationship. Identical repeated
candidate content is idempotent; reusing a candidate ID with different bytes
is a collision. The probe cache key binds the full candidate records (including
base, head, tree and ancestry) and policy revision; equal trees with different
ancestry therefore have different identities.

An empty edge list does not establish mergeability. Pairwise checks cannot
capture all three-way effects, so every multi-candidate generation explicitly
requires combined-generation verification. This component does not invoke Git,
run merge probes, rewrite changes, choose a target writer, publish, or accept a
generation. Real merge probes, one-writer runtime behavior, semantic/build
impact checks, frozen review flow and external acceptance remain pending.
