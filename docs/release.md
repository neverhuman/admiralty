# Release control

The [release status](operators/release-status.md) and
[execution ledger](plans/admiralty-v2/EXECUTION.md) own acceptance and exact
source evidence. The [changelog](../CHANGELOG.md) records delivered components.
Cargo workspace and JavaScript package manifests contain the version; a healthy
`0.1.0` service does not mean the planned native major release is published.

Before publication, reconcile every remaining parent criterion, run the full
local application matrix and supported provider/fleet/recovery/install journeys,
and disposition standard-mode audit blockers. Record exact source, independent
review, commands/exits/counts and artifact hashes. A role communication cycle
does not prove admitted coding, independent acceptance, settlement or recovery.

Use [source packaging](operators/source-package.md),
[native distribution qualification](operators/native-distribution-qualification.md),
[archive verification](operators/native-archive.md) and
[private installation](operators/native-install.md) for the implemented tooling.
Complete clean-machine/support-target, SBOM/provenance, rights and signing
qualification before [publication](operators/publication.md). Preserve failed
attempts alongside successful evidence. Do not push release tags until the
release plan and maintainer's authorization cover them.

Production deployment has one designated writer. Use the
[deployment procedure](operators/deployment.md) and
[upgrade/recovery rules](operators/upgrades.md), retain a verified prior bundle,
and verify manifest files, actual process identities and HTTP readback after
cutover. Rollback and unknown effects remain explicit decisions; never create a
second Store writer or assume that an exited PID has released every lock.
