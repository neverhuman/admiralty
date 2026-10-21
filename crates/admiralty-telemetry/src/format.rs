//! Line encoding shared by the compact and JSON sinks and by the digest.
//!
//! Every rendered value is cleaned (no control or ANSI escape bytes), bounded
//! and redacted by key before it can reach a terminal, a file or an agent.
use std::time::{SystemTime, UNIX_EPOCH};

/// Longest rendered value, in bytes, before an ellipsis replaces the tail.
pub const MAX_VALUE_BYTES: usize = 256;
/// Longest rendered line, in bytes. Fields that do not fit are dropped and
/// counted in a trailing `dropped=N` field instead of being cut mid-value.
pub const MAX_LINE_BYTES: usize = 1024;

const REDACTED: &str = "[redacted]";
const SECRET_KEY_PARTS: [&str; 10] = [
    "token",
    "secret",
    "password",
    "passwd",
    "authorization",
    "cookie",
    "credential",
    "api_key",
    "apikey",
    "private_key",
];

/// True when a field name suggests credential material. Matching is
/// case-insensitive and substring based, so `bearer_token` and `X-Api-Key`
/// are both redacted.
pub fn secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace('-', "_");
    SECRET_KEY_PARTS.iter().any(|part| key.contains(part))
}

/// Value-level redaction for credentials that arrive under an innocent key.
fn secret_value(value: &str) -> bool {
    let lower = value.trim_start().to_ascii_lowercase();
    lower.starts_with("bearer ") || lower.starts_with("basic ")
}

/// Remove control characters and ANSI/OSC escape sequences. Newlines become
/// spaces so one event is always one line.
pub fn clean(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI: ESC [ ... final byte in @..~ ; OSC: ESC ] ... BEL or ESC \
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            }
        } else if c == '\n' || c == '\r' || c == '\t' {
            out.push(' ');
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// Truncate on a character boundary, marking the cut with an ellipsis.
pub fn bounded(mut value: String, max: usize) -> String {
    if value.len() <= max {
        return value;
    }
    let mut cut = max.saturating_sub('…'.len_utf8());
    while cut > 0 && !value.is_char_boundary(cut) {
        cut -= 1;
    }
    value.truncate(cut);
    value.push('…');
    value
}

/// Render a string for a `key=value` line: bare when it is a simple token,
/// otherwise JSON-quoted so a parser can always recover the exact text.
pub fn compact_string(value: &str) -> String {
    let simple = !value.is_empty()
        && value
            .chars()
            .all(|c| !c.is_whitespace() && c != '"' && c != '=' && c != '\\' && !c.is_control());
    if simple {
        value.to_owned()
    } else {
        serde_json::Value::String(value.to_owned()).to_string()
    }
}

/// At most two decimals, trailing zeros removed (dashboard display precision).
pub fn decimal(value: f64) -> String {
    if !value.is_finite() {
        return "null".into();
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{value:.0}");
    }
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// Clean, redact and bound one string value for its key.
pub fn safe_string(key: &str, value: &str) -> String {
    if secret_key(key) || secret_value(value) {
        return REDACTED.to_owned();
    }
    bounded(clean(value), MAX_VALUE_BYTES)
}

/// Field keys are short ASCII identifiers; anything else is normalized so a
/// hostile key cannot forge another field or break the line grammar.
pub fn safe_key(key: &str) -> String {
    let mut out = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect::<String>();
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// RFC 3339 UTC timestamp with millisecond precision, without a date crate.
pub fn utc_millis(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = duration.as_secs();
    let millis = duration.subsec_millis();
    let (year, month, day) = civil_from_days((seconds / 86_400) as i64);
    let rem = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Seconds-precision variant used in digests, where millis are noise.
pub fn utc_seconds(time: SystemTime) -> String {
    let mut text = utc_millis(time);
    // "YYYY-MM-DDTHH:MM:SS.mmmZ" -> "YYYY-MM-DDTHH:MM:SSZ"
    text.replace_range(19..23, "");
    text
}

/// Time-of-day only ("HH:MM:SSZ"), for compact digest lines within one window.
pub fn utc_clock(time: SystemTime) -> String {
    utc_seconds(time)[11..].to_owned()
}

/// Parse the RFC 3339 UTC forms this crate writes (and the common
/// `...Z`/`+00:00` variants other Admiralty components emit).
pub fn parse_utc(text: &str) -> Option<SystemTime> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || !matches!(bytes[10], b'T' | b' ')
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<u64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    let mut rest = &text[19..];
    let mut nanos = 0u32;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        let mut value = fraction[..digits.min(9)].parse::<u32>().ok()?;
        for _ in digits.min(9)..9 {
            value *= 10;
        }
        nanos = value;
        rest = &fraction[digits..];
    }
    if rest != "Z" && rest != "+00:00" {
        return None;
    }
    let days = days_from_civil(year as i64, month as u32, day as u32);
    let seconds = u64::try_from(days).ok()? * 86_400 + hour * 3600 + minute * 60 + second.min(60);
    Some(UNIX_EPOCH + std::time::Duration::new(seconds, nanos))
}

// Howard Hinnant's proleptic Gregorian conversions.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_round_trip_across_leap_years_and_epochs() {
        for seconds in [
            0u64,
            951_782_400,
            1_709_164_800,
            1_792_000_000,
            4_102_444_800,
        ] {
            let time = UNIX_EPOCH + Duration::from_millis(seconds * 1000 + 512);
            let text = utc_millis(time);
            assert_eq!(parse_utc(&text), Some(time), "{text}");
        }
        assert_eq!(utc_millis(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            utc_seconds(UNIX_EPOCH + Duration::from_secs(951_825_600)),
            "2000-02-29T12:00:00Z"
        );
        assert_eq!(parse_utc("2026-10-21T07:44:03+00:00").map(|_| ()), Some(()));
        assert_eq!(parse_utc("2026-10-21T07:44:03-06:00"), None);
        assert_eq!(parse_utc("not a time"), None);
    }

    #[test]
    fn values_are_cleaned_bounded_quoted_and_redacted() {
        assert_eq!(clean("a\u{1b}[31mred\u{1b}[0m\nb\u{7}"), "ared b");
        assert_eq!(clean("x\u{1b}]0;title\u{7}y"), "xy");
        assert_eq!(compact_string("plain/path:1"), "plain/path:1");
        assert_eq!(compact_string("two words"), "\"two words\"");
        assert_eq!(compact_string("k=v"), "\"k=v\"");
        assert_eq!(compact_string(""), "\"\"");
        assert_eq!(safe_string("bearer_token", "abc"), REDACTED);
        assert_eq!(safe_string("X-Api-Key", "abc"), REDACTED);
        assert_eq!(safe_string("header", "Bearer abc"), REDACTED);
        let long = safe_string("detail", &"é".repeat(400));
        assert!(long.len() <= MAX_VALUE_BYTES && long.ends_with('…'));
        assert_eq!(safe_key("route path=x"), "route_path_x");
        assert_eq!(safe_key(""), "_");
    }
}
