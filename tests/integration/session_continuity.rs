#![cfg(unix)]

use admiralty_runtime::session::daemon::{DaemonConfig, SessionDaemon};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use tempfile::tempdir;

fn exchange(socket: &Path, request: &Value) -> std::io::Result<Value> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let body = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    stream.write_all(&(body.len() as u32).to_be_bytes())?;
    stream.write_all(&body)?;

    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    let size = u32::from_be_bytes(header) as usize;
    let mut response = vec![0; size];
    stream.read_exact(&mut response)?;
    serde_json::from_slice(&response).map_err(std::io::Error::other)
}

fn await_offset(socket: &Path, run_id: &str, offset: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let response = exchange(socket, &json!({"command":"status","run_id":run_id}))
            .expect("session daemon status response");
        if response["result"]["status"]["output_offset"] == offset {
            return response;
        }
        assert!(Instant::now() < deadline, "expected output offset {offset}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn await_exit(socket: &Path, run_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let response = exchange(socket, &json!({"command":"status","run_id":run_id}))
            .expect("session daemon status response");
        if response["result"]["status"]["state"] == "exited" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "child should exit after its input"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

fn tail_bytes(response: &Value) -> Vec<u8> {
    response["result"]["chunks"]
        .as_array()
        .expect("tail chunks")
        .iter()
        .flat_map(|chunk| {
            chunk["bytes"]
                .as_array()
                .expect("tail bytes")
                .iter()
                .map(|byte| byte.as_u64().expect("byte value") as u8)
        })
        .collect()
}

#[test]
fn daemon_client_reconnect_keeps_child_and_output_offsets_continuous() {
    let temporary = tempdir().expect("private test fixture directory");
    let spool = temporary.path().join("spool");
    fs::create_dir(&spool).expect("create spool directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))
            .expect("make temp root private");
        fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).expect("make spool private");
    }
    let socket = temporary.path().join("session.sock");
    let daemon = SessionDaemon::bind(DaemonConfig {
        socket: socket.clone(),
        spool_root: spool.clone(),
    })
    .expect("bind owned session daemon");
    let _server = thread::spawn(move || daemon.serve().expect("serve daemon"));

    let launch = exchange(
        &socket,
        &json!({
            "command":"launch",
            "spec":{
                "run_id":"continuous",
                "program":"/bin/sh",
                "args":["-c","printf A; IFS= read -r line; printf B"],
                "cwd":temporary.path(),
                "spool_dir":spool.join("continuous")
            }
        }),
    )
    .expect("launch child");
    assert_eq!(launch["result"]["kind"], "launched");

    let initial = await_offset(&socket, "continuous", "1");
    let initial_pid = initial["result"]["status"]["pid"].clone();
    let initial_identity = initial["result"]["status"]["start_identity"].clone();
    let prefix = exchange(
        &socket,
        &json!({"command":"tail","run_id":"continuous","from":"0","max_bytes":1}),
    )
    .expect("read first output range");
    assert_eq!(tail_bytes(&prefix), b"A");
    assert_eq!(prefix["result"]["next_offset"], "1");

    let input = exchange(
        &socket,
        &json!({
            "command":"input",
            "run_id":"continuous",
            "expected_epoch":"0",
            "bytes":[120,10]
        }),
    )
    .expect("deliver one input frame after reconnect");
    assert_eq!(input["result"]["input_epoch"], "1");

    let continued = await_offset(&socket, "continuous", "2");
    assert_eq!(continued["result"]["status"]["pid"], initial_pid);
    assert_eq!(
        continued["result"]["status"]["start_identity"],
        initial_identity
    );
    let suffix = exchange(
        &socket,
        &json!({"command":"tail","run_id":"continuous","from":"1","max_bytes":8}),
    )
    .expect("read output after reconnect");
    assert_eq!(tail_bytes(&suffix), b"B");
    assert_eq!(suffix["result"]["next_offset"], "2");
    await_exit(&socket, "continuous");
}
