# Fleet placement and disconnected continuation

Placement consumes an explicit bounded inventory. Node IDs are opaque; neither
hostnames nor a fixed fleet size select a node. Current capability, account pool,
capacity, pressure, locality and ownership observations determine proposals.
Unknown, future, stale or unreconciled ownership closes placement for the entire
topology. An observed active root prevents another node from taking that root.

A disconnected node may only propose continuation of its already active root
under the exact project/task/node/contract/authority and boot generation in its
unexpired isolated permit. Remaining budget must cover the requested work.
Current free spool must cover the permit's reserved spool, and that reserve must
meet both the permit minimum and current policy minimum. Spool growth after
disconnect can therefore stop continuation even while the permit is unexpired.
No proposal grants execution, publication or new authority.

Reconnect, reboot and a shutdown request do not prove termination or release
custody. Keep active-root ownership and unknown liabilities until the controlling
durable boundary has reconciled termination, spool acknowledgement and budget
settlement. Do not remove an unreachable node from inventory to make work fit:
missing observations must remain unknown and block ownership reconciliation.
Placement does not replace Store transactions, contiguous inbox ACKs, custody
closure or authenticated execution admission.

Run `cargo test -p admiralty-testkit --test fleet_partition --locked` for
synthetic partition, spool depletion, reboot, reconnect and clock-skew checks,
including four random UUID nodes. These tests never connect to fleet hosts.
