//! Small Python / bash compatibility helpers so the Rust ports print the same
//! bytes the retired scripts printed (float repr, `round`, `str.splitlines`,
//! `json.dump` separators and `ensure_ascii`, `printf %b`).

use std::io::{self, Write};

use serde::Serialize;
use serde_json::ser::{CharEscape, Formatter, Serializer};

/// Python `str.isspace()` for one character (Rust `White_Space` plus the
/// information separators U+001C..U+001F that Python also treats as space).
pub fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python `str.strip()`.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// Python `str.rstrip()`.
pub fn py_rstrip(s: &str) -> &str {
    s.trim_end_matches(is_py_space)
}

/// Python `str.splitlines()` (no keepends): splits on `\n`, `\r`, `\r\n`,
/// `\v`, `\f`, `\x1c`, `\x1d`, `\x1e`, `\x85`, U+2028 and U+2029.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut iter = s.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        match c {
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            '\r' => {
                out.push(&s[start..i]);
                if matches!(iter.peek(), Some((_, '\n'))) {
                    iter.next();
                    start = i + 2;
                } else {
                    start = i + 1;
                }
            }
            _ => {}
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// First `n` characters (Python `s[:n]`, bash `${s:0:n}` in a UTF-8 locale).
pub fn take_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Last `n` characters (Python `s[-n:]`).
pub fn last_chars(s: &str, n: usize) -> &str {
    let count = s.chars().count();
    if count <= n {
        return s;
    }
    match s.char_indices().nth(count - n) {
        Some((idx, _)) => &s[idx..],
        None => s,
    }
}

/// Python `repr(float)` / `str(float)`.
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.to_string();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0" } else { "0.0" }.to_string();
    }
    // Rust's `{:e}` yields the shortest round-trip digits, like Python's repr.
    let sci = format!("{x:e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let decpt = exp + 1;
    let mut body = String::new();
    if -4 < decpt && decpt <= 16 {
        if decpt <= 0 {
            body.push_str("0.");
            for _ in 0..(-decpt) {
                body.push('0');
            }
            body.push_str(&digits);
        } else {
            let point = usize::try_from(decpt).unwrap_or(0);
            if point >= digits.len() {
                body.push_str(&digits);
                for _ in digits.len()..point {
                    body.push('0');
                }
                body.push_str(".0");
            } else {
                body.push_str(&digits[..point]);
                body.push('.');
                body.push_str(&digits[point..]);
            }
        }
    } else {
        body.push_str(&digits[..1]);
        if digits.len() > 1 {
            body.push('.');
            body.push_str(&digits[1..]);
        }
        body.push('e');
        body.push(if exp < 0 { '-' } else { '+' });
        body.push_str(&format!("{:02}", exp.abs()));
    }
    if negative { format!("-{body}") } else { body }
}

/// Python `round(x, ndigits)` for floats (correctly rounded, ties to even on
/// the exact binary value — identical to Rust's exact `{:.N}` formatting).
pub fn py_round(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.ndigits$}").parse().unwrap_or(x)
}

/// Python `round(x)` (no ndigits): nearest integer, ties to even.
pub fn py_round_int(x: f64) -> i64 {
    // Saturating float-to-int conversion; values here are small.
    x.round_ties_even() as i64
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
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr(list_of_str)`.
pub fn py_list_repr(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| py_str_repr(s)).collect();
    format!("[{}]", inner.join(", "))
}

/// Layout of Python `json.dump` output.
#[derive(Debug, Clone, Copy)]
pub enum JsonLayout {
    /// `json.dump(x)` default: `", "` and `": "` separators, one line.
    Compact,
    /// `json.dump(x, indent=N)`.
    Indent(usize),
}

/// serde_json formatter reproducing Python's `json` module byte-for-byte for
/// the value shapes these tools emit.
pub struct PyJsonFormatter {
    layout: JsonLayout,
    ensure_ascii: bool,
    level: usize,
    has_value: bool,
}

impl PyJsonFormatter {
    pub fn new(layout: JsonLayout, ensure_ascii: bool) -> Self {
        Self {
            layout,
            ensure_ascii,
            level: 0,
            has_value: false,
        }
    }

    fn newline_indent<W: ?Sized + Write>(&self, w: &mut W) -> io::Result<()> {
        if let JsonLayout::Indent(n) = self.layout {
            w.write_all(b"\n")?;
            for _ in 0..(self.level * n) {
                w.write_all(b" ")?;
            }
        }
        Ok(())
    }
}

impl Formatter for PyJsonFormatter {
    fn write_f64<W: ?Sized + Write>(&mut self, w: &mut W, value: f64) -> io::Result<()> {
        w.write_all(py_float_repr(value).as_bytes())
    }

    fn write_f32<W: ?Sized + Write>(&mut self, w: &mut W, value: f32) -> io::Result<()> {
        w.write_all(py_float_repr(f64::from(value)).as_bytes())
    }

    fn write_string_fragment<W: ?Sized + Write>(
        &mut self,
        w: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        if !self.ensure_ascii {
            return w.write_all(fragment.as_bytes());
        }
        for c in fragment.chars() {
            if (' '..='~').contains(&c) {
                let mut buf = [0u8; 4];
                w.write_all(c.encode_utf8(&mut buf).as_bytes())?;
            } else {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    write!(w, "\\u{unit:04x}")?;
                }
            }
        }
        Ok(())
    }

    fn write_char_escape<W: ?Sized + Write>(
        &mut self,
        w: &mut W,
        char_escape: CharEscape,
    ) -> io::Result<()> {
        let s: &[u8] = match char_escape {
            CharEscape::Quote => b"\\\"",
            CharEscape::ReverseSolidus => b"\\\\",
            CharEscape::Solidus => b"/",
            CharEscape::Backspace => b"\\b",
            CharEscape::FormFeed => b"\\f",
            CharEscape::LineFeed => b"\\n",
            CharEscape::CarriageReturn => b"\\r",
            CharEscape::Tab => b"\\t",
            CharEscape::AsciiControl(byte) => {
                return write!(w, "\\u{:04x}", u32::from(byte));
            }
        };
        w.write_all(s)
    }

    fn begin_array<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.level += 1;
        self.has_value = false;
        w.write_all(b"[")
    }

    fn end_array<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.level -= 1;
        if self.has_value {
            self.newline_indent(w)?;
        }
        w.write_all(b"]")
    }

    fn begin_array_value<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        match self.layout {
            JsonLayout::Compact => {
                if !first {
                    w.write_all(b", ")?;
                }
                Ok(())
            }
            JsonLayout::Indent(_) => {
                if !first {
                    w.write_all(b",")?;
                }
                self.newline_indent(w)
            }
        }
    }

    fn end_array_value<W: ?Sized + Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }

    fn begin_object<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.level += 1;
        self.has_value = false;
        w.write_all(b"{")
    }

    fn end_object<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        self.level -= 1;
        if self.has_value {
            self.newline_indent(w)?;
        }
        w.write_all(b"}")
    }

    fn begin_object_key<W: ?Sized + Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
        self.begin_array_value(w, first)
    }

    fn begin_object_value<W: ?Sized + Write>(&mut self, w: &mut W) -> io::Result<()> {
        w.write_all(b": ")
    }

    fn end_object_value<W: ?Sized + Write>(&mut self, _w: &mut W) -> io::Result<()> {
        self.has_value = true;
        Ok(())
    }
}

/// Serialize like Python `json.dumps(value, indent=..., ensure_ascii=...)`.
pub fn py_json<T: Serialize + ?Sized>(
    value: &T,
    layout: JsonLayout,
    ensure_ascii: bool,
) -> io::Result<String> {
    let mut buf = Vec::new();
    let mut ser = Serializer::with_formatter(&mut buf, PyJsonFormatter::new(layout, ensure_ascii));
    value.serialize(&mut ser).map_err(io::Error::other)?;
    String::from_utf8(buf).map_err(io::Error::other)
}

/// Expand backslash escapes exactly like bash `printf '%b'`, then apply the
/// `$(...)` capture rules (NUL bytes dropped, trailing newlines stripped).
pub fn bash_printf_b(input: &str) -> Vec<u8> {
    let s = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        i += 1;
        if c != b'\\' || i >= s.len() {
            out.push(c);
            continue;
        }
        let e = s[i];
        i += 1;
        match e {
            b'a' => out.push(0x07),
            b'b' => out.push(0x08),
            b'e' | b'E' => out.push(0x1b),
            b'f' => out.push(0x0c),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(0x0b),
            b'\\' => out.push(b'\\'),
            b'0'..=b'7' => {
                let mut value = u32::from(e - b'0');
                let mut remaining = if value == 0 { 3 } else { 2 };
                while remaining > 0 && i < s.len() && (b'0'..=b'7').contains(&s[i]) {
                    value = value * 8 + u32::from(s[i] - b'0');
                    i += 1;
                    remaining -= 1;
                }
                out.push((value & 0xff) as u8);
            }
            b'x' => {
                let start = i;
                let mut value = 0u32;
                while i < s.len() && i - start < 2 && s[i].is_ascii_hexdigit() {
                    value = value * 16 + hex_value(s[i]);
                    i += 1;
                }
                if i == start {
                    // "missing hex digit": keep the backslash, reprocess 'x'.
                    out.push(b'\\');
                    i -= 1;
                } else {
                    out.push((value & 0xff) as u8);
                }
            }
            b'u' | b'U' => {
                let width = if e == b'u' { 4 } else { 8 };
                let start = i;
                let mut value = 0u32;
                while i < s.len() && i - start < width && s[i].is_ascii_hexdigit() {
                    value = value.wrapping_mul(16).wrapping_add(hex_value(s[i]));
                    i += 1;
                }
                if i == start {
                    out.push(b'\\');
                    i -= 1;
                } else if let Some(ch) = char::from_u32(value) {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
            }
            b'c' => break,
            _ => {
                // Unknown escapes (and \' \" \? under %b) keep the backslash;
                // the character itself is processed normally afterwards.
                out.push(b'\\');
                i -= 1;
            }
        }
    }
    out.retain(|b| *b != 0);
    while out.last() == Some(&b'\n') {
        out.pop();
    }
    out
}

fn hex_value(b: u8) -> u32 {
    match b {
        b'0'..=b'9' => u32::from(b - b'0'),
        b'a'..=b'f' => u32::from(b - b'a' + 10),
        b'A'..=b'F' => u32::from(b - b'A' + 10),
        _ => 0,
    }
}

/// `io::Error` text without Rust's ` (os error N)` suffix, as coreutils
/// print it (`No such file or directory`).
pub fn io_error_text(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.rfind(" (os error ") {
        Some(i) if s.ends_with(')') => s[..i].to_string(),
        _ => s,
    }
}

/// `$(cat FILE)`: bytes with NULs dropped and trailing newlines stripped.
pub fn command_substitution(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = bytes.iter().copied().filter(|b| *b != 0).collect();
    while out.last() == Some(&b'\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_repr_matches_python() {
        let cases = [
            (0.868, "0.868"),
            (1.0, "1.0"),
            (0.6, "0.6"),
            (100.0, "100.0"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (-2.5, "-2.5"),
            (1_761_000_000.123_456, "1761000000.123456"),
            (0.1 + 0.2, "0.30000000000000004"),
            (123_456_789_012_345_680.0, "1.2345678901234568e+17"),
            (-0.0, "-0.0"),
        ];
        for (x, want) in cases {
            assert_eq!(py_float_repr(x), want, "repr({x})");
        }
    }

    #[test]
    fn round_matches_python_half_even() {
        assert_eq!(py_round(0.25, 1), 0.2);
        assert_eq!(py_round(0.125, 2), 0.12);
        assert_eq!(py_round(2.675, 2), 2.67);
        assert_eq!(py_round(99.95, 1), 100.0);
        assert_eq!(py_round_int(2.5), 2);
        assert_eq!(py_round_int(3.5), 4);
        assert_eq!(py_round_int(-0.5), 0);
    }

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(
            splitlines("a\nb\r\nc\rd\x0be\x0cf\x1cg\u{85}h\u{2028}i"),
            ["a", "b", "c", "d", "e", "f", "g", "h", "i"]
        );
        assert_eq!(splitlines("abc\n"), ["abc"]);
        assert_eq!(splitlines("\n"), [""]);
        assert!(splitlines("").is_empty());
        assert_eq!(splitlines("a\n\nb"), ["a", "", "b"]);
        assert_eq!(splitlines("a\r\r\nb"), ["a", "", "b"]);
    }

    #[test]
    fn strip_includes_python_separators() {
        assert_eq!(py_strip("\x1f  x \u{a0}"), "x");
        assert_eq!(py_rstrip(" x \t"), " x");
    }

    #[test]
    fn json_layouts_match_python() {
        let v = serde_json::json!({"a": [1, 2.0, "é°"], "b": {}, "c": [], "d": null});
        assert_eq!(
            py_json(&v, JsonLayout::Compact, true).expect("json"),
            "{\"a\": [1, 2.0, \"\\u00e9\\u00b0\"], \"b\": {}, \"c\": [], \"d\": null}"
        );
        assert_eq!(
            py_json(&v, JsonLayout::Indent(1), true).expect("json"),
            "{\n \"a\": [\n  1,\n  2.0,\n  \"\\u00e9\\u00b0\"\n ],\n \"b\": {},\n \"c\": [],\n \"d\": null\n}"
        );
        assert_eq!(
            py_json(
                &serde_json::json!(["\u{7f}\u{1}\t\u{1f600}"]),
                JsonLayout::Compact,
                true
            )
            .expect("json"),
            "[\"\\u007f\\u0001\\t\\ud83d\\ude00\"]"
        );
        assert_eq!(
            py_json(
                &serde_json::json!(["\u{7f}é"]),
                JsonLayout::Indent(2),
                false
            )
            .expect("json"),
            "[\n  \"\u{7f}é\"\n]"
        );
    }

    #[test]
    fn printf_b_matches_bash() {
        assert_eq!(bash_printf_b("y\\r"), b"y\r");
        assert_eq!(bash_printf_b("\\033[Z"), b"\x1b[Z");
        assert_eq!(bash_printf_b("\\0101\\x41\\u00e9\\cZZ"), b"AA\xc3\xa9");
        assert_eq!(bash_printf_b("a\\101b"), b"aAb");
        assert_eq!(bash_printf_b("y\\n\\n"), b"y");
        assert_eq!(bash_printf_b("\\q\\\"x\\"), b"\\q\\\"x\\");
        assert_eq!(bash_printf_b("\\xZZ"), b"\\xZZ");
        assert_eq!(bash_printf_b("a\\0b"), b"ab");
        assert_eq!(bash_printf_b("\\e\\E\\\\"), b"\x1b\x1b\\");
    }

    #[test]
    fn python_reprs() {
        assert_eq!(py_str_repr("a'b\"\n"), r#"'a\'b"\n'"#);
        assert_eq!(py_str_repr("it's"), r#""it's""#);
        assert_eq!(
            py_list_repr(&["ssh".to_string(), "-o".to_string()]),
            "['ssh', '-o']"
        );
    }

    #[test]
    fn char_slicing() {
        assert_eq!(take_chars("héllo", 2), "hé");
        assert_eq!(take_chars("hi", 5), "hi");
        assert_eq!(last_chars("héllo", 3), "llo");
        assert_eq!(last_chars("hi", 5), "hi");
    }
}
