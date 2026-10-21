//! Isolated real `admiraltyd` incident-journal smoke, including restart
//! (port of the retired `scripts/check/excursions-smoke.py`).
//!
//! Use a new scratch directory under protected, owner-controlled ancestors.
//! This starts only the supplied binary on an ephemeral loopback port; no
//! deployment.

use std::fs::{self, File};
use std::io::Write;
use std::net::TcpListener;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::http::{self, HttpResponse, Request, SystemRuntime};
use crate::process::{token_urlsafe, wait_timeout};
use crate::py::{self, Dump, PyDict, PyValue};
use crate::py_dict;

/// Largest accepted response body.
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const REQUEST_SECONDS: f64 = 10.0;
const READINESS_SECONDS: f64 = 20.0;
const HEALTH_SECONDS: f64 = 1.0;
const STOP_WAIT: Duration = Duration::from_secs(5);

/// `excursions-smoke.py` arguments.
#[derive(Debug, Clone, clap::Args)]
pub struct ExcursionsArgs {
    /// The `admiraltyd` binary to start (resolved like `Path.resolve(strict=True)`).
    #[arg(long)]
    pub binary: PathBuf,
    /// A new scratch directory to create with mode 0700.
    #[arg(long)]
    pub scratch: PathBuf,
}

type Outcome<T> = Result<T, String>;

fn assertion(condition: bool, what: &str) -> Outcome<()> {
    if condition {
        Ok(())
    } else {
        Err(format!("AssertionError: {what}"))
    }
}

fn index<'a>(value: &'a PyValue, key: &str) -> Outcome<&'a PyValue> {
    value.index(key)
}

fn item(value: &PyValue, position: usize) -> Outcome<&PyValue> {
    match value {
        PyValue::List(items) => items
            .get(position)
            .ok_or_else(|| "IndexError: list index out of range".to_owned()),
        PyValue::Dict(_) => Err(format!("KeyError: {position}")),
        other => Err(format!(
            "TypeError: '{}' object is not subscriptable",
            other.type_name()
        )),
    }
}

fn py_len(value: &PyValue) -> Outcome<usize> {
    match value {
        PyValue::List(items) => Ok(items.len()),
        PyValue::Dict(dict) => Ok(dict.len()),
        PyValue::Str(text) => Ok(text.chars().count()),
        other => Err(format!(
            "TypeError: object of type '{}' has no len()",
            other.type_name()
        )),
    }
}

fn with(base: &PyValue, key: &str, value: impl Into<PyValue>) -> PyValue {
    let mut copy = base.clone();
    if let Some(dict) = copy.as_dict_mut() {
        dict.insert(key, value);
    }
    copy
}

struct Smoke {
    binary: PathBuf,
    scratch: PathBuf,
    port: u16,
    token: String,
}

impl Smoke {
    fn post(
        &self,
        path: &str,
        value: &PyValue,
        expected: u16,
        credential: &str,
    ) -> Outcome<PyValue> {
        let body = py::dumps(value, Dump::DEFAULT);
        let authorization = format!("Bearer {credential}");
        let full_path = format!("/v2/incidents/{path}");
        let request = Request {
            method: "POST",
            path: &full_path,
            host_header: format!("127.0.0.1:{}", self.port),
            headers: &[
                ("Authorization", &authorization),
                ("Content-Type", "application/json"),
                ("Connection", "close"),
            ],
            body: Some(body.as_bytes()),
        };
        let deadline = http::monotonic() + REQUEST_SECONDS;
        let response = http::exchange(
            &SystemRuntime,
            "127.0.0.1",
            self.port,
            &request,
            deadline,
            MAX_RESPONSE_BYTES,
        )
        .map_err(|failure| format!("request {path} failed: {failure:?}"))?;
        let HttpResponse { status, body, .. } = response;
        if status != expected {
            return Err(format!(
                "AssertionError: ({}, {status}, {expected})",
                py::repr_str(path)
            ));
        }
        py::loads_bytes(&body, false).map_err(|error| format!("JSONDecodeError: {error}"))
    }

    fn post_ok(&self, path: &str, value: &PyValue) -> Outcome<PyValue> {
        self.post(path, value, 200, &self.token)
    }

    fn healthy(&self) -> bool {
        let request = Request {
            method: "GET",
            path: "/healthz",
            host_header: format!("127.0.0.1:{}", self.port),
            headers: &[("Connection", "close")],
            body: None,
        };
        let deadline = http::monotonic() + HEALTH_SECONDS;
        http::exchange(
            &SystemRuntime,
            "127.0.0.1",
            self.port,
            &request,
            deadline,
            MAX_RESPONSE_BYTES,
        )
        .is_ok_and(|response| response.status == 200)
    }

    fn start(&self, log: &File) -> Outcome<Child> {
        let out = log.try_clone().map_err(|e| e.to_string())?;
        let err = log.try_clone().map_err(|e| e.to_string())?;
        let mut process = Command::new(&self.binary)
            .arg("--database")
            .arg(self.scratch.join("incidents.db"))
            .arg("--listen")
            .arg(format!("127.0.0.1:{}", self.port))
            .arg("--token-file")
            .arg(self.scratch.join("token"))
            .arg("--native-config")
            .arg(self.scratch.join("config.json"))
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .spawn()
            .map_err(|e| e.to_string())?;
        let deadline = http::monotonic() + READINESS_SECONDS;
        let outcome = (|| {
            while http::monotonic() < deadline {
                if process.try_wait().map_err(|e| e.to_string())?.is_some() {
                    return Err(
                        "RuntimeError: isolated binary exited before readiness; inspect scratch/server.log"
                            .to_owned(),
                    );
                }
                if self.healthy() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err("RuntimeError: isolated binary readiness deadline".to_owned())
        })();
        match outcome {
            Ok(()) => Ok(process),
            Err(error) => {
                stop(&mut process)?;
                Err(error)
            }
        }
    }
}

fn stop(process: &mut Child) -> Outcome<()> {
    if process.try_wait().map_err(|e| e.to_string())?.is_none() {
        let pid = rustix::process::Pid::from_child(process);
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        if wait_timeout(process, STOP_WAIT)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            let _ = process.kill();
            if wait_timeout(process, STOP_WAIT)
                .map_err(|e| e.to_string())?
                .is_none()
            {
                return Err("TimeoutExpired: isolated binary did not exit after kill".into());
            }
        }
    }
    Ok(())
}

fn write_exclusive(path: &Path, text: &str) -> Outcome<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

fn incident(scope: &PyValue, identity: &str, parent: Option<&str>) -> PyValue {
    py_dict! {
        "scope" => scope.clone(),
        "event" => py_dict! {
            "event_id" => identity,
            "family_signature" => "storage-timeout",
            "kind" => if parent.is_some() { "escalation" } else { "failure" },
            "cause_parent" => parent,
            "evidence_digests" => Vec::<PyValue>::new(),
            "observed_tick" => PyValue::Null,
        },
    }
}

/// Run the smoke; the result document on success.
pub fn execute(args: &ExcursionsArgs) -> Outcome<PyValue> {
    let binary = fs::canonicalize(&args.binary)
        .map_err(|e| format!("FileNotFoundError: {}: {e}", args.binary.display()))?;
    let scratch = if args.scratch.is_absolute() {
        args.scratch.clone()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(&args.scratch)
    };
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&scratch)
        .map_err(|e| format!("{}: {e}", scratch.display()))?;
    let token = token_urlsafe().map_err(|e| e.to_string())?;
    let scope = py_dict! {
        "project" => "p27-smoke",
        "root" => "root",
        "task" => "task",
        "contract_revision" => 1,
        "contract_digest" => format!("sha256:{}", "a".repeat(64)),
        "protected_oracle_digest" => format!("sha256:{}", "b".repeat(64)),
    };
    let config = py_dict! {
        "schema_version" => 1,
        "operator" => PyValue::Null,
        "agents" => Vec::<PyValue>::new(),
        "excursions" => vec![py_dict! {
            "scope" => scope.clone(),
            "actions" => vec![PyValue::from("read"), PyValue::from("observe"), PyValue::from("propose")],
            "evidence_digests" => Vec::<PyValue>::new(),
        }],
    };
    write_exclusive(&scratch.join("token"), &token)?;
    write_exclusive(
        &scratch.join("config.json"),
        &py::dumps(&config, Dump::DEFAULT),
    )?;
    let port = {
        let available = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        available.local_addr().map_err(|e| e.to_string())?.port()
    };
    let smoke = Smoke {
        binary,
        scratch: scratch.clone(),
        port,
        token,
    };
    let log = File::create(scratch.join("server.log")).map_err(|e| e.to_string())?;

    let mut process = smoke.start(&log)?;
    let first_run = (|| -> Outcome<(PyValue, PyValue)> {
        let scopes = smoke.post_ok("scopes", &PyDict::new().into())?;
        assertion(
            py::py_eq(&scopes, &PyValue::List(vec![scope.clone()])),
            "post('scopes', {}) == [scope]",
        )?;
        smoke.post("read", &scope, 401, "wrong-token")?;
        smoke.post("read", &with(&scope, "root", "foreign"), 403, &smoke.token)?;
        let initial = py_dict! {
            "scope" => scope.clone(),
            "command_id" => "initial",
            "expected_revision" => 0,
            "operation" => py_dict! {
                "kind" => "observe",
                "incidents" => vec![
                    incident(&scope, "failure", None),
                    incident(&scope, "escalation", Some("failure")),
                ],
            },
        };
        let receipt = smoke.post_ok("commands", &initial)?;
        let read = smoke.post_ok("read", &scope)?;
        assertion(
            py_len(index(index(&read, "state")?, "episodes")?)? == 1,
            "len(post('read', scope)['state']['episodes']) == 1",
        )?;
        let recurrence = py_dict! {
            "scope" => scope.clone(),
            "command_id" => "recurrence",
            "expected_revision" => 1,
            "operation" => py_dict! {
                "kind" => "observe",
                "incidents" => vec![incident(&scope, "recurrence", None)],
            },
        };
        smoke.post_ok("commands", &recurrence)?;
        let state = smoke.post_ok("read", &scope)?;
        assertion(
            py_len(index(index(&state, "state")?, "episodes")?)? == 2,
            "len(state['state']['episodes']) == 2",
        )?;
        let fields = py_dict! {
            "scope" => "p27-smoke/root/task",
            "steps" => "Replay recorded failure and escalation",
            "expected" => "One incident per causal lineage",
            "actual" => "Writer contention observed",
            "tried_approaches" => "Do not add retries; prior retry failed",
            "candidate_fix" => "Inspect bounded queue ownership",
            "measurable_outcome" => "No duplicate episodes",
        };
        let episode_id = index(
            item(index(index(&state, "state")?, "episodes")?, 0)?,
            "episode_id",
        )?;
        let proposal = py_dict! {
            "scope" => scope.clone(),
            "command_id" => "proposal",
            "expected_revision" => 2,
            "operation" => py_dict! {
                "kind" => "propose",
                "episode_id" => episode_id.clone(),
                "reproduction" => py_dict! { "fields" => fields.clone() },
            },
        };
        let mut unsafe_proposal = proposal.clone();
        if let Some(target) = unsafe_proposal
            .as_dict_mut()
            .and_then(|d| d.get_mut("operation"))
            .and_then(PyValue::as_dict_mut)
            .and_then(|d| d.get_mut("reproduction"))
            .and_then(PyValue::as_dict_mut)
            .and_then(|d| d.get_mut("fields"))
            .and_then(PyValue::as_dict_mut)
        {
            target.insert("actual", "token=secret-fixture");
        }
        smoke.post("commands", &unsafe_proposal, 422, &smoke.token)?;
        smoke.post_ok("commands", &proposal)?;
        let replay = smoke.post_ok("commands", &initial)?;
        assertion(
            index(&replay, "replayed")?.truthy()
                && py::py_eq(index(&replay, "receipt")?, index(&receipt, "receipt")?),
            "replay['replayed'] and replay['receipt'] == receipt['receipt']",
        )?;
        smoke.post(
            "commands",
            &with(&initial, "command_id", "stale"),
            409,
            &smoke.token,
        )?;
        let expected = smoke.post_ok("read", &scope)?;
        let local = index(item(index(&expected, "proposals")?, 0)?, "proposal")?;
        assertion(
            index(local, "execution_granted")?.is_bool(false)
                && index(local, "external_publication_allowed")?.is_bool(false),
            "local['execution_granted'] is False and local['external_publication_allowed'] is False",
        )?;
        assertion(
            py::py_eq(
                index(local, "proposed_investigator_max_model_turns")?,
                &PyValue::Int(1),
            ),
            "local['proposed_investigator_max_model_turns'] == 1",
        )?;
        assertion(
            index(local, "uncertainty")?.contains(&PyValue::from("unknown"))?,
            "'unknown' in local['uncertainty']",
        )?;
        assertion(
            py::py_eq(
                index(index(local, "reproduction_contract")?, "fields")?,
                &fields,
            ),
            "local['reproduction_contract']['fields'] == fields",
        )?;
        Ok((expected, proposal))
    })();
    stop(&mut process)?;
    let (expected, proposal) = first_run?;

    let mut process = smoke.start(&log)?;
    let second_run = (|| -> Outcome<()> {
        let read = smoke.post_ok("read", &scope)?;
        assertion(
            py::py_eq(&read, &expected),
            "post('read', scope) == expected",
        )?;
        let replayed = smoke.post_ok("commands", &proposal)?;
        assertion(
            index(&replayed, "replayed")?.is_bool(true),
            "post('commands', proposal)['replayed'] is True",
        )
    })();
    stop(&mut process)?;
    second_run?;

    let checks = [
        "explicit_startup_grants",
        "scope_isolation",
        "causal_lineage",
        "recurrence",
        "secret_rejection",
        "bounded_local_proposal",
        "exact_retry",
        "stale_cas",
        "real_binary_restart",
    ];
    let result = py_dict! {
        "status" => "passed",
        "checks" => checks.iter().map(|check| PyValue::from(*check)).collect::<Vec<_>>(),
        "scratch" => scratch.to_string_lossy().into_owned(),
    };
    let mut document = py::dumps(&result, Dump::INDENT2);
    document.push('\n');
    fs::write(scratch.join("result.json"), document).map_err(|e| e.to_string())?;
    Ok(result)
}

/// Run the excursions smoke: print the result JSON (exit 0) or the failure
/// on stderr (exit 1).
pub fn run(args: ExcursionsArgs) -> crate::RunResult {
    match execute(&args) {
        Ok(result) => {
            println!("{}", py::dumps(&result, Dump::DEFAULT));
            Ok(0)
        }
        Err(error) => {
            eprintln!("excursions smoke failed: {error}");
            Ok(1)
        }
    }
}
