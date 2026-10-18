# Configuration precedence and trust boundary

The pure `admiralty-core::config` prototype accepts bounded JSON layers in
this order: Defaults, User, Project, Runtime. Defaults are required and must
provide every public field. Later layers may omit fields; a supplied scalar
replaces the prior value. `nodes` and `models` are whole-list replacements,
including an explicit empty list. Resolution rejects duplicate, skipped-back,
or otherwise out-of-order layers and does not read files, environment
variables, sockets, or remote URLs.

`denied_paths` is intentionally different: entries from every layer are
combined as a sorted set. A later empty list cannot remove an earlier denial.
Paths must be relative component paths without absolute roots, traversal,
empty/dot components, backslashes, NUL, or colons. Per-path provenance records
the highest-precedence public layer that supplied the denial; a matching
private denial is attributed to `private`.

`PrivatePolicy` is a separate caller-supplied trusted value. It is not
deserializable from public config and unknown `private_policy` keys are rejected
by strict input structs. Public agent/context limits above private maxima fail
closed, even if a later public layer would lower them. The private policy and
its denials are retained unchanged in the resolved result. Provenance marks
private contributions explicitly instead of presenting them as Runtime.

This module only parses and resolves values. It grants no execution,
verification, acceptance, or integration authority; callers remain responsible
for establishing the trusted private policy and for any later policy checks.
