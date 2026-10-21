//! Python `csv` module (excel dialect) reader/writer semantics.

use crate::pyjson::{Map, Value};

/// Python's default `csv.field_size_limit()`.
pub const FIELD_LIMIT: usize = 131_072;

#[derive(Clone, Copy, PartialEq)]
enum State {
    StartRecord,
    StartField,
    InField,
    InQuoted,
    QuoteInQuoted,
    EatCrnl,
}

/// Split text into lines like a `newline=''` text file (ends kept).
fn lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                result.push(&text[start..=index]);
                start = index + 1;
            }
            b'\r' => {
                let end = if bytes.get(index + 1) == Some(&b'\n') {
                    index + 1
                } else {
                    index
                };
                result.push(&text[start..=end]);
                index = end;
                start = end + 1;
            }
            _ => {}
        }
        index += 1;
    }
    if start < bytes.len() {
        result.push(&text[start..]);
    }
    result
}

/// `list(csv.reader(text))`; `None` on a csv error (field limit or a
/// character after an unquoted line end).
pub fn read_rows(text: &str) -> Option<Vec<Vec<String>>> {
    let mut rows = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut field_len = 0usize;
    let mut state = State::StartRecord;
    let mut in_record = false;
    for line in lines(text) {
        let characters = line.chars().map(Some).chain(std::iter::once(None));
        for character in characters {
            // `None` is the end-of-line marker.
            loop {
                match state {
                    State::StartRecord => match character {
                        None => {
                            // Empty line: an empty row.
                        }
                        Some('\n' | '\r') => state = State::EatCrnl,
                        Some(_) => {
                            state = State::StartField;
                            continue;
                        }
                    },
                    State::StartField => match character {
                        None | Some('\n' | '\r') => {
                            fields.push(std::mem::take(&mut field));
                            field_len = 0;
                            state = if character.is_none() {
                                State::StartRecord
                            } else {
                                State::EatCrnl
                            };
                        }
                        Some('"') => state = State::InQuoted,
                        Some(',') => {
                            fields.push(std::mem::take(&mut field));
                            field_len = 0;
                        }
                        Some(other) => {
                            field.push(other);
                            field_len += 1;
                            state = State::InField;
                        }
                    },
                    State::InField => match character {
                        None | Some('\n' | '\r') => {
                            fields.push(std::mem::take(&mut field));
                            field_len = 0;
                            state = if character.is_none() {
                                State::StartRecord
                            } else {
                                State::EatCrnl
                            };
                        }
                        Some(',') => {
                            fields.push(std::mem::take(&mut field));
                            field_len = 0;
                            state = State::StartField;
                        }
                        Some(other) => {
                            field.push(other);
                            field_len += 1;
                        }
                    },
                    State::InQuoted => match character {
                        None => {}
                        Some('"') => state = State::QuoteInQuoted,
                        Some(other) => {
                            field.push(other);
                            field_len += 1;
                        }
                    },
                    State::QuoteInQuoted => match character {
                        Some('"') => {
                            field.push('"');
                            field_len += 1;
                            state = State::InQuoted;
                        }
                        Some(',') => {
                            fields.push(std::mem::take(&mut field));
                            field_len = 0;
                            state = State::StartField;
                        }
                        None | Some('\n' | '\r') => {
                            fields.push(std::mem::take(&mut field));
                            field_len = 0;
                            state = if character.is_none() {
                                State::StartRecord
                            } else {
                                State::EatCrnl
                            };
                        }
                        Some(other) => {
                            field.push(other);
                            field_len += 1;
                            state = State::InField;
                        }
                    },
                    State::EatCrnl => match character {
                        Some('\n' | '\r') => {}
                        None => state = State::StartRecord,
                        Some(_) => return None,
                    },
                }
                break;
            }
            if field_len > FIELD_LIMIT {
                return None;
            }
            in_record = state != State::StartRecord;
        }
        if !in_record {
            rows.push(std::mem::take(&mut fields));
        }
    }
    if in_record && (field_len != 0 || state == State::InQuoted) {
        fields.push(field);
        rows.push(fields);
    }
    Some(rows)
}

/// `list(csv.DictReader(text))`: values are `Str`, missing ones `Null`.
pub fn read_dicts(text: &str) -> Option<Vec<Value>> {
    let rows = read_rows(text)?;
    let mut iterator = rows.into_iter();
    let Some(fieldnames) = iterator.next() else {
        return Some(Vec::new());
    };
    let mut result = Vec::new();
    for row in iterator {
        if row.is_empty() {
            continue;
        }
        let mut map = Map::new();
        for (index, name) in fieldnames.iter().enumerate() {
            let value = row.get(index).cloned().map_or(Value::Null, Value::Str);
            map.insert(name.clone(), value);
        }
        // Extra cells go to the `None` restkey, which no caller reads.
        result.push(Value::Object(map));
    }
    Some(result)
}

/// One `csv.writer(lineterminator='\n').writerow(fields)` line.
pub fn write_row(fields: &[String], out: &mut String) {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let quote =
            field.contains([',', '"', '\n', '\r']) || (fields.len() == 1 && field.is_empty());
        if quote {
            out.push('"');
            out.push_str(&field.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(field);
        }
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_matches_python_minimal_quoting() {
        let mut out = String::new();
        let row: Vec<String> = ["a b", " lead", "trail ", "x\ry", "q\"q", "c,d", "", "\t"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        write_row(&row, &mut out);
        write_row(&[String::new()], &mut out);
        write_row(&[], &mut out);
        assert_eq!(
            out,
            "a b, lead,trail ,\"x\ry\",\"q\"\"q\",\"c,d\",,\t\n\"\"\n\n"
        );
    }

    #[test]
    fn reader_round_trips_python_output() {
        let text = "h1,h2,h3\na,\"b,\"\"c\"\"\nd\",e\r\n\nshort\nx,y,z,extra\n";
        let rows = read_rows(text).expect("valid csv");
        assert_eq!(
            rows,
            vec![
                vec!["h1".to_owned(), "h2".to_owned(), "h3".to_owned()],
                vec!["a".to_owned(), "b,\"c\"\nd".to_owned(), "e".to_owned()],
                vec![],
                vec!["short".to_owned()],
                vec![
                    "x".to_owned(),
                    "y".to_owned(),
                    "z".to_owned(),
                    "extra".to_owned()
                ],
            ]
        );
        let dicts = read_dicts(text).expect("valid csv");
        assert_eq!(dicts.len(), 3);
        assert_eq!(
            dicts[1].as_object().and_then(|row| row.get("h2")),
            Some(&Value::Null)
        );
        assert_eq!(read_dicts(""), Some(Vec::new()));
        let unterminated = read_rows("a,\"b").expect("non-strict");
        assert_eq!(unterminated, vec![vec!["a".to_owned(), "b".to_owned()]]);
    }
}
