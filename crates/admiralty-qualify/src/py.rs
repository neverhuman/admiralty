//! Python-compatible JSON values and string helpers.
//!
//! The retired harnesses printed, compared and parsed JSON through the Python
//! standard library. This module reproduces the observable rules the ports
//! depend on: insertion-ordered objects, `json.dumps` encoding options
//! (`sort_keys`, separators, `ensure_ascii`, `indent`), `json.loads(bytes)`
//! encoding detection, Python equality/truthiness/membership, and the
//! whitespace/integer/`repr` conventions used in diagnostics.

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};

/// A decoded JSON value with Python `json` semantics.
#[derive(Debug, Clone)]
pub enum PyValue {
    /// `None` / `null`.
    Null,
    /// `bool`.
    Bool(bool),
    /// `int` (values beyond `u64`/`i64` are decoded as floats, see [`loads_bytes`]).
    Int(i128),
    /// `float`.
    Float(f64),
    /// `str`.
    Str(String),
    /// `list`.
    List(Vec<PyValue>),
    /// insertion-ordered `dict`.
    Dict(PyDict),
}

/// An insertion-ordered dictionary with Python `dict` update semantics: a
/// repeated key keeps its first position and takes the last value.
#[derive(Debug, Clone, Default)]
pub struct PyDict {
    entries: Vec<(String, PyValue)>,
    index: HashMap<String, usize>,
}

impl PyDict {
    /// An empty dictionary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace `key`, keeping the first insertion position.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<PyValue>) {
        let key = key.into();
        let value = value.into();
        if let Some(&position) = self.index.get(&key) {
            if let Some(slot) = self.entries.get_mut(position) {
                slot.1 = value;
            }
            return;
        }
        self.index.insert(key.clone(), self.entries.len());
        self.entries.push((key, value));
    }

    /// `dict.get(key)`.
    pub fn get(&self, key: &str) -> Option<&PyValue> {
        self.index
            .get(key)
            .and_then(|&position| self.entries.get(position))
            .map(|(_, value)| value)
    }

    /// Mutable access to an existing key.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut PyValue> {
        let position = *self.index.get(key)?;
        self.entries.get_mut(position).map(|(_, value)| value)
    }

    /// `key in dict`.
    pub fn contains_key(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    /// `len(dict)`.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `not dict`.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Items in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &PyValue)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value))
    }

    /// Keys in insertion order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(key, _)| key.as_str())
    }

    /// Values in insertion order.
    pub fn values(&self) -> impl Iterator<Item = &PyValue> {
        self.entries.iter().map(|(_, value)| value)
    }
}

/// Build an insertion-ordered [`PyDict`] value: `py_dict! { "a" => 1, "b" => "x" }`.
#[macro_export]
macro_rules! py_dict {
    () => { $crate::py::PyValue::Dict($crate::py::PyDict::new()) };
    ($($key:expr => $value:expr),+ $(,)?) => {{
        let mut dict = $crate::py::PyDict::new();
        $(dict.insert($key, $crate::py::PyValue::from($value));)+
        $crate::py::PyValue::Dict(dict)
    }};
}

impl From<bool> for PyValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}
impl From<&str> for PyValue {
    fn from(value: &str) -> Self {
        Self::Str(value.to_owned())
    }
}
impl From<String> for PyValue {
    fn from(value: String) -> Self {
        Self::Str(value)
    }
}
impl From<&String> for PyValue {
    fn from(value: &String) -> Self {
        Self::Str(value.clone())
    }
}
impl From<i32> for PyValue {
    fn from(value: i32) -> Self {
        Self::Int(i128::from(value))
    }
}
impl From<i64> for PyValue {
    fn from(value: i64) -> Self {
        Self::Int(i128::from(value))
    }
}
impl From<u32> for PyValue {
    fn from(value: u32) -> Self {
        Self::Int(i128::from(value))
    }
}
impl From<u64> for PyValue {
    fn from(value: u64) -> Self {
        Self::Int(i128::from(value))
    }
}
impl From<usize> for PyValue {
    fn from(value: usize) -> Self {
        Self::Int(value as i128)
    }
}
impl From<f64> for PyValue {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}
impl From<Vec<PyValue>> for PyValue {
    fn from(value: Vec<PyValue>) -> Self {
        Self::List(value)
    }
}
impl From<PyDict> for PyValue {
    fn from(value: PyDict) -> Self {
        Self::Dict(value)
    }
}
impl<T: Into<PyValue>> From<Option<T>> for PyValue {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}

impl PyValue {
    /// The dictionary, when this is a `dict`.
    pub fn as_dict(&self) -> Option<&PyDict> {
        match self {
            Self::Dict(dict) => Some(dict),
            _ => None,
        }
    }

    /// The mutable dictionary, when this is a `dict`.
    pub fn as_dict_mut(&mut self) -> Option<&mut PyDict> {
        match self {
            Self::Dict(dict) => Some(dict),
            _ => None,
        }
    }

    /// The list, when this is a `list`.
    pub fn as_list(&self) -> Option<&[PyValue]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    /// The string, when this is a `str`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(text) => Some(text),
            _ => None,
        }
    }

    /// `value.get(key)` for a `dict`, `None` otherwise (`dict.get` semantics).
    pub fn get(&self, key: &str) -> Option<&PyValue> {
        self.as_dict().and_then(|dict| dict.get(key))
    }

    /// `value is True` / `value is False`.
    pub fn is_bool(&self, expected: bool) -> bool {
        matches!(self, Self::Bool(actual) if *actual == expected)
    }

    /// Python truthiness (`bool(value)`).
    pub fn truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(value) => *value,
            Self::Int(value) => *value != 0,
            Self::Float(value) => *value != 0.0,
            Self::Str(value) => !value.is_empty(),
            Self::List(items) => !items.is_empty(),
            Self::Dict(dict) => !dict.is_empty(),
        }
    }

    /// Python type name used in `TypeError`/`KeyError` diagnostics.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "NoneType",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Float(_) => "float",
            Self::Str(_) => "str",
            Self::List(_) => "list",
            Self::Dict(_) => "dict",
        }
    }

    /// `value[key]` with Python `KeyError`/`TypeError` failures.
    pub fn index(&self, key: &str) -> Result<&PyValue, String> {
        match self {
            Self::Dict(dict) => dict
                .get(key)
                .ok_or_else(|| format!("KeyError: {}", repr_str(key))),
            Self::List(_) => {
                Err("TypeError: list indices must be integers or slices, not str".into())
            }
            Self::Str(_) => Err("TypeError: string indices must be integers, not 'str'".into()),
            other => Err(format!(
                "TypeError: '{}' object is not subscriptable",
                other.type_name()
            )),
        }
    }

    /// `item in self` with Python container semantics.
    pub fn contains(&self, item: &PyValue) -> Result<bool, String> {
        match self {
            Self::Str(haystack) => match item {
                Self::Str(needle) => Ok(haystack.contains(needle.as_str())),
                other => Err(format!(
                    "TypeError: 'in <string>' requires string as left operand, not {}",
                    other.type_name()
                )),
            },
            Self::List(items) => Ok(items.iter().any(|candidate| py_eq(candidate, item))),
            Self::Dict(dict) => match item {
                Self::Str(key) => Ok(dict.contains_key(key)),
                Self::List(_) | Self::Dict(_) => Err(format!(
                    "TypeError: unhashable type: '{}'",
                    item.type_name()
                )),
                _ => Ok(false),
            },
            other => Err(format!(
                "TypeError: argument of type '{}' is not iterable",
                other.type_name()
            )),
        }
    }
}

fn numeric(value: &PyValue) -> Option<f64> {
    match value {
        PyValue::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        PyValue::Int(number) => Some(*number as f64),
        PyValue::Float(number) => Some(*number),
        _ => None,
    }
}

fn integral(value: &PyValue) -> Option<i128> {
    match value {
        PyValue::Bool(flag) => Some(i128::from(*flag)),
        PyValue::Int(number) => Some(*number),
        _ => None,
    }
}

/// Python `==` for JSON values (`True == 1 == 1.0`, order-insensitive dicts).
pub fn py_eq(left: &PyValue, right: &PyValue) -> bool {
    if let (Some(a), Some(b)) = (integral(left), integral(right)) {
        return a == b;
    }
    if let (Some(a), Some(b)) = (numeric(left), numeric(right)) {
        return a == b;
    }
    match (left, right) {
        (PyValue::Null, PyValue::Null) => true,
        (PyValue::Str(a), PyValue::Str(b)) => a == b,
        (PyValue::List(a), PyValue::List(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| py_eq(x, y))
        }
        (PyValue::Dict(a), PyValue::Dict(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| py_eq(value, other)))
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Encoding (json.dumps)
// ---------------------------------------------------------------------------

/// `json.dumps` options.
#[derive(Debug, Clone, Copy)]
pub struct Dump {
    /// `sort_keys=`.
    pub sort_keys: bool,
    /// Item separator.
    pub item_sep: &'static str,
    /// Key separator.
    pub key_sep: &'static str,
    /// `ensure_ascii=`.
    pub ensure_ascii: bool,
    /// `indent=`.
    pub indent: Option<usize>,
}

impl Dump {
    /// `json.dumps(value)`.
    pub const DEFAULT: Dump = Dump {
        sort_keys: false,
        item_sep: ", ",
        key_sep: ": ",
        ensure_ascii: true,
        indent: None,
    };
    /// `json.dumps(value, separators=(",", ":"))`.
    pub const COMPACT: Dump = Dump {
        sort_keys: false,
        item_sep: ",",
        key_sep: ":",
        ensure_ascii: true,
        indent: None,
    };
    /// `json.dumps(value, sort_keys=True, separators=(",", ":"))`.
    pub const COMPACT_SORTED: Dump = Dump {
        sort_keys: true,
        ..Dump::COMPACT
    };
    /// `json.dumps(value, indent=2)`.
    pub const INDENT2: Dump = Dump {
        sort_keys: false,
        item_sep: ",",
        key_sep: ": ",
        ensure_ascii: true,
        indent: Some(2),
    };
    /// `json.dumps(value, indent=2, sort_keys=True)`.
    pub const INDENT2_SORTED: Dump = Dump {
        sort_keys: true,
        ..Dump::INDENT2
    };
}

/// `json.dumps(value, **options)`.
pub fn dumps(value: &PyValue, options: Dump) -> String {
    let mut out = String::new();
    encode(value, options, 0, &mut out);
    out
}

fn newline_indent(options: Dump, level: usize) -> Option<String> {
    options
        .indent
        .map(|width| format!("\n{}", " ".repeat(width * level)))
}

fn encode(value: &PyValue, options: Dump, level: usize, out: &mut String) {
    match value {
        PyValue::Null => out.push_str("null"),
        PyValue::Bool(true) => out.push_str("true"),
        PyValue::Bool(false) => out.push_str("false"),
        PyValue::Int(number) => out.push_str(&number.to_string()),
        PyValue::Float(number) => out.push_str(&float_repr(*number)),
        PyValue::Str(text) => encode_str(text, options.ensure_ascii, out),
        PyValue::List(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            let inner = newline_indent(options, level + 1);
            if let Some(indent) = &inner {
                out.push_str(indent);
            }
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    out.push_str(options.item_sep);
                    if let Some(indent) = &inner {
                        out.push_str(indent);
                    }
                }
                encode(item, options, level + 1, out);
            }
            if let Some(indent) = newline_indent(options, level) {
                out.push_str(&indent);
            }
            out.push(']');
        }
        PyValue::Dict(dict) => {
            if dict.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            let inner = newline_indent(options, level + 1);
            if let Some(indent) = &inner {
                out.push_str(indent);
            }
            let mut items: Vec<(&str, &PyValue)> = dict.iter().collect();
            if options.sort_keys {
                items.sort_by(|a, b| a.0.cmp(b.0));
            }
            for (position, (key, item)) in items.into_iter().enumerate() {
                if position > 0 {
                    out.push_str(options.item_sep);
                    if let Some(indent) = &inner {
                        out.push_str(indent);
                    }
                }
                encode_str(key, options.ensure_ascii, out);
                out.push_str(options.key_sep);
                encode(item, options, level + 1, out);
            }
            if let Some(indent) = newline_indent(options, level) {
                out.push_str(&indent);
            }
            out.push('}');
        }
    }
}

fn encode_str(text: &str, ensure_ascii: bool, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            printable if !ensure_ascii || (' '..='~').contains(&printable) => out.push(printable),
            other => {
                let mut units = [0u16; 2];
                for unit in other.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

/// Python `float.__repr__` as used by `json.dumps` (`NaN`, `Infinity`).
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let debug = format!("{value:?}");
    match debug.split_once('e') {
        None => debug,
        Some((mantissa, exponent)) => {
            let (sign, digits) = match exponent.strip_prefix('-') {
                Some(digits) => ('-', digits),
                None => ('+', exponent),
            };
            format!("{mantissa}e{sign}{digits:0>2}")
        }
    }
}

// ---------------------------------------------------------------------------
// Decoding (json.loads)
// ---------------------------------------------------------------------------

/// A `json.loads` failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonError {
    /// An `object_pairs_hook` rejected a repeated key.
    Duplicate,
    /// Any decode or syntax failure (the message wording differs from Python's).
    Syntax(String),
}

impl fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate => formatter.write_str("duplicate JSON key"),
            Self::Syntax(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for JsonError {}

#[derive(Clone, Copy)]
struct Seed<'a> {
    reject_duplicates: bool,
    duplicate: &'a Cell<bool>,
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = PyValue;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<PyValue, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = PyValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<PyValue, E> {
        Ok(PyValue::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<PyValue, E> {
        Ok(PyValue::Int(i128::from(value)))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<PyValue, E> {
        Ok(PyValue::Int(i128::from(value)))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<PyValue, E> {
        Ok(PyValue::Float(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<PyValue, E> {
        Ok(PyValue::Str(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<PyValue, E> {
        Ok(PyValue::Str(value))
    }

    fn visit_unit<E: de::Error>(self) -> Result<PyValue, E> {
        Ok(PyValue::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<PyValue, E> {
        Ok(PyValue::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<PyValue, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = sequence.next_element_seed(self)? {
            items.push(item);
        }
        Ok(PyValue::List(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<PyValue, A::Error> {
        let mut dict = PyDict::new();
        let mut repeated = false;
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value_seed(self)?;
            // Like an object_pairs_hook, only reject once the object parsed.
            repeated |= dict.contains_key(&key);
            dict.insert(key, value);
        }
        if repeated && self.reject_duplicates {
            self.duplicate.set(true);
            return Err(de::Error::custom("duplicate JSON key"));
        }
        Ok(PyValue::Dict(dict))
    }
}

/// `json.loads(text)` for a `str` argument (a leading BOM is rejected).
pub fn loads_str(text: &str, reject_duplicates: bool) -> Result<PyValue, JsonError> {
    if text.starts_with('\u{feff}') {
        return Err(JsonError::Syntax(
            "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)".into(),
        ));
    }
    let duplicate = Cell::new(false);
    let seed = Seed {
        reject_duplicates,
        duplicate: &duplicate,
    };
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let parsed = seed
        .deserialize(&mut deserializer)
        .and_then(|value| deserializer.end().map(|()| value));
    parsed.map_err(|error| {
        if duplicate.get() {
            JsonError::Duplicate
        } else {
            JsonError::Syntax(error.to_string())
        }
    })
}

/// `json.loads(raw)` for a `bytes` argument, including Python's UTF-8/16/32
/// detection and BOM handling.
pub fn loads_bytes(raw: &[u8], reject_duplicates: bool) -> Result<PyValue, JsonError> {
    let text = decode_json_bytes(raw).map_err(JsonError::Syntax)?;
    let duplicate = Cell::new(false);
    let seed = Seed {
        reject_duplicates,
        duplicate: &duplicate,
    };
    let mut deserializer = serde_json::Deserializer::from_str(&text);
    let parsed = seed
        .deserialize(&mut deserializer)
        .and_then(|value| deserializer.end().map(|()| value));
    parsed.map_err(|error| {
        if duplicate.get() {
            JsonError::Duplicate
        } else {
            JsonError::Syntax(error.to_string())
        }
    })
}

fn decode_json_bytes(raw: &[u8]) -> Result<String, String> {
    const UTF32_BE: &[u8] = &[0, 0, 0xfe, 0xff];
    const UTF32_LE: &[u8] = &[0xff, 0xfe, 0, 0];
    const UTF16_BE: &[u8] = &[0xfe, 0xff];
    const UTF16_LE: &[u8] = &[0xff, 0xfe];
    const UTF8_BOM: &[u8] = &[0xef, 0xbb, 0xbf];
    if raw.starts_with(UTF32_BE) {
        return decode_utf32(&raw[4..], true);
    }
    if raw.starts_with(UTF32_LE) {
        return decode_utf32(&raw[4..], false);
    }
    if raw.starts_with(UTF16_BE) {
        return decode_utf16(&raw[2..], true);
    }
    if raw.starts_with(UTF16_LE) {
        return decode_utf16(&raw[2..], false);
    }
    if let Some(rest) = raw.strip_prefix(UTF8_BOM) {
        return decode_utf8(rest);
    }
    if raw.len() >= 4 {
        if raw[0] == 0 {
            return if raw[1] != 0 {
                decode_utf16(raw, true)
            } else {
                decode_utf32(raw, true)
            };
        }
        if raw[1] == 0 {
            return if raw[2] != 0 || raw[3] != 0 {
                decode_utf16(raw, false)
            } else {
                decode_utf32(raw, false)
            };
        }
    } else if raw.len() == 2 {
        if raw[0] == 0 {
            return decode_utf16(raw, true);
        }
        if raw[1] == 0 {
            return decode_utf16(raw, false);
        }
    }
    decode_utf8(raw)
}

fn decode_utf8(raw: &[u8]) -> Result<String, String> {
    String::from_utf8(raw.to_vec())
        .map_err(|error| format!("'utf-8' codec can't decode bytes: {error}"))
}

fn decode_utf16(raw: &[u8], big_endian: bool) -> Result<String, String> {
    if !raw.len().is_multiple_of(2) {
        return Err("'utf-16' codec can't decode bytes: truncated data".into());
    }
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|pair| {
            let bytes = [pair[0], pair[1]];
            if big_endian {
                u16::from_be_bytes(bytes)
            } else {
                u16::from_le_bytes(bytes)
            }
        })
        .collect();
    String::from_utf16(&units)
        .map_err(|error| format!("'utf-16' codec can't decode bytes: {error}"))
}

fn decode_utf32(raw: &[u8], big_endian: bool) -> Result<String, String> {
    if !raw.len().is_multiple_of(4) {
        return Err("'utf-32' codec can't decode bytes: truncated data".into());
    }
    raw.chunks_exact(4)
        .map(|quad| {
            let bytes = [quad[0], quad[1], quad[2], quad[3]];
            let scalar = if big_endian {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            };
            char::from_u32(scalar).ok_or_else(|| {
                "'utf-32' codec can't decode bytes: code point not in range".to_owned()
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// str helpers
// ---------------------------------------------------------------------------

/// `str.isspace()` for one character.
pub fn is_space(character: char) -> bool {
    matches!(
        character,
        '\t'..='\r'
            | '\u{1c}'..='\u{20}'
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// `str.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `str.split()` (no separator) limited to `maxsplit` splits.
pub fn split_whitespace(text: &str, maxsplit: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text.trim_start_matches(is_space);
    while !rest.is_empty() {
        if parts.len() == maxsplit {
            parts.push(rest);
            break;
        }
        match rest.find(is_space) {
            Some(end) => {
                parts.push(&rest[..end]);
                rest = rest[end..].trim_start_matches(is_space);
            }
            None => {
                parts.push(rest);
                break;
            }
        }
    }
    parts
}

/// `int(text, base)` for base 10 or 16, including sign, whitespace, `0x`
/// prefix and single underscores between digits. `ascii_space` selects the
/// stripping rules of `int(bytes)`.
pub fn parse_int(text: &str, base: u32, ascii_space: bool) -> Option<i128> {
    let trimmed = if ascii_space {
        text.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r'))
    } else {
        strip(text)
    };
    let (negative, unsigned) = match trimmed.as_bytes().first() {
        Some(b'-') => (true, &trimmed[1..]),
        Some(b'+') => (false, &trimmed[1..]),
        _ => (false, trimmed),
    };
    let mut digits = unsigned;
    let mut underscore_allowed = false;
    if base == 16
        && let Some(rest) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
    {
        digits = rest;
        underscore_allowed = true;
    }
    if digits.is_empty() {
        return None;
    }
    let mut value: i128 = 0;
    let mut previous_underscore = false;
    let mut seen_digit = false;
    for character in digits.chars() {
        if character == '_' {
            if previous_underscore || (!seen_digit && !underscore_allowed) {
                return None;
            }
            previous_underscore = true;
            continue;
        }
        let digit = character.to_digit(base)?;
        value = value
            .checked_mul(i128::from(base))?
            .checked_add(i128::from(digit))?;
        previous_underscore = false;
        seen_digit = true;
    }
    if previous_underscore || !seen_digit {
        return None;
    }
    Some(if negative { -value } else { value })
}

fn is_printable(character: char) -> bool {
    let code = character as u32;
    if code < 0x7f {
        return code >= 0x20;
    }
    // Approximation of Py_UNICODE_ISPRINTABLE for the categories that occur
    // in practice (controls, separators, format characters, private use).
    !(character.is_control()
        || matches!(
            code,
            0xa0 | 0xad
                | 0x1680
                | 0x2000..=0x200f
                | 0x2028..=0x202f
                | 0x205f..=0x206f
                | 0x3000
                | 0xd800..=0xf8ff
                | 0xfeff
                | 0xfff0..=0xfffb
                | 0xfffe..=0xffff
        ))
}

/// Python `repr(str)`.
pub fn repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_printable(c) => out.push(c),
            c if (c as u32) <= 0xff => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) <= 0xffff => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push(quote);
    out
}

/// `text[:limit]` counted in code points.
pub fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// `text[-limit:]` counted in code points.
pub fn tail_chars(text: &str, limit: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(limit)).collect()
}

/// Python 3 `round(x)` (ties to even) to an integer.
pub fn round_half_even(value: f64) -> i64 {
    value.round_ties_even() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dumps_matches_python_layouts() {
        let value = py_dict! {
            "b" => 1,
            "a" => vec![PyValue::from("é"), PyValue::Null, PyValue::from(true)],
            "c" => PyDict::new(),
            "d" => Vec::<PyValue>::new(),
        };
        assert_eq!(
            dumps(&value, Dump::DEFAULT),
            r#"{"b": 1, "a": ["\u00e9", null, true], "c": {}, "d": []}"#
        );
        assert_eq!(
            dumps(&value, Dump::COMPACT_SORTED),
            r#"{"a":["\u00e9",null,true],"b":1,"c":{},"d":[]}"#
        );
        assert_eq!(
            dumps(&value, Dump::INDENT2),
            "{\n  \"b\": 1,\n  \"a\": [\n    \"\\u00e9\",\n    null,\n    true\n  ],\n  \"c\": {},\n  \"d\": []\n}"
        );
        let raw = Dump {
            ensure_ascii: false,
            ..Dump::COMPACT_SORTED
        };
        assert_eq!(
            dumps(&PyValue::from("é\u{7f}\u{1}"), raw),
            "\"é\u{7f}\\u0001\""
        );
        assert_eq!(
            dumps(&PyValue::from("\u{7f}\u{1f600}"), Dump::COMPACT),
            "\"\\u007f\\ud83d\\ude00\""
        );
    }

    #[test]
    fn float_repr_matches_python() {
        assert_eq!(float_repr(1.0), "1.0");
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(1.5e-5), "1.5e-05");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(123456789.0), "123456789.0");
        assert_eq!(float_repr(f64::NAN), "NaN");
    }

    #[test]
    fn loads_detects_duplicates_after_object_and_encodings() {
        assert_eq!(
            loads_bytes(br#"{"files":[],"files":[]}"#, true).err(),
            Some(JsonError::Duplicate)
        );
        let last = loads_bytes(br#"{"a":1,"b":2,"a":3}"#, false).expect("valid JSON");
        let keys: Vec<&str> = last
            .as_dict()
            .map(|d| d.keys().collect())
            .unwrap_or_default();
        assert_eq!(keys, ["a", "b"]);
        assert!(py_eq(
            last.get("a").unwrap_or(&PyValue::Null),
            &PyValue::Int(3)
        ));
        let utf16: Vec<u8> = "{\"x\": 1}"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(loads_bytes(&utf16, true).is_ok());
        assert!(loads_bytes(b"\xef\xbb\xbf{}", true).is_ok());
        assert!(loads_str("\u{feff}{}", true).is_err());
    }

    #[test]
    fn python_equality_and_membership() {
        assert!(py_eq(&PyValue::Bool(true), &PyValue::Int(1)));
        assert!(py_eq(&PyValue::Int(1), &PyValue::Float(1.0)));
        assert!(!py_eq(&PyValue::Str("1".into()), &PyValue::Int(1)));
        let text = PyValue::from("unknown cost");
        assert_eq!(text.contains(&PyValue::from("unknown")), Ok(true));
        let list = PyValue::List(vec![PyValue::from("unknown")]);
        assert_eq!(list.contains(&PyValue::from("unknown")), Ok(true));
    }

    #[test]
    fn python_int_and_whitespace_rules() {
        assert_eq!(parse_int(" +2_00\r\n", 10, false), Some(200));
        assert_eq!(parse_int("0x1_a", 16, true), Some(26));
        assert_eq!(parse_int("_1", 10, false), None);
        assert_eq!(parse_int("1__0", 10, false), None);
        assert_eq!(parse_int("", 10, false), None);
        assert_eq!(
            split_whitespace("HTTP/1.1 200 OK fine\r\n", 2),
            ["HTTP/1.1", "200", "OK fine\r\n"]
        );
        assert_eq!(split_whitespace("HTTP/1.1\x1c200", 2), ["HTTP/1.1", "200"]);
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("a\u{1}b"), "'a\\x01b'");
        assert_eq!(round_half_even(2.5), 2);
        assert_eq!(round_half_even(3.5), 4);
    }
}
