//! Public permit API smoke probe. Synthetic claims are rejected deliberately;
//! this example does not launch a process or qualify a live resource backend.
use admiralty_core::{AttemptId, ProjectId, TaskId, nodes::NodeId};
use admiralty_runtime::resource_gate::{
    AdmissionIdentity, ResourcePermitAuthority, ResourcePermitClaims, ResourceGateError,
    VerifiedResourcePermit,
};
use std::{collections::BTreeSet, error::Error, sync::Arc};

struct RejectUnissuedPermit;

impl ResourcePermitAuthority for RejectUnissuedPermit {
    fn is_current(&self, _: &ResourcePermitClaims) -> bool {
        false
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let claims = ResourcePermitClaims {
        issuer_ref: "unissued-probe".into(),
        permit_id: "unissued-probe".into(),
        reservation_ref: "unissued-probe".into(),
        identity: AdmissionIdentity {
            project_id: ProjectId::new("probe")?,
            task_id: TaskId::new("probe")?,
            attempt_id: AttemptId::new("probe")?,
            node_id: NodeId::new("probe")?,
            lease_id: "unissued-probe".into(),
        },
        host_incarnation: "unissued-probe".into(),
        expires_at_unix_ms: 0,
        lease_expires_at_unix_ms: 0,
        registered_cpu_scope: BTreeSet::new(),
        cpu_set: BTreeSet::new(),
        memory_max_bytes: 0,
        max_tasks: 0,
        max_concurrency: 0,
        launch_spec_digest: String::new(),
        worker_environment_digest: String::new(),
        backend_config_digest: String::new(),
    };

    match VerifiedResourcePermit::verify_current(claims, Arc::new(RejectUnissuedPermit)) {
        Err(ResourceGateError::PermitUnavailable) => {
            println!("PASS: unauthenticated permit claims fail closed");
            Ok(())
        }
        Err(error) => Err(error.into()),
        Ok(_) => Err("rejected probe authority unexpectedly issued a permit".into()),
    }
}
