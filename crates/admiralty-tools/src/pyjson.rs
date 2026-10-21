//! Python-compatible JSON values.
//!
//! The retired tools relied on Python `json` and `dict` semantics: objects keep
//! insertion order (an overwrite keeps the original position), `json.dumps`
//! prints floats with `repr`, escapes with `ensure_ascii`, and equality treats
//! `True == 1 == 1.0`. This module reproduces exactly the behavior the ports
//! need so that every output byte and every comparison keeps parity.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::{self, Write as _};

use crate::error::ToolError;

/// A JSON value with Python semantics.
#[derive(Clone, Debug)]
pub enum Value {
    /// `None` / `null`.
    Null,
    /// `bool`.
    Bool(bool),
    /// `int` (bounded to `i128`).
    Int(i128),
    /// `float`.
    Float(f64),
    /// `str`.
    Str(String),
    /// `list`.
    Array(Vec<Value>),
    /// `dict` with insertion order.
    Object(Map),
}

/// Insertion-ordered string-keyed map (Python `dict`).
#[derive(Clone, Debug, Default)]
pub struct Map {
    entries: Vec<(String, Value)>,
}

impl Map {
    /// Empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn position(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|(name, _)| name == key)
    }

    /// `dict.get(key)`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.position(key).map(|index| &self.entries[index].1)
    }

    /// Mutable lookup.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.position(key).map(|index| &mut self.entries[index].1)
    }

    /// `key in dict`.
    pub fn contains_key(&self, key: &str) -> bool {
        self.position(key).is_some()
    }

    /// `dict[key] = value`; an existing key keeps its position.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) -> Option<Value> {
        let key = key.into();
        let value = value.into();
        match self.position(&key) {
            Some(index) => Some(std::mem::replace(&mut self.entries[index].1, value)),
            None => {
                self.entries.push((key, value));
                None
            }
        }
    }

    /// `dict.pop(key, None)`.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.position(key).map(|index| self.entries.remove(index).1)
    }

    /// Iterate in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.entries.iter().map(|(key, value)| (key, value))
    }

    /// Iterate keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.iter().map(|(key, _)| key)
    }

    /// Iterate values in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|(_, value)| value)
    }

    /// `dict.get(key, default)` with key-presence (not truthiness) semantics.
    pub fn get_or<'a>(&'a self, key: &str, default: &'a Value) -> &'a Value {
        self.get(key).unwrap_or(default)
    }
}

impl FromIterator<(String, Value)> for Map {
    fn from_iter<T: IntoIterator<Item = (String, Value)>>(iter: T) -> Self {
        let mut map = Map::new();
        for (key, value) in iter {
            map.insert(key, value);
        }
        map
    }
}

impl PartialEq for Map {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(key, value)| other.get(key).is_some_and(|theirs| value == theirs))
    }
}

/// Build an insertion-ordered object: `pyobj! { "a" => 1, "b" => "x" }`.
#[macro_export]
macro_rules! pyobj {
    () => { $crate::pyjson::Value::Object($crate::pyjson::Map::new()) };
    ($($key:expr => $value:expr),+ $(,)?) => {{
        let mut map = $crate::pyjson::Map::new();
        $( map.insert($key, $crate::pyjson::Value::from($value)); )+
        $crate::pyjson::Value::Object(map)
    }};
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Str(value.to_owned())
    }
}
impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Str(value)
    }
}
impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Self::Str(value.clone())
    }
}
impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}
impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::Int(i128::from(value))
    }
}
impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(i128::from(value))
    }
}
impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Self::Int(i128::from(value))
    }
}
impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Self::Int(i128::from(value))
    }
}
impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Self::Int(value as i128)
    }
}
impl From<i128> for Value {
    fn from(value: i128) -> Self {
        Self::Int(value)
    }
}
impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}
impl From<Vec<Value>> for Value {
    fn from(value: Vec<Value>) -> Self {
        Self::Array(value)
    }
}
impl From<Map> for Value {
    fn from(value: Map) -> Self {
        Self::Object(value)
    }
}
impl From<&Value> for Value {
    fn from(value: &Value) -> Self {
        value.clone()
    }
}
impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}

/// Shared `None` used as a default.
pub static NULL: Value = Value::Null;

impl Value {
    /// `isinstance(v, str)`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    /// `isinstance(v, dict)`.
    pub fn as_object(&self) -> Option<&Map> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    /// Mutable dict view.
    pub fn as_object_mut(&mut self) -> Option<&mut Map> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    /// `isinstance(v, list)`.
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// Mutable list view.
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// `v is None`.
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// `type(v) is int` (excludes `bool`).
    pub fn as_exact_int(&self) -> Option<i128> {
        match self {
            Self::Int(number) => Some(*number),
            _ => None,
        }
    }

    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(flag) => *flag,
            Self::Int(number) => *number != 0,
            Self::Float(number) => *number != 0.0,
            Self::Str(text) => !text.is_empty(),
            Self::Array(items) => !items.is_empty(),
            Self::Object(map) => !map.is_empty(),
        }
    }

    /// Python type name for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "NoneType",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "str",
            Self::Array(_) => "list",
            Self::Object(_) => "dict",
        }
    }

    /// `dict.get(key)` on a value that must be a dict (else `AttributeError`).
    pub fn get(&self, key: &str) -> Result<Option<&Value>, ToolError> {
        match self {
            Self::Object(map) => Ok(map.get(key)),
            other => Err(ToolError::type_error(format!(
                "'{}' object has no attribute 'get'",
                other.type_name()
            ))),
        }
    }

    /// `dict.get(key)` returning `None` for a missing key; dict required.
    pub fn get_value(&self, key: &str) -> Result<&Value, ToolError> {
        Ok(self.get(key)?.unwrap_or(&NULL))
    }

    /// `(v or {}).get(key)`: falsy values act as an empty dict.
    pub fn or_empty_get(&self, key: &str) -> Result<&Value, ToolError> {
        if self.truthy() {
            self.get_value(key)
        } else {
            Ok(&NULL)
        }
    }

    /// `dict[key]` (`KeyError` when missing; `TypeError` when not a dict).
    pub fn index_key(&self, key: &str) -> Result<&Value, ToolError> {
        match self {
            Self::Object(map) => map.get(key).ok_or_else(|| ToolError::Key(key.to_owned())),
            other => Err(ToolError::type_error(format!(
                "'{}' object is not subscriptable",
                other.type_name()
            ))),
        }
    }

    fn numeric(&self) -> Option<Numeric> {
        match self {
            Self::Bool(flag) => Some(Numeric::Int(i128::from(*flag))),
            Self::Int(number) => Some(Numeric::Int(*number)),
            Self::Float(number) => Some(Numeric::Float(*number)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum Numeric {
    Int(i128),
    Float(f64),
}

fn numeric_cmp(left: Numeric, right: Numeric) -> Option<Ordering> {
    match (left, right) {
        (Numeric::Int(a), Numeric::Int(b)) => Some(a.cmp(&b)),
        (Numeric::Float(a), Numeric::Float(b)) => a.partial_cmp(&b),
        (Numeric::Int(a), Numeric::Float(b)) => int_float_cmp(a, b),
        (Numeric::Float(a), Numeric::Int(b)) => int_float_cmp(b, a).map(Ordering::reverse),
    }
}

fn int_float_cmp(integer: i128, float: f64) -> Option<Ordering> {
    if float.is_nan() {
        return None;
    }
    if float.is_infinite() {
        return Some(if float > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    }
    let floor = float.floor();
    // |floor| < 2^127 for every finite f64 that matters here; larger values
    // saturate, which keeps the ordering correct for i128 inputs.
    let whole = floor as i128;
    match integer.cmp(&whole) {
        Ordering::Equal if float > floor => Some(Ordering::Less),
        other => Some(other),
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        if let (Some(left), Some(right)) = (self.numeric(), other.numeric()) {
            return numeric_cmp(left, right) == Some(Ordering::Equal);
        }
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Array(a), Self::Array(b)) => a == b,
            (Self::Object(a), Self::Object(b)) => a == b,
            _ => false,
        }
    }
}

/// Python `a < b` ordering for sort keys (tuples are `Array`).
pub fn py_cmp(left: &Value, right: &Value) -> Result<Ordering, ToolError> {
    if let (Some(a), Some(b)) = (left.numeric(), right.numeric()) {
        return Ok(numeric_cmp(a, b).unwrap_or(Ordering::Equal));
    }
    match (left, right) {
        (Value::Str(a), Value::Str(b)) => Ok(a.cmp(b)),
        (Value::Array(a), Value::Array(b)) => {
            for (x, y) in a.iter().zip(b) {
                if x != y {
                    return py_cmp(x, y);
                }
            }
            Ok(a.len().cmp(&b.len()))
        }
        _ => Err(ToolError::type_error(format!(
            "'<' not supported between instances of '{}' and '{}'",
            left.type_name(),
            right.type_name()
        ))),
    }
}

/// Stable sort by a Python key (`sorted(items, key=...)`), failing like
/// Python when two keys are incomparable.
pub fn py_sorted<T>(
    items: Vec<T>,
    key: impl Fn(&T) -> Result<Value, ToolError>,
) -> Result<Vec<T>, ToolError> {
    let mut keyed = items
        .into_iter()
        .map(|item| key(&item).map(|value| (value, item)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut failure = None;
    keyed.sort_by(|a, b| match py_cmp(&a.0, &b.0) {
        Ok(ordering) => ordering,
        Err(error) => {
            failure.get_or_insert(error);
            Ordering::Equal
        }
    });
    match failure {
        Some(error) => Err(error),
        None => Ok(keyed.into_iter().map(|(_, item)| item).collect()),
    }
}

/// Python dict-key identity for hashable scalars and tuples.
pub fn py_hash_key(value: &Value) -> Result<String, ToolError> {
    let mut key = String::new();
    push_hash_key(value, &mut key)?;
    Ok(key)
}

/// Python dict-key identity for a tuple of values.
pub fn py_tuple_key(values: &[&Value]) -> Result<String, ToolError> {
    let mut key = String::from("(");
    for value in values {
        push_hash_key(value, &mut key)?;
    }
    key.push(')');
    Ok(key)
}

fn push_hash_key(value: &Value, key: &mut String) -> Result<(), ToolError> {
    match value {
        Value::Null => key.push_str("N;"),
        Value::Bool(_) | Value::Int(_) | Value::Float(_) => match value {
            Value::Float(number) if number.fract() != 0.0 || !number.is_finite() => {
                let _ = write!(key, "F{};", number.to_bits());
            }
            Value::Float(number) => {
                let _ = write!(key, "I{};", *number as i128);
            }
            Value::Bool(flag) => {
                let _ = write!(key, "I{};", i128::from(*flag));
            }
            Value::Int(number) => {
                let _ = write!(key, "I{number};");
            }
            _ => {}
        },
        Value::Str(text) => {
            let _ = write!(key, "S{}:{};", text.len(), text);
        }
        other => {
            return Err(ToolError::type_error(format!(
                "unhashable type: '{}'",
                other.type_name()
            )));
        }
    }
    Ok(())
}

/// Insertion-ordered dictionary keyed by Python hash identity.
#[derive(Clone, Debug)]
pub struct OrderedDict<V> {
    index: HashMap<String, usize>,
    entries: Vec<V>,
}

impl<V> Default for OrderedDict<V> {
    fn default() -> Self {
        Self {
            index: HashMap::new(),
            entries: Vec::new(),
        }
    }
}

impl<V> OrderedDict<V> {
    /// Empty dictionary.
    pub fn new() -> Self {
        Self::default()
    }

    /// `d[key] = value`; existing keys keep their position.
    pub fn insert(&mut self, key: String, value: V) {
        match self.index.get(&key) {
            Some(&position) => self.entries[position] = value,
            None => {
                self.index.insert(key, self.entries.len());
                self.entries.push(value);
            }
        }
    }

    /// Lookup.
    pub fn get(&self, key: &str) -> Option<&V> {
        self.index.get(key).map(|&position| &self.entries[position])
    }

    /// Mutable lookup.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.index
            .get(key)
            .copied()
            .map(move |position| &mut self.entries[position])
    }

    /// Values in insertion order.
    pub fn into_values(self) -> Vec<V> {
        self.entries
    }

    /// Values in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------- decoding

/// Decoder options mirroring the hooks the Python tools passed.
#[derive(Clone, Copy, Debug, Default)]
pub struct LoadOptions {
    /// `object_pairs_hook` rejecting duplicate keys.
    pub reject_duplicates: bool,
    /// `parse_constant` rejecting `NaN`/`Infinity`.
    pub reject_constants: bool,
}

/// Decoding failure.
#[derive(Clone, Debug, PartialEq)]
pub enum LoadError {
    /// Malformed JSON.
    Syntax(String),
    /// Duplicate object key under `reject_duplicates`.
    DuplicateKey(String),
    /// `NaN`/`Infinity` under `reject_constants`.
    NonFinite(String),
    /// Nesting exceeded the interpreter recursion bound.
    TooDeep,
    /// Input was not UTF-8.
    Encoding,
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(message) => formatter.write_str(message),
            Self::DuplicateKey(key) => write!(formatter, "duplicate JSON field: {key}"),
            Self::NonFinite(token) => write!(formatter, "non-finite JSON: {token}"),
            Self::TooDeep => formatter.write_str("maximum recursion depth exceeded"),
            Self::Encoding => formatter.write_str("input is not valid UTF-8"),
        }
    }
}

impl std::error::Error for LoadError {}

const MAX_DEPTH: usize = 990;

/// `json.loads(text)` for a `str`.
pub fn loads(text: &str, options: LoadOptions) -> Result<Value, LoadError> {
    if text.starts_with('\u{feff}') {
        return Err(LoadError::Syntax(
            "Unexpected UTF-8 BOM (decode using utf-8-sig)".to_owned(),
        ));
    }
    let mut parser = Parser {
        bytes: text.as_bytes(),
        text,
        position: 0,
        options,
        depth: 0,
    };
    parser.skip_whitespace();
    let value = parser.value()?;
    parser.skip_whitespace();
    if parser.position != parser.bytes.len() {
        return Err(parser.error("Extra data"));
    }
    Ok(value)
}

/// `json.loads(data)` for `bytes` (UTF-8, optional BOM).
pub fn loads_bytes(data: &[u8], options: LoadOptions) -> Result<Value, LoadError> {
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let text = std::str::from_utf8(data).map_err(|_| LoadError::Encoding)?;
    loads(text, options)
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    position: usize,
    options: LoadOptions,
    depth: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> LoadError {
        let char_index = self.text[..self.position].chars().count();
        LoadError::Syntax(format!("{message}: char {char_index}"))
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn literal(&mut self, word: &str) -> bool {
        if self.bytes[self.position..].starts_with(word.as_bytes()) {
            self.position += word.len();
            true
        } else {
            false
        }
    }

    fn constant(&mut self, token: &str, value: f64) -> Result<Value, LoadError> {
        if self.options.reject_constants {
            return Err(LoadError::NonFinite(token.to_owned()));
        }
        let _ = value;
        Ok(Value::Float(value))
    }

    fn value(&mut self) -> Result<Value, LoadError> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b'n') if self.literal("null") => Ok(Value::Null),
            Some(b't') if self.literal("true") => Ok(Value::Bool(true)),
            Some(b'f') if self.literal("false") => Ok(Value::Bool(false)),
            Some(b'N') if self.literal("NaN") => self.constant("NaN", f64::NAN),
            Some(b'I') if self.literal("Infinity") => self.constant("Infinity", f64::INFINITY),
            Some(b'-') if self.bytes[self.position..].starts_with(b"-Infinity") => {
                self.position += "-Infinity".len();
                self.constant("-Infinity", f64::NEG_INFINITY)
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.error("Expecting value")),
        }
    }

    fn digits(&mut self) {
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.position += 1;
        }
    }

    fn number(&mut self) -> Result<Value, LoadError> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        match self.peek() {
            Some(b'1'..=b'9') => self.digits(),
            Some(b'0') => self.position += 1,
            _ => {
                self.position = start;
                return Err(self.error("Expecting value"));
            }
        }
        let mut is_float = false;
        if self.peek() == Some(b'.')
            && matches!(self.bytes.get(self.position + 1), Some(b'0'..=b'9'))
        {
            is_float = true;
            self.position += 2;
            self.digits();
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let exponent_start = self.position;
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            let digits_start = self.position;
            self.digits();
            if self.position > digits_start {
                is_float = true;
            } else {
                self.position = exponent_start;
            }
        }
        let token = &self.text[start..self.position];
        if is_float {
            token
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| self.error("Invalid number"))
        } else {
            match token.parse::<i128>() {
                Ok(number) => Ok(Value::Int(number)),
                Err(_) => token
                    .parse::<f64>()
                    .map(Value::Float)
                    .map_err(|_| self.error("Invalid number")),
            }
        }
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        let slice = self.bytes.get(at..at + 4)?;
        let text = std::str::from_utf8(slice).ok()?;
        if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        u32::from_str_radix(text, 16).ok()
    }

    fn string(&mut self) -> Result<String, LoadError> {
        // Opening quote.
        self.position += 1;
        let mut result = String::new();
        loop {
            let chunk_start = self.position;
            while let Some(byte) = self.peek() {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.position += 1;
            }
            result.push_str(&self.text[chunk_start..self.position]);
            match self.peek() {
                None => return Err(self.error("Unterminated string starting at")),
                Some(b'"') => {
                    self.position += 1;
                    return Ok(result);
                }
                Some(b'\\') => {
                    let escape = self.bytes.get(self.position + 1).copied();
                    match escape {
                        Some(b'"') => result.push('"'),
                        Some(b'\\') => result.push('\\'),
                        Some(b'/') => result.push('/'),
                        Some(b'b') => result.push('\u{8}'),
                        Some(b'f') => result.push('\u{c}'),
                        Some(b'n') => result.push('\n'),
                        Some(b'r') => result.push('\r'),
                        Some(b't') => result.push('\t'),
                        Some(b'u') => {
                            let Some(first) = self.hex4(self.position + 2) else {
                                return Err(self.error("Invalid \\uXXXX escape"));
                            };
                            let mut code = first;
                            let mut consumed = 6;
                            if (0xd800..=0xdbff).contains(&first)
                                && self.bytes.get(self.position + 6) == Some(&b'\\')
                                && self.bytes.get(self.position + 7) == Some(&b'u')
                                && let Some(second) = self.hex4(self.position + 8)
                                && (0xdc00..=0xdfff).contains(&second)
                            {
                                code = 0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
                                consumed = 12;
                            }
                            let Some(character) = char::from_u32(code) else {
                                return Err(self.error("Unpaired surrogate escape"));
                            };
                            result.push(character);
                            self.position += consumed;
                            continue;
                        }
                        _ => return Err(self.error("Invalid \\escape")),
                    }
                    self.position += 2;
                }
                Some(_) => return Err(self.error("Invalid control character at")),
            }
        }
    }

    fn enter(&mut self) -> Result<(), LoadError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            Err(LoadError::TooDeep)
        } else {
            Ok(())
        }
    }

    fn object(&mut self) -> Result<Value, LoadError> {
        self.enter()?;
        self.position += 1;
        let mut map = Map::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            self.depth -= 1;
            return Ok(Value::Object(map));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.error("Expecting property name enclosed in double quotes"));
            }
            let key = self.string()?;
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err(self.error("Expecting ':' delimiter"));
            }
            self.position += 1;
            self.skip_whitespace();
            let value = self.value()?;
            if self.options.reject_duplicates && map.contains_key(&key) {
                return Err(LoadError::DuplicateKey(key));
            }
            map.insert(key, value);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.position += 1;
                    self.skip_whitespace();
                }
                Some(b'}') => {
                    self.position += 1;
                    self.depth -= 1;
                    return Ok(Value::Object(map));
                }
                _ => return Err(self.error("Expecting ',' delimiter")),
            }
        }
    }

    fn array(&mut self) -> Result<Value, LoadError> {
        self.enter()?;
        self.position += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            self.depth -= 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.position += 1;
                    self.skip_whitespace();
                }
                Some(b']') => {
                    self.position += 1;
                    self.depth -= 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error("Expecting ',' delimiter")),
            }
        }
    }
}

// ---------------------------------------------------------------- encoding

/// `json.dumps` options.
#[derive(Clone, Copy, Debug)]
pub struct DumpOptions {
    /// `sort_keys`.
    pub sort_keys: bool,
    /// `indent`.
    pub indent: Option<usize>,
    /// First element of `separators`.
    pub item_separator: &'static str,
    /// Second element of `separators`.
    pub key_separator: &'static str,
    /// `ensure_ascii`.
    pub ensure_ascii: bool,
    /// `allow_nan`.
    pub allow_nan: bool,
}

impl DumpOptions {
    /// `json.dumps(v)` defaults.
    pub const PYTHON: Self = Self {
        sort_keys: false,
        indent: None,
        item_separator: ", ",
        key_separator: ": ",
        ensure_ascii: true,
        allow_nan: true,
    };

    /// `sort_keys=True`.
    pub const fn sorted(mut self) -> Self {
        self.sort_keys = true;
        self
    }

    /// `indent=n` (also switches the default item separator to `","`).
    pub const fn indent(mut self, indent: usize) -> Self {
        self.indent = Some(indent);
        self.item_separator = ",";
        self
    }

    /// `separators=(item, key)`.
    pub const fn separators(mut self, item: &'static str, key: &'static str) -> Self {
        self.item_separator = item;
        self.key_separator = key;
        self
    }

    /// `ensure_ascii=False`.
    pub const fn unicode(mut self) -> Self {
        self.ensure_ascii = false;
        self
    }

    /// `allow_nan=False`.
    pub const fn strict_floats(mut self) -> Self {
        self.allow_nan = false;
        self
    }
}

/// `json.dumps(value, **options)`.
pub fn dumps(value: &Value, options: &DumpOptions) -> Result<String, ToolError> {
    let mut out = String::new();
    encode(value, options, 0, &mut out)?;
    Ok(out)
}

/// `json.dumps(value)` with Python defaults (never fails: NaN allowed).
pub fn dumps_default(value: &Value) -> String {
    let mut out = String::new();
    // allow_nan=True cannot fail.
    let _ = encode(value, &DumpOptions::PYTHON, 0, &mut out);
    out
}

fn newline_indent(options: &DumpOptions, level: usize, out: &mut String) {
    if let Some(width) = options.indent {
        out.push('\n');
        for _ in 0..width * level {
            out.push(' ');
        }
    }
}

fn encode(
    value: &Value,
    options: &DumpOptions,
    level: usize,
    out: &mut String,
) -> Result<(), ToolError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(number) => {
            let _ = write!(out, "{number}");
        }
        Value::Float(number) => {
            if number.is_finite() {
                out.push_str(&float_repr(*number));
            } else if !options.allow_nan {
                return Err(ToolError::type_error(
                    "Out of range float values are not JSON compliant",
                ));
            } else if number.is_nan() {
                out.push_str("NaN");
            } else if *number > 0.0 {
                out.push_str("Infinity");
            } else {
                out.push_str("-Infinity");
            }
        }
        Value::Str(text) => encode_string(text, options.ensure_ascii, out),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return Ok(());
            }
            out.push('[');
            newline_indent(options, level + 1, out);
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(options.item_separator);
                    newline_indent(options, level + 1, out);
                }
                encode(item, options, level + 1, out)?;
            }
            newline_indent(options, level, out);
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            if options.sort_keys {
                entries.sort_by(|a, b| a.0.cmp(b.0));
            }
            out.push('{');
            newline_indent(options, level + 1, out);
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(options.item_separator);
                    newline_indent(options, level + 1, out);
                }
                encode_string(key, options.ensure_ascii, out);
                out.push_str(options.key_separator);
                encode(item, options, level + 1, out)?;
            }
            newline_indent(options, level, out);
            out.push('}');
        }
    }
    Ok(())
}

/// Python `json` string encoder.
pub fn encode_string(text: &str, ensure_ascii: bool, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            control if (control as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            ascii if !ensure_ascii || (ascii as u32) < 0x7f => out.push(ascii),
            other => {
                let mut units = [0u16; 2];
                for unit in other.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

/// Python `float.__repr__`.
pub fn float_repr(number: f64) -> String {
    if number.is_nan() {
        return "nan".to_owned();
    }
    if number.is_infinite() {
        return if number > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    if number == 0.0 {
        return if number.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .to_owned();
    }
    let scientific = format!("{:e}", number.abs());
    let (mantissa, exponent) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let sign = if number < 0.0 { "-" } else { "" };
    let decimal_point = exponent + 1;
    let length = digits.len() as i32;
    if decimal_point > -4 && decimal_point <= 16 {
        if decimal_point <= 0 {
            format!(
                "{sign}0.{}{digits}",
                "0".repeat(usize::try_from(-decimal_point).unwrap_or(0))
            )
        } else if decimal_point < length {
            let split = usize::try_from(decimal_point).unwrap_or(0);
            format!("{sign}{}.{}", &digits[..split], &digits[split..])
        } else {
            format!(
                "{sign}{digits}{}.0",
                "0".repeat(usize::try_from(decimal_point - length).unwrap_or(0))
            )
        }
    } else {
        let (head, tail) = digits.split_at(1);
        let fraction = if tail.is_empty() {
            String::new()
        } else {
            format!(".{tail}")
        };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        format!(
            "{sign}{head}{fraction}e{exponent_sign}{:02}",
            exponent.abs()
        )
    }
}

// ---------------------------------------------------------------- str/repr

/// Python `str(value)`.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::Str(text) => text.clone(),
        other => py_repr(other),
    }
}

/// Python `repr(value)`.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Int(number) => number.to_string(),
        Value::Float(number) => float_repr(*number),
        Value::Str(text) => py_repr_str(text),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(key, item)| format!("{}: {}", py_repr_str(key), py_repr(item)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

fn is_printable(character: char) -> bool {
    let code = character as u32;
    if character.is_control() {
        return false;
    }
    !matches!(code,
        0x00a0 | 0x00ad | 0x0600..=0x0605 | 0x061c | 0x06dd | 0x070f | 0x1680 | 0x180e
        | 0x2000..=0x200f | 0x2028..=0x202f | 0x205f..=0x2064 | 0x2066..=0x206f | 0x3000
        | 0xd800..=0xf8ff | 0xfeff | 0xfff9..=0xfffb | 0x110bd | 0x1d173..=0x1d17a
        | 0xe0001 | 0xe0020..=0xe007f | 0xf0000..=0x10ffff)
}

/// Python `repr(str)`.
pub fn py_repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for character in text.chars() {
        let code = character as u32;
        match character {
            _ if character == quote || character == '\\' => {
                out.push('\\');
                out.push(character);
            }
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if code < 0x20 || code == 0x7f => {
                let _ = write!(out, "\\x{code:02x}");
            }
            _ if code < 0x7f => out.push(character),
            _ if is_printable(character) => out.push(character),
            _ if code <= 0xff => {
                let _ = write!(out, "\\x{code:02x}");
            }
            _ if code <= 0xffff => {
                let _ = write!(out, "\\u{code:04x}");
            }
            _ => {
                let _ = write!(out, "\\U{code:08x}");
            }
        }
    }
    out.push(quote);
    out
}

// ---------------------------------------------------------------- builtins

/// Python `float(value)`.
pub fn py_float(value: &Value) -> Result<f64, ToolError> {
    match value {
        Value::Bool(flag) => Ok(f64::from(u8::from(*flag))),
        Value::Int(number) => Ok(*number as f64),
        Value::Float(number) => Ok(*number),
        Value::Str(text) => parse_python_float(text).ok_or_else(|| {
            ToolError::type_error(format!(
                "could not convert string to float: {}",
                py_repr_str(text)
            ))
        }),
        other => Err(ToolError::type_error(format!(
            "float() argument must be a string or a real number, not '{}'",
            other.type_name()
        ))),
    }
}

fn strip_underscores(text: &str) -> Option<String> {
    if !text.contains('_') {
        return Some(text.to_owned());
    }
    let characters: Vec<char> = text.chars().collect();
    for (index, character) in characters.iter().enumerate() {
        if *character == '_' {
            let before = index.checked_sub(1).and_then(|i| characters.get(i));
            let after = characters.get(index + 1);
            if !(before.is_some_and(char::is_ascii_digit)
                && after.is_some_and(char::is_ascii_digit))
            {
                return None;
            }
        }
    }
    Some(characters.into_iter().filter(|c| *c != '_').collect())
}

fn parse_python_float(text: &str) -> Option<f64> {
    let trimmed = text.trim();
    let cleaned = strip_underscores(trimmed)?;
    let body = cleaned.trim_start_matches(['+', '-']);
    let lowered = body.to_ascii_lowercase();
    let special = matches!(lowered.as_str(), "inf" | "infinity" | "nan");
    if !special
        && !body
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
    {
        return None;
    }
    if cleaned.len() - body.len() > 1 {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

/// Python `int(value)`.
pub fn py_int(value: &Value) -> Result<i128, ToolError> {
    match value {
        Value::Bool(flag) => Ok(i128::from(*flag)),
        Value::Int(number) => Ok(*number),
        Value::Float(number) if number.is_finite() => Ok(number.trunc() as i128),
        Value::Float(_) => Err(ToolError::type_error(
            "cannot convert float NaN or infinity to integer",
        )),
        Value::Str(text) => {
            let cleaned = strip_underscores(text.trim());
            cleaned
                .as_deref()
                .and_then(|digits| {
                    let body = digits.trim_start_matches(['+', '-']);
                    (digits.len() - body.len() <= 1
                        && !body.is_empty()
                        && body.chars().all(|c| c.is_ascii_digit()))
                    .then(|| digits.parse::<i128>().ok())
                    .flatten()
                })
                .ok_or_else(|| {
                    ToolError::type_error(format!(
                        "invalid literal for int() with base 10: {}",
                        py_repr_str(text)
                    ))
                })
        }
        other => Err(ToolError::type_error(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            other.type_name()
        ))),
    }
}

/// Python `round(value, ndigits)` for `int`/`float`.
pub fn py_round(value: &Value, ndigits: usize) -> Result<Value, ToolError> {
    match value {
        Value::Bool(flag) => Ok(Value::Int(i128::from(*flag))),
        Value::Int(number) => Ok(Value::Int(*number)),
        Value::Float(number) => Ok(Value::Float(round_float(*number, ndigits))),
        other => Err(ToolError::type_error(format!(
            "type {} doesn't define __round__ method",
            other.type_name()
        ))),
    }
}

/// Correctly rounded half-even decimal rounding (Python `round(float, n)`).
pub fn round_float(number: f64, ndigits: usize) -> f64 {
    if !number.is_finite() {
        return number;
    }
    format!("{number:.ndigits$}").parse().unwrap_or(number)
}

/// Python `len(value)`.
pub fn py_len(value: &Value) -> Result<usize, ToolError> {
    match value {
        Value::Str(text) => Ok(text.chars().count()),
        Value::Array(items) => Ok(items.len()),
        Value::Object(map) => Ok(map.len()),
        other => Err(ToolError::type_error(format!(
            "object of type '{}' has no len()",
            other.type_name()
        ))),
    }
}

/// Python iteration (`for x in value`).
pub fn py_iter(value: &Value) -> Result<Vec<Value>, ToolError> {
    match value {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(map) => Ok(map.keys().map(|key| Value::Str(key.clone())).collect()),
        Value::Str(text) => Ok(text.chars().map(|c| Value::Str(c.to_string())).collect()),
        other => Err(ToolError::type_error(format!(
            "'{}' object is not iterable",
            other.type_name()
        ))),
    }
}

/// Python `value[index]` for a non-negative integer index.
pub fn py_index(value: &Value, index: usize) -> Result<Value, ToolError> {
    match value {
        Value::Array(items) => items
            .get(index)
            .cloned()
            .ok_or_else(|| ToolError::type_error("list index out of range")),
        Value::Str(text) => text
            .chars()
            .nth(index)
            .map(|c| Value::Str(c.to_string()))
            .ok_or_else(|| ToolError::type_error("string index out of range")),
        Value::Object(_) => Err(ToolError::Key(index.to_string())),
        other => Err(ToolError::type_error(format!(
            "'{}' object is not subscriptable",
            other.type_name()
        ))),
    }
}

/// Python `value[:count]` for `str` and `list`.
pub fn py_prefix(value: &Value, count: usize) -> Result<Value, ToolError> {
    match value {
        Value::Str(text) => Ok(Value::Str(text.chars().take(count).collect())),
        Value::Array(items) => Ok(Value::Array(items.iter().take(count).cloned().collect())),
        other => Err(ToolError::type_error(format!(
            "'{}' object is not subscriptable",
            other.type_name()
        ))),
    }
}

/// Python `a + b` for numbers.
pub fn py_add(left: &Value, right: &Value) -> Result<Value, ToolError> {
    match (left.numeric(), right.numeric()) {
        (Some(Numeric::Int(a)), Some(Numeric::Int(b))) => Ok(Value::Int(a.saturating_add(b))),
        (Some(a), Some(b)) => Ok(Value::Float(numeric_f64(a) + numeric_f64(b))),
        _ => Err(ToolError::type_error(format!(
            "unsupported operand type(s) for +: '{}' and '{}'",
            left.type_name(),
            right.type_name()
        ))),
    }
}

fn numeric_f64(number: Numeric) -> f64 {
    match number {
        Numeric::Int(value) => value as f64,
        Numeric::Float(value) => value,
    }
}

/// Python `a / b` true division for numbers.
pub fn py_div(left: &Value, right: &Value) -> Result<f64, ToolError> {
    match (left.numeric(), right.numeric()) {
        (Some(a), Some(b)) => {
            let divisor = numeric_f64(b);
            if divisor == 0.0 {
                Err(ToolError::type_error("division by zero"))
            } else {
                Ok(numeric_f64(a) / divisor)
            }
        }
        _ => Err(ToolError::type_error("unsupported operand type(s) for /")),
    }
}

/// Python `str.strip()` whitespace set.
pub fn is_py_whitespace(character: char) -> bool {
    character.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&character)
}

/// Python `str.strip()`.
pub fn py_strip(text: &str) -> &str {
    text.trim_matches(is_py_whitespace)
}

/// Python universal-newline translation (`\r\n` and `\r` become `\n`).
pub fn universal_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        loads(text, LoadOptions::default()).expect("valid JSON")
    }

    #[test]
    fn float_repr_matches_python() {
        let cases = [
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (1.5e-7, "1.5e-07"),
            (123456789.123, "123456789.123"),
            (-0.0, "-0.0"),
            (2.5e300, "2.5e+300"),
            (0.1, "0.1"),
            (1.0, "1.0"),
            (100.0, "100.0"),
            (-12.5, "-12.5"),
            (5e-324, "5e-324"),
        ];
        for (number, expected) in cases {
            assert_eq!(float_repr(number), expected, "{number}");
        }
    }

    #[test]
    fn dumps_matches_python_defaults_and_indent() {
        // JSON escapes are assembled from a backslash so the source stays ASCII.
        let escape = |hex: &str| format!("{}u{hex}", '\\');
        let escaped = format!(
            "{}{}{}{}",
            escape("00e9"),
            escape("007f"),
            escape("d83d"),
            escape("de00")
        );
        let source =
            format!(r#"{{"b": [1, 2.0, {{"c": null}}], "a": "{escaped}", "e": [], "d": {{}}}}"#);
        let value = parse(&source);
        assert_eq!(dumps_default(&value), source);
        let sorted = dumps(&value, &DumpOptions::PYTHON.sorted().indent(2)).expect("dumps");
        assert_eq!(
            sorted,
            format!(
                "{{\n  \"a\": \"{escaped}\",\n  \"b\": [\n    1,\n    2.0,\n    {{\n      \"c\": null\n    }}\n  ],\n  \"d\": {{}},\n  \"e\": []\n}}"
            )
        );
        let unicode =
            dumps(&value, &DumpOptions::PYTHON.unicode().separators(",", ":")).expect("dumps");
        assert_eq!(
            unicode,
            "{\"b\":[1,2.0,{\"c\":null}],\"a\":\"\u{e9}\u{7f}\u{1f600}\",\"e\":[],\"d\":{}}"
        );
    }

    #[test]
    fn loads_rejects_duplicates_constants_and_garbage() {
        let strict = LoadOptions {
            reject_duplicates: true,
            reject_constants: true,
        };
        assert!(matches!(
            loads(r#"{"a":1,"a":2}"#, strict),
            Err(LoadError::DuplicateKey(_))
        ));
        assert!(matches!(
            loads(r#"{"v":NaN}"#, strict),
            Err(LoadError::NonFinite(_))
        ));
        assert!(matches!(
            loads(r#"{"v":-Infinity}"#, strict),
            Err(LoadError::NonFinite(_))
        ));
        assert!(loads("[1,]", strict).is_err());
        assert!(loads("01", strict).is_err());
        assert!(loads("{} x", strict).is_err());
        assert!(loads("\"a\u{1}\"", strict).is_err());
        let lenient = parse(r#"{"a":1,"b":2,"a":3,"n":NaN}"#);
        let map = lenient.as_object().expect("object");
        assert_eq!(map.keys().cloned().collect::<Vec<_>>(), ["a", "b", "n"]);
        assert_eq!(map.get("a"), Some(&Value::Int(3)));
        let deep = "[".repeat(2000) + &"]".repeat(2000);
        assert_eq!(loads(&deep, strict), Err(LoadError::TooDeep));
    }

    #[test]
    fn python_equality_and_ordering() {
        assert_eq!(Value::Bool(true), Value::Int(1));
        assert_eq!(Value::Int(1), Value::Float(1.0));
        assert_ne!(Value::Float(f64::NAN), Value::Float(f64::NAN));
        assert_eq!(parse(r#"{"a":1,"b":2}"#), parse(r#"{"b":2,"a":1.0}"#));
        assert!(py_cmp(&Value::Null, &Value::Str("x".into())).is_err());
        let left = Value::Array(vec![Value::Str("a".into()), Value::Null]);
        let right = Value::Array(vec![Value::Str("b".into()), Value::Null]);
        assert_eq!(py_cmp(&left, &right).expect("comparable"), Ordering::Less);
    }

    #[test]
    fn repr_matches_python() {
        assert_eq!(py_repr_str("it's \"q\""), r#"'it\'s "q"'"#);
        assert_eq!(py_repr_str("it's"), r#""it's""#);
        assert_eq!(
            py_repr_str("\u{0}\u{7f}\u{85}\u{a0}\u{ad}\u{2028}\u{e9}"),
            format!("'{0}x00{0}x7f{0}x85{0}xa0{0}xad{0}u2028\u{e9}'", '\\')
        );
        let value = parse(r#"{"a": [1, "x", null, true, 1.5]}"#);
        assert_eq!(py_str(&value), "{'a': [1, 'x', None, True, 1.5]}");
    }

    #[test]
    fn numeric_builtins_follow_python() {
        assert_eq!(round_float(0.125, 2), 0.12);
        assert_eq!(round_float(2.675, 2), 2.67);
        assert_eq!(round_float(1.25, 1), 1.2);
        assert_eq!(
            py_float(&Value::Str(" 1_000.5 ".into())).expect("float"),
            1000.5
        );
        assert!(py_float(&Value::Str("1__0".into())).is_err());
        assert_eq!(py_int(&Value::Float(-2.7)).expect("int"), -2);
        assert_eq!(py_int(&Value::Str(" 42 ".into())).expect("int"), 42);
        assert!(py_int(&Value::Str("4.2".into())).is_err());
    }

    #[test]
    fn stable_sort_by_python_key_keeps_insertion_order_for_ties() {
        let rows = vec![("b", 1), ("a", 2), ("b", 0), ("a", 1)];
        let rows = py_sorted(rows, |row| Ok(Value::Str(row.0.to_owned()))).expect("sort");
        assert_eq!(rows, vec![("a", 2), ("a", 1), ("b", 1), ("b", 0)]);
        let mixed = vec![Value::Int(1), Value::Str("x".into())];
        assert!(py_sorted(mixed, |value| Ok(value.clone())).is_err());
    }
}
