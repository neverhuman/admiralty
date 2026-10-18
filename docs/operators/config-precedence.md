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
empty/dot components, backslashes, controls, or colons. This initial portable
subset is ASCII, at most 4096 bytes and 128 components, and rejects wildcard
characters, Windows device names and trailing dots/spaces. It rejects
unsupported names rather than silently normalizing a denial into a different
path. Per-path provenance records
the highest-precedence public layer that supplied the denial; a matching
private denial is attributed to `private`.

`PrivatePolicy` is a separate caller-supplied trusted value. It is not
deserializable from public config and unknown `private_policy` keys are rejected
by strict input structs. Public agent/context limits above private maxima fail
closed, even if a later public layer would lower them. The private policy and
its denials are retained unchanged in the resolved result. Provenance marks
private contributions explicitly instead of presenting them as Runtime.
Every supplied model list is checked for nonzero concurrency and the private
agent maximum, even when a later empty list replaces it. Duplicate model/node
IDs are rejected in each layer rather than being hidden by replacement. The
effective model concurrency must also fit the final public agent limit; node
references are checked against the final node list so cross-layer references
remain possible.

Each public JSON layer is limited to 256 KiB, including layers supplied as
already-constructed Rust values. Lists contain at most 256 entries; IDs are
bounded portable ASCII; other public text is bounded and excludes controls.
Duplicate JSON fields and unknown nested fields fail decoding. Callers still
must bound file/body reads before calling `parse_layer`; this component does
not open any input itself.

The existing `provenance` map records the winning origin for each field and
denied path. `provenance_history` additionally retains ordered contributing
origins and their original `source_ref` values, so replacement does not erase
the earlier source identity. Private entries have origin `private` and no
invented public source reference. `source_layers` retains each ordered public
layer's typed SHA-256 digest; changing a shadowed value changes its receipt even
when the effective result stays the same. Digests bind content, not the
authenticity of a caller-supplied source label. Inputs and the trusted policy
are not mutated, and full earlier values are not copied into provenance.

This module only parses and resolves values. It grants no execution,
verification, acceptance, or integration authority; callers remain responsible
for establishing the trusted private policy and for any later policy checks.
In particular, `denied_paths` is a retained policy set, not a filesystem access
check: a runtime must compare component boundaries and account for canonical
paths, symlinks and platform case aliases at the operation boundary. This
prototype does not load the daemon's configuration or connect resolved limits
to worker/resource permits. File ownership, source trust, listen-address
authorization and provider/credential validation remain caller integration
requirements. Importing it does not enable native fleet execution or make
public limits into private authority.
