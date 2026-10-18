use crate::{TaskContract, content_digest};
use serde::{Deserialize, Serialize};
use std::io::Write;

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
    if epoch.trim().is_empty() {
        return Err("context epoch is required".into());
    }
    // Stop while writing, before making an oversized serialized copy. JSON
    // escaping counts against the budget and cannot introduce protocol lines.
    let mut output = CapsuleBytes {
        bytes: Vec::new(),
        max_bytes: max_estimated_tokens.saturating_mul(4),
    };
    let mut digest_start = 0;
    const DIGEST_PLACEHOLDER: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    let rendered = (|| -> std::io::Result<()> {
        write!(
            output,
            "ADMIRALTY/1 task={} revision={} epoch=",
            contract.task_id, contract.revision
        )?;
        output.json(epoch)?;
        output.write_all(b"\ncontract=")?;
        digest_start = output.bytes.len();
        output.write_all(DIGEST_PLACEHOLDER.as_bytes())?;
        output.field("goal", &contract.objective)?;
        output.field("authority", &contract.authority_ref)?;
        output.field("project", &contract.project_id)?;
        output.field("root", &contract.root_id)?;
        output.field("dependencies", &contract.dependencies)?;
        output.field("write", &contract.owned_paths)?;
        output.field("deny", &contract.forbidden_paths)?;
        output.field("non_goals", &contract.non_goals)?;
        for criterion in &contract.criteria {
            output.write_all(b"\ncriterion ")?;
            output.json(&criterion.id)?;
            output.write_all(b"=")?;
            output.json(&criterion.text)?;
            output.write_all(b" checks=")?;
            output.json(&criterion.check_ids)?;
        }
        output.field("checks", &contract.checks)?;
        output.field("read_first", &contract.read_first)?;
        write!(
            output,
            "\nattempt_limit={}\nsimulated={}\nnext=implement within scope; submit immutable candidate; do not publish or claim acceptance\n",
            contract.max_attempts, contract.simulated
        )?;
        Ok(())
    })();
    if rendered.is_err() {
        return Err(format!(
            "essential capsule exceeds approximately {max_estimated_tokens} tokens; expand budget or split task, never truncate constraints"
        ));
    }
    // Hash only after proving the essential rendering fits, using the original
    // contract rather than a context-dependent or truncated representation.
    let digest = content_digest(contract).map_err(|e| e.to_string())?;
    output.bytes[digest_start..digest_start + DIGEST_PLACEHOLDER.len()]
        .copy_from_slice(digest.as_bytes());
    let text = String::from_utf8(output.bytes).map_err(|e| e.to_string())?;
    let estimated_tokens = text.len().div_ceil(4);
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

struct CapsuleBytes {
    bytes: Vec<u8>,
    max_bytes: usize,
}

impl CapsuleBytes {
    fn json<T: Serialize + ?Sized>(&mut self, value: &T) -> std::io::Result<()> {
        serde_json::to_writer(self, value).map_err(std::io::Error::other)
    }

    fn field<T: Serialize + ?Sized>(&mut self, name: &str, value: &T) -> std::io::Result<()> {
        write!(self, "\n{name}=")?;
        self.json(value)
    }
}

impl Write for CapsuleBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|len| len > self.max_bytes)
        {
            return Err(std::io::Error::other(
                "essential capsule byte budget exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn bounded_renderer_refuses_oversized_fragment_without_copying_it() {
        let mut output = CapsuleBytes {
            bytes: Vec::new(),
            max_bytes: 32,
        };
        output.write_all(b"goal=").expect("small prefix fits");
        let large_source = "x".repeat(1024 * 1024);
        assert!(output.json(&large_source).is_err());
        assert!(output.bytes.len() <= 32);
        assert!(
            output.bytes.capacity() <= 32,
            "budget cannot allocate the large source fragment"
        );
    }

    #[test]
    fn json_escaping_is_part_of_the_byte_budget() {
        let mut output = CapsuleBytes {
            bytes: Vec::new(),
            max_bytes: 5,
        };
        assert!(
            output.json("\n\n").is_err(),
            "two source bytes need six JSON bytes"
        );
        assert!(output.bytes.len() <= 5);
    }

    #[test]
    fn streaming_contract_digest_keeps_existing_json_sha256_identity() {
        use sha2::{Digest, Sha256};
        let value =
            serde_json::json!({"goal":"do not publish\nkeep 5 ms","checks":["test","--exact"]});
        let previous = format!(
            "sha256:{:x}",
            Sha256::digest(serde_json::to_vec(&value).expect("fixture JSON"))
        );
        assert_eq!(
            content_digest(&value).expect("streaming fixture digest"),
            previous
        );
    }
}
