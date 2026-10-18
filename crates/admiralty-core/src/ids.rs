use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error(
    "identifier must contain 1..128 ASCII letters, digits, underscores, hyphens or dots, and cannot be dot/dot-dot"
)]
pub struct InvalidId;

macro_rules! identifier {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, InvalidId> {
                let value = value.into();
                if value.is_empty() || value.len() > 128 || value == "." || value == ".."
                    || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)) {
                    return Err(InvalidId);
                }
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str { &self.0 }
        }
        impl TryFrom<String> for $name { type Error = InvalidId; fn try_from(v:String)->Result<Self,Self::Error>{Self::new(v)} }
        impl From<$name> for String { fn from(v:$name)->Self{v.0} }
        impl fmt::Display for $name { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{self.0.fmt(f)} }
    )+};
}
identifier!(
    ProjectId,
    RootId,
    TaskId,
    AttemptId,
    SessionId,
    PrincipalId,
    MessageId,
    ProducerId,
    CommandId
);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contracts_reject_path_and_empty_identifiers() {
        for id in ["", "..", "../x", "a/b", "a b", "💣"] {
            assert!(TaskId::new(id).is_err());
            assert!(serde_json::from_value::<TaskId>(serde_json::json!(id)).is_err());
        }
        assert!(TaskId::new("P00.a-1").is_ok());
    }
}
