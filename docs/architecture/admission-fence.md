# Process-local admission commit fence

`StoreAuthorities.admission_fence` is an opt-in trusted-startup policy lease.
The default is `None`, preserving the existing sampled-authority profile.
Request JSON cannot install, replace or bypass it. This component installs no
authenticated catalog loader, startup issuer or default execution route.

The Store clones the installed fence Arc and acquires its guard before BEGIN,
before any protected authority sample or write. That outer guard survives the
entire slotted append, including exact replay, incident-only commits, rollback
and errors. Both original budget and slot authorities remain mandatory; the
guard is an additional constraint, never a grant. With this admission-only
profile installed, every TaskClaimed event requires budget and slot inputs.
Standalone reservation/slot mutations are denied, including setup, settlement
and stop-journal writes: no independently authenticated mutation path is
installed by this component. Ordinary non-claim events remain available. The
unfenced profile retains its existing compatibility and sampled semantics.

The concrete `PairedAdmissionCohort` produces budget, slot and fence adapters
as one opaque bundle. Its private catalog owns bounded exact-rule digests;
there are no external mutable registry references or caller policy callbacks.
The catalog has no public request/config/JSON constructor. Only scoped unit
fixtures construct test catalogs; a real authenticated loader is separate work.
`proofs_for` looks up an already installed exact rule and cannot add one.

The complete binding includes original source/sequence, canonical event bytes
digest, observation time, original effect bytes/key/digests/schedule, entire
budget/slot command digests and their scope/intent associations. Private proof
types bind the same coordinator Arc identity, current epoch and distinct
budget/slot/read roles. A second coordinator with identical textual values,
mixed epochs, a changed binding or exchanged proof roles does not authorize.
Contract serialization is bounded before digest allocation.

One RwLock owns all participating rules, epoch and revocation state. A retained
read guard excludes successful rotation/revocation until the SQL operation
returns. `try_revoke` and `try_rotate` use exclusive nonblocking acquisition and
an expected epoch; Busy changes nothing and does not claim revocation succeeded.
Applied advances the checked epoch and makes old proofs ineligible for the next
operation. Callbacks may perform a bounded nonblocking read of that same state
while the lease is held; all writes use try_write, with no queued blocking
writer, Store reentry, I/O or callback mutation. Contention/poison/overflow deny.
Dropping this guard releases a local policy lease, never a durable hold or debt.

The coherence claim covers only the paired cohort's own participating state.
A generic fence over unrelated callbacks cannot make those callbacks coherent.
A local epoch cannot retain an external expiring credential or authenticate
physical pool ownership/allotment, external consumers or model configuration.
Those require independent trusted installation and retained capabilities.
The Store lease ends before controller dispatch and physical process start;
it does not fence those later boundaries or attest live filesystem inode custody.
There is no provider execution, settlement/release, authority restart recovery,
general execution qualification or parent acceptance from this component.

Actual Store tests use private installed test catalogs and the same concrete
paired authorities. A bounded test-only wrapper pauses an actual delegated
policy while the real Store writer holds its lease; an independent revoker
observes Busy. Later Applied revocation rejects old proofs. Tests also cover
current exact replay with original receipts, weaker-path rejection, late CAS
rollback and outbox-incident-only commit with retained admission invariants.
Primitive tests cover read roles/cohort identity, rotation, overflow and poison.
These are simulated Store fixtures and never coding-provider turns.
