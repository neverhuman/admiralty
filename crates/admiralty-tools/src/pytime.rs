//! Python `datetime` behaviors the retired tools relied on.

use chrono::{
    DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc,
};

use crate::error::ToolError;

/// A parsed `datetime.fromisoformat` result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IsoDateTime {
    /// Wall-clock value.
    pub naive: NaiveDateTime,
    /// UTC offset in microseconds when timezone-aware.
    pub offset_micros: Option<i64>,
}

impl IsoDateTime {
    /// `.astimezone(timezone.utc)` for aware values; `None` when naive or the
    /// conversion overflows the datetime range (Python `OverflowError`).
    pub fn to_utc(&self) -> Option<DateTime<Utc>> {
        let offset = self.offset_micros?;
        let utc = self
            .naive
            .checked_sub_signed(Duration::microseconds(offset))?;
        (1..=9999).contains(&utc.year()).then(|| utc.and_utc())
    }

    /// `.timestamp()`: aware values exactly, naive values as local time.
    pub fn timestamp(&self) -> Option<f64> {
        let utc = match self.offset_micros {
            Some(offset) => self
                .naive
                .checked_sub_signed(Duration::microseconds(offset))?
                .and_utc(),
            None => Local
                .from_local_datetime(&self.naive)
                .earliest()?
                .with_timezone(&Utc),
        };
        let micros = utc.timestamp_micros();
        Some(micros as f64 / 1_000_000.0)
    }
}

fn digits(bytes: &[u8], start: usize, count: usize) -> Option<u32> {
    let slice = bytes.get(start..start + count)?;
    let mut value = 0u32;
    for byte in slice {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value * 10 + u32::from(byte - b'0');
    }
    Some(value)
}

fn separator_location(bytes: &[u8]) -> usize {
    let length = bytes.len();
    if length == 7 {
        return 7;
    }
    if bytes.get(4) == Some(&b'-') {
        if bytes.get(5) == Some(&b'W') {
            if length > 8 && bytes.get(8) == Some(&b'-') {
                if length > 10 && bytes.get(10).is_some_and(u8::is_ascii_digit) {
                    return 8;
                }
                return 10;
            }
            return 8;
        }
        10
    } else if bytes.get(4) == Some(&b'W') {
        let mut index = 7;
        while index < length && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index < 9 {
            index
        } else if index % 2 == 0 {
            7
        } else {
            8
        }
    } else {
        8
    }
}

fn parse_date(bytes: &[u8]) -> Option<NaiveDate> {
    let year = digits(bytes, 0, 4)?;
    let mut position = 4;
    let uses_separator = bytes.get(position) == Some(&b'-');
    if uses_separator {
        position += 1;
    }
    if bytes.get(position) == Some(&b'W') {
        // ISO week dates: supported by Python but never produced by the
        // tools' sources; treated as unparseable (`None`).
        return None;
    }
    let month = digits(bytes, position, 2)?;
    position += 2;
    if uses_separator {
        if bytes.get(position) != Some(&b'-') {
            return None;
        }
        position += 1;
    }
    let day = digits(bytes, position, 2)?;
    position += 2;
    if position != bytes.len() {
        return None;
    }
    NaiveDate::from_ymd_opt(i32::try_from(year).ok()?, month, day)
}

/// `parse_hh_mm_ss_ff`: returns (h, m, s, us, trailing-garbage flag).
#[allow(clippy::needless_range_loop)]
fn parse_clock(bytes: &[u8]) -> Option<(u32, u32, u32, u32, bool)> {
    let mut values = [0u32; 3];
    let mut position = 0usize;
    let mut has_separator = true;
    let end = bytes.len();
    for index in 0..3 {
        values[index] = digits(bytes, position, 2)?;
        position += 2;
        let character = bytes.get(position).copied().unwrap_or(0);
        position += 1;
        if index == 0 {
            has_separator = character == b':';
        }
        if position >= end {
            let (hour, minute, second) = (values[0], values[1], values[2]);
            return Some((hour, minute, second, 0, position == end && character != 0));
        } else if has_separator && character == b':' {
            continue;
        } else if character == b'.' || character == b',' {
            break;
        } else if !has_separator {
            position -= 1;
        } else {
            return None;
        }
    }
    // CPython parses a fraction after the loop whether it ended on '.'/','
    // or after three components.
    let remaining = end - position;
    let to_parse = remaining.min(6);
    let mut micro = digits(bytes, position, to_parse)?;
    position += to_parse;
    const CORRECTION: [u32; 5] = [100_000, 10_000, 1000, 100, 10];
    if to_parse < 6 {
        micro *= CORRECTION[to_parse.checked_sub(1)?];
    }
    while position < end && bytes[position].is_ascii_digit() {
        position += 1;
    }
    Some((values[0], values[1], values[2], micro, position != end))
}

/// Python 3.11+ `datetime.fromisoformat(text)` (week dates unsupported).
pub fn fromisoformat(text: &str) -> Option<IsoDateTime> {
    let bytes = text.as_bytes();
    if bytes.len() < 7 {
        return None;
    }
    let split = separator_location(bytes);
    let date = parse_date(bytes.get(..split)?)?;
    if bytes.len() <= split {
        return Some(IsoDateTime {
            naive: date.and_hms_opt(0, 0, 0)?,
            offset_micros: None,
        });
    }
    // Skip exactly one separator character (any Unicode scalar).
    let rest = text.get(split..)?;
    let separator_width = rest.chars().next()?.len_utf8();
    let time = rest.get(separator_width..)?.as_bytes();
    let tz_position = time
        .iter()
        .position(|byte| matches!(byte, b'Z' | b'+' | b'-'))
        .unwrap_or(time.len());
    let (hour, minute, second, micro, garbage) = parse_clock(&time[..tz_position])?;
    let mut offset_micros = None;
    if tz_position == time.len() {
        if garbage {
            return None;
        }
    } else if time[tz_position] == b'Z' {
        if tz_position + 1 != time.len() {
            return None;
        }
        offset_micros = Some(0);
    } else {
        let sign: i64 = if time[tz_position] == b'-' { -1 } else { 1 };
        let (tz_hour, tz_minute, tz_second, tz_micro, tz_garbage) =
            parse_clock(&time[tz_position + 1..])?;
        if tz_garbage {
            return None;
        }
        let total = (i64::from(tz_hour) * 3600 + i64::from(tz_minute) * 60 + i64::from(tz_second))
            * 1_000_000
            + i64::from(tz_micro);
        if total >= 86_400_000_000 {
            return None;
        }
        offset_micros = Some(sign * total);
    }
    let naive = date.and_hms_micro_opt(hour, minute, second, micro)?;
    Some(IsoDateTime {
        naive,
        offset_micros,
    })
}

/// `datetime.fromtimestamp(ts, timezone.utc)` (microsecond half-even rounding).
pub fn fromtimestamp_utc(timestamp: f64) -> Result<NaiveDateTime, ToolError> {
    if timestamp.is_nan() {
        return Err(ToolError::type_error("Invalid value NaN (not a number)"));
    }
    if !timestamp.is_finite() {
        return Err(ToolError::type_error(
            "cannot convert float infinity to integer",
        ));
    }
    let mut whole = timestamp.trunc();
    let mut micros = ((timestamp - whole) * 1e6).round_ties_even();
    if micros >= 1e6 {
        micros -= 1e6;
        whole += 1.0;
    } else if micros < 0.0 {
        micros += 1e6;
        whole -= 1.0;
    }
    let out_of_range = || ToolError::type_error("year is out of range");
    if whole.abs() > 1e15 {
        return Err(out_of_range());
    }
    let seconds = whole as i64;
    let micros = micros as u32;
    let value = DateTime::from_timestamp(seconds, micros * 1000).ok_or_else(out_of_range)?;
    if !(1..=9999).contains(&value.year()) {
        return Err(out_of_range());
    }
    Ok(value.naive_utc())
}

/// `strftime('%Y-%m-%dT%H:%M:%SZ')` with glibc year formatting.
pub fn strftime_z(value: &NaiveDateTime) -> String {
    format!(
        "{}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        value.year(),
        value.month(),
        value.day(),
        value.hour(),
        value.minute(),
        value.second()
    )
}

/// `aware_utc.isoformat(timespec="seconds")` → `YYYY-MM-DDTHH:MM:SS+00:00`.
pub fn isoformat_seconds(value: &DateTime<Utc>) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+00:00",
        value.year(),
        value.month(),
        value.day(),
        value.hour(),
        value.minute(),
        value.second()
    )
}

/// `datetime.fromtimestamp(ts, timezone.utc).isoformat(timespec="seconds")`.
pub fn iso_from_timestamp(timestamp: f64) -> Result<String, ToolError> {
    Ok(isoformat_seconds(&fromtimestamp_utc(timestamp)?.and_utc()))
}

/// `datetime.min.replace(tzinfo=timezone.utc).isoformat(timespec="seconds")`.
pub const MIN_ISO: &str = "0001-01-01T00:00:00+00:00";

/// Current UTC time `isoformat(timespec="seconds")`.
pub fn now_iso() -> String {
    isoformat_seconds(&Utc::now())
}

/// Stat time as Python's float `st_mtime`.
pub fn stat_time(seconds: i64, nanoseconds: i64) -> f64 {
    seconds as f64 + nanoseconds as f64 * 1e-9
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(text: &str) -> Option<String> {
        fromisoformat(text)?
            .to_utc()
            .map(|value| isoformat_seconds(&value))
    }

    #[test]
    fn fromisoformat_matches_python_forms() {
        assert_eq!(
            utc("2026-10-18T12:00:00+00:00").as_deref(),
            Some("2026-10-18T12:00:00+00:00")
        );
        assert_eq!(
            utc("2026-10-18T14:30:15.123456+02:30").as_deref(),
            Some("2026-10-18T12:00:15+00:00")
        );
        assert_eq!(
            utc("20261018T1230+0000").as_deref(),
            Some("2026-10-18T12:30:00+00:00")
        );
        assert_eq!(
            utc("2026-10-18 12:00+00").as_deref(),
            Some("2026-10-18T12:00:00+00:00")
        );
        assert!(fromisoformat("2026-10-18").is_some_and(|v| v.offset_micros.is_none()));
        assert!(fromisoformat("2026-10-18T12:00:00").is_some_and(|v| v.offset_micros.is_none()));
        let half = fromisoformat("2026-10-18T12.5").expect("python accepts fractional hour");
        assert_eq!(half.naive.nanosecond(), 500_000_000);
        for bad in [
            "",
            "2026",
            "2026-13-01",
            "2026-10-18T25:00",
            "2026-10-18T12:00:",
            "nonsense",
            "2026-10-18T12:00:00+24:00",
            "1234",
        ] {
            assert!(fromisoformat(bad).is_none(), "{bad}");
        }
        assert!(fromisoformat("0001-01-01T00:00:00+01:00").is_some_and(|v| v.to_utc().is_none()));
    }

    #[test]
    fn fromtimestamp_rounds_like_python() {
        let value = fromtimestamp_utc(1.9999996).expect("in range");
        assert_eq!(strftime_z(&value), "1970-01-01T00:00:02Z");
        let value = fromtimestamp_utc(-0.5).expect("in range");
        assert_eq!(strftime_z(&value), "1969-12-31T23:59:59Z");
        assert!(fromtimestamp_utc(f64::NAN).is_err());
        assert!(fromtimestamp_utc(1e20).is_err());
        assert_eq!(
            iso_from_timestamp(1_760_788_800.7).expect("in range"),
            "2025-10-18T12:00:00+00:00"
        );
    }
}
