//! Filesystem helpers with Python `pathlib`/`os` semantics.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

/// Current real user ID (`os.getuid()`).
pub fn getuid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// The home directory as Python's `os.path.expanduser('~')` reports it.
pub fn home_dir() -> String {
    let raw = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".to_owned());
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// `Path(raw).expanduser()` for `~` and `~/...`.
pub fn expanduser(raw: &Path) -> PathBuf {
    let bytes = raw.as_os_str().as_bytes();
    if bytes == b"~" {
        return PathBuf::from(home_dir());
    }
    if let Some(rest) = bytes.strip_prefix(b"~/") {
        let mut path = PathBuf::from(home_dir());
        path.push(std::ffi::OsStr::from_bytes(rest));
        return path;
    }
    raw.to_path_buf()
}

/// `PurePath(raw).parts` without the root, plus whether it is absolute.
/// Empty and `.` components are dropped as `pathlib` does.
pub fn parts(raw: &Path) -> (bool, Vec<OsString>) {
    let mut absolute = false;
    let mut items = Vec::new();
    for component in raw.components() {
        match component {
            Component::RootDir => absolute = true,
            Component::CurDir => {}
            Component::ParentDir => items.push(OsString::from("..")),
            Component::Normal(part) => items.push(part.to_os_string()),
            Component::Prefix(prefix) => items.push(prefix.as_os_str().to_os_string()),
        }
    }
    (absolute, items)
}

/// Normalized `Path(raw)` (pathlib drops `.` and repeated separators).
pub fn normalized(raw: &Path) -> PathBuf {
    let (absolute, items) = parts(raw);
    let mut path = if absolute {
        PathBuf::from("/")
    } else {
        PathBuf::new()
    };
    for item in items {
        path.push(item);
    }
    path
}

/// `Path(raw).absolute()` (no symlink resolution).
pub fn absolute(raw: &Path) -> io::Result<PathBuf> {
    let path = normalized(raw);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(normalized(&std::env::current_dir()?.join(path)))
    }
}

/// `Path.is_symlink()` (false on any error).
pub fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Ancestors of an absolute path, starting with the path itself.
pub fn self_and_parents(path: &Path) -> Vec<PathBuf> {
    path.ancestors().map(Path::to_path_buf).collect()
}

/// Non-strict `Path.resolve()`: canonicalize the longest existing prefix.
pub fn resolve(path: &Path) -> PathBuf {
    if let Ok(real) = fs::canonicalize(path) {
        return real;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if parent != path => resolve(parent).join(name),
        _ => path.to_path_buf(),
    }
}

/// Owner UID through `stat` (follows symlinks).
pub fn owner(path: &Path) -> io::Result<u32> {
    use std::os::unix::fs::MetadataExt;
    Ok(fs::metadata(path)?.uid())
}

/// Same file identity (`os.path.samefile`).
pub fn samefile(left: &Path, right: &Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let a = fs::metadata(left)?;
    let b = fs::metadata(right)?;
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}

/// fsync a directory (`os.open(dir, O_RDONLY)` + `os.fsync`).
pub fn fsync_dir(path: &Path) -> io::Result<()> {
    fs::File::open(path)?.sync_all()
}

/// Replacement function type (normally `std::fs::rename`, i.e. `os.replace`).
pub type ReplaceFn<'a> = &'a dyn Fn(&Path, &Path) -> io::Result<()>;

/// The production replacement (`os.replace`).
pub fn os_replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

/// `tempfile.mkstemp(prefix=..., dir=...)` + write + fsync + `os.replace` +
/// directory fsync; the temporary file never survives a failure.
pub fn atomic_write(
    target: &Path,
    prefix: &str,
    data: &[u8],
    replace: ReplaceFn<'_>,
) -> io::Result<()> {
    let directory = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::Builder::new()
        .prefix(prefix)
        .rand_bytes(8)
        .tempfile_in(directory)?;
    temporary.write_all(data)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    let temporary = temporary.into_temp_path();
    replace(&temporary, target)?;
    // The rename consumed the temporary name; disarm deletion.
    let _ = temporary.keep();
    fsync_dir(directory)
}

/// Python `fnmatch.translate` + `fullmatch` (case-sensitive).
pub fn fnmatchcase(name: &str, pattern: &str) -> bool {
    let characters: Vec<char> = pattern.chars().collect();
    let mut expression = String::from("(?s)^");
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        index += 1;
        match character {
            '*' => {
                while index < characters.len() && characters[index] == '*' {
                    index += 1;
                }
                expression.push_str(".*");
            }
            '?' => expression.push('.'),
            '[' => {
                let mut end = index;
                if end < characters.len() && characters[end] == '!' {
                    end += 1;
                }
                if end < characters.len() && characters[end] == ']' {
                    end += 1;
                }
                while end < characters.len() && characters[end] != ']' {
                    end += 1;
                }
                if end >= characters.len() {
                    expression.push_str("\\[");
                } else {
                    let mut class: String = characters[index..end].iter().collect();
                    index = end + 1;
                    let negate = class.starts_with('!');
                    if negate {
                        class.remove(0);
                    }
                    let escaped: String = class
                        .chars()
                        .map(|c| match c {
                            '\\' | '[' | ']' | '^' | '&' | '~' => format!("\\{c}"),
                            other => other.to_string(),
                        })
                        .collect();
                    expression.push('[');
                    if negate {
                        expression.push('^');
                    }
                    expression.push_str(&escaped);
                    expression.push(']');
                }
            }
            other => expression.push_str(&regex::escape(&other.to_string())),
        }
    }
    expression.push_str("\\z");
    regex::Regex::new(&expression).is_ok_and(|compiled| compiled.is_match(name))
}

/// `sorted(glob.glob(os.path.join(directory, pattern)))` for a basename
/// pattern: hidden names are skipped as Python's glob does.
pub fn glob_sorted(directory: &Path, pattern: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.') && fnmatchcase(name, pattern))
        .collect();
    names.sort();
    names.into_iter().map(|name| directory.join(name)).collect()
}
