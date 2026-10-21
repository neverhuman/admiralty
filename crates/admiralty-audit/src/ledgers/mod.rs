//! Per-project ledger/receipt recomputation (read-only).

pub mod admiralty;
pub mod dope;
pub mod jarambit;
pub mod joil;
pub mod jopedime;

use crate::py::{self, J, PyErr, PyResult};

/// `collections.defaultdict(list)` keyed by problem code, in first-insertion order.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Buckets {
    items: Vec<(String, Vec<String>)>,
}

impl Buckets {
    pub fn push(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.items.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => v.push(value),
            None => self.items.push((key.to_string(), vec![value])),
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Vec<String>)> {
        self.items.iter().map(|(k, v)| (k, v))
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn get(&self, key: &str) -> Option<&Vec<String>> {
        self.items.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    /// `{k: sorted(set(v)) for k, v in bad.items()}`.
    pub fn to_json_sorted(&self) -> J {
        let mut o = py::Obj::new();
        for (k, v) in &self.items {
            o.set(k.clone(), J::from(py::sorted_set(v.iter().cloned())));
        }
        J::Obj(o)
    }
}

/// `any(pred(x) for x in items)` with short-circuit and exception propagation.
pub fn any_py(items: &[J], mut pred: impl FnMut(&J) -> PyResult<bool>) -> PyResult<bool> {
    for x in items {
        if pred(x)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `all(pred(x) for x in items)`.
pub fn all_py(items: &[J], mut pred: impl FnMut(&J) -> PyResult<bool>) -> PyResult<bool> {
    for x in items {
        if !pred(x)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `a + b` for two lists.
pub fn concat_lists(a: &J, b: &J) -> PyResult<Vec<J>> {
    match (a, b) {
        (J::Arr(x), J::Arr(y)) => Ok(x.iter().chain(y).cloned().collect()),
        (J::Str(_), other) => Err(PyErr::type_error(format!(
            "can only concatenate str (not \"{}\") to str",
            other.type_name()
        ))),
        (J::Arr(_), other) => Err(PyErr::type_error(format!(
            "can only concatenate list (not \"{}\") to list",
            other.type_name()
        ))),
        (x, y) => Err(PyErr::type_error(format!(
            "unsupported operand type(s) for +: '{}' and '{}'",
            x.type_name(),
            y.type_name()
        ))),
    }
}

/// `x or []` iterated as a list (`for c in x or []`).
pub fn iter_or_empty(x: &J) -> PyResult<Vec<J>> {
    if x.truthy() {
        x.py_iter()
    } else {
        Ok(Vec::new())
    }
}

/// `str(x.get(k, ''))`.
pub fn get_str_default(x: &J, key: &str) -> PyResult<String> {
    let empty = J::Str(String::new());
    Ok(py::py_str(x.pget_or(key, &empty)?))
}

/// `str(x.get(k) or '')`.
pub fn get_str_or_empty(x: &J, key: &str) -> PyResult<String> {
    let v = x.pget(key)?;
    Ok(if v.truthy() {
        py::py_str(v)
    } else {
        String::new()
    })
}

/// Python `fullmatch` of a fixed hex length (`[0-9a-f]{n}`).
pub fn is_hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
