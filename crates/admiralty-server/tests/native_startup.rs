#![cfg(unix)]
use admiralty_core::{DomainEvent, SourceIdentity, TaskContract};
use admiralty_server::{ServerConfig, app_with_native_routes, startup::open_native_services};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

type Error = Box<dyn std::error::Error + Send + Sync>;
const OWNER: &str = "owner-test-012345678901234567890123456789";
const AGENT: &str = "agent-test-012345678901234567890123456789";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Result<Self, Error> {
        let path = std::env::temp_dir()
            .canonicalize()?
            .join(format!("native-startup-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
    fn write(&self, name: &str, bytes: &[u8]) -> Result<PathBuf, Error> {
        let path = self.0.join(name);
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?
            .write_all(bytes)?;
        Ok(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Server {
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    async fn new(router: axum::Router) -> Result<Self, Error> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self { address, task })
    }
    async fn call(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: &str,
    ) -> Result<(u16, String, String), Error> {
        let address = self.address;
        let (method, path, token, body) = (
            method.to_owned(),
            path.to_owned(),
            token.map(str::to_owned),
            body.to_owned(),
        );
        tokio::task::spawn_blocking(move || {
            let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
            stream.set_read_timeout(Some(Duration::from_secs(3)))?;
            stream.set_write_timeout(Some(Duration::from_secs(3)))?;
            let auth = token.map(|value| format!("Authorization: Bearer {value}\r\n")).unwrap_or_default();
            write!(stream, "{method} {path} HTTP/1.1\r\nHost: {address}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())?;
            let mut bytes = Vec::new();
            stream.take(1024*1024+1).read_to_end(&mut bytes)?;
            if bytes.len() > 1024*1024 { return Err("HTTP test response exceeds bound".into()); }
            let response = String::from_utf8(bytes)?;
            let (headers, body) = response.split_once("\r\n\r\n").ok_or("missing HTTP separator")?;
            let status = headers.split_whitespace().nth(1).ok_or("missing status")?.parse()?;
            Ok((status,headers.to_owned(),body.to_owned()))
        }).await?
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn contract(project: &str, task: &str) -> Result<TaskContract, Error> {
    Ok(serde_json::from_value(
        json!({"schema_version":1,"project_id":project,"root_id":"root","task_id":task,"revision":1,
        "title":"Native configuration smoke","objective":"Retain exact scope", "non_goals":["Do not read secrets"],
        "owned_paths":["src"],"forbidden_paths":["secrets"],"read_first":[],"dependencies":[],
        "criteria":[{"id":"C1","text":"Check passes","check_ids":["check"]}],
        "checks":[{"id":"check","program":"cargo","args":["check"],"cwd":".","timeout_secs":10}],
        "authority_ref":"owner-grant","max_attempts":1,"simulated":false}),
    )?)
}
fn config() -> Result<ServerConfig, Error> {
    Ok(ServerConfig {
        listen: "127.0.0.1:0".parse()?,
        simulated: false,
        token: Some(OWNER.into()),
        assets: None,
        legacy: None,
        models_file: None,
    })
}

#[tokio::test]
async fn protected_startup_mounts_real_commands_and_compact_scoped_agent_reads() -> Result<(), Error>
{
    let fixture = Fixture::new()?;
    let token_file = fixture.write("agent-token", AGENT.as_bytes())?;
    let native_file = fixture.write("native.json", &serde_json::to_vec(&json!({
        "schema_version":1,"operator":{"principal":"owner","projects":["allowed"]},
        "agents":[{"credential_id":"worker-credential","token_file":token_file,"actor":"worker", "grants":[{
            "scope":{"project_id":"allowed","recipient":"worker","context_epoch":"epoch1"},"action":"read_pending"
        }]}]
    }))?)?;
    let native = open_native_services(&fixture.0.join("native.sqlite"), &native_file, OWNER)?;
    let server = Server::new(app_with_native_routes(
        native.store.clone(),
        config()?,
        Some(native.routes),
    )?)
    .await?;
    assert_eq!(server.call("POST", "/v2/tasks", None, "{").await?.0, 401);
    let request = json!({"request_id":uuid::Uuid::new_v4(),"contract":contract("allowed","task")?})
        .to_string();
    assert_eq!(
        server
            .call("POST", "/v2/tasks", Some(OWNER), &request)
            .await?
            .0,
        200
    );
    let replay: Value = serde_json::from_str(
        &server
            .call("POST", "/v2/tasks", Some(OWNER), &request)
            .await?
            .2,
    )?;
    assert_eq!(replay["duplicate"], true);
    assert_eq!(replay["execution_granted"], false);
    native.store.append(
        SourceIdentity {
            producer_id: "foreign-seed".into(),
            incarnation: "one".into(),
            sequence: "1".into(),
        },
        DomainEvent::TaskSubmitted {
            contract: contract("foreign", "other")?,
        },
        "2026-10-18T20:00:00Z".into(),
    )?;
    let before = native.store.snapshot()?.cursor;
    let response = server
        .call(
            "GET",
            "/v2/agents/inbox/context?project_id=allowed&task_id=task&context_epoch=epoch1",
            Some(AGENT),
            "",
        )
        .await?;
    assert_eq!(response.0, 200);
    let context: Value = serde_json::from_str(&response.2)?;
    assert!(context["capsule"].is_object());
    assert!(context.get("source_cursor").is_none());
    assert!(
        context["task_view_digest"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:"))
    );
    assert_eq!(
        server
            .call(
                "GET",
                "/v2/agents/inbox/context?project_id=foreign&task_id=other&context_epoch=epoch1",
                Some(AGENT),
                ""
            )
            .await?
            .0,
        403
    );
    assert_eq!(
        server
            .call("GET", "/v2/agents/inbox/history", Some(AGENT), "")
            .await?
            .0,
        404
    );
    assert_eq!(native.store.snapshot()?.cursor, before);
    drop(server);
    drop(native.store);
    Ok(())
}

#[tokio::test]
async fn native_redirect_preserves_paths_queries_and_rejects_remote_targets() -> Result<(), Error> {
    assert!(admiralty_server::redirect_app("192.0.2.1:18890".parse()?).is_err());
    let server = Server::new(admiralty_server::redirect_app("127.0.0.1:18890".parse()?)?).await?;
    let response = server
        .call("GET", "/legacy/?view=detailed", None, "")
        .await?;
    assert_eq!(response.0, 307);
    assert!(
        response
            .1
            .to_ascii_lowercase()
            .contains("location: http://127.0.0.1:18890/legacy/?view=detailed")
    );
    Ok(())
}

#[test]
fn native_configuration_rejects_owner_credential_reuse_by_agent() -> Result<(), Error> {
    let fixture = Fixture::new()?;
    let token_file = fixture.write("agent-token", OWNER.as_bytes())?;
    let path = fixture.write(
        "native.json",
        &serde_json::to_vec(&json!({"schema_version":1,"agents":[{
            "credential_id":"agent","token_file":token_file,"actor":"worker","grants":[]
        }]}))?,
    )?;
    assert!(
        open_native_services(Path::new(&fixture.0.join("native.sqlite")), &path, OWNER).is_err()
    );
    assert!(!fixture.0.join("native.sqlite").exists());
    Ok(())
}
