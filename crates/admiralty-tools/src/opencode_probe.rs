//! One explicit, bounded OpenCode SSH qualification; never reads credential files.
//!
//! Port of the retired `tools/probe_opencode.py`. The qualification workload
//! (owner oracle, task prompt and remote harness) is sent verbatim to the
//! node over SSH stdin/argv as data; see [`workload`]. The local side only
//! validates arguments, launches one fixed `ssh` invocation with a 180-second
//! deadline, and writes an exclusive receipt.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use sha2::{Digest, Sha256};

use crate::error::{ToolError, ToolResult};
use crate::fsx;
use crate::proc;
use crate::pyjson::{self, DumpOptions, LoadOptions, Value};

pub mod workload;

pub use workload::{ORACLE, PROMPT, REMOTE};

/// Remote interpreter that runs [`REMOTE`] on the qualified node.
pub const REMOTE_PYTHON: &str = "/usr/bin/python3";
/// Receipt assurance statement.
pub const ASSURANCE: &str = "One trusted-local helper task and independent owner oracle; no isolation, fleet scheduling, provider restart or throughput qualification.";

static NODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}\z").expect("static node pattern")
});

/// `admiralty opencode probe` arguments (same flags as the Python argparse).
#[derive(clap::Args, Debug, Clone)]
pub struct ProbeArgs {
    /// Registered safe SSH alias.
    #[arg(long)]
    pub node: String,
    /// Absolute remote directory receiving the qualification fixture.
    #[arg(long = "remote-parent")]
    pub remote_parent: String,
    /// Absolute remote coding helper.
    #[arg(long = "remote-helper")]
    pub remote_helper: String,
    /// Absolute remote `opencode` binary.
    #[arg(long)]
    pub opencode: String,
    /// Requested provider/model.
    #[arg(long)]
    pub model: String,
    /// New local receipt file in an existing owned directory.
    #[arg(long)]
    pub output: PathBuf,
    /// Explicitly authorized setup correction, one new task in this existing owned fixture.
    #[arg(long = "correct-fixture")]
    pub correct_fixture: Option<String>,
    /// Explicitly authorize exactly one bounded real coding task.
    #[arg(long)]
    pub qualify: bool,
    /// Local SSH client (default `/usr/bin/ssh`).
    #[arg(long = "ssh-binary", default_value = "/usr/bin/ssh")]
    pub ssh_binary: PathBuf,
}

/// Python `shlex.quote`.
pub fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_owned();
    }
    let safe = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if safe {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

/// Python `shlex.join`.
pub fn shell_join(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| shell_quote(value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Validated probe plan (everything decided before any SSH effect).
#[derive(Clone, Debug, PartialEq)]
pub struct ProbePlan {
    /// Remote fixture root.
    pub root: String,
    /// JSON payload sent on stdin.
    pub payload: String,
    /// Full local argv (`ssh ...`).
    pub argv: Vec<String>,
    /// Absolute receipt path.
    pub output: PathBuf,
}

/// Argument validation (Python `parser.error` cases) and payload assembly.
pub fn plan(args: &ProbeArgs, fixture_id: &str) -> Result<ProbePlan, String> {
    if !NODE.is_match(&args.node) {
        return Err("node must be a registered safe SSH alias".to_owned());
    }
    for value in [&args.remote_parent, &args.remote_helper, &args.opencode] {
        if !value.starts_with('/')
            || value.split('/').any(|part| part == "..")
            || value.contains('\0')
        {
            return Err(
                "remote paths must be absolute, non-traversing paths on that node".to_owned(),
            );
        }
    }
    if !args.qualify {
        return Err(
            "real qualification requires --qualify; use installed CLI --help for read-only probes"
                .to_owned(),
        );
    }
    let output =
        fsx::absolute(&fsx::expanduser(&args.output)).map_err(|error| error.to_string())?;
    let parent = output.parent().map(PathBuf::from).unwrap_or_default();
    if output.exists() || fsx::is_symlink(&output) || !parent.is_dir() {
        return Err("output must be a new receipt file in an existing owned directory".to_owned());
    }
    if std::fs::metadata(&parent).map_or(true, |meta| meta.uid() != fsx::getuid()) {
        return Err("output parent must be owned by current user".to_owned());
    }
    let parent_root = args.remote_parent.trim_end_matches('/');
    let mut root = format!("{parent_root}/admiralty-qualification-{fixture_id}");
    if let Some(correction) = &args.correct_fixture {
        let pattern = format!(
            r"^{}/admiralty-qualification-[0-9a-f]{{16}}\z",
            regex::escape(parent_root)
        );
        let matches = Regex::new(&pattern).is_ok_and(|expression| expression.is_match(correction));
        if !matches {
            return Err(
                "correction must name an owned qualification fixture immediately under remote parent"
                    .to_owned(),
            );
        }
        root.clone_from(correction);
    }
    let payload = crate::pyobj! {
        "root" => root.as_str(),
        "helper" => args.remote_helper.as_str(),
        "opencode" => args.opencode.as_str(),
        "model" => args.model.as_str(),
        "oracle" => ORACLE,
        "prompt" => PROMPT,
        "corrected" => args.correct_fixture.is_some(),
    };
    let argv = vec![
        args.ssh_binary.display().to_string(),
        "-T".to_owned(),
        "-o".to_owned(),
        "BatchMode=yes".to_owned(),
        "-o".to_owned(),
        "ConnectTimeout=8".to_owned(),
        args.node.clone(),
        shell_join(&[REMOTE_PYTHON, "-c", REMOTE]),
    ];
    Ok(ProbePlan {
        root,
        payload: pyjson::dumps_default(&payload),
        argv,
        output,
    })
}

fn decode(bytes: &[u8]) -> ToolResult<String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| ToolError::type_error(format!("'utf-8' codec can't decode: {error}")))?;
    Ok(pyjson::universal_newlines(text))
}

/// Execute a validated plan; prints the compact summary and returns the exit code.
pub fn execute(plan: &ProbePlan, stdout: &mut dyn Write, timeout: Duration) -> ToolResult<i32> {
    let mut command = Command::new(&plan.argv[0]);
    command.args(&plan.argv[1..]);
    let captured = proc::run_bounded(
        &mut command,
        Some(plan.payload.as_bytes()),
        timeout,
        true,
        None,
    )?;
    let stdout_text = decode(&captured.stdout)?;
    let stderr_text = decode(&captured.stderr)?;
    if captured.code() != 0 {
        let unresolved = crate::pyobj! {
            "state" => "unresolved",
            "remote_root" => plan.root.as_str(),
            "ssh_exit" => captured.code(),
            "stderr_sha256" => sha256_hex(stderr_text.as_bytes()),
        };
        writeln!(stdout, "{}", pyjson::dumps_default(&unresolved))?;
        return Ok(2);
    }
    let mut receipt = pyjson::loads(&stdout_text, LoadOptions::default())
        .map_err(|error| ToolError::type_error(format!("remote receipt is not JSON: {error}")))?;
    let expected = Value::Str(sha256_hex(ORACLE.as_bytes()));
    let before = receipt.index_key("oracle_sha256_before")?.clone();
    let after = receipt.index_key("oracle_sha256_after")?.clone();
    let matches = before == expected && expected == after;
    let map = receipt
        .as_object_mut()
        .ok_or_else(|| ToolError::type_error("remote receipt must be an object"))?;
    map.insert("oracle_owner_expected_sha256", expected);
    map.insert("owner_oracle_matches", matches);
    map.insert("assurance", ASSURANCE);
    let mut text = pyjson::dumps(&receipt, &DumpOptions::PYTHON.indent(2))?;
    text.push('\n');
    let mut handle = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&plan.output)?;
    handle.write_all(text.as_bytes())?;
    let field = |key: &str| receipt.index_key(key).cloned();
    let summary = crate::pyobj! {
        "receipt" => plan.output.display().to_string(),
        "remote_root" => plan.root.as_str(),
        "helper_exit" => field("helper_exit")?,
        "verifier_exit" => field("verifier_exit")?,
        "oracle_unchanged" => field("owner_oracle_matches")?,
        "scope_valid" => field("scope_valid")?,
        "model_observed" => field("model_observed")?,
        "elapsed_s" => field("elapsed_s")?,
    };
    writeln!(stdout, "{}", pyjson::dumps_default(&summary))?;
    let zero = Value::Int(0);
    let helper = field("helper_exit")?;
    let verifier = field("verifier_exit")?;
    let passed = helper == verifier
        && verifier == zero
        && field("owner_oracle_matches")?.truthy()
        && field("scope_valid")?.truthy()
        && !field("timed_out")?.truthy();
    Ok(if passed { 0 } else { 1 })
}

/// Run `opencode probe`; returns the exit code.
pub fn run(args: &ProbeArgs) -> Result<i32, crate::BoxError> {
    let fixture: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(16)
        .collect();
    let plan = match plan(args, &fixture) {
        Ok(plan) => plan,
        Err(message) => {
            eprintln!("probe_opencode: error: {message}");
            return Ok(2);
        }
    };
    let mut stdout = std::io::stdout().lock();
    match execute(&plan, &mut stdout, Duration::from_secs(180)) {
        Ok(code) => Ok(code),
        Err(error) => {
            eprintln!("probe_opencode: {error}");
            Ok(1)
        }
    }
}

#[cfg(test)]
#[path = "opencode_probe_tests.rs"]
mod tests;
