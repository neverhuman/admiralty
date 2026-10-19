//! Enforce the maintainer's bounded legacy-Python exception during onboarding.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    process::Command,
};

fn rows(document: &str) -> Vec<Vec<&str>> {
    document
        .lines()
        .filter_map(|line| {
            let cells: Vec<_> = line.split('|').map(str::trim).collect();
            (cells.len() > 3 && cells[1].starts_with('`') && cells[1].ends_with(".py`"))
                .then(|| cells[1..cells.len() - 1].to_vec())
        })
        .collect()
}

fn validate(
    actual: &BTreeMap<String, usize>,
    allowed: &BTreeSet<String>,
    exceptions: &str,
    plan: &str,
) -> Result<(), String> {
    let mut justifications = BTreeMap::new();
    for row in rows(exceptions) {
        let path = row[0].trim_matches('`').to_owned();
        if row[1].is_empty() || justifications.insert(path.clone(), row[1]).is_some() {
            return Err(format!("missing or duplicate Python exception: {path}"));
        }
    }
    let mut inventory = BTreeMap::new();
    for row in rows(plan) {
        let path = row[0].trim_matches('`').to_owned();
        let count = row[1]
            .replace(',', "")
            .parse::<usize>()
            .map_err(|_| format!("invalid Python line count: {path}"))?;
        if row.len() != 5
            || row[2..].iter().any(|cell| cell.is_empty())
            || inventory.insert(path.clone(), count).is_some()
        {
            return Err(format!(
                "incomplete or duplicate Python migration row: {path}"
            ));
        }
    }
    for path in actual.keys() {
        if !allowed.contains(path) {
            return Err(format!(
                "net-new Python requires explicit maintainer exception: {path}"
            ));
        }
    }
    if actual.keys().collect::<Vec<_>>() != justifications.keys().collect::<Vec<_>>() {
        return Err(
            "Python exceptions must match existing files exactly (no missing or stale rows)".into(),
        );
    }
    if actual != &inventory {
        return Err(
            "Python migration inventory must match existing paths and physical line counts".into(),
        );
    }
    Ok(())
}

#[test]
fn existing_python_has_complete_exceptions_and_no_unapproved_growth() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("git")
        .current_dir(&root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            ":(icase)*.py",
            ":(icase)*.pyi",
            ":(icase)*.pyw",
        ])
        .output()
        .expect("enumerate Python without a shell");
    assert!(output.status.success(), "git file inventory failed");
    let mut actual = BTreeMap::new();
    for bytes in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let path = std::str::from_utf8(bytes).expect("UTF-8 source path");
        let file = root.join(path);
        assert!(
            fs::symlink_metadata(&file)
                .expect("source metadata")
                .file_type()
                .is_file(),
            "Python source must be a regular file: {path}"
        );
        let count = fs::read_to_string(file)
            .expect("UTF-8 Python source")
            .lines()
            .count();
        actual.insert(path.to_owned(), count);
    }
    let allowed: BTreeSet<_> = include_str!("../fixtures/python-baseline.txt")
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect();
    validate(
        &actual,
        &allowed,
        &fs::read_to_string(root.join("PYTHON_EXCEPTIONS.md")).expect("exception policy"),
        &fs::read_to_string(root.join("PY_TO_RUST_PLAN.md")).expect("migration policy"),
    )
    .expect("maintainer Python policy must remain enforced");
}

#[test]
fn guard_rejects_new_python_even_if_it_adds_its_own_exception() {
    let actual = BTreeMap::from([("new.py".into(), 1)]);
    assert!(
        validate(
            &actual,
            &BTreeSet::new(),
            "| `new.py` | self-approved |",
            "| `new.py` | 1 | tool | retire | ADM-PY02 |"
        )
        .expect_err("a new Python file cannot approve itself")
        .contains("net-new Python")
    );
}

#[test]
fn guard_rejects_missing_stale_blank_duplicate_and_wrong_count_rows() {
    let actual = BTreeMap::from([("old.py".into(), 2)]);
    let allowed = BTreeSet::from(["old.py".into()]);
    let exception = "| `old.py` | retained for parity |";
    let plan = "| `old.py` | 2 | tool | retire | ADM-PY02 |";
    assert!(validate(&actual, &allowed, exception, plan).is_ok());
    for bad in [
        "".to_owned(),
        "| `old.py` | |".into(),
        format!("{exception}\n{exception}"),
        format!("{exception}\n| `gone.py` | stale |"),
    ] {
        assert!(validate(&actual, &allowed, &bad, plan).is_err());
    }
    for bad in [
        "".to_owned(),
        plan.replace("| 2 |", "| 3 |"),
        format!("{plan}\n{plan}"),
    ] {
        assert!(validate(&actual, &allowed, exception, &bad).is_err());
    }
    // Retiring source is allowed only when both inventories drop the same row.
    assert!(validate(&BTreeMap::new(), &allowed, "", "").is_ok());
}
