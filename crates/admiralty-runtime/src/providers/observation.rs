//! Strict bounded JSONL observations shared by offline provider prototypes.
//!
//! Values returned before `finish` are provisional. Parsing does not establish
//! process identity, journal continuity, capture completion, admission or authority.
use serde::Deserialize;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::fmt;
use thiserror::Error;

pub const MAX_EVENT_BYTES: usize = 256 * 1024;
pub const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_EVENTS: usize = 8192;

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum ObservationError {
    #[error("provider JSONL observation exceeds bounds")]
    Bounds,
    #[error("provider JSONL observation is malformed or ambiguous")]
    Json,
    #[error("provider JSONL observation has an unterminated frame")]
    Framing,
    #[error("provider JSONL decoder is already finished")]
    Closed,
}

#[derive(Default)]
pub struct JsonlDecoder {
    pending: Vec<u8>,
    total_bytes: usize,
    events: usize,
    failure: Option<ObservationError>,
    finished: bool,
}

impl JsonlDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    fn fail<T>(&mut self, error: ObservationError) -> Result<T, ObservationError> {
        self.failure = Some(error);
        self.pending.clear();
        Err(error)
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Value>, ObservationError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.finished {
            return Err(ObservationError::Closed);
        }
        if bytes.len() > MAX_STREAM_BYTES.saturating_sub(self.total_bytes) {
            return self.fail(ObservationError::Bounds);
        }
        self.total_bytes += bytes.len();
        let mut values = Vec::new();
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            let terminated = part.last() == Some(&b'\n');
            let segment = if terminated {
                &part[..part.len() - 1]
            } else {
                part
            };
            if segment.len() > MAX_EVENT_BYTES.saturating_sub(self.pending.len()) {
                return self.fail(ObservationError::Bounds);
            }
            self.pending.extend_from_slice(segment);
            if !terminated {
                continue;
            }
            if self.pending.iter().all(u8::is_ascii_whitespace) {
                self.pending.clear();
                continue;
            }
            if self.events >= MAX_EVENTS {
                return self.fail(ObservationError::Bounds);
            }
            let value = match serde_json::from_slice::<StrictValue>(&self.pending) {
                Ok(StrictValue(value)) if value.is_object() => value,
                _ => return self.fail(ObservationError::Json),
            };
            self.pending.clear();
            self.events += 1;
            values.push(value);
        }
        Ok(values)
    }

    /// Close an explicitly complete byte stream. A partial final line is never EOF.
    pub fn finish(&mut self) -> Result<(), ObservationError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if self.finished {
            return Err(ObservationError::Closed);
        }
        if !self.pending.is_empty() {
            return self.fail(ObservationError::Framing);
        }
        self.finished = true;
        Ok(())
    }
}

pub fn decode_complete(bytes: &[u8]) -> Result<Vec<Value>, ObservationError> {
    let mut decoder = JsonlDecoder::new();
    let values = decoder.push(bytes)?;
    decoder.finish()?;
    Ok(values)
}

// Value alone accepts duplicate fields. Check every nested object before any
// provider interprets status, identity, permission or usage counters.
struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StrictVisitor;
        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = StrictValue;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("unambiguous bounded JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Bool(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(v.into())))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Number(v.into())))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| StrictValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(v.into())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::String(v)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                self.visit_unit()
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some(value) = seq.next_element::<StrictValue>()? {
                    if out.len() >= MAX_EVENTS {
                        return Err(de::Error::custom("JSON array bound"));
                    }
                    out.push(value.0);
                }
                Ok(StrictValue(Value::Array(out)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut out = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.len() >= MAX_EVENTS || out.contains_key(&key) {
                        return Err(de::Error::custom("duplicate field or JSON map bound"));
                    }
                    out.insert(key, map.next_value::<StrictValue>()?.0);
                }
                Ok(StrictValue(Value::Object(out)))
            }
        }
        deserializer.deserialize_any(StrictVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_split_preserves_complete_crlf_unicode_objects() {
        let bytes =
            "{\"type\":\"assistant\",\"text\":\"π🦀\"}\r\n\n{\"type\":\"result\"}\n".as_bytes();
        let expected = decode_complete(bytes).expect("valid bounded test fixture");
        assert_eq!(expected.len(), 2);
        for split in 0..=bytes.len() {
            let mut decoder = JsonlDecoder::new();
            let mut actual = decoder
                .push(&bytes[..split])
                .expect("valid bounded test fixture");
            actual.extend(
                decoder
                    .push(&bytes[split..])
                    .expect("valid bounded test fixture"),
            );
            decoder.finish().expect("valid bounded test fixture");
            assert_eq!(actual, expected, "split {split}");
        }
    }

    #[test]
    fn duplicates_scalars_invalid_utf8_and_trailing_json_fail_sticky() {
        for bytes in [
            &b"{\"is_error\":true,\"is_error\":false}\n"[..],
            b"{\"usage\":{\"output_tokens\":100,\"output_tokens\":1}}\n",
            b"{\"array\":[{\"x\":1,\"x\":2}]}\n",
            b"null\n",
            b"true\n",
            b"[]\n",
            b"42\n",
            b"\"event\"\n",
            b"{\"a\":1} {\"b\":2}\n",
            b"{\"x\":\"\xff\"}\n",
        ] {
            let mut decoder = JsonlDecoder::new();
            assert_eq!(decoder.push(bytes), Err(ObservationError::Json));
            assert_eq!(decoder.push(b"{}\n"), Err(ObservationError::Json));
            assert_eq!(decoder.finish(), Err(ObservationError::Json));
        }
    }

    #[test]
    fn partial_line_is_not_eof_and_finished_decoder_cannot_resume() {
        let mut decoder = JsonlDecoder::new();
        assert!(
            decoder
                .push(b"{}")
                .expect("valid bounded test fixture")
                .is_empty()
        );
        assert_eq!(decoder.finish(), Err(ObservationError::Framing));
        assert_eq!(decoder.push(b"\n"), Err(ObservationError::Framing));
        let mut complete = JsonlDecoder::new();
        complete.push(b"{}\n").expect("valid bounded test fixture");
        complete.finish().expect("valid bounded test fixture");
        assert_eq!(complete.push(b"{}\n"), Err(ObservationError::Closed));
        assert_eq!(complete.finish(), Err(ObservationError::Closed));
        assert!(decode_complete(b"{}\n ").is_err());
        assert!(
            decode_complete(b"\n\r\n")
                .expect("valid bounded test fixture")
                .is_empty()
        );
    }

    #[test]
    fn exact_line_event_and_aggregate_bounds_are_enforced() {
        let mut line = b"{\"s\":\"".to_vec();
        line.resize(MAX_EVENT_BYTES - 2, b'x');
        line.extend_from_slice(b"\"}\n");
        assert_eq!(
            decode_complete(&line)
                .expect("valid bounded test fixture")
                .len(),
            1
        );
        line.insert(6, b'x');
        assert_eq!(decode_complete(&line), Err(ObservationError::Bounds));
        let mut events = b"{}\n".repeat(MAX_EVENTS);
        assert_eq!(
            decode_complete(&events)
                .expect("valid bounded test fixture")
                .len(),
            MAX_EVENTS
        );
        events.extend_from_slice(b"{}\n");
        assert_eq!(decode_complete(&events), Err(ObservationError::Bounds));
        let mut decoder = JsonlDecoder::new();
        let blanks = vec![b'\n'; MAX_EVENT_BYTES];
        for _ in 0..(MAX_STREAM_BYTES / MAX_EVENT_BYTES) {
            decoder.push(&blanks).expect("valid bounded test fixture");
        }
        assert_eq!(decoder.push(b"\n"), Err(ObservationError::Bounds));
        assert_eq!(decoder.finish(), Err(ObservationError::Bounds));
    }
}
