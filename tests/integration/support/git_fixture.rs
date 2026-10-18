//! Offline Unix Git fixtures with no inherited Git configuration or hook input.
use std::{
    fs,
    io::{self, Read},
    os::{
        fd::OwnedFd,
        unix::{
            fs::{DirBuilderExt, PermissionsExt},
            net::UnixStream,
        },
    },
    path::Path,
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const MAX_OUTPUT: usize = 64 * 1024;
const DEADLINE: Duration = Duration::from_secs(10);

pub fn private_fixture() -> TempDir {
    tempfile::Builder::new()
        .prefix("publication-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(fs::canonicalize(std::env::temp_dir()).expect("canonical temporary base"))
        .expect("create canonical owner-only fixture at creation")
}

pub fn private_directory(path: &Path) {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .expect("create owner-only fixture directory");
}

#[derive(Debug, PartialEq, Eq)]
enum FailureKind {
    Deadline,
    OutputLimit,
    Io(String),
}

#[derive(Debug)]
struct Failure {
    kind: FailureKind,
    reaped: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn drain(
    stream: &mut UnixStream,
    bytes: &mut Vec<u8>,
    eof: &mut bool,
    limit: usize,
) -> Result<(), FailureKind> {
    if *eof {
        return Ok(());
    }
    let mut buffer = [0; 4096];
    // Limit syscalls as well as bytes; continuously fragmented output or
    // interrupted reads must return control to the absolute-deadline check.
    for _ in 0..32 {
        let maximum = buffer.len().min(limit - bytes.len() + 1);
        match stream.read(&mut buffer[..maximum]) {
            Ok(0) => {
                *eof = true;
                return Ok(());
            }
            Ok(count) => {
                let retained = count.min(limit - bytes.len());
                bytes.extend_from_slice(&buffer[..retained]);
                if count > retained {
                    return Err(FailureKind::OutputLimit);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FailureKind::Io(error.to_string())),
        }
    }
    Ok(())
}

fn cleanup(mut child: Child, mut status: Option<ExitStatus>) -> bool {
    if status.is_none() {
        status = child.try_wait().ok().flatten();
        if status.is_none() {
            let _ = child.kill();
        }
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while status.is_none() && Instant::now() < deadline {
        status = child.try_wait().ok().flatten();
        if status.is_none() {
            thread::sleep(Duration::from_millis(5));
        }
    }
    if status.is_none() {
        // OS-level uninterruptibility is reported as incomplete cleanup. Retain
        // only this owned handle for eventual reaping; never signal descendants.
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
    status.is_some()
}

fn bounded_output(
    mut command: Command,
    timeout: Duration,
    limit: usize,
) -> Result<Output, Failure> {
    assert!(!timeout.is_zero() && timeout <= DEADLINE);
    assert!((1..=MAX_OUTPUT).contains(&limit));
    // Socket pairs provide safe nonblocking reads without reader threads,
    // unsafe fcntl calls or waiting forever for an inherited stdout pipe.
    let (mut stdout, stdout_child) = UnixStream::pair().expect("fixture stdout socket pair");
    let (mut stderr, stderr_child) = UnixStream::pair().expect("fixture stderr socket pair");
    stdout
        .set_nonblocking(true)
        .expect("nonblocking fixture stdout");
    stderr
        .set_nonblocking(true)
        .expect("nonblocking fixture stderr");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::from(OwnedFd::from(stdout_child)))
        .stderr(Stdio::from(OwnedFd::from(stderr_child)))
        .spawn()
        .expect("start exact owned offline fixture child");
    drop(command); // Close the parent's configured write ends before EOF checks.
    let (mut out, mut err, mut out_eof, mut err_eof, mut status) =
        (Vec::new(), Vec::new(), false, false, None);
    let deadline = Instant::now() + timeout;
    let failure = loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(value) => status = value,
                Err(error) => break FailureKind::Io(error.to_string()),
            }
        }
        if let Err(error) = drain(&mut stdout, &mut out, &mut out_eof, limit) {
            break error;
        }
        if let Err(error) = drain(&mut stderr, &mut err, &mut err_eof, limit) {
            break error;
        }
        if let Some(status) = status
            && out_eof
            && err_eof
        {
            // try_wait already observed/reaped this direct child. wait returns
            // its cached status here; it cannot wait on a running process.
            assert_eq!(child.wait().expect("cached reaped fixture status"), status);
            return Ok(Output {
                status,
                stdout: out,
                stderr: err,
            });
        }
        if Instant::now() >= deadline {
            break FailureKind::Deadline;
        }
        thread::sleep(Duration::from_millis(5));
    };
    // Closing these sockets cannot leave a detached blocking reader behind.
    drop((stdout, stderr));
    Err(Failure {
        kind: failure,
        reaped: cleanup(child, status),
        stdout: out,
        stderr: err,
    })
}

fn command(repository: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("/usr/bin/git");
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "tag.gpgSign=false",
        ])
        .arg("-C")
        .arg(repository)
        .args(args);
    if args.first() == Some(&"init") {
        command.arg("--template=");
    }
    command
}

fn checked(command: Command) -> Output {
    let output = bounded_output(command, DEADLINE, MAX_OUTPUT).unwrap_or_else(|failure| {
        panic!(
            "bounded Git fixture failure: {:?}, reaped={}, stdout={} stderr={}",
            failure.kind,
            failure.reaped,
            String::from_utf8_lossy(&failure.stdout),
            String::from_utf8_lossy(&failure.stderr)
        )
    });
    assert!(
        output.status.success(),
        "Git fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub fn git(repository: &Path, args: &[&str]) -> Output {
    checked(command(repository, args))
}

pub fn text(output: Output) -> String {
    String::from_utf8(output.stdout)
        .expect("Git fixture text is UTF-8")
        .trim()
        .to_owned()
}

pub fn commit(repository: &Path, message: &str, timestamp: &str) -> String {
    git(repository, &["add", "--", "."]);
    let mut command = command(
        repository,
        &[
            "-c",
            "user.name=Offline Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            message,
        ],
    );
    command
        .env("GIT_AUTHOR_DATE", timestamp)
        .env("GIT_COMMITTER_DATE", timestamp);
    checked(command);
    text(git(repository, &["rev-parse", "HEAD"]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    const CHILD: &str = "git_fixture::tests::child_control";

    fn child(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("owned test binary"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("PUBLICATION_FIXTURE_CONTROL", mode)
            .args(["--ignored", "--exact", CHILD, "--nocapture"]);
        command
    }

    #[test]
    #[ignore = "finite owned fixture child, explicitly invoked by bounded parent tests"]
    fn child_control() {
        match std::env::var("PUBLICATION_FIXTURE_CONTROL")
            .expect("explicit parent mode")
            .as_str()
        {
            "deadline" => {
                println!("ready");
                thread::sleep(Duration::from_secs(3));
            }
            "output" => {
                std::io::stdout()
                    .write_all(&vec![b'x'; MAX_OUTPUT + 1])
                    .expect("write bounded overflow fixture");
            }
            "poisoned-git-env" => {
                let repository = std::env::var_os("PUBLICATION_FIXTURE_REPOSITORY")
                    .expect("owned fixture repository");
                let repository = Path::new(&repository);
                git(repository, &["init", "--quiet", "--initial-branch=main"]);
                fs::write(repository.join("file"), b"owned candidate")
                    .expect("write owned candidate");
                commit(repository, "owned candidate", "2000-01-01T00:00:00Z");
            }
            _ => panic!("unknown owned fixture child mode"),
        }
    }

    #[test]
    fn git_ignores_inherited_repository_and_hook_overrides() {
        let root = private_fixture();
        let foreign = root.path().join("foreign");
        private_directory(&foreign);
        git(&foreign, &["init", "--quiet", "--initial-branch=main"]);
        fs::write(foreign.join("file"), b"foreign baseline").expect("foreign fixture baseline");
        commit(&foreign, "foreign baseline", "2000-01-01T00:00:00Z");
        let intended = root.path().join("intended");
        private_directory(&intended);
        let hooks = root.path().join("hooks");
        private_directory(&hooks);
        let marker = root.path().join("hook-ran");
        let hook = hooks.join("post-commit");
        let quoted_marker = marker
            .to_str()
            .expect("fixture hook path is UTF-8")
            .replace('\'', "'\\''");
        fs::write(
            &hook,
            format!("#!/bin/sh\n/usr/bin/touch '{quoted_marker}'\n"),
        )
        .expect("private hook control");
        fs::set_permissions(hook, fs::Permissions::from_mode(0o700))
            .expect("executable control hook");
        // Prove the detector is live using only this disposable repository.
        let hooks_arg = format!("core.hooksPath={}", hooks.display());
        let mut positive = command(
            &foreign,
            &[
                "-c",
                &hooks_arg,
                "-c",
                "user.name=Offline Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "private hook positive control",
            ],
        );
        positive
            .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:01Z")
            .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:01Z");
        checked(positive);
        assert!(marker.exists(), "the private hook detector must first fire");
        fs::remove_file(&marker).expect("reset private hook detector");
        let before = text(git(&foreign, &["rev-parse", "HEAD"]));
        let mut command = child("poisoned-git-env");
        command
            .env("PUBLICATION_FIXTURE_REPOSITORY", &intended)
            .env("GIT_DIR", foreign.join(".git"))
            .env("GIT_WORK_TREE", &foreign)
            .env("GIT_INDEX_FILE", foreign.join("poisoned-index"))
            .env("GIT_TEMPLATE_DIR", &hooks)
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "core.hooksPath")
            .env("GIT_CONFIG_VALUE_0", &hooks);
        checked(command);
        assert_eq!(text(git(&foreign, &["rev-parse", "HEAD"])), before);
        assert!(!foreign.join("poisoned-index").exists());
        assert!(!marker.exists(), "no inherited hook may execute");
        assert!(intended.join(".git").is_dir());
        assert_eq!(
            fs::read(intended.join("file")).expect("owned file"),
            b"owned candidate"
        );
    }

    #[test]
    fn fixture_deadline_and_capture_limit_reap_only_the_owned_child() {
        let started = Instant::now();
        let timeout = bounded_output(child("deadline"), Duration::from_millis(100), MAX_OUTPUT)
            .expect_err("finite sleeping child must time out");
        assert_eq!(timeout.kind, FailureKind::Deadline);
        assert!(timeout.reaped);
        assert!(started.elapsed() < Duration::from_secs(3));
        let overflow = bounded_output(child("output"), DEADLINE, MAX_OUTPUT)
            .expect_err("one byte over output cap rejects");
        assert_eq!(overflow.kind, FailureKind::OutputLimit);
        assert!(overflow.reaped);
        assert_eq!(overflow.stdout.len(), MAX_OUTPUT);
    }
}
