//! Python-compatible value model and helpers.
//!
//! The audit's JSON/Markdown outputs are consumed by a dashboard, by
//! super_captain prompts and by the ticker, so they must keep the exact shape
//! the Python implementation produced: insertion-ordered objects, Python
//! `json.dumps` formatting (`ensure_ascii`, `", "`/`": "` separators), Python
//! `str()`/`repr()` of values inside f-strings, Python truthiness/equality,
//! code-point string slicing, banker's rounding and `datetime.fromisoformat`.
//! Everything here is pure and deterministic.

use std::cmp::Ordering;
use std::fmt;
use std::sync::LazyLock;

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike};
use regex::Regex;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

/// An aware datetime (Python `datetime` with `tzinfo`).
pub type Dt = DateTime<FixedOffset>;

/// Injectable `sha -> bool` predicate (e.g. "is this sha on origin/main").
pub type ShaPredicate<'a> = &'a dyn Fn(&str) -> bool;
/// Injectable `name -> datetime` lookup (commit time, last terminal input, last push).
pub type TimeLookup<'a> = &'a dyn Fn(&str) -> Option<Dt>;
/// `nvidia-smi` rows: `(util_pct, mem_mib)`.
pub type GpuRows = Vec<(i64, i64)>;
/// Injectable GPU sampler.
pub type GpuSampler<'a> = &'a dyn Fn(&str) -> Option<GpuRows>;

// ---------------------------------------------------------------------------
// Errors: a Python exception that the original code would have raised.
// ---------------------------------------------------------------------------

/// A Python exception (type name + `str(e)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyErr {
    pub kind: &'static str,
    pub msg: String,
}

impl PyErr {
    pub fn new(kind: &'static str, msg: impl Into<String>) -> Self {
        Self {
            kind,
            msg: msg.into(),
        }
    }
    /// `KeyError(key)`; `str()` is the key's repr.
    pub fn key(key: &str) -> Self {
        Self::new("KeyError", py_str_repr(key))
    }
    pub fn type_error(msg: impl Into<String>) -> Self {
        Self::new("TypeError", msg)
    }
    pub fn value_error(msg: impl Into<String>) -> Self {
        Self::new("ValueError", msg)
    }
    pub fn attr_get(j: &J) -> Self {
        Self::new(
            "AttributeError",
            format!("'{}' object has no attribute 'get'", j.type_name()),
        )
    }
    pub fn index() -> Self {
        Self::new("IndexError", "list index out of range")
    }
    /// An `OSError` raised by `open(path)`.
    pub fn os(path: &str, e: &std::io::Error) -> Self {
        let code = e.raw_os_error().unwrap_or(0);
        let full = e.to_string();
        let strerror = match full.rfind(" (os error ") {
            Some(i) => full[..i].to_string(),
            None => full,
        };
        let kind = match code {
            2 => "FileNotFoundError",
            13 | 1 => "PermissionError",
            21 => "IsADirectoryError",
            20 => "NotADirectoryError",
            _ => "OSError",
        };
        Self::new(
            kind,
            format!("[Errno {code}] {strerror}: {}", py_str_repr(path)),
        )
    }
}

impl fmt::Display for PyErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for PyErr {}

pub type PyResult<T> = Result<T, PyErr>;

// ---------------------------------------------------------------------------
// Insertion-ordered JSON value.
// ---------------------------------------------------------------------------

/// A JSON value with Python `dict` (insertion-ordered) semantics.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum J {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Obj),
}

/// The shared `None`.
pub static NULL: J = J::Null;

/// Insertion-ordered object (Python `dict` with string keys).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Obj {
    entries: Vec<(String, J)>,
}

impl Obj {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self, key: &str) -> Option<&J> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut J> {
        self.entries
            .iter_mut()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }
    /// `d[key] = value` (keeps the original position of an existing key).
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<J>) {
        let key = key.into();
        let value = value.into();
        if let Some(slot) = self.get_mut(&key) {
            *slot = value;
        } else {
            self.entries.push((key, value));
        }
    }
    /// `d.pop(key, None)`.
    pub fn remove(&mut self, key: &str) -> Option<J> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(index).1)
    }
    pub fn contains(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }
    /// `d.setdefault(key, default)`.
    pub fn setdefault(&mut self, key: &str, default: J) -> &mut J {
        let index = match self.entries.iter().position(|(k, _)| k == key) {
            Some(i) => i,
            None => {
                self.entries.push((key.to_string(), default));
                self.entries.len() - 1
            }
        };
        &mut self.entries[index].1
    }
    /// The object stored under `key`, created (or replaced when not an object) as needed.
    pub fn obj_entry(&mut self, key: &str) -> &mut Obj {
        let slot = self.setdefault(key, J::Obj(Obj::new()));
        if !matches!(slot, J::Obj(_)) {
            *slot = J::Obj(Obj::new());
        }
        match slot {
            J::Obj(o) => o,
            _ => unreachable!("slot was just set to an object"),
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &J)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut J)> {
        self.entries.iter_mut().map(|(k, v)| (&*k, v))
    }
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.iter().map(|(k, _)| k)
    }
    pub fn values(&self) -> impl Iterator<Item = &J> {
        self.entries.iter().map(|(_, v)| v)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl FromIterator<(String, J)> for Obj {
    fn from_iter<I: IntoIterator<Item = (String, J)>>(iter: I) -> Self {
        let mut obj = Obj::new();
        for (k, v) in iter {
            obj.set(k, v);
        }
        obj
    }
}

/// Build an insertion-ordered [`J::Obj`]: `jobj! { "a" => 1, "b" => "x" }`.
#[macro_export]
macro_rules! jobj {
    () => { $crate::py::J::Obj($crate::py::Obj::new()) };
    ($($k:expr => $v:expr),+ $(,)?) => {{
        let mut o = $crate::py::Obj::new();
        $( o.set($k, $crate::py::J::from($v)); )+
        $crate::py::J::Obj(o)
    }};
}

impl From<bool> for J {
    fn from(v: bool) -> Self {
        J::Bool(v)
    }
}
impl From<i64> for J {
    fn from(v: i64) -> Self {
        J::Int(v)
    }
}
impl From<i32> for J {
    fn from(v: i32) -> Self {
        J::Int(i64::from(v))
    }
}
impl From<u32> for J {
    fn from(v: u32) -> Self {
        J::Int(i64::from(v))
    }
}
impl From<usize> for J {
    fn from(v: usize) -> Self {
        J::Int(i64::try_from(v).unwrap_or(i64::MAX))
    }
}
impl From<f64> for J {
    fn from(v: f64) -> Self {
        J::Float(v)
    }
}
impl From<&str> for J {
    fn from(v: &str) -> Self {
        J::Str(v.to_string())
    }
}
impl From<String> for J {
    fn from(v: String) -> Self {
        J::Str(v)
    }
}
impl From<&String> for J {
    fn from(v: &String) -> Self {
        J::Str(v.clone())
    }
}
impl From<Obj> for J {
    fn from(v: Obj) -> Self {
        J::Obj(v)
    }
}
impl From<Vec<J>> for J {
    fn from(v: Vec<J>) -> Self {
        J::Arr(v)
    }
}
impl From<Vec<String>> for J {
    fn from(v: Vec<String>) -> Self {
        J::Arr(v.into_iter().map(J::Str).collect())
    }
}
impl From<&J> for J {
    fn from(v: &J) -> Self {
        v.clone()
    }
}
impl<T: Into<J>> From<Option<T>> for J {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(x) => x.into(),
            None => J::Null,
        }
    }
}

impl J {
    /// Python type name used in exception messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            J::Null => "NoneType",
            J::Bool(_) => "bool",
            J::Int(_) => "int",
            J::Float(_) => "float",
            J::Str(_) => "str",
            J::Arr(_) => "list",
            J::Obj(_) => "dict",
        }
    }
    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            J::Null => false,
            J::Bool(b) => *b,
            J::Int(i) => *i != 0,
            J::Float(f) => *f != 0.0,
            J::Str(s) => !s.is_empty(),
            J::Arr(a) => !a.is_empty(),
            J::Obj(o) => !o.is_empty(),
        }
    }
    pub fn is_null(&self) -> bool {
        matches!(self, J::Null)
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_obj(&self) -> Option<&Obj> {
        match self {
            J::Obj(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_obj_mut(&mut self) -> Option<&mut Obj> {
        match self {
            J::Obj(o) => Some(o),
            _ => None,
        }
    }
    pub fn as_arr(&self) -> Option<&Vec<J>> {
        match self {
            J::Arr(a) => Some(a),
            _ => None,
        }
    }
    /// `isinstance(x, int)` (true for bool too).
    pub fn as_inst_int(&self) -> Option<i64> {
        match self {
            J::Int(i) => Some(*i),
            J::Bool(b) => Some(i64::from(*b)),
            _ => None,
        }
    }
    /// `type(x) is int`.
    pub fn is_exact_int(&self) -> bool {
        matches!(self, J::Int(_))
    }
    /// Numeric value for bool/int/float.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            J::Int(i) => Some(*i as f64),
            J::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            J::Float(f) => Some(*f),
            _ => None,
        }
    }
    /// `x.get(key)` — `AttributeError` when `x` is not a dict; `None` when missing.
    pub fn pget(&self, key: &str) -> PyResult<&J> {
        match self {
            J::Obj(o) => Ok(o.get(key).unwrap_or(&NULL)),
            other => Err(PyErr::attr_get(other)),
        }
    }
    /// `x.get(key, default)`.
    pub fn pget_or<'a>(&'a self, key: &str, default: &'a J) -> PyResult<&'a J> {
        match self {
            J::Obj(o) => Ok(o.get(key).unwrap_or(default)),
            other => Err(PyErr::attr_get(other)),
        }
    }
    /// `x[key]` for a string key.
    pub fn pidx(&self, key: &str) -> PyResult<&J> {
        match self {
            J::Obj(o) => o.get(key).ok_or_else(|| PyErr::key(key)),
            J::Arr(_) => Err(PyErr::type_error(
                "list indices must be integers or slices, not str",
            )),
            J::Str(_) => Err(PyErr::type_error(
                "string indices must be integers, not 'str'",
            )),
            other => Err(PyErr::type_error(format!(
                "'{}' object is not subscriptable",
                other.type_name()
            ))),
        }
    }
    /// `key in x` for a dict (string key).
    pub fn has_key(&self, key: &str) -> PyResult<bool> {
        py_contains(self, &J::Str(key.to_string()))
    }
    /// `for item in x` (dict iterates keys, str iterates characters).
    pub fn py_iter(&self) -> PyResult<Vec<J>> {
        match self {
            J::Arr(a) => Ok(a.clone()),
            J::Obj(o) => Ok(o.keys().map(|k| J::Str(k.clone())).collect()),
            J::Str(s) => Ok(s.chars().map(|c| J::Str(c.to_string())).collect()),
            other => Err(PyErr::type_error(format!(
                "'{}' object is not iterable",
                other.type_name()
            ))),
        }
    }
    /// `len(x)`.
    pub fn py_len(&self) -> PyResult<usize> {
        match self {
            J::Arr(a) => Ok(a.len()),
            J::Obj(o) => Ok(o.len()),
            J::Str(s) => Ok(s.chars().count()),
            other => Err(PyErr::type_error(format!(
                "object of type '{}' has no len()",
                other.type_name()
            ))),
        }
    }
}

/// `a or b`.
pub fn or<'a>(a: &'a J, b: &'a J) -> &'a J {
    if a.truthy() { a } else { b }
}

/// `needle in haystack`.
pub fn py_contains(haystack: &J, needle: &J) -> PyResult<bool> {
    match haystack {
        J::Arr(a) => Ok(a.iter().any(|x| py_eq(x, needle))),
        J::Obj(o) => match needle {
            J::Str(s) => Ok(o.contains(s)),
            J::Arr(_) | J::Obj(_) => Err(PyErr::type_error(format!(
                "unhashable type: '{}'",
                needle.type_name()
            ))),
            // JSON object keys are always strings: a non-string key never matches.
            _ => Ok(false),
        },
        J::Str(s) => match needle {
            J::Str(n) => Ok(s.contains(n.as_str())),
            other => Err(PyErr::type_error(format!(
                "'in <string>' requires string as left operand, not {}",
                other.type_name()
            ))),
        },
        other => Err(PyErr::type_error(format!(
            "argument of type '{}' is not iterable",
            other.type_name()
        ))),
    }
}

/// `sep.join(x)` where every item must be a `str`.
pub fn py_join(sep: &str, x: &J) -> PyResult<String> {
    let items = x.py_iter()?;
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        match item {
            J::Str(s) => out.push(s.as_str()),
            other => {
                return Err(PyErr::type_error(format!(
                    "sequence item {i}: expected str instance, {} found",
                    other.type_name()
                )));
            }
        }
    }
    Ok(out.join(sep))
}

/// `a + b` where `b` is a `str` (string concatenation).
pub fn py_add_str(a: &J, b: &str) -> PyResult<String> {
    match a {
        J::Str(s) => Ok(format!("{s}{b}")),
        other => Err(PyErr::type_error(format!(
            "unsupported operand type(s) for +: '{}' and 'str'",
            other.type_name()
        ))),
    }
}

fn int_float_eq(i: i64, f: f64) -> bool {
    f.fract() == 0.0
        && (-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&f)
        && (f as i64) == i
}

/// Python `==`.
pub fn py_eq(a: &J, b: &J) -> bool {
    match (a, b) {
        (J::Null, J::Null) => true,
        (J::Str(x), J::Str(y)) => x == y,
        (J::Arr(x), J::Arr(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q)),
        (J::Obj(x), J::Obj(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| py_eq(v, w)))
        }
        (J::Float(x), J::Float(y)) => x == y,
        (J::Float(f), other) | (other, J::Float(f)) => match other.as_inst_int() {
            Some(i) => int_float_eq(i, *f),
            None => false,
        },
        _ => match (a.as_inst_int(), b.as_inst_int()) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

/// Python `==` against an integer literal.
pub fn eq_int(a: &J, n: i64) -> bool {
    py_eq(a, &J::Int(n))
}

/// Python `==` against a string literal.
pub fn eq_str(a: &J, s: &str) -> bool {
    matches!(a, J::Str(x) if x == s)
}

/// Python `<`-ordering (raises `TypeError` for unorderable types).
pub fn py_cmp(a: &J, b: &J) -> PyResult<Ordering> {
    let unorderable = || {
        PyErr::type_error(format!(
            "'<' not supported between instances of '{}' and '{}'",
            a.type_name(),
            b.type_name()
        ))
    };
    match (a, b) {
        (J::Str(x), J::Str(y)) => Ok(x.cmp(y)),
        (J::Arr(x), J::Arr(y)) => {
            for (p, q) in x.iter().zip(y) {
                if !py_eq(p, q) {
                    return py_cmp(p, q);
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        _ => match (a.as_inst_int(), b.as_inst_int()) {
            (Some(x), Some(y)) => Ok(x.cmp(&y)),
            _ => match (a.as_f64(), b.as_f64()) {
                (Some(x), Some(y)) => Ok(x.partial_cmp(&y).unwrap_or(Ordering::Equal)),
                _ => Err(unorderable()),
            },
        },
    }
}

/// `sorted(values)` with Python's ordering (stable; `TypeError` on mixed types).
pub fn py_sorted(mut values: Vec<J>) -> PyResult<Vec<J>> {
    let mut error = None;
    values.sort_by(|a, b| match py_cmp(a, b) {
        Ok(o) => o,
        Err(e) => {
            error.get_or_insert(e);
            Ordering::Equal
        }
    });
    match error {
        Some(e) => Err(e),
        None => Ok(values),
    }
}

/// `sorted(set(values))`.
pub fn py_sorted_set(values: Vec<J>) -> PyResult<Vec<J>> {
    let mut unique: Vec<J> = Vec::new();
    for v in values {
        if matches!(v, J::Arr(_) | J::Obj(_)) {
            return Err(PyErr::type_error(format!(
                "unhashable type: '{}'",
                v.type_name()
            )));
        }
        if !unique.iter().any(|u| py_eq(u, &v)) {
            unique.push(v);
        }
    }
    py_sorted(unique)
}

/// `sorted(set(strings))`.
pub fn sorted_set(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let set: std::collections::BTreeSet<String> = values.into_iter().collect();
    set.into_iter().collect()
}

// ---------------------------------------------------------------------------
// str() / repr()
// ---------------------------------------------------------------------------

/// Python `str(x)`.
pub fn py_str(j: &J) -> String {
    match j {
        J::Str(s) => s.clone(),
        other => py_repr(other),
    }
}

/// Python `repr(x)`.
pub fn py_repr(j: &J) -> String {
    match j {
        J::Null => "None".to_string(),
        J::Bool(true) => "True".to_string(),
        J::Bool(false) => "False".to_string(),
        J::Int(i) => i.to_string(),
        J::Float(f) => py_float_repr(*f),
        J::Str(s) => py_str_repr(s),
        J::Arr(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        J::Obj(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", py_str_repr(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn py_printable(c: char) -> bool {
    let cp = c as u32;
    if cp < 0x20 || (0x7f..=0xa0).contains(&cp) {
        return false;
    }
    !matches!(cp,
        0xad | 0x600..=0x605 | 0x61c | 0x6dd | 0x70f | 0x1680 | 0x180e | 0x2000..=0x200f
        | 0x2028..=0x202f | 0x205f..=0x2064 | 0x2066..=0x206f | 0x3000 | 0xd800..=0xf8ff
        | 0xfdd0..=0xfdef | 0xfeff | 0xfff9..=0xfffb | 0xfffe | 0xffff | 0x110bd
        | 0x1d173..=0x1d17a | 0xe0001 | 0xe0020..=0xe007f | 0xf0000..=0x10ffff)
}

/// Python `repr(str)`.
pub fn py_str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if py_printable(c) => out.push(c),
            c => {
                let cp = c as u32;
                if cp <= 0xff {
                    out.push_str(&format!("\\x{cp:02x}"));
                } else if cp <= 0xffff {
                    out.push_str(&format!("\\u{cp:04x}"));
                } else {
                    out.push_str(&format!("\\U{cp:08x}"));
                }
            }
        }
    }
    out.push(quote);
    out
}

/// Python `repr(float)`.
pub fn py_float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0.0".to_string()
        } else {
            "0.0".to_string()
        };
    }
    let sign = if f < 0.0 { "-" } else { "" };
    let sci = format!("{:e}", f.abs());
    let (mantissa, exponent) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let exp: i32 = exponent.parse().unwrap_or(0);
    let n = digits.len() as i32;
    let body = if (-4..16).contains(&exp) {
        if exp >= 0 {
            if n <= exp + 1 {
                format!("{digits}{}.0", "0".repeat((exp + 1 - n) as usize))
            } else {
                let (a, b) = digits.split_at((exp + 1) as usize);
                format!("{a}.{b}")
            }
        } else {
            format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
        }
    } else {
        let m = if digits.len() == 1 {
            digits.clone()
        } else {
            format!("{}.{}", &digits[..1], &digits[1..])
        };
        format!("{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    };
    format!("{sign}{body}")
}

/// The string Python's `json.dumps` uses for a non-string dict key.
pub fn json_key(j: &J) -> String {
    match j {
        J::Str(s) => s.clone(),
        J::Null => "null".to_string(),
        J::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        J::Int(i) => i.to_string(),
        J::Float(f) => json_float(*f),
        other => py_repr(other),
    }
}

// ---------------------------------------------------------------------------
// JSON (Python json module parity)
// ---------------------------------------------------------------------------

struct JVisitor;

impl<'de> Visitor<'de> for JVisitor {
    type Value = J;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<J, E> {
        Ok(J::Bool(v))
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<J, E> {
        Ok(J::Int(v))
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<J, E> {
        Ok(match i64::try_from(v) {
            Ok(i) => J::Int(i),
            Err(_) => J::Float(v as f64),
        })
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<J, E> {
        Ok(J::Float(v))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<J, E> {
        Ok(J::Str(v.to_string()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<J, E> {
        Ok(J::Str(v))
    }
    fn visit_unit<E: de::Error>(self) -> Result<J, E> {
        Ok(J::Null)
    }
    fn visit_none<E: de::Error>(self) -> Result<J, E> {
        Ok(J::Null)
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<J, D::Error> {
        J::deserialize(d)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<J, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = seq.next_element::<J>()? {
            out.push(v);
        }
        Ok(J::Arr(out))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<J, A::Error> {
        let mut obj = Obj::new();
        while let Some((k, v)) = map.next_entry::<String, J>()? {
            obj.set(k, v);
        }
        Ok(J::Obj(obj))
    }
}

impl<'de> Deserialize<'de> for J {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<J, D::Error> {
        d.deserialize_any(JVisitor)
    }
}

/// `json.loads(text)`; `None` on any error.
pub fn loads(text: &str) -> Option<J> {
    serde_json::from_str::<J>(text).ok()
}

/// Convert a `serde_json::Value` (e.g. a serialized config) into [`J`].
pub fn from_serde(v: &serde_json::Value) -> J {
    match v {
        serde_json::Value::Null => J::Null,
        serde_json::Value::Bool(b) => J::Bool(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => J::Int(i),
            None => J::Float(n.as_f64().unwrap_or(0.0)),
        },
        serde_json::Value::String(s) => J::Str(s.clone()),
        serde_json::Value::Array(a) => J::Arr(a.iter().map(from_serde).collect()),
        serde_json::Value::Object(o) => {
            J::Obj(o.iter().map(|(k, v)| (k.clone(), from_serde(v))).collect())
        }
    }
}

fn json_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".to_string()
    } else if f.is_infinite() {
        if f > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        }
    } else {
        py_float_repr(f)
    }
}

fn json_escape(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
}

fn write_json(out: &mut String, j: &J, indent: Option<usize>, level: usize) {
    match j {
        J::Null => out.push_str("null"),
        J::Bool(true) => out.push_str("true"),
        J::Bool(false) => out.push_str("false"),
        J::Int(i) => out.push_str(&i.to_string()),
        J::Float(f) => out.push_str(&json_float(*f)),
        J::Str(s) => json_escape(out, s),
        J::Arr(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, item) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(if indent.is_some() { "," } else { ", " });
                }
                if let Some(n) = indent {
                    out.push('\n');
                    out.push_str(&" ".repeat(n * (level + 1)));
                }
                write_json(out, item, indent, level + 1);
            }
            if let Some(n) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(n * level));
            }
            out.push(']');
        }
        J::Obj(o) => {
            if o.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, v)) in o.iter().enumerate() {
                if i > 0 {
                    out.push_str(if indent.is_some() { "," } else { ", " });
                }
                if let Some(n) = indent {
                    out.push('\n');
                    out.push_str(&" ".repeat(n * (level + 1)));
                }
                json_escape(out, k);
                out.push_str(": ");
                write_json(out, v, indent, level + 1);
            }
            if let Some(n) = indent {
                out.push('\n');
                out.push_str(&" ".repeat(n * level));
            }
            out.push('}');
        }
    }
}

/// `json.dumps(j)` (indent `None`) or `json.dumps(j, indent=n)`.
pub fn dumps(j: &J, indent: Option<usize>) -> String {
    let mut out = String::new();
    write_json(&mut out, j, indent, 0);
    out
}

// ---------------------------------------------------------------------------
// Strings
// ---------------------------------------------------------------------------

/// `str.isspace()` for one character (Python `\s` in str patterns).
pub fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python `\w` for one character.
pub fn py_isword(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}
pub fn py_lstrip(s: &str) -> &str {
    s.trim_start_matches(py_isspace)
}
pub fn py_rstrip(s: &str) -> &str {
    s.trim_end_matches(py_isspace)
}
/// `s.strip(chars)`.
pub fn py_strip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_matches(|c| chars.contains(c))
}
/// `s.lstrip(chars)`.
pub fn py_lstrip_chars<'a>(s: &'a str, chars: &str) -> &'a str {
    s.trim_start_matches(|c| chars.contains(c))
}

/// `s.split()` (no separator).
pub fn py_split_ws(s: &str) -> Vec<&str> {
    s.split(py_isspace).filter(|p| !p.is_empty()).collect()
}

/// `s.splitlines(keepends)`.
pub fn py_splitlines(s: &str, keepends: bool) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut iter = s.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        let sep_len = match c {
            '\r' => {
                if let Some(&(_, '\n')) = iter.peek() {
                    iter.next();
                    2
                } else {
                    1
                }
            }
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => c.len_utf8(),
            _ => continue,
        };
        let end = i + sep_len;
        out.push(if keepends {
            &s[start..end]
        } else {
            &s[start..i]
        });
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Text-mode universal-newline translation (`\r\n` and `\r` become `\n`).
pub fn universal_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// `len(s)` in code points.
pub fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// Code-point index of a byte offset.
pub fn char_index(s: &str, byte: usize) -> usize {
    s.get(..byte).map_or(0, |p| p.chars().count())
}

fn byte_at_char(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

/// `s[a:b]` (code-point indices, clamped).
pub fn char_slice(s: &str, a: usize, b: usize) -> &str {
    if b <= a {
        return "";
    }
    let start = byte_at_char(s, a);
    let end = byte_at_char(s, b);
    &s[start..end.max(start)]
}

/// `s[:n]`.
pub fn prefix(s: &str, n: usize) -> &str {
    &s[..byte_at_char(s, n)]
}

/// `s[-n:]` (n > 0).
pub fn suffix(s: &str, n: usize) -> &str {
    let len = char_len(s);
    if n >= len {
        s
    } else {
        &s[byte_at_char(s, len - n)..]
    }
}

/// `round(x)` (round half to even) as an integer.
pub fn py_round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

static DIGIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d$").expect("digit regex"));

fn is_unicode_digit(c: char) -> bool {
    let mut buf = [0u8; 4];
    DIGIT_RE.is_match(c.encode_utf8(&mut buf))
}

/// Decimal value of a Python `\d` character.
pub fn digit_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(c as u32 - '0' as u32);
    }
    if !is_unicode_digit(c) {
        return None;
    }
    let cp = c as u32;
    let mut start = cp;
    while start > 0 && cp - start < 60 {
        match char::from_u32(start - 1) {
            Some(prev) if is_unicode_digit(prev) => start -= 1,
            _ => break,
        }
    }
    Some((cp - start) % 10)
}

/// `int(s)` for a run of `\d` characters (as matched by a regex group).
pub fn py_int(s: &str) -> Option<i64> {
    let t = py_strip(s);
    let (neg, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty() {
        return None;
    }
    let mut value: i64 = 0;
    for c in digits.chars() {
        let d = i64::from(digit_value(c)?);
        value = value.saturating_mul(10).saturating_add(d);
    }
    Some(if neg { -value } else { value })
}

/// `str.isdigit()` restricted to decimal digits.
pub fn py_isdigit(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| digit_value(c).is_some())
}

/// `float(s)`.
pub fn py_float(s: &str) -> Option<f64> {
    let t = py_strip(s);
    let cleaned: String = t.chars().filter(|c| *c != '_').collect();
    cleaned.parse::<f64>().ok()
}

/// `int(float(s))` (truncation; fails on nan/inf like Python).
pub fn py_int_of_float(s: &str) -> Option<i64> {
    let f = py_float(s)?;
    if !f.is_finite() {
        return None;
    }
    Some(f.trunc() as i64)
}

// ---------------------------------------------------------------------------
// Datetimes (datetime.fromisoformat / isoformat parity)
// ---------------------------------------------------------------------------

/// Result of `datetime.fromisoformat`: naive fields plus optional offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PyDt {
    pub naive: NaiveDateTime,
    pub offset: Option<FixedOffset>,
}

impl PyDt {
    pub fn aware(&self) -> Option<Dt> {
        let off = self.offset?;
        off.from_local_datetime(&self.naive).single()
    }
}

fn ascii_num(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn parse_hh_mm_ss_ff(t: &[char]) -> Option<[u32; 4]> {
    let len = t.len();
    let mut comps = [0u32; 4];
    let mut pos = 0;
    let mut has_sep = false;
    for (comp, slot) in comps.iter_mut().take(3).enumerate() {
        if len < pos + 2 {
            return None;
        }
        let two: String = t[pos..pos + 2].iter().collect();
        *slot = ascii_num(&two)?;
        pos += 2;
        let next = t.get(pos).copied();
        if comp == 0 {
            has_sep = next == Some(':');
        }
        let Some(next) = next else { break };
        if comp >= 2 {
            break;
        }
        if has_sep && next != ':' {
            return None;
        }
        if has_sep {
            pos += 1;
        }
    }
    if pos < len {
        if t[pos] != '.' && t[pos] != ',' {
            return None;
        }
        pos += 1;
        let remainder = len - pos;
        let to_parse = remainder.min(6);
        let frac: String = t[pos..pos + to_parse].iter().collect();
        let mut micros = ascii_num(&frac)?;
        for _ in to_parse..6 {
            micros *= 10;
        }
        if remainder > to_parse && !t[pos + to_parse..].iter().all(|c| c.is_ascii_digit()) {
            return None;
        }
        comps[3] = micros;
    }
    Some(comps)
}

fn find_date_separator(d: &[char]) -> Option<usize> {
    let len = d.len();
    if len == 7 {
        return Some(7);
    }
    if d[4] == '-' {
        if d[5] == 'W' {
            if len < 8 {
                return None;
            }
            if len > 8 && d[8] == '-' {
                if len == 9 {
                    return None;
                }
                if len > 10 && d[10].is_ascii_digit() {
                    return Some(8);
                }
                return Some(10);
            }
            return Some(8);
        }
        return Some(10);
    }
    if d[4] == 'W' {
        let mut idx = 7;
        while idx < len {
            if !d[idx].is_ascii_digit() {
                break;
            }
            idx += 1;
        }
        if idx < 9 {
            return Some(idx);
        }
        return Some(if idx % 2 == 0 { 7 } else { 8 });
    }
    Some(8)
}

fn parse_iso_date(d: &[char]) -> Option<NaiveDate> {
    if d.len() < 7 {
        return None;
    }
    let year: String = d[0..4].iter().collect();
    let year = ascii_num(&year)?;
    let has_sep = d[4] == '-';
    let mut pos = 4 + usize::from(has_sep);
    if d.get(pos) == Some(&'W') {
        // ISO week dates: YYYY-Www[-D] / YYYYWww[D]
        pos += 1;
        let week: String = d.get(pos..pos + 2)?.iter().collect();
        let week = ascii_num(&week)?;
        pos += 2;
        let mut day = 1;
        if d.len() > pos {
            if (d[pos] == '-') != has_sep {
                return None;
            }
            pos += usize::from(has_sep);
            let dd: String = d.get(pos..pos + 1)?.iter().collect();
            day = ascii_num(&dd)?;
        }
        let weekday = chrono::Weekday::try_from(u8::try_from(day.checked_sub(1)?).ok()?).ok()?;
        return NaiveDate::from_isoywd_opt(i32::try_from(year).ok()?, week, weekday);
    }
    let month: String = d.get(pos..pos + 2)?.iter().collect();
    let month = ascii_num(&month)?;
    pos += 2;
    if (d.get(pos) == Some(&'-')) != has_sep {
        return None;
    }
    pos += usize::from(has_sep);
    let day: String = d.get(pos..pos + 2)?.iter().collect();
    if pos + 2 != d.len() {
        return None;
    }
    let day = ascii_num(&day)?;
    NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)
}

fn parse_iso_time(t: &[char]) -> Option<(NaiveTime, Option<FixedOffset>)> {
    if t.len() < 2 {
        return None;
    }
    let tz_pos = t.iter().position(|c| matches!(c, 'Z' | '+' | '-'));
    let time_part = match tz_pos {
        Some(p) => &t[..p],
        None => t,
    };
    let comps = parse_hh_mm_ss_ff(time_part)?;
    let mut offset = None;
    if let Some(p) = tz_pos {
        if t[p] == 'Z' {
            if p + 1 != t.len() {
                return None;
            }
            offset = Some(FixedOffset::east_opt(0)?);
        } else {
            let tz = &t[p + 1..];
            if matches!(tz.len(), 0 | 1 | 3) {
                return None;
            }
            let tzc = parse_hh_mm_ss_ff(tz)?;
            if tzc[3] != 0 {
                return None;
            }
            let secs = i32::try_from(tzc[0] * 3600 + tzc[1] * 60 + tzc[2]).ok()?;
            if secs >= 86400 {
                return None;
            }
            let sign = if t[p] == '-' { -1 } else { 1 };
            offset = Some(FixedOffset::east_opt(sign * secs)?);
        }
    }
    let time = NaiveTime::from_hms_micro_opt(comps[0], comps[1], comps[2], comps[3])?;
    Some((time, offset))
}

/// `datetime.fromisoformat(s)` (Python 3.11+ grammar).
pub fn fromisoformat(s: &str) -> Option<PyDt> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 7 {
        return None;
    }
    let sep = find_date_separator(&chars)?;
    let date = parse_iso_date(chars.get(..sep)?)?;
    let rest = chars.get(sep + 1..).unwrap_or(&[]);
    let (time, offset) = if rest.is_empty() {
        (NaiveTime::from_hms_opt(0, 0, 0)?, None)
    } else {
        parse_iso_time(rest)?
    };
    Some(PyDt {
        naive: NaiveDateTime::new(date, time),
        offset,
    })
}

/// `datetime.fromisoformat(s.replace('Z', '+00:00'))` returning only aware values.
pub fn fromiso_aware(s: &str) -> Option<Dt> {
    fromisoformat(&s.replace('Z', "+00:00"))?.aware()
}

/// `parse_utc(v)` from sc_audit.py: falsy → None; naive → UTC.
pub fn parse_utc(v: &J) -> Option<Dt> {
    if !v.truthy() {
        return None;
    }
    let parsed = fromisoformat(&py_str(v).replace('Z', "+00:00"))?;
    match parsed.offset {
        Some(_) => parsed.aware(),
        None => utc().from_local_datetime(&parsed.naive).single(),
    }
}

/// The UTC offset.
pub fn utc() -> FixedOffset {
    FixedOffset::east_opt(0).expect("a zero UTC offset is always valid")
}

fn fmt_offset(off: &FixedOffset) -> String {
    let secs = off.local_minus_utc();
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.unsigned_abs();
    let (h, m, s) = (a / 3600, (a % 3600) / 60, a % 60);
    if s == 0 {
        format!("{sign}{h:02}:{m:02}")
    } else {
        format!("{sign}{h:02}:{m:02}:{s:02}")
    }
}

/// `dt.isoformat(timespec='seconds')`.
pub fn iso_seconds(dt: &Dt) -> String {
    format!(
        "{}{}",
        dt.format("%Y-%m-%dT%H:%M:%S"),
        fmt_offset(dt.offset())
    )
}

/// `dt.isoformat(timespec='minutes')`.
pub fn iso_minutes(dt: &Dt) -> String {
    format!("{}{}", dt.format("%Y-%m-%dT%H:%M"), fmt_offset(dt.offset()))
}

/// `dt.strftime('%H:%MZ')` (local fields of the datetime, literal Z).
pub fn hhmm_z(dt: &Dt) -> String {
    format!("{:02}:{:02}Z", dt.hour(), dt.minute())
}

/// `(a - b).total_seconds()`.
pub fn total_seconds(a: &Dt, b: &Dt) -> f64 {
    let d = a.signed_duration_since(*b);
    match d.num_microseconds() {
        Some(us) => us as f64 / 1_000_000.0,
        None => d.num_milliseconds() as f64 / 1000.0,
    }
}

/// `(now - datetime.fromisoformat(since)).total_seconds() / 60` with Python's exceptions.
pub fn since_age_min(now: &Dt, since: &J) -> PyResult<f64> {
    let s = match since {
        J::Str(s) => s,
        other => {
            return Err(PyErr::type_error(format!(
                "fromisoformat: argument must be str, not {}",
                other.type_name()
            )));
        }
    };
    let parsed = fromisoformat(s).ok_or_else(|| {
        PyErr::value_error(format!("Invalid isoformat string: {}", py_str_repr(s)))
    })?;
    let t = parsed.aware().ok_or_else(|| {
        PyErr::type_error("can't subtract offset-naive and offset-aware datetimes")
    })?;
    Ok(total_seconds(now, &t) / 60.0)
}

/// `datetime.fromtimestamp(ts, timezone.utc)`.
pub fn utc_from_timestamp(ts: f64) -> Option<Dt> {
    if !ts.is_finite() {
        return None;
    }
    let secs = ts.floor();
    let micros = ((ts - secs) * 1_000_000.0).round_ties_even() as i64;
    let base = utc().timestamp_opt(secs as i64, 0).single()?;
    Some(base + chrono::Duration::microseconds(micros))
}

/// Python `now.timestamp()` (float seconds since the epoch).
pub fn timestamp_f64(dt: &Dt) -> f64 {
    let micros = dt.timestamp_micros();
    micros as f64 / 1_000_000.0
}

/// `now.replace(hour=h, minute=m, second=0, microsecond=0)`.
pub fn replace_hm(now: &Dt, hour: u32, minute: u32) -> Option<Dt> {
    let naive = now.date_naive().and_hms_opt(hour, minute, 0)?;
    now.offset().from_local_datetime(&naive).single()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_repr_matches_python() {
        assert_eq!(py_float_repr(1.0), "1.0");
        assert_eq!(py_float_repr(0.5), "0.5");
        assert_eq!(py_float_repr(1e16), "1e+16");
        assert_eq!(py_float_repr(1e-5), "1e-05");
        assert_eq!(py_float_repr(0.0001), "0.0001");
        assert_eq!(py_float_repr(123.456), "123.456");
        assert_eq!(py_float_repr(-2.5), "-2.5");
        assert_eq!(py_float_repr(1234567890123456.0), "1234567890123456.0");
        assert_eq!(
            py_float_repr(1.2345678901234568e17),
            "1.2345678901234568e+17"
        );
    }

    #[test]
    fn repr_and_dumps_match_python() {
        let v = jobj! {"red" => 3, "amber" => 1, "must_act" => 2};
        assert_eq!(py_repr(&v), "{'red': 3, 'amber': 1, 'must_act': 2}");
        assert_eq!(py_str_repr("it's"), "\"it's\"");
        assert_eq!(py_str_repr("a\"b'c"), "'a\"b\\'c'");
        let j = jobj! {"a" => vec![J::Int(1), J::Str("é".into())], "b" => J::Obj(Obj::new()), "c" => J::Arr(vec![]), "d" => J::Null};
        assert_eq!(
            dumps(&j, None),
            "{\"a\": [1, \"\\u00e9\"], \"b\": {}, \"c\": [], \"d\": null}"
        );
        assert_eq!(
            dumps(&j, Some(1)),
            "{\n \"a\": [\n  1,\n  \"\\u00e9\"\n ],\n \"b\": {},\n \"c\": [],\n \"d\": null\n}"
        );
        assert_eq!(
            dumps(&J::Str("😀\u{7f}/".into()), None),
            "\"\\ud83d\\ude00\\u007f/\""
        );
    }

    #[test]
    fn loads_preserves_order_and_duplicates_keep_first_position() {
        let j = loads(r#"{"z": 1, "a": 2, "z": 3}"#).expect("json");
        assert_eq!(dumps(&j, None), "{\"z\": 3, \"a\": 2}");
    }

    #[test]
    fn python_equality_and_truthiness() {
        assert!(py_eq(&J::Bool(false), &J::Int(0)));
        assert!(py_eq(&J::Float(1.0), &J::Int(1)));
        assert!(!py_eq(&J::Str("0".into()), &J::Int(0)));
        assert!(!J::Obj(Obj::new()).truthy());
        assert!(J::Str(" ".into()).truthy());
    }

    #[test]
    fn fromisoformat_grammar() {
        let a = fromisoformat("2026-10-18T13:00:00").expect("naive");
        assert!(a.offset.is_none());
        assert!(fromisoformat("not-a-time").is_none());
        assert!(fromiso_aware("2026-10-18T14:01:00Z").is_some());
        assert!(fromiso_aware("2026-10-18T17:06Z").is_some());
        let b = fromisoformat("2026-10-18T12:47:06.123+02:00").expect("aware");
        assert_eq!(b.offset.map(|o| o.local_minus_utc()), Some(7200));
        assert!(fromisoformat("garbage").is_none());
        assert!(fromisoformat("None").is_none());
        let d = fromisoformat("2026-10-18").expect("date only");
        assert_eq!(d.naive.hour(), 0);
    }

    #[test]
    fn rounding_is_half_even() {
        assert_eq!(py_round(2.5), 2);
        assert_eq!(py_round(3.5), 4);
        assert_eq!(py_round(62.9), 63);
    }

    #[test]
    fn splitlines_and_slicing() {
        assert_eq!(
            py_splitlines("a\r\nb\rc\n", true),
            vec!["a\r\n", "b\r", "c\n"]
        );
        assert_eq!(py_splitlines("a\nb", false), vec!["a", "b"]);
        assert_eq!(prefix("héllo", 2), "hé");
        assert_eq!(suffix("héllo", 3), "llo");
        assert_eq!(char_slice("héllo", 1, 3), "él");
    }

    #[test]
    fn unicode_digits_parse_like_python_int() {
        assert_eq!(py_int("١٢"), Some(12));
        assert_eq!(py_int("-5"), Some(-5));
        assert_eq!(py_int("x"), None);
    }
}
