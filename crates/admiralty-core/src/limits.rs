//! Shared streaming JSON admission bounds. Does not allocate a serialized copy.
use serde::Serialize;
use std::io::{self, Write};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum JsonLimitError {
    #[error("serialized JSON exceeds its byte budget")]
    TooLarge,
    #[error("value cannot be encoded as JSON")]
    InvalidEncoding,
}

/// Counts escaped UTF-8 JSON bytes while streaming to a discard sink. The input
/// is already caller-owned; custom Serialize implementations remain trusted code.
pub fn bounded_json_size<T: Serialize + ?Sized>(
    value: &T,
    max_bytes: usize,
) -> Result<usize, JsonLimitError> {
    struct Counter {
        used: usize,
        max: usize,
        exceeded: bool,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            match self.used.checked_add(bytes.len()) {
                Some(next) if next <= self.max => {
                    self.used = next;
                    Ok(bytes.len())
                }
                _ => {
                    self.exceeded = true;
                    Err(io::Error::other("JSON byte budget exceeded"))
                }
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut sink = Counter {
        used: 0,
        max: max_bytes,
        exceeded: false,
    };
    let encoded = serde_json::to_writer(&mut sink, value);
    if sink.exceeded {
        Err(JsonLimitError::TooLarge)
    } else if encoded.is_err() {
        Err(JsonLimitError::InvalidEncoding)
    } else {
        Ok(sink.used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_includes_escaping_and_utf8_without_truncating() {
        let value = "\"☃\n";
        let expected = serde_json::to_vec(value).expect("fixture JSON").len();
        assert_eq!(bounded_json_size(value, expected), Ok(expected));
        assert_eq!(
            bounded_json_size(value, expected - 1),
            Err(JsonLimitError::TooLarge)
        );
        assert_eq!(bounded_json_size(&(), 0), Err(JsonLimitError::TooLarge));
    }

    #[test]
    fn nested_input_is_bounded_before_copying_its_serialized_form() {
        let value = vec![vec!["x".repeat(1_000_000)]];
        assert_eq!(
            bounded_json_size(&value, 1024),
            Err(JsonLimitError::TooLarge)
        );
    }

    #[test]
    fn encoding_failure_is_distinct_from_exhausted_budget() {
        let value = std::collections::BTreeMap::from([(vec![1, 2], "invalid JSON object key")]);
        assert_eq!(
            bounded_json_size(&value, 1024),
            Err(JsonLimitError::InvalidEncoding)
        );
    }
}
