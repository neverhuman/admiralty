# Fleet placement proposals

`admiralty-core::placement` is a pure decision component. It consumes a task's project, root, task, immutable contract digest, required capabilities, locality references and budget requirement, plus bounded node observations and an explicit decision time. It returns a deterministically ordered list of feasible candidate nodes and per-node rejection reasons. Locality score ranks first; validated node ID breaks ties. The component makes no clock, randomness, transport, process, filesystem or hardware calls.

Placement requires fresh observations for project visibility, capabilities, account pool, capacity, pressure and spool reserve. Unknown or stale values are rejected. Serialized input is capped at 1 MiB; offers at 256; collections at 1,024; strings at 4,096 bytes; node IDs at 128 bytes. All externally serialized u64 values use canonical decimal strings and checked parsing.
The same streaming bound applies to already-typed inputs. `decode_input`
strictly decodes a bounded request/offers/policy/recorded-clock envelope;
unknown and duplicate JSON fields fail closed. The caller must bound any
file/body read before making the byte slice. Context strings reject blanks and
controls, collections reject duplicate entries, and contract identities require
a complete SHA-256 digest rather than a prefix. No input is mutated.

Each offer must include a fresh `ownership_observed_at` and
`ownership_revision` matching the controller's expected policy revision. A
missing/stale/future ownership observation, unknown revision, or reconciliation
requirement on any offered node blocks placement across the complete offered
topology. A disconnected or nonordinary reported root is not free ownership.
The caller must supply the complete canonical topology; this pure function
cannot discover omitted nodes or verify self-reported ownership. Root IDs in
active-root records are treated conservatively as topology-wide identities,
so ambiguous cross-project root names may block rather than grant placement.

Any observed ordinary active root blocks another placement, including evidence from a disconnected node. Candidate selection does not settle conflicts in ownership reports: duplicate active-root evidence is an explicit error. The same disconnected node can only be proposed to continue its own active root under an existing immutable isolated permit bound to the same root/contract, boot identity and authority generation, with sufficient remaining budget and spool reserve. Reboot, generation change or an explicit reconciliation requirement rejects it.
An isolated continuation also binds exact project, task, node ID and authority
reference, and must have an explicit expiry strictly after the decision tick.
A disconnected node without a pre-existing active root cannot use a permit to
start a new root. Current spool observation must remain known and fresh in
addition to the permit's reserved spool/budget quantities. A permit-shaped
record is still a caller assertion; it must be retrieved and validated through
the authoritative grant/custody service before any effect.

Use `propose_placement_recorded` with a known decision tick and the prior
canonical decision tick. Unknown time and rollback fail explicitly; logical
tick zero is valid. The older `propose_placement(..., u64)` wrapper supplies a
known tick without a prior-history floor and is not a substitute for recording
that history. No wall clock is read. Proposal `source_digest` binds the full
request, sorted node offers, policy and clock context; project/root/task,
contract digest and authority reference remain in the result. The digest is
stable across offer delivery order but is not proof that any observation or
grant is authentic.

Every proposal sets `execution_granted`, `publication_granted` and `authority_granted` to false. A candidate is not an admission or execution permit. This pure component does not implement node enrollment, partition transport, reconciliation, publication, shutdown, runner isolation, or hardware qualification. Qualification remains pending actual partition transport and integration with authoritative grant/custody services.
No live placement loop is activated. Native integration still requires an
authenticated complete topology, recorded monotonic clocks, exact task/grant
lookup, current fenced leases, atomic resource reservation and dispatch intent,
effect-boundary custody rechecks and uncertain-effect reconciliation. Fresh
capacity counters are observations; they are never a reservation or permission
to start work.
