//! Unit tests for the history exporter helpers. Expected values were
//! produced by CPython 3.13 with the retired `tools/export_history.py`.

use super::*;

/// The secret patterns (pattern 8 re-expressed from Python's `["\\']` class)
/// find the same first match offsets as Python's `re.search`.
#[test]
fn secret_patterns_match_python_first_offsets() {
    let cases: [(&str, [Option<usize>; 10]); 20] = [
        (
            "password = \"abcdefghijklmnop1234\"",
            [
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(0),
                None,
                None,
            ],
        ),
        (
            "API-KEY: 'ABCDEFGHIJKLMNOPQ'",
            [
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(0),
                None,
                None,
            ],
        ),
        (
            "apikey=abc",
            [None, None, None, None, None, None, None, None, None, None],
        ),
        (
            "Secret'=  'abcdefghijklmnopqrstu",
            [
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(0),
                None,
                None,
            ],
        ),
        (
            "mysecret: abcdefghijklmnopqrst",
            [None, None, None, None, None, None, None, None, None, None],
        ),
        (
            "BEARER:abcdefghijklmnopqr",
            [
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(0),
                None,
                None,
            ],
        ),
        (
            "caf\u{e9}secret=abcdefghijklmnopqr",
            [None, None, None, None, None, None, None, None, None, None],
        ),
        (
            "x sk-abcdefghijklmnopqrstuvwxyz",
            [
                None,
                None,
                Some(2),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
        ),
        (
            "ask-abcdefghijklmnopqrstuvwxyz",
            [None, None, None, None, None, None, None, None, None, None],
        ),
        (
            "eyJabcdefghijklmnopqrstu.abcdefghijklmnopqrstu.sig",
            [
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(0),
                None,
            ],
        ),
        (
            "Authorization:   Basic abc",
            [None, None, None, None, None, None, None, None, None, None],
        ),
        (
            "authorization: abcdefghijklmn",
            [
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some(0),
            ],
        ),
        (
            "xoxb-1234567890",
            [
                None,
                None,
                None,
                None,
                Some(0),
                None,
                None,
                None,
                None,
                None,
            ],
        ),
        (
            "AIzaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            [
                None,
                None,
                None,
                None,
                None,
                None,
                Some(0),
                None,
                None,
                None,
            ],
        ),
        (
            "AIzaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            [None, None, None, None, None, None, None, None, None, None],
        ),
        (
            "AKIAABCDEFGHIJKLMNOP",
            [
                None,
                None,
                None,
                Some(0),
                None,
                None,
                None,
                None,
                None,
                None,
            ],
        ),
        (
            "gho_aaaaaaaaaaaaaaaaaaaa",
            [
                Some(0),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
        ),
        (
            "github_pat_____________________",
            [
                None,
                Some(0),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
        ),
        (
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            [
                None,
                None,
                None,
                None,
                None,
                Some(0),
                None,
                None,
                None,
                None,
            ],
        ),
        (
            "password: \u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
            [None, None, None, None, None, None, None, None, None, None],
        ),
    ];
    for (sample, expected) in cases {
        let found: Vec<Option<usize>> = SECRET_RES
            .iter()
            .map(|(_, expression)| {
                expression
                    .find(sample)
                    .map(|m| sample[..m.start()].chars().count())
            })
            .collect();
        assert_eq!(found, expected, "{sample:?}");
    }
    assert_eq!(SECRET_RES[7].0, "(?i)\\b(api[_-]?key|secret|passwd|passwor");
}

#[test]
fn clean_rewrites_home_and_truncates_by_character() {
    let home = "/home/synthetic";
    let long = format!("{}{home}/tail", "\u{e9}".repeat(450));
    let value = crate::pyobj! {
        "path" => format!("{home}/repo and {home}"),
        "nested" => Value::Array(vec![Value::Str(format!("{home}x")), Value::Int(1)]),
        "long" => long,
    };
    let cleaned = clean(&value, home);
    assert_eq!(
        cleaned.get_value("path").ok(),
        Some(&Value::from("~/repo and ~"))
    );
    assert_eq!(
        cleaned.get_value("nested").ok(),
        Some(&Value::Array(vec![Value::from("~x"), Value::Int(1)]))
    );
    let long = cleaned
        .get_value("long")
        .ok()
        .and_then(Value::as_str)
        .expect("long string");
    assert_eq!(long.chars().count(), 400);
    assert!(long.chars().all(|c| c == '\u{e9}'));
}

#[test]
fn iso_and_parse_iso_follow_python_datetime() {
    assert_eq!(
        iso(&Value::Int(1_760_760_000)).expect("int"),
        "2025-10-18T04:00:00Z"
    );
    assert_eq!(
        iso(&Value::from("1760760123.9999996")).expect("numeric string"),
        "2025-10-18T04:02:04Z"
    );
    assert!(iso(&Value::from("not a number")).is_err());
    assert!(iso(&Value::Null).is_err());
    assert_eq!(
        parse_iso(&Value::from("2026-10-18T03:00:00-06:00")),
        Some(1_792_314_000.0)
    );
    assert_eq!(parse_iso(&Value::from("garbage")), None);
    assert_eq!(parse_iso(&Value::Int(1234)), None);
    // Naive values are local time, exactly like Python's naive `.timestamp()`.
    let naive = chrono::NaiveDate::from_ymd_opt(2026, 10, 18)
        .and_then(|date| date.and_hms_opt(3, 0, 0))
        .expect("naive");
    let local = chrono::TimeZone::from_local_datetime(&chrono::Local, &naive)
        .earliest()
        .expect("local")
        .timestamp() as f64;
    assert_eq!(parse_iso(&Value::from("2026-10-18T03:00:00")), Some(local));
}

#[test]
fn project_of_matches_python_keys() {
    for (cwd, project) in [
        ("/home/x/joil/a", "joil"),
        ("/srv/DOPE", "dope"),
        ("/x/Jope-Dime", "jopedime"),
        ("/x/jopedimeX", "jopedime"),
        ("/opt/jarambit", "jarambit"),
        ("/joilx", "joil"),
        ("relative", "other"),
        ("", "other"),
    ] {
        assert_eq!(
            project_of(&Value::from(cwd)).expect("project"),
            project,
            "{cwd}"
        );
    }
    assert_eq!(project_of(&Value::Null).expect("none"), "other");
    assert!(project_of(&Value::Int(5)).is_err());
}
