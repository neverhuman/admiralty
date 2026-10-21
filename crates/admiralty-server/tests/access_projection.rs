use admiralty_server::{ServerConfig, app, legacy::LegacyConfig};
use serde_json::Value;
use std::{
    io::{self, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::timeout,
};

const MAX_HTTP_BYTES: usize = 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(3);
const TOKEN: &str = "fixture-private-bearer-token-longer-than-32";

struct Fixture {
    directory: PathBuf,
    store: Option<admiralty_store::Store>,
}

impl Fixture {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "admiralty-access-projection-{}",
            uuid::Uuid::new_v4()
        ));
        create_private_dir(&directory).expect("create owned private fixture directory");
        let store = admiralty_store::Store::open(directory.join("fixture.sqlite"))
            .expect("open owned fixture store");
        Self {
            directory,
            store: Some(store),
        }
    }

    fn store(&self) -> admiralty_store::Store {
        self.store.as_ref().expect("fixture store retained").clone()
    }

    fn models_file(&self) -> PathBuf {
        let path = self.directory.join("models.json");
        write_private_file(
            &path,
            br#"{
              "schema_version":1,
              "revision":"9007199254740993",
              "models":[{
                "id":"fixture-model","label":"Fixture model","provider":"openai_compatible",
                "provider_model":"fixture/model","tier":0,"enabled":true,"node_id":"fixture-node",
                "endpoint":"http://127.0.0.1:8000/v1","credential_file_ref":"credentials/fixture-token",
                "max_concurrency":2,"capabilities":["coding"],"task_max_attempts":1,"escalate_to":null
              }]
            }"#,
        )
        .expect("write owned private model registry");
        path
    }

    fn legacy_config(&self, public_read: bool) -> LegacyConfig {
        let status_file = self.directory.join("legacy-status.json");
        let index_file = self.directory.join("legacy-index.html");
        write_private_file(
            &status_file,
            br#"{"projects":{"fixture":{"status":"read-only"}},"order":["fixture"],"generated_utc":"2026-10-18T00:00:00Z"}"#,
        )
        .expect("write owned legacy status fixture");
        write_private_file(&index_file, b"<!doctype html><title>fixture legacy</title>")
            .expect("write owned legacy index fixture");
        LegacyConfig {
            status_file,
            index_file: Some(index_file),
            public_read,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.store.take();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

#[cfg(unix)]
fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)
}

struct TestServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl TestServer {
    async fn start(config: ServerConfig, store: admiralty_store::Store) -> Self {
        let router = app(store, config).expect("construct actual application router");
        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .expect("bind private ephemeral loopback listener");
        let address = listener
            .local_addr()
            .expect("read fixture listener address");
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve test-owned application");
        });
        Self { address, task }
    }

    async fn stop(&mut self) {
        self.task.abort();
        let _ = timeout(IO_TIMEOUT, &mut self.task).await;
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

impl HttpResponse {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("bounded API response is valid JSON")
    }
}

async fn request(
    address: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&[u8]>,
) -> HttpResponse {
    let body = body.unwrap_or_default();
    assert!(
        body.len() <= MAX_HTTP_BYTES,
        "fixture request stays bounded"
    );
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if !body.is_empty() {
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    request.push_str("\r\n");
    let request_len = request
        .len()
        .checked_add(body.len())
        .expect("request size checked");
    assert!(
        request_len <= MAX_HTTP_BYTES,
        "fixture request stays bounded"
    );
    let stream = timeout(IO_TIMEOUT, TcpStream::connect(address))
        .await
        .expect("bounded local connect")
        .expect("connect to own ephemeral server");
    let mut request_bytes = request.into_bytes();
    request_bytes.extend_from_slice(body);
    timeout(IO_TIMEOUT, async {
        let mut written = 0;
        while written < request_bytes.len() {
            stream.writable().await?;
            match stream.try_write(&request_bytes[written..]) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "empty write")),
                Ok(count) => written += count,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            }
        }
        Ok::<(), io::Error>(())
    })
    .await
    .expect("bounded request write")
    .expect("write local request");

    let mut response = Vec::new();
    let mut chunk = [0u8; 8192];
    timeout(IO_TIMEOUT, async {
        loop {
            stream.readable().await?;
            match stream.try_read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    if count > MAX_HTTP_BYTES.saturating_sub(response.len()) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "response exceeds bound",
                        ));
                    }
                    response.extend_from_slice(&chunk[..count]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            }
        }
        Ok::<(), io::Error>(())
    })
    .await
    .expect("bounded response read")
    .expect("read local response");
    assert!(response.len() <= MAX_HTTP_BYTES, "response remains bounded");
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP response has header boundary");
    let head = std::str::from_utf8(&response[..split]).expect("HTTP header is ASCII");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
        .expect("HTTP response has status");
    HttpResponse {
        status,
        body: response[split + 4..].to_vec(),
    }
}

fn config(
    simulated: bool,
    token: Option<String>,
    models_file: Option<PathBuf>,
    legacy: Option<LegacyConfig>,
) -> ServerConfig {
    ServerConfig {
        listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        simulated,
        token,
        assets: None,
        legacy,
        models_file,
    }
}

#[tokio::test]
async fn real_fleet_and_models_require_exact_bearer_and_report_unqualified_reads() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let mut server = TestServer::start(
        config(
            false,
            Some(TOKEN.to_owned()),
            Some(fixture.models_file()),
            Some(fixture.legacy_config(true)),
        ),
        store,
    )
    .await;

    let health = request(server.address, "GET", "/healthz", None, None).await;
    assert_eq!(health.status, 200);
    assert_eq!(health.json()["status"], "ok");
    assert_eq!(health.json()["native_execution_qualified"], false);

    for path in ["/v1/fleet", "/v1/models"] {
        let absent = request(server.address, "GET", path, None, None).await;
        assert_eq!(absent.status, 401, "unauthenticated {path} must deny");
        let wrong = request(server.address, "GET", path, Some("wrong-token"), None).await;
        assert_eq!(wrong.status, 401, "wrong bearer for {path} must deny");
    }

    let fleet = request(server.address, "GET", "/v1/fleet", Some(TOKEN), None).await;
    assert_eq!(fleet.status, 200);
    assert_eq!(fleet.json()["simulated"], false);
    assert_eq!(fleet.json()["native_execution_qualified"], false);

    let models = request(server.address, "GET", "/v1/models", Some(TOKEN), None).await;
    assert_eq!(models.status, 200);
    assert_eq!(models.json()["native_execution_qualified"], false);
    assert_eq!(models.json()["revision"], "9007199254740993");
    assert_eq!(
        models.json()["models"]
            .as_array()
            .expect("model array")
            .len(),
        1
    );
    assert_eq!(models.json()["models"][0]["id"], "fixture-model");
    assert!(models.json()["models"][0].get("endpoint").is_none());
    assert!(
        models.json()["models"][0]
            .get("credential_file_ref")
            .is_none()
    );

    let selection = br#"{"request":{"required_capabilities":["coding"]},"capacity":{}}"#;
    let unauthorized_select = request(
        server.address,
        "POST",
        "/v1/models/select",
        None,
        Some(selection),
    )
    .await;
    assert_eq!(unauthorized_select.status, 401);
    let selected = request(
        server.address,
        "POST",
        "/v1/models/select",
        Some(TOKEN),
        Some(selection),
    )
    .await;
    assert_eq!(selected.status, 200);
    assert_eq!(selected.json()["status"], "selected");
    assert_eq!(selected.json()["model_id"], "fixture-model");
    assert!(selected.json().get("native_execution_qualified").is_none());
    assert!(selected.json().get("execution_granted").is_none());

    // Public legacy access is only the explicitly configured read projection.
    let legacy = request(server.address, "GET", "/v1/legacy", None, None).await;
    assert_eq!(legacy.status, 200);
    assert_eq!(legacy.json()["projects"]["fixture"]["status"], "read-only");
    let brief = request(server.address, "GET", "/v1/legacy/brief", None, None).await;
    assert_eq!(brief.status, 200);
    assert_eq!(brief.json()["totals"]["git_missing"], 1);
    assert!(brief.body.len() <= admiralty_server::fleet_brief::MAX_BRIEF_BYTES);
    let unknown = request(server.address, "GET", "/v1/not-a-route", Some(TOKEN), None).await;
    assert_eq!(unknown.status, 404);

    server.stop().await;
}

#[tokio::test]
async fn private_brief_has_the_same_bearer_boundary_as_its_source() {
    let fixture = Fixture::new();
    let mut server = TestServer::start(
        config(
            false,
            Some(TOKEN.to_owned()),
            None,
            Some(fixture.legacy_config(false)),
        ),
        fixture.store(),
    )
    .await;
    for path in ["/v1/legacy", "/v1/legacy/brief"] {
        assert_eq!(
            request(server.address, "GET", path, None, None)
                .await
                .status,
            401
        );
        assert_eq!(
            request(server.address, "GET", path, Some("wrong"), None)
                .await
                .status,
            401
        );
        assert_eq!(
            request(server.address, "GET", path, Some(TOKEN), None)
                .await
                .status,
            200
        );
    }
    server.stop().await;
}

#[tokio::test]
async fn demo_is_labeled_and_rejects_real_registry_or_legacy_attachments() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let mut demo = TestServer::start(config(true, None, None, None), store.clone()).await;
    let fleet = request(demo.address, "GET", "/v1/fleet", None, None).await;
    assert_eq!(fleet.status, 200);
    assert_eq!(fleet.json()["simulated"], true);
    assert_eq!(fleet.json()["native_execution_qualified"], false);
    demo.stop().await;

    let with_models = app(
        store.clone(),
        config(true, None, Some(fixture.models_file()), None),
    );
    assert!(
        with_models.is_err(),
        "demo must reject a real model registry"
    );

    let with_legacy = app(
        store,
        config(true, None, None, Some(fixture.legacy_config(false))),
    );
    assert!(with_legacy.is_err(), "demo must reject real legacy sources");
}
