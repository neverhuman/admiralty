//! Process helpers shared by every task: `command -v` lookup, bash `%q`
//! rendering, exit-status mapping, `rtk proxy` wrapping and child environment
//! hygiene.

use std::{
    ffi::{OsStr, OsString},
    io,
    os::unix::{ffi::OsStrExt, fs::PermissionsExt, process::ExitStatusExt},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus},
    thread,
    time::Duration,
};

/// Environment variables that `cargo run` sets for the xtask process itself.
/// They are removed from children so stages see the same environment the
/// former shell scripts gave them.
const CARGO_RUN_ONLY_VARS: [&str; 6] = [
    "CARGO_MANIFEST_DIR",
    "CARGO_MANIFEST_PATH",
    "CARGO_CRATE_NAME",
    "CARGO_BIN_NAME",
    "CARGO_PRIMARY_PACKAGE",
    "CARGO_RUSTC_CURRENT_DIR",
];

/// Equivalent of `command -v NAME` restricted to executables: searches each
/// entry of `path` (an empty entry means the current directory) for an
/// executable regular file.
pub fn find_in_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if name.contains('/') {
        let candidate = PathBuf::from(name);
        return is_executable(&candidate).then_some(candidate);
    }
    let path = path?;
    std::env::split_paths(path).find_map(|dir| {
        let dir = if dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            dir
        };
        let candidate = dir.join(name);
        is_executable(&candidate).then_some(candidate)
    })
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Locate `rtk` on the caller's `PATH`, as `command -v rtk` did.
pub fn find_rtk() -> Option<PathBuf> {
    find_in_path("rtk", std::env::var_os("PATH").as_deref())
}

/// Build the argv actually executed: `rtk proxy ARGV...` when `rtk` is
/// available, otherwise `ARGV` unchanged.
pub fn wrap_rtk(rtk: Option<&Path>, argv: &[OsString]) -> Vec<OsString> {
    match rtk {
        Some(rtk) => {
            let mut wrapped = Vec::with_capacity(argv.len() + 2);
            wrapped.push(rtk.as_os_str().to_owned());
            wrapped.push(OsString::from("proxy"));
            wrapped.extend(argv.iter().cloned());
            wrapped
        }
        None => argv.to_vec(),
    }
}

/// Create a `Command` for `argv` (first element is the program) with the
/// cargo-run-only variables removed and the overlay applied.
pub fn command(argv: &[OsString], overlay: &[(OsString, OsString)]) -> io::Result<Command> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command line"))?;
    let mut command = Command::new(program);
    command.args(args);
    for (key, _) in std::env::vars_os() {
        if key.as_bytes().starts_with(b"CARGO_PKG_")
            || CARGO_RUN_ONLY_VARS.iter().any(|name| key == *name)
        {
            command.env_remove(&key);
        }
    }
    for (key, value) in overlay {
        command.env(key, value);
    }
    Ok(command)
}

/// Spawn, retrying briefly on `ETXTBSY` (an executable that another thread
/// of this process still had open for writing while forking).
pub fn spawn(command: &mut Command) -> io::Result<Child> {
    const ETXTBSY: i32 = 26;
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Err(error) if error.raw_os_error() == Some(ETXTBSY) && attempts < 50 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(20));
            }
            other => return other,
        }
    }
}

/// Run to completion and return the exit status.
pub fn status(command: &mut Command) -> io::Result<ExitStatus> {
    spawn(command)?.wait()
}

/// Shell-compatible exit code: the exit status, or `128 + signal`.
pub fn exit_code(status: ExitStatus) -> i32 {
    match (status.code(), status.signal()) {
        (Some(code), _) => code,
        (None, Some(signal)) => 128 + signal,
        (None, None) => 1,
    }
}

/// Render one argument exactly like bash's `printf '%q'`.
pub fn bash_quote(arg: &OsStr) -> String {
    let bytes = arg.as_bytes();
    if bytes.is_empty() {
        return "''".to_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) if !text.chars().any(char::is_control) => backslash_quote(text),
        _ => ansi_c_quote(bytes),
    }
}

fn backslash_quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() * 2);
    let mut previous: Option<char> = None;
    for (index, c) in text.chars().enumerate() {
        let escape = matches!(
            c,
            ' ' | '!'
                | '"'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | ','
                | ';'
                | '<'
                | '>'
                | '?'
                | '['
                | '\\'
                | ']'
                | '^'
                | '`'
                | '{'
                | '|'
                | '}'
        ) || (c == '#' && index == 0)
            || (c == '~' && (index == 0 || matches!(previous, Some(':' | '='))));
        if escape {
            quoted.push('\\');
        }
        quoted.push(c);
        previous = Some(c);
    }
    quoted
}

fn ansi_c_quote(bytes: &[u8]) -> String {
    let mut quoted = String::from("$'");
    let text = String::from_utf8_lossy(bytes);
    let valid = std::str::from_utf8(bytes).is_ok();
    let push_octal = |quoted: &mut String, byte: u8| {
        quoted.push('\\');
        quoted.push(char::from(b'0' + ((byte >> 6) & 7)));
        quoted.push(char::from(b'0' + ((byte >> 3) & 7)));
        quoted.push(char::from(b'0' + (byte & 7)));
    };
    if valid {
        for c in text.chars() {
            match c {
                '\u{1b}' => quoted.push_str("\\E"),
                '\u{7}' => quoted.push_str("\\a"),
                '\u{b}' => quoted.push_str("\\v"),
                '\u{8}' => quoted.push_str("\\b"),
                '\u{c}' => quoted.push_str("\\f"),
                '\n' => quoted.push_str("\\n"),
                '\r' => quoted.push_str("\\r"),
                '\t' => quoted.push_str("\\t"),
                '\\' => quoted.push_str("\\\\"),
                '\'' => quoted.push_str("\\'"),
                c if c.is_ascii_control() => push_octal(&mut quoted, c as u8),
                c => quoted.push(c),
            }
        }
    } else {
        for &byte in bytes {
            match byte {
                b'\\' => quoted.push_str("\\\\"),
                b'\'' => quoted.push_str("\\'"),
                0x20..=0x7e => quoted.push(char::from(byte)),
                _ => push_octal(&mut quoted, byte),
            }
        }
    }
    quoted.push('\'');
    quoted
}

/// `printf '%q ' "$@"` (note the trailing space after every argument).
pub fn quoted_command_line(argv: &[OsString]) -> String {
    argv.iter().fold(String::new(), |mut line, arg| {
        line.push_str(&bash_quote(arg));
        line.push(' ');
        line
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_quote_matches_printf_q_for_stage_arguments() {
        let cases = [
            ("pnpm", "pnpm"),
            ("--frozen-lockfile", "--frozen-lockfile"),
            ("-D", "-D"),
            ("--", "--"),
            ("NODE_ENV=production", "NODE_ENV=production"),
            ("e2e/tsconfig.json", "e2e/tsconfig.json"),
            ("", "''"),
            ("a b", "a\\ b"),
            ("it's", "it\\'s"),
            ("$HOME", "\\$HOME"),
            ("a,b", "a\\,b"),
            ("#x#", "\\#x#"),
            ("~/a=~b:~c", "\\~/a=\\~b:\\~c"),
            ("a\nb", "$'a\\nb'"),
            ("tab\there", "$'tab\\there'"),
            ("esc\u{1b}", "$'esc\\E'"),
            ("bell\u{1}x", "$'bell\\001x'"),
            ("quote'\n", "$'quote\\'\\n'"),
        ];
        for (input, expected) in cases {
            assert_eq!(bash_quote(OsStr::new(input)), expected, "input {input:?}");
        }
    }

    #[test]
    fn bash_quote_agrees_with_host_bash_when_available() {
        let Some(bash) = ["/bin/bash", "/usr/bin/bash"]
            .into_iter()
            .map(Path::new)
            .find(|p| p.exists())
        else {
            return;
        };
        let inputs = [
            "plain",
            "with space",
            "semi;colon",
            "glob*?[x]",
            "brace{a,b}",
            "pipe|amp&",
            "dollar$`tick`",
            "#hash",
            "mid#hash",
            "~tilde",
            "a=~b",
            "x:~y",
            "back\\slash",
            "dq\"sq'",
            "caret^bang!",
            "paren(x)<y>",
            "nl\nx",
            "",
            "/tmp/adm.AbC123/canary",
        ];
        for input in inputs {
            let output = Command::new(bash)
                .args(["-c", "printf '%q' \"$1\"", "bash", input])
                .env("LC_ALL", "C")
                .output()
                .expect("run bash printf %q");
            assert!(output.status.success());
            assert_eq!(
                bash_quote(OsStr::new(input)),
                String::from_utf8_lossy(&output.stdout),
                "input {input:?}"
            );
        }
    }

    #[test]
    fn quoted_command_line_keeps_trailing_space() {
        let argv: Vec<OsString> = ["cargo", "fmt", "--all", "--", "--check"]
            .into_iter()
            .map(OsString::from)
            .collect();
        assert_eq!(quoted_command_line(&argv), "cargo fmt --all -- --check ");
    }

    #[test]
    fn find_in_path_requires_executable_regular_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = dir.path().join("tool");
        std::fs::write(&tool, "#!/bin/sh\n").expect("write tool");
        let path = OsString::from(dir.path());
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert_eq!(find_in_path("tool", Some(&path)), None);
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(find_in_path("tool", Some(&path)), Some(tool));
        std::fs::create_dir(dir.path().join("subdir")).expect("mkdir");
        assert_eq!(find_in_path("subdir", Some(&path)), None);
        assert_eq!(find_in_path("tool", None), None);
    }

    #[test]
    fn wrap_rtk_prefixes_proxy_only_when_available() {
        let argv = vec![OsString::from("pnpm"), OsString::from("build")];
        assert_eq!(wrap_rtk(None, &argv), argv);
        assert_eq!(
            wrap_rtk(Some(Path::new("/opt/rtk")), &argv),
            vec![
                OsString::from("/opt/rtk"),
                OsString::from("proxy"),
                OsString::from("pnpm"),
                OsString::from("build")
            ]
        );
    }

    #[test]
    fn exit_code_maps_signals_like_the_shell() {
        assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
        assert_eq!(exit_code(ExitStatus::from_raw(9)), 137);
    }
}
