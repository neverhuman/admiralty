use crate::{TaskContract, content_digest};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextCapsule {
    pub task_id: String,
    pub revision: u32,
    pub context_epoch: String,
    pub contract_digest: String,
    pub text: String,
    pub estimated_tokens: usize,
    pub tokenizer: String,
}

pub fn compile_task_capsule(
    contract: &TaskContract,
    epoch: &str,
    max_estimated_tokens: usize,
) -> Result<ContextCapsule, String> {
    contract.validate().map_err(|e| e.to_string())?;
    if epoch.is_empty() {
        return Err("context epoch is required".into());
    }
    let digest = content_digest(contract).map_err(|e| e.to_string())?;
    // JSON quoting keeps newlines and hostile source text from masquerading as protocol fields.
    let fact = |value: &str| serde_json::to_string(value).map_err(|e| e.to_string());
    let mut text = format!(
        "ADMIRALTY/1 task={} revision={} epoch={}\ncontract={}\ngoal={}\nauthority={}\n",
        contract.task_id,
        contract.revision,
        fact(epoch)?,
        digest,
        fact(&contract.objective)?,
        fact(&contract.authority_ref)?
    );
    text.push_str(&format!(
        "project={}\nroot={}\ndependencies={}\n",
        fact(contract.project_id.as_str())?,
        fact(contract.root_id.as_str())?,
        serde_json::to_string(&contract.dependencies).map_err(|e| e.to_string())?
    ));
    text.push_str(&format!(
        "write={}\ndeny={}\nnon_goals={}\n",
        serde_json::to_string(&contract.owned_paths).map_err(|e| e.to_string())?,
        serde_json::to_string(&contract.forbidden_paths).map_err(|e| e.to_string())?,
        serde_json::to_string(&contract.non_goals).map_err(|e| e.to_string())?
    ));
    for criterion in &contract.criteria {
        text.push_str(&format!(
            "criterion {}={} checks={}\n",
            fact(&criterion.id)?,
            fact(&criterion.text)?,
            serde_json::to_string(&criterion.check_ids).map_err(|e| e.to_string())?
        ));
    }
    text.push_str(&format!("checks={}\nread_first={}\nattempt_limit={}\nsimulated={}\nnext=implement within scope; submit immutable candidate; do not publish or claim acceptance\n",serde_json::to_string(&contract.checks).map_err(|e|e.to_string())?,serde_json::to_string(&contract.read_first).map_err(|e|e.to_string())?,contract.max_attempts,contract.simulated));
    let estimated_tokens = text.len().div_ceil(4);
    if estimated_tokens > max_estimated_tokens {
        return Err(format!(
            "essential capsule needs approximately {estimated_tokens} tokens, limit {max_estimated_tokens}; expand budget or split task, never truncate constraints"
        ));
    }
    Ok(ContextCapsule {
        task_id: contract.task_id.to_string(),
        revision: contract.revision,
        context_epoch: epoch.into(),
        contract_digest: digest,
        text,
        estimated_tokens,
        tokenizer: "utf8_bytes_div4_estimate".into(),
    })
}
