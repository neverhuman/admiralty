use admiralty_runtime::recovery::{Blocker, DecimalU64, plan_recovery};
use std::{
    io::{self, Read},
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 64 * 1024;
const CHILD_TIMEOUT: Duration = Duration::from_secs(30);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);
type Capture = Option<io::Result<Vec<u8>>>;

fn capture_pipe(reader: impl Read + Send + 'static, limit: usize) -> Receiver<io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        // One extra byte distinguishes exact-bound EOF from oversized output.
        // Stop reading at that bound; neither retained memory nor draining work
        // can grow without limit when a child floods a pipe.
        let result = reader.take((limit + 1) as u64).read_to_end(&mut bytes);
        let _ = sender.send(result.map(|_| bytes));
    });
    receiver
}

fn poll_capture(
    name: &str,
    receiver: &Receiver<io::Result<Vec<u8>>>,
    capture: &mut Capture,
    limit: usize,
    failure: &mut Option<String>,
) {
    if capture.is_some() {
        return;
    }
    let mut result = match receiver.try_recv() {
        Ok(result) => result,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err(io::Error::other("capture reader disconnected")),
    };
    match &mut result {
        Ok(bytes) if bytes.len() > limit => {
            bytes.truncate(limit);
            failure.get_or_insert_with(|| format!("{name} output bound exceeded"));
        }
        Err(_) => {
            failure.get_or_insert_with(|| format!("{name} capture failed"));
        }
        _ => {}
    }
    *capture = Some(result);
}

fn diagnostic(capture: &Capture) -> String {
    match capture {
        Some(Ok(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
        Some(Err(error)) => format!("capture error: {error}"),
        None => "capture did not reach bounded EOF".into(),
    }
}

/// Fixed offline fixtures only. Total wait and each capture are bounded. Only
/// the Child owned here is killed/reaped; inherited pipes never authorize
/// signalling descendants or a historical numeric PID to manufacture EOF.
fn bounded_output(mut command: Command, timeout: Duration, limit: usize) -> Result<Output, String> {
    assert!(!timeout.is_zero() && timeout <= CHILD_TIMEOUT);
    assert!((1..=OUTPUT_LIMIT).contains(&limit));
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("start owned fixture child: {error}"))?;
    let stdout_receiver = capture_pipe(child.stdout.take().expect("owned piped stdout"), limit);
    let stderr_receiver = capture_pipe(child.stderr.take().expect("owned piped stderr"), limit);
    let deadline = Instant::now() + timeout;
    let (mut stdout, mut stderr, mut status, mut failure) = (None, None, None, None);
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(observed) => status = observed,
                Err(error) => {
                    failure.get_or_insert_with(|| format!("poll owned child: {error}"));
                }
            }
        }
        poll_capture("stdout", &stdout_receiver, &mut stdout, limit, &mut failure);
        poll_capture("stderr", &stderr_receiver, &mut stderr, limit, &mut failure);
        if failure.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            failure = Some("owned fixture deadline expired".into());
            break;
        }
        if let Some(status) = status
            && stdout.is_some()
            && stderr.is_some()
        {
            return Ok(Output {
                status,
                stdout: stdout
                    .expect("completed stdout capture")
                    .expect("checked stdout result"),
                stderr: stderr
                    .expect("completed stderr capture")
                    .expect("checked stderr result"),
            });
        }
        thread::sleep(Duration::from_millis(5));
    }
    let kill = if status.is_none() {
        child.kill().map_err(|error| error.to_string())
    } else {
        Ok(())
    };
    let cleanup_deadline = Instant::now() + CLEANUP_TIMEOUT;
    loop {
        if status.is_none()
            && let Ok(observed) = child.try_wait()
        {
            status = observed;
        }
        poll_capture("stdout", &stdout_receiver, &mut stdout, limit, &mut failure);
        poll_capture("stderr", &stderr_receiver, &mut stderr, limit, &mut failure);
        if (status.is_some() && stdout.is_some() && stderr.is_some())
            || Instant::now() >= cleanup_deadline
        {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let reaped = status.is_some();
    if !reaped {
        // Retain the owned Child handle for eventual reaping without making the
        // test foreground wait forever on an OS-level uninterruptible process.
        thread::spawn(move || {
            let _ = child.wait();
        });
    }
    Err(format!(
        "{}; reaped={reaped}; kill={kill:?}; status={status:?}\nstdout:\n{}\nstderr:\n{}",
        failure.expect("failure reached bounded cleanup"),
        diagnostic(&stdout),
        diagnostic(&stderr)
    ))
}

#[path = "support/recovery.rs"]
mod recovery_fixture;

#[test]
fn missing_artifact_or_stale_external_fence_blocks_restore_proposal() {
    let (manifest, mut observation, mut fence) = recovery_fixture::valid_recovery_evidence();
    observation.available_artifacts.clear();
    let missing = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("missing evidence remains a non-authorizing proposal");
    assert!(
        missing
            .blockers
            .iter()
            .any(|b| matches!(b, Blocker::MissingArtifact(_)))
    );
    assert!(!missing.restore_authorized());
    assert!(!missing.effects_authorized());

    observation.available_artifacts = manifest.artifact_closure.clone();
    fence.new_generation = DecimalU64::new(5);
    let stale = plan_recovery(&manifest, &observation, Some(&fence))
        .expect("stale fencing evidence remains a non-authorizing proposal");
    assert!(stale.blockers.contains(&Blocker::StaleExternalGeneration));
    assert!(!stale.restore_authorized());
    assert!(!stale.effects_authorized());
}

#[test]
fn p30_package_builder_runs_its_real_offline_self_test() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root resolves");
    let mut command = Command::new("python3");
    command
        .args(["scripts/release/package.py", "--self-test"])
        .current_dir(root);
    let output = bounded_output(command, CHILD_TIMEOUT, OUTPUT_LIMIT)
        .expect("package self-test completes within owned child and capture limits");
    assert!(
        output.status.success(),
        "package builder self-test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(
        report.contains("Ran 10 tests"),
        "unexpected self-test report: {report}"
    );
}

#[test]
fn offline_child_timeout_kills_and_reaps_only_the_owned_child() {
    let mut command = Command::new("python3");
    command.args(["-c", "import time; time.sleep(10)"]);
    let started = Instant::now();
    let error = bounded_output(command, Duration::from_millis(100), 128)
        .expect_err("sleeping owned fixture exceeds its deadline");
    assert!(error.contains("deadline expired"), "{error}");
    assert!(error.contains("reaped=true"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn offline_child_output_cap_fails_with_retained_bounded_diagnostic() {
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "import sys; sys.stdout.write('x' * 4096); sys.stdout.flush()",
    ]);
    let error = bounded_output(command, Duration::from_secs(5), 128)
        .expect_err("oversized owned output fails before unbounded capture");
    assert!(error.contains("output bound exceeded"), "{error}");
    assert!(error.contains("reaped=true"), "{error}");
    assert!(
        error.contains(&"x".repeat(128)),
        "bounded stdout is retained"
    );
    assert!(
        !error.contains(&"x".repeat(129)),
        "oversized suffix is omitted"
    );
}

#[test]
fn offline_nonzero_child_exit_preserves_its_diagnostic_and_status() {
    let mut command = Command::new("python3");
    command.args([
        "-c",
        "import sys; sys.stderr.write('retained failure marker\\n'); sys.exit(7)",
    ]);
    let output = bounded_output(command, Duration::from_secs(5), 128)
        .expect("finite failed child still yields its bounded evidence");
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stderr, b"retained failure marker\n");
}

#[test]
fn restore_without_external_fence_cannot_propose_resume_or_authorize_effects() {
    let (manifest, observation, _fence) = recovery_fixture::valid_recovery_evidence();
    let proposal = admiralty_runtime::recovery::plan_recovery(&manifest, &observation, None)
        .expect("missing fencing evidence remains a blocked proposal");

    assert!(proposal.blockers.contains(&Blocker::MissingExternalFence));
    assert!(!proposal.resume_proposed);
    assert!(!proposal.restore_authorized());
    assert!(!proposal.effects_authorized());
}
