//! Ports of the Python `ipaddress.ip_address` and `urllib.parse.urlsplit`
//! rules the canary listener validation relied on (CPython 3.13 semantics).

use crate::py::repr_str;

/// An address accepted by Python `ipaddress.ip_address(str)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyIp {
    /// `IPv4Address`.
    V4(u32),
    /// `IPv6Address` with an optional scope id.
    V6 { ip: u128, scope: Option<String> },
}

impl PyIp {
    /// `address.is_loopback` (IPv4-mapped IPv6 follows its IPv4 address).
    pub fn is_loopback(&self) -> bool {
        match self {
            Self::V4(ip) => ip >> 24 == 127,
            Self::V6 { ip, .. } => match ipv4_mapped(*ip) {
                Some(v4) => v4 >> 24 == 127,
                None => *ip == 1,
            },
        }
    }

    /// `address.compressed` (`str(address)`).
    pub fn compressed(&self) -> String {
        match self {
            Self::V4(ip) => v4_string(*ip),
            Self::V6 { ip, scope } => {
                let text = match ipv4_mapped(*ip) {
                    Some(v4) => format!("{}:{}", v6_string(ip >> 32), v4_string(v4)),
                    None => v6_string(*ip),
                };
                match scope {
                    Some(scope) if !scope.is_empty() => format!("{text}%{scope}"),
                    _ => text,
                }
            }
        }
    }

    /// Whether this is an IPv6 address (the `Host` header brackets it).
    pub fn is_v6(&self) -> bool {
        matches!(self, Self::V6 { .. })
    }
}

fn ipv4_mapped(ip: u128) -> Option<u32> {
    if ip >> 32 == 0xffff {
        Some((ip & 0xffff_ffff) as u32)
    } else {
        None
    }
}

fn v4_string(ip: u32) -> String {
    let [a, b, c, d] = ip.to_be_bytes();
    format!("{a}.{b}.{c}.{d}")
}

fn v6_string(ip: u128) -> String {
    let mut hextets: Vec<String> = (0..8)
        .map(|index| format!("{:x}", (ip >> (112 - 16 * index)) & 0xffff))
        .collect();
    let mut best_start: isize = -1;
    let mut best_len = 0usize;
    let mut start: isize = -1;
    let mut len = 0usize;
    for (index, hextet) in hextets.iter().enumerate() {
        if hextet == "0" {
            len += 1;
            if start == -1 {
                start = index as isize;
            }
            if len > best_len {
                best_len = len;
                best_start = start;
            }
        } else {
            len = 0;
            start = -1;
        }
    }
    if best_len > 1 {
        let begin = best_start as usize;
        let end = begin + best_len;
        if end == hextets.len() {
            hextets.push(String::new());
        }
        hextets.splice(begin..end, [String::new()]);
        if begin == 0 {
            hextets.insert(0, String::new());
        }
    }
    hextets.join(":")
}

fn parse_v4(text: &str) -> Result<u32, String> {
    if text.is_empty() {
        return Err("Address cannot be empty".into());
    }
    let octets: Vec<&str> = text.split('.').collect();
    if octets.len() != 4 {
        return Err(format!("Expected 4 octets in {}", repr_str(text)));
    }
    let mut value: u32 = 0;
    for octet in octets {
        let parsed =
            parse_octet(octet).map_err(|error| format!("{error} in {}", repr_str(text)))?;
        value = (value << 8) | u32::from(parsed);
    }
    Ok(value)
}

fn parse_octet(octet: &str) -> Result<u8, String> {
    if octet.is_empty() {
        return Err("Empty octet not permitted".into());
    }
    if !octet.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "Only decimal digits permitted in {}",
            repr_str(octet)
        ));
    }
    if octet.len() > 3 {
        return Err(format!(
            "At most 3 characters permitted in {}",
            repr_str(octet)
        ));
    }
    if octet != "0" && octet.starts_with('0') {
        return Err(format!(
            "Leading zeros are not permitted in {}",
            repr_str(octet)
        ));
    }
    let value: u32 = octet.parse().map_err(|_| "invalid octet".to_owned())?;
    u8::try_from(value).map_err(|_| format!("Octet {value} (> 255) not permitted"))
}

fn parse_hextet(hextet: &str) -> Result<u128, String> {
    if !hextet.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("Only hex digits permitted in {}", repr_str(hextet)));
    }
    if hextet.len() > 4 {
        return Err(format!(
            "At most 4 characters permitted in {}",
            repr_str(hextet)
        ));
    }
    u128::from_str_radix(hextet, 16).map_err(|_| {
        format!(
            "invalid literal for int() with base 16: {}",
            repr_str(hextet)
        )
    })
}

fn parse_v6(text: &str) -> Result<u128, String> {
    const HEXTETS: usize = 8;
    if text.is_empty() {
        return Err("Address cannot be empty".into());
    }
    if text.chars().count() > 45 {
        return Err(format!(
            "At most 45 characters expected in {}",
            repr_str(text)
        ));
    }
    let max_parts = HEXTETS + 1;
    let mut parts: Vec<String> = text.splitn(max_parts + 1, ':').map(str::to_owned).collect();
    if parts.len() < 3 {
        return Err(format!("At least 3 parts expected in {}", repr_str(text)));
    }
    if parts.last().is_some_and(|last| last.contains('.')) {
        let last = parts.pop().unwrap_or_default();
        let v4 = parse_v4(&last).map_err(|error| format!("{error} in {}", repr_str(text)))?;
        parts.push(format!("{:x}", (v4 >> 16) & 0xffff));
        parts.push(format!("{:x}", v4 & 0xffff));
    }
    if parts.len() > max_parts {
        return Err(format!(
            "At most {} colons permitted in {}",
            max_parts - 1,
            repr_str(text)
        ));
    }
    let mut skip_index = None;
    for (index, part) in parts.iter().enumerate().take(parts.len() - 1).skip(1) {
        if part.is_empty() {
            if skip_index.is_some() {
                return Err(format!("At most one '::' permitted in {}", repr_str(text)));
            }
            skip_index = Some(index);
        }
    }
    let (parts_hi, parts_lo, parts_skipped);
    let leading = "Leading ':' only permitted as part of '::' in";
    let trailing = "Trailing ':' only permitted as part of '::' in";
    if let Some(skip) = skip_index {
        let mut hi = skip;
        let mut lo = parts.len() - skip - 1;
        if parts[0].is_empty() {
            hi -= 1;
            if hi != 0 {
                return Err(format!("{leading} {}", repr_str(text)));
            }
        }
        if parts[parts.len() - 1].is_empty() {
            lo -= 1;
            if lo != 0 {
                return Err(format!("{trailing} {}", repr_str(text)));
            }
        }
        let skipped = HEXTETS as isize - (hi + lo) as isize;
        if skipped < 1 {
            return Err(format!(
                "Expected at most {} other parts with '::' in {}",
                HEXTETS - 1,
                repr_str(text)
            ));
        }
        parts_hi = hi;
        parts_lo = lo;
        parts_skipped = skipped as usize;
    } else {
        if parts.len() != HEXTETS {
            return Err(format!(
                "Exactly {HEXTETS} parts expected without '::' in {}",
                repr_str(text)
            ));
        }
        if parts[0].is_empty() {
            return Err(format!("{leading} {}", repr_str(text)));
        }
        if parts[parts.len() - 1].is_empty() {
            return Err(format!("{trailing} {}", repr_str(text)));
        }
        parts_hi = parts.len();
        parts_lo = 0;
        parts_skipped = 0;
    }
    let mut value: u128 = 0;
    let wrap = |error: String| format!("{error} in {}", repr_str(text));
    for part in &parts[..parts_hi] {
        value = (value << 16) | parse_hextet(part).map_err(wrap)?;
    }
    // `::` alone skips all eight hextets; a 128-bit shift of zero is zero.
    value = value.checked_shl(16 * parts_skipped as u32).unwrap_or(0);
    for part in &parts[parts.len() - parts_lo..] {
        value = (value << 16) | parse_hextet(part).map_err(wrap)?;
    }
    Ok(value)
}

/// `ipaddress.ip_address(text)`; the error is the Python `ValueError` text.
pub fn ip_address(text: &str) -> Result<PyIp, String> {
    let failure = || {
        format!(
            "{} does not appear to be an IPv4 or IPv6 address",
            repr_str(text)
        )
    };
    if !text.contains('/') {
        if let Ok(v4) = parse_v4(text) {
            return Ok(PyIp::V4(v4));
        }
        let (address, scope) = match text.split_once('%') {
            None => (text, None),
            Some((_, scope)) if scope.is_empty() || scope.contains('%') => return Err(failure()),
            Some((address, scope)) => (address, Some(scope.to_owned())),
        };
        if let Ok(v6) = parse_v6(address) {
            return Ok(PyIp::V6 { ip: v6, scope });
        }
    }
    Err(failure())
}

/// The fields of `urllib.parse.urlsplit` the canary inspects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitResult {
    /// `scheme`.
    pub scheme: String,
    /// `netloc`.
    pub netloc: String,
    /// `path`.
    pub path: String,
    /// `query`.
    pub query: String,
    /// `fragment`.
    pub fragment: String,
}

const C0_CONTROL_OR_SPACE: &[char] = &[
    '\u{0}', '\u{1}', '\u{2}', '\u{3}', '\u{4}', '\u{5}', '\u{6}', '\u{7}', '\u{8}', '\t', '\n',
    '\u{b}', '\u{c}', '\r', '\u{e}', '\u{f}', '\u{10}', '\u{11}', '\u{12}', '\u{13}', '\u{14}',
    '\u{15}', '\u{16}', '\u{17}', '\u{18}', '\u{19}', '\u{1a}', '\u{1b}', '\u{1c}', '\u{1d}',
    '\u{1e}', '\u{1f}', ' ',
];

fn is_scheme_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
}

/// `urllib.parse.urlsplit(url)` for ASCII input; errors are `ValueError` text.
pub fn urlsplit(url: &str) -> Result<SplitResult, String> {
    let mut url: String = url
        .trim_start_matches(C0_CONTROL_OR_SPACE)
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut scheme = String::new();
    let mut netloc = String::new();
    let mut query = String::new();
    let mut fragment = String::new();
    if let Some(colon) = url.find(':') {
        let first = url.chars().next().unwrap_or(' ');
        if colon > 0 && first.is_ascii_alphabetic() && url[..colon].chars().all(is_scheme_char) {
            scheme = url[..colon].to_ascii_lowercase();
            url = url[colon + 1..].to_owned();
        }
    }
    if url.starts_with("//") {
        let delimiter = ['/', '?', '#']
            .iter()
            .filter_map(|c| url[2..].find(*c).map(|position| position + 2))
            .min()
            .unwrap_or(url.len());
        netloc = url[2..delimiter].to_owned();
        url = url[delimiter..].to_owned();
        let open = netloc.contains('[');
        let close = netloc.contains(']');
        if open != close {
            return Err("Invalid IPv6 URL".into());
        }
        if open && close {
            check_bracketed_netloc(&netloc)?;
        }
    }
    if let Some((before, after)) = url.split_once('#') {
        fragment = after.to_owned();
        url = before.to_owned();
    }
    if let Some((before, after)) = url.split_once('?') {
        query = after.to_owned();
        url = before.to_owned();
    }
    Ok(SplitResult {
        scheme,
        netloc,
        path: url,
        query,
        fragment,
    })
}

fn check_bracketed_netloc(netloc: &str) -> Result<(), String> {
    let host_and_port = netloc.rsplit_once('@').map_or(netloc, |(_, rest)| rest);
    let hostname = match host_and_port.split_once('[') {
        Some((before, bracketed)) => {
            if !before.is_empty() {
                return Err("Invalid IPv6 URL".into());
            }
            let (hostname, port) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            if !port.is_empty() && !port.starts_with(':') {
                return Err("Invalid IPv6 URL".into());
            }
            hostname
        }
        None => host_and_port
            .split_once(':')
            .map_or(host_and_port, |(host, _)| host),
    };
    if hostname.starts_with(['v', 'V']) {
        let body = &hostname[1..];
        let valid = body.split_once('.').is_some_and(|(hex, rest)| {
            !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()) && !rest.is_empty()
        });
        if !valid {
            return Err("IPvFuture address is invalid".into());
        }
        return Ok(());
    }
    match ip_address(hostname)? {
        PyIp::V4(_) => Err("An IPv4 address cannot be in brackets".into()),
        PyIp::V6 { .. } => Ok(()),
    }
}

impl SplitResult {
    fn userinfo(&self) -> (Option<String>, Option<String>) {
        match self.netloc.rsplit_once('@') {
            Some((userinfo, _)) => match userinfo.split_once(':') {
                Some((user, password)) => (Some(user.to_owned()), Some(password.to_owned())),
                None => (Some(userinfo.to_owned()), None),
            },
            None => (None, None),
        }
    }

    fn hostinfo(&self) -> (String, Option<String>) {
        let hostinfo = self
            .netloc
            .rsplit_once('@')
            .map_or(self.netloc.as_str(), |(_, rest)| rest);
        let (hostname, port) = match hostinfo.split_once('[') {
            Some((_, bracketed)) => {
                let (hostname, rest) = bracketed.split_once(']').unwrap_or((bracketed, ""));
                let port = rest.split_once(':').map_or("", |(_, port)| port);
                (hostname, port)
            }
            None => hostinfo.split_once(':').unwrap_or((hostinfo, "")),
        };
        let port = (!port.is_empty()).then(|| port.to_owned());
        (hostname.to_owned(), port)
    }

    /// `username`.
    pub fn username(&self) -> Option<String> {
        self.userinfo().0
    }

    /// `password`.
    pub fn password(&self) -> Option<String> {
        self.userinfo().1
    }

    /// `hostname` (lowercased outside an IPv6 zone).
    pub fn hostname(&self) -> Option<String> {
        let (hostname, _) = self.hostinfo();
        if hostname.is_empty() {
            return None;
        }
        Some(match hostname.split_once('%') {
            Some((host, zone)) => format!("{}%{zone}", host.to_lowercase()),
            None => hostname.to_lowercase(),
        })
    }

    /// `port`; errors are the Python `ValueError` text.
    pub fn port(&self) -> Result<Option<u32>, String> {
        let (_, port) = self.hostinfo();
        let Some(port) = port else {
            return Ok(None);
        };
        if !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!(
                "Port could not be cast to integer value as {}",
                repr_str(&port)
            ));
        }
        let trimmed = port.trim_start_matches('0');
        if trimmed.len() > 5 {
            return Err("Port out of range 0-65535".into());
        }
        let value: u32 = if trimmed.is_empty() {
            0
        } else {
            trimmed
                .parse()
                .map_err(|_| "Port out of range 0-65535".to_owned())?
        };
        if value > 65535 {
            return Err("Port out of range 0-65535".into());
        }
        Ok(Some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_address_matches_python_forms() {
        assert_eq!(
            ip_address("127.0.0.1").map(|ip| ip.compressed()),
            Ok("127.0.0.1".into())
        );
        assert!(ip_address("127.0.0.01").is_err());
        assert!(ip_address("256.0.0.1").is_err());
        assert_eq!(
            ip_address("::1").map(|ip| ip.compressed()),
            Ok("::1".into())
        );
        assert_eq!(
            ip_address("0:0:0:0:0:0:0:1").map(|ip| ip.compressed()),
            Ok("::1".into())
        );
        assert_eq!(
            ip_address("::ffff:7f00:1").map(|ip| ip.compressed()),
            Ok("::ffff:127.0.0.1".into())
        );
        assert!(ip_address("::ffff:127.0.0.1").is_ok_and(|ip| ip.is_loopback()));
        assert_eq!(
            ip_address("fe80::1%eth0").map(|ip| ip.compressed()),
            Ok("fe80::1%eth0".into())
        );
        assert_eq!(
            ip_address("1:0:0:2:0:0:0:3").map(|ip| ip.compressed()),
            Ok("1:0:0:2::3".into())
        );
        assert_eq!(
            ip_address("1:0:2:3:4:5:6:7").map(|ip| ip.compressed()),
            Ok("1:0:2:3:4:5:6:7".into())
        );
        assert!(ip_address("example.com").is_err());
        assert!(ip_address("::1%").is_err());
    }

    #[test]
    fn urlsplit_matches_python_netloc_rules() {
        let parts = urlsplit("http://[::1]:12345").expect("valid URL");
        assert_eq!(parts.hostname().as_deref(), Some("::1"));
        assert_eq!(parts.port(), Ok(Some(12345)));
        assert!(urlsplit("http://[127.0.0.1]:1").is_err());
        assert!(urlsplit("http://[::1:1").is_err());
        let removed = urlsplit("http://127.0.0.1:1\t2").expect("tab is removed");
        assert_eq!(removed.port(), Ok(Some(12)));
        let user = urlsplit("http://u@127.0.0.1:1").expect("valid URL");
        assert_eq!(user.username().as_deref(), Some("u"));
        assert!(urlsplit("http://h:x").expect("valid").port().is_err());
        assert!(urlsplit("http://h:70000").expect("valid").port().is_err());
        assert_eq!(urlsplit("http://h:").expect("valid").port(), Ok(None));
    }
}
