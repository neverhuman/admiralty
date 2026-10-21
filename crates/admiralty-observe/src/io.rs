//! Bounded read-only command/file observations and atomic owned output.
use crate::Result;
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;

pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn read_json(path: &Path, max: u64) -> Result<Value> {
    let bytes = read_file(path, max)?;
    Ok(serde_json::from_slice(&bytes)?)
}
/// The same bounded, no-follow, regular-file read as `read_json`, returning the
/// exact observed bytes so callers can record a content digest.
pub fn read_bytes(path: &Path, max: u64) -> Result<Vec<u8>> {
    read_file(path, max)
}
fn read_file(path: &Path, max: u64) -> Result<Vec<u8>> {
    let mut file = open_regular(path)?;
    let before = file.metadata()?;
    if before.len() > max {
        return Err("source exceeds configured byte limit".into());
    }
    let mut bytes = Vec::new();
    (&mut file).take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err("source exceeds configured byte limit".into());
    }
    let after = file.metadata()?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err("source changed during observation".into());
    }
    Ok(bytes)
}
fn open_regular(path: &Path) -> Result<File> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{Mode, OFlags, open};
        File::from(open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?)
    };
    #[cfg(not(unix))]
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err("source is not a regular file".into());
    }
    Ok(file)
}
pub fn read_text_tail(path: &Path, max: usize) -> Result<String> {
    let mut file = open_regular(path)?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(max as u64)))?;
    let mut bytes = Vec::new();
    file.take(max as u64).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
pub fn mtime_utc(path: &Path) -> Option<String> {
    let t = fs::metadata(path).ok()?.modified().ok()?;
    Some(
        chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}
pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err("output exceeds byte limit".into());
    }
    let parent = path.parent().ok_or("output parent missing")?;
    let temporary = parent.join(format!(".observe-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut f = options.open(&temporary)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Imported observations can contain arbitrary nested source fields. They may
/// never bypass the credential scrubber merely by using valid JSON.
pub fn public_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(clean(&s)),
        Value::Array(values) => Value::Array(values.into_iter().map(public_value).collect()),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    let normalized = key.to_ascii_lowercase().replace(['-', ' ', '_'], "");
                    let secret = matches!(
                        normalized.as_str(),
                        "token"
                            | "accesstoken"
                            | "refreshtoken"
                            | "password"
                            | "passwd"
                            | "secret"
                            | "apikey"
                            | "clientsecret"
                            | "privatekey"
                            | "authorization"
                            | "credentialvalue"
                    );
                    (
                        clean(&key),
                        if secret {
                            Value::String("[redacted]".into())
                        } else {
                            public_value(value)
                        },
                    )
                })
                .collect(),
        ),
        other => other,
    }
}

/// Commands are fixed by compiled observation adapters. Configured values are
/// individual argv entries; never executable shell text. Only owned children
/// may be signalled, before reaping their group leader.
pub async fn run(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    timeout_secs: u64,
    max_bytes: usize,
) -> Result<CommandOutput> {
    if timeout_secs == 0 || timeout_secs > 60 || max_bytes == 0 || max_bytes > 64 * 1024 * 1024 {
        return Err("invalid command observation bounds".into());
    }
    let basename = program
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if matches!(
        basename,
        "python" | "python3" | "python2" | "pypy" | "pypy3"
    ) {
        return Err("Python is not part of the native observer".into());
    }
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    if basename == "git" {
        cmd.env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_ALLOW_PROTOCOL", "")
            .env("GIT_TERMINAL_PROMPT", "0");
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_")
                && !matches!(
                    key.to_str(),
                    Some(
                        "GIT_OPTIONAL_LOCKS"
                            | "GIT_CONFIG_NOSYSTEM"
                            | "GIT_CONFIG_GLOBAL"
                            | "GIT_NO_REPLACE_OBJECTS"
                            | "GIT_NO_LAZY_FETCH"
                            | "GIT_ALLOW_PROTOCOL"
                            | "GIT_TERMINAL_PROMPT"
                    )
                )
            {
                cmd.env_remove(key);
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.as_std_mut().process_group(0);
    }
    let mut child = cmd.spawn()?;
    let pid = child.id();
    let mut stdout = child.stdout.take().ok_or("stdout unavailable")?;
    let mut stderr = child.stderr.take().ok_or("stderr unavailable")?;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut out_open = true;
    let mut err_open = true;
    let mut status = None;
    let deadline = tokio::time::sleep(Duration::from_secs(timeout_secs));
    tokio::pin!(deadline);
    let mut a = [0u8; 8192];
    let mut b = [0u8; 8192];
    let failure = loop {
        if status.is_some() && !out_open && !err_open {
            break None;
        }
        tokio::select! {
            result=child.wait(),if status.is_none()=>{match result{Ok(s)=>status=Some(s.code().unwrap_or(-1)),Err(_)=>break Some("command wait failed")}},
            result=stdout.read(&mut a),if out_open=>{match result{Ok(0)=>out_open=false,Ok(n)=>{if out.len()+n>max_bytes{break Some("command stdout exceeded byte limit")}out.extend_from_slice(&a[..n]);},Err(_)=>break Some("command stdout failed")}},
            result=stderr.read(&mut b),if err_open=>{match result{Ok(0)=>err_open=false,Ok(n)=>{if err.len()+n>max_bytes{break Some("command stderr exceeded byte limit")}err.extend_from_slice(&b[..n]);},Err(_)=>break Some("command stderr failed")}},
            _=&mut deadline=>break Some("command observation timed out"),
        }
    };
    if let Some(reason) = failure {
        if status.is_none() {
            #[cfg(unix)]
            if let Some(pid) = pid.and_then(|p| rustix::process::Pid::from_raw(p as i32)) {
                let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
            }
            let _ = child.start_kill();
            if tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .is_err()
            {
                return Err(
                    "command observation failed; owned child cleanup remains unobserved".into(),
                );
            }
        }
        return Err(reason.into());
    }
    Ok(CommandOutput {
        status: status.unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: clean(&String::from_utf8_lossy(&err)),
    })
}

/// Sanitize observations before they can enter public-facing tails. A line with
/// credential syntax is withheld rather than relying on a token length guess.
pub fn clean(input: &str) -> String {
    let mut plain = String::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for x in chars.by_ref() {
                        if ('@'..='~').contains(&x) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(x) = chars.next() {
                        if x == '\u{7}' || (x == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            if x == '\u{1b}' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if c == '\n' || c == '\t' || !c.is_control() {
            plain.push(c);
        }
    }
    plain
        .lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            let compact = lower
                .chars()
                .filter(|c| !c.is_whitespace() && !matches!(c, '\'' | '"' | '`'))
                .collect::<String>();
            let dangerous = [
                "authorization:",
                "password=",
                "password:",
                "passwd=",
                "secret=",
                "token=",
                "token:",
                "api_key=",
                "api_key:",
                "api-key:",
                "apikey=",
                "apikey:",
                "api_key:",
                "-----begin",
            ];
            let credential_prefix = [
                "ghp_",
                "gho_",
                "ghu_",
                "ghs_",
                "github_pat_",
                "sk-",
                "xoxb-",
                "xoxp-",
                "akia",
                "aiza",
            ]
            .iter()
            .any(|prefix| {
                lower.match_indices(prefix).any(|(i, _)| {
                    i == 0
                        || !lower.as_bytes()[i - 1].is_ascii_alphanumeric()
                            && lower.as_bytes()[i - 1] != b'_'
                            && lower.as_bytes()[i - 1] != b'-'
                })
            });
            if credential_prefix
                || lower.contains("bearer ")
                || dangerous.iter().any(|s| compact.contains(s))
            {
                "[credential-bearing line withheld]".to_owned()
            } else if line.contains(".private") || line.contains("CONTROL") {
                "[private control reference withheld]".to_owned()
            } else {
                line.chars().take(2000).collect::<String>()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_lines_and_escape_sequences_are_removed() {
        assert_eq!(
            clean(
                "\u{1b}[32mGemini 4 Argon\u{1b}[0m\nAuthorization: Bearer secret\n.private/CONTROL"
            ),
            "Gemini 4 Argon\n[credential-bearing line withheld]\n[private control reference withheld]"
        );
    }
    #[test]
    fn task_identifiers_are_not_mistaken_for_key_prefixes() {
        assert_eq!(clean("task-123 checks passed"), "task-123 checks passed");
        assert_eq!(
            clean(concat!("key: ", "sk-", "abcdef123456789012345")),
            "[credential-bearing line withheld]"
        );
    }
    #[test]
    fn quoted_and_spaced_secret_assignments_are_withheld() {
        for line in [
            r#"{ "token": "plainsecret" }"#,
            "api_key = plainsecret",
            "API_KEY: plainsecret",
            "password : plainsecret",
            "Authorization : Bearer plainsecret",
        ] {
            assert_eq!(clean(line), "[credential-bearing line withheld]");
        }
    }
    #[test]
    fn nested_imported_json_cannot_publish_secret_fields() {
        let value = public_value(
            serde_json::json!({"claim":{"token":"plainsecret","clientSecret":"plainsecret","accessToken":"plainsecret","refresh_token":"plainsecret","private-key":"plainsecret"},"packets":[{"id":"task-1","note":"API_KEY = plainsecret"}],"token=plainsecret":"value"}),
        );
        assert_eq!(value["claim"]["token"], "[redacted]");
        assert_eq!(value["packets"][0]["id"], "task-1");
        assert!(!value.to_string().contains("plainsecret"));
    }
    #[test]
    fn bounded_sources_and_atomic_outputs() {
        let dir = tempfile::tempdir().expect("fixture");
        let p = dir.path().join("s.json");
        atomic_json(&p, &serde_json::json!({"a":1})).expect("write");
        assert_eq!(read_json(&p, 100).expect("read")["a"], 1);
        assert!(read_json(&p, 2).is_err());
    }
    #[tokio::test]
    async fn command_overflow_and_timeout_fail_closed() {
        assert!(
            run(
                Path::new("sh"),
                &["-c".into(), "printf 123456789".into()],
                None,
                1,
                4
            )
            .await
            .is_err()
        );
        assert!(
            run(
                Path::new("sh"),
                &["-c".into(), "sleep 5".into()],
                None,
                1,
                1024
            )
            .await
            .is_err()
        );
    }
}
