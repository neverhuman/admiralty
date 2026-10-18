//! Deterministic, offline provider-process fixtures for test harnesses.
//!
//! The byte chunks are synthetic provider output. They never start a real
//! provider, read credentials, or access the network.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FakeMode {
    Success,
    Quiet,
    Malformed,
    Crash,
    BlockedInput,
    Noisy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FakeProviderSpec {
    pub mode: FakeMode,
    pub seed: u64,
}

impl FakeProviderSpec {
    pub const fn new(mode: FakeMode, seed: u64) -> Self {
        Self { mode, seed }
    }

    /// Return deterministic process-output chunks, preserving read boundaries.
    pub fn chunks(self) -> Vec<Vec<u8>> {
        match self.mode {
            FakeMode::Quiet => Vec::new(),
            FakeMode::Success => vec![self.event(1, "message", "hello café", 0)],
            FakeMode::Malformed => {
                let mut chunks = vec![self.event(1, "message", "hello", 0)];
                chunks.push(b"{\"type\":\"usage\",\"input_tokens\":".to_vec());
                // A multi-byte character split across reads exercises incremental UTF-8.
                chunks.push(b"\"text\":\"caf\xc3".to_vec());
                chunks.push(b"\xa9\",\"simulated\":true}\n".to_vec());
                chunks
            }
            FakeMode::Crash => vec![self.event(1, "process_exit", "synthetic crash", 2)],
            FakeMode::BlockedInput => vec![self.event(1, "input_required", "permission", 0)],
            FakeMode::Noisy => (0..32)
                .map(|sequence| self.event(sequence + 1, "delta", "synthetic output", sequence))
                .collect(),
        }
    }

    /// Return all chunks concatenated as a JSONL byte stream.
    pub fn jsonl_bytes(self) -> Vec<u8> {
        self.chunks().into_iter().flatten().collect()
    }

    /// Synthetic process exit status, if the mode represents a process exit.
    pub const fn exit_code(self) -> Option<i32> {
        match self.mode {
            FakeMode::Crash => Some(2),
            _ => None,
        }
    }

    fn event(self, sequence: u64, kind: &str, text: &str, turn: u64) -> Vec<u8> {
        let input_tokens = 10 + (self.seed % 7);
        let output_tokens = 2 + (sequence % 5);
        format!(
            "{{\"type\":\"{kind}\",\"source\":\"fake-provider\",\"source_sequence\":{sequence},\"turn\":{turn},\"model_segment\":\"synthetic-model-a\",\"text\":\"{text}\",\"usage\":{{\"input_tokens\":{input_tokens},\"cache_write_tokens\":1,\"cache_read_tokens\":2,\"output_tokens\":{output_tokens},\"reasoning_tokens\":0}},\"simulated\":true}}\n"
        )
        .into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::{FakeMode, FakeProviderSpec};

    #[test]
    fn output_is_reproducible_and_seeded() {
        let spec = FakeProviderSpec::new(FakeMode::Success, 42);
        assert_eq!(spec.jsonl_bytes(), spec.jsonl_bytes());
        assert!(
            String::from_utf8(spec.jsonl_bytes())
                .unwrap()
                .contains("\"input_tokens\":10")
        );
        assert_ne!(
            spec.jsonl_bytes(),
            FakeProviderSpec::new(FakeMode::Success, 43).jsonl_bytes()
        );
    }

    #[test]
    fn structured_chunks_are_explicitly_simulated_and_have_usage_segments() {
        let bytes = FakeProviderSpec::new(FakeMode::Success, 7).jsonl_bytes();
        let line = String::from_utf8(bytes).unwrap();
        for field in [
            "\"source_sequence\":1",
            "\"turn\":0",
            "\"model_segment\":\"synthetic-model-a\"",
            "\"cache_write_tokens\":1",
            "\"cache_read_tokens\":2",
            "\"simulated\":true",
        ] {
            assert!(line.contains(field), "missing {field}");
        }
    }

    #[test]
    fn modes_cover_quiet_malformed_blocked_noisy_and_crash() {
        assert!(
            FakeProviderSpec::new(FakeMode::Quiet, 0)
                .chunks()
                .is_empty()
        );
        let malformed = FakeProviderSpec::new(FakeMode::Malformed, 0).chunks();
        assert!(
            malformed
                .iter()
                .any(|chunk| chunk == b"{\"type\":\"usage\",\"input_tokens\":")
        );
        assert!(malformed.iter().any(|chunk| chunk.ends_with(&[0xc3])));
        assert!(malformed.iter().any(|chunk| chunk.starts_with(&[0xa9])));
        assert!(
            FakeProviderSpec::new(FakeMode::BlockedInput, 0)
                .jsonl_bytes()
                .windows(b"input_required".len())
                .any(|window| window == b"input_required")
        );
        assert_eq!(FakeProviderSpec::new(FakeMode::Noisy, 0).chunks().len(), 32);
        assert_eq!(
            FakeProviderSpec::new(FakeMode::Crash, 0).exit_code(),
            Some(2)
        );
    }
}
