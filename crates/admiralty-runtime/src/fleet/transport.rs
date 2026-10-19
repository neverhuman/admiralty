//! TLS 1.3, bounded frames and persist-before-ACK observation delivery.
use super::{
    decode,
    enrollment::{Controller, Credentials},
    invalid, json,
};
use admiralty_core::nodes::{DecimalU64, NodeIdentity, NodeScope};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName},
    server::WebPkiClientVerifier,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

pub const PROTOCOL: u32 = 1;
pub const MAX_FRAME: usize = 64 * 1024;
pub const MAX_PAYLOAD: usize = 8 * 1024;
pub const MAX_RECEIPTS: u64 = 4096;
const ALPN: &[u8] = b"admiralty-node/1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub protocol: u32,
    pub identity: NodeIdentity,
    pub scope: NodeScope,
    pub producer: String,
    pub sequence: DecimalU64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ack {
    pub stream: String,
    pub through: DecimalU64,
    pub replay_from: Option<DecimalU64>,
}

pub fn server_config(
    credentials: &Credentials,
    client_certificates: Vec<Vec<u8>>,
) -> io::Result<Arc<ServerConfig>> {
    credentials.validate()?;
    let mut roots = RootCertStore::empty();
    for cert in client_certificates {
        roots.add(CertificateDer::from(cert)).map_err(invalid)?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .map_err(invalid)?;
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(invalid)?
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![CertificateDer::from(credentials.certificate.clone())],
            PrivatePkcs8KeyDer::from(credentials.key.clone()).into(),
        )
        .map_err(invalid)?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    // Resumption must not skip the current certificate authentication turn.
    config.send_tls13_tickets = 0;
    Ok(Arc::new(config))
}

pub fn client_config(
    credentials: &Credentials,
    server_certificate: Vec<u8>,
) -> io::Result<Arc<ClientConfig>> {
    credentials.validate()?;
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(server_certificate))
        .map_err(invalid)?;
    let mut config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(invalid)?
            .with_root_certificates(roots)
            .with_client_auth_cert(
                vec![CertificateDer::from(credentials.certificate.clone())],
                PrivatePkcs8KeyDer::from(credentials.key.clone()).into(),
            )
            .map_err(invalid)?;
    config.alpn_protocols = vec![ALPN.to_vec()];
    Ok(Arc::new(config))
}

pub fn connect(
    address: SocketAddr,
    name: String,
    config: Arc<ClientConfig>,
) -> io::Result<StreamOwned<ClientConnection, TcpStream>> {
    let socket = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    deadlines(&socket)?;
    let mut stream = StreamOwned::new(
        ClientConnection::new(config, ServerName::try_from(name).map_err(invalid)?)
            .map_err(invalid)?,
        socket,
    );
    stream.conn.complete_io(&mut stream.sock)?;
    if stream.conn.alpn_protocol() != Some(ALPN) {
        return Err(invalid("incompatible node protocol"));
    }
    Ok(stream)
}

/// Handle one request. A caller may repeat this on the same authenticated stream;
/// authorization is reloaded inside each durable writer turn, including retries.
pub fn serve_once(
    socket: TcpStream,
    config: Arc<ServerConfig>,
    controller: &mut Controller,
) -> io::Result<()> {
    deadlines(&socket)?;
    let mut stream = StreamOwned::new(ServerConnection::new(config).map_err(invalid)?, socket);
    stream.conn.complete_io(&mut stream.sock)?;
    if stream.conn.alpn_protocol() != Some(ALPN) {
        return Err(invalid("incompatible node protocol"));
    }
    let certificate = stream
        .conn
        .peer_certificates()
        .and_then(|chain| chain.first())
        .ok_or_else(|| invalid("client certificate required"))?
        .as_ref()
        .to_vec();
    let request: Request = read_frame(&mut stream)?;
    let response = match request {
        Request::Observe(observation) => {
            Response::Ack(controller.receive(&certificate, &observation)?)
        }
        Request::Transfer(chunk) => Response::Transfer(controller.transfer(&certificate, &chunk)?),
    };
    write_frame(&mut stream, &response)?;
    stream.conn.send_close_notify();
    stream.flush()
}

#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Request {
    Observe(Observation),
    Transfer(super::transfer::Chunk),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Response {
    Ack(Ack),
    Transfer(super::transfer::TransferAck),
}

pub fn write_frame(stream: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    let bytes = json(value)?;
    if bytes.len() > MAX_FRAME {
        return Err(invalid("node frame exceeds limit"));
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()
}

pub fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut impl Read) -> io::Result<T> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(invalid("node frame exceeds limit"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    decode(&bytes)
}

fn deadlines(socket: &TcpStream) -> io::Result<()> {
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))
}

impl Controller {
    pub fn receive(&mut self, certificate: &[u8], observation: &Observation) -> io::Result<Ack> {
        if observation.protocol != PROTOCOL
            || observation.payload.len() > MAX_PAYLOAD
            || uuid::Uuid::parse_str(&observation.producer).is_err()
        {
            return Err(invalid("incompatible or oversized observation"));
        }
        let sequence = observation.sequence.parse().map_err(invalid)?;
        if sequence == 0 {
            return Err(invalid("sequence starts at 1"));
        }
        let stream = super::digest(&json(&(
            &observation.identity,
            &observation.scope,
            &observation.producer,
        ))?);
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(invalid)?;
        Self::authorize(&tx, certificate, &observation.identity, &observation.scope)?;
        // No holes are stored, so the count is the durable contiguous watermark.
        let through: u64 = tx
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE stream=?1",
                [&stream],
                |r| r.get::<_, u32>(0).map(u64::from),
            )
            .map_err(invalid)?;
        let next = through
            .checked_add(1)
            .ok_or_else(|| invalid("sequence overflow"))?;
        if sequence > next {
            return Ok(Ack {
                stream: stream.clone(),
                through: DecimalU64::new(through),
                replay_from: Some(DecimalU64::new(next)),
            });
        }
        if sequence <= through {
            let prior: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT payload FROM observations WHERE stream=?1 AND seq=?2",
                    params![stream, sequence.to_string()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(invalid)?;
            if prior.as_deref() != Some(observation.payload.as_slice()) {
                return Err(invalid("producer sequence payload collision"));
            }
        } else {
            let count: u64 = tx
                .query_row("SELECT COUNT(*) FROM observations", [], |r| {
                    r.get::<_, u32>(0).map(u64::from)
                })
                .map_err(invalid)?;
            if count >= MAX_RECEIPTS {
                return Err(invalid("durable observation spool full"));
            }
            tx.execute(
                "INSERT INTO observations VALUES(?1,?2,?3)",
                params![stream, sequence.to_string(), observation.payload],
            )
            .map_err(invalid)?;
        }
        tx.commit().map_err(invalid)?;
        Ok(Ack {
            stream: stream.clone(),
            through: DecimalU64::new(through.max(sequence)),
            replay_from: None,
        })
    }
}

/// Persistent producer outbox. Rows survive reconnect/reboot until a scoped ACK.
pub struct Spool {
    db: rusqlite::Connection,
}

impl Spool {
    pub fn open(root: &std::path::Path) -> io::Result<Self> {
        super::enrollment::private_directory(root)?;
        super::enrollment::private_file(&root.join("spool.sqlite"))?;
        let db = rusqlite::Connection::open(root.join("spool.sqlite")).map_err(invalid)?;
        db.busy_timeout(Duration::from_secs(5)).map_err(invalid)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS producer(id INTEGER PRIMARY KEY CHECK(id=1), uuid TEXT NOT NULL, next TEXT NOT NULL, ack TEXT NOT NULL, binding BLOB);
            CREATE TABLE IF NOT EXISTS pending(seq TEXT PRIMARY KEY, observation BLOB NOT NULL);").map_err(invalid)?;
        db.execute(
            "INSERT OR IGNORE INTO producer VALUES(1,?1,'1','0',NULL)",
            [uuid::Uuid::new_v4().to_string()],
        )
        .map_err(invalid)?;
        std::fs::File::open(root)?.sync_all()?;
        Ok(Self { db })
    }

    pub fn append(
        &mut self,
        identity: NodeIdentity,
        scope: NodeScope,
        payload: Vec<u8>,
    ) -> io::Result<Observation> {
        if payload.len() > MAX_PAYLOAD {
            return Err(invalid("payload exceeds spool bound"));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(invalid)?;
        let count: u64 = tx
            .query_row("SELECT COUNT(*) FROM pending", [], |r| {
                r.get::<_, u32>(0).map(u64::from)
            })
            .map_err(invalid)?;
        if count >= 1024 {
            return Err(invalid("producer spool full"));
        }
        let (mut producer, mut next, binding): (String, String, Option<Vec<u8>>) = tx
            .query_row(
                "SELECT uuid,next,binding FROM producer WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(invalid)?;
        let current_binding = json(&(&identity, &scope))?;
        if binding.as_ref().is_some_and(|old| old != &current_binding) {
            if count != 0 {
                return Err(invalid(
                    "drain old incarnation before changing node identity",
                ));
            }
            producer = uuid::Uuid::new_v4().to_string();
            next = "1".into();
            tx.execute(
                "UPDATE producer SET uuid=?1,ack='0' WHERE id=1",
                [&producer],
            )
            .map_err(invalid)?;
        }
        let next: u64 = next.parse().map_err(invalid)?;
        let after = next
            .checked_add(1)
            .ok_or_else(|| invalid("sequence overflow"))?;
        tx.execute(
            "UPDATE producer SET binding=?1 WHERE id=1",
            [current_binding],
        )
        .map_err(invalid)?;
        let observation = Observation {
            protocol: PROTOCOL,
            identity,
            scope,
            producer,
            sequence: DecimalU64::new(next),
            payload,
        };
        tx.execute(
            "INSERT INTO pending VALUES(?1,?2)",
            params![next.to_string(), json(&observation)?],
        )
        .map_err(invalid)?;
        tx.execute(
            "UPDATE producer SET next=?1 WHERE id=1",
            [after.to_string()],
        )
        .map_err(invalid)?;
        tx.commit().map_err(invalid)?;
        Ok(observation)
    }

    pub fn pending(&self) -> io::Result<Vec<Observation>> {
        let mut statement = self
            .db
            .prepare("SELECT observation FROM pending ORDER BY length(seq),seq LIMIT 1024")
            .map_err(invalid)?;
        let rows = statement
            .query_map([], |r| r.get::<_, Vec<u8>>(0))
            .map_err(invalid)?;
        rows.map(|r| decode(&r.map_err(invalid)?)).collect()
    }

    /// Call only with an ACK obtained for this outbox over authenticated TLS.
    pub fn acknowledge(&mut self, ack: &Ack) -> io::Result<()> {
        let through = ack.through.parse().map_err(invalid)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(invalid)?;
        let (next, prior, producer, binding): (String, String, String, Vec<u8>) = tx
            .query_row(
                "SELECT next,ack,uuid,binding FROM producer WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(invalid)?;
        let (identity, scope): (NodeIdentity, NodeScope) = decode(&binding)?;
        if ack.stream != super::digest(&json(&(&identity, &scope, &producer))?) {
            return Err(invalid("ACK belongs to a different producer incarnation"));
        }
        if through >= next.parse::<u64>().map_err(invalid)?
            || through < prior.parse::<u64>().map_err(invalid)?
        {
            return Err(invalid("ACK outside producer window"));
        }
        tx.execute("DELETE FROM pending WHERE length(seq)<length(?1) OR (length(seq)=length(?1) AND seq<=?1)", [through.to_string()]).map_err(invalid)?;
        tx.execute(
            "UPDATE producer SET ack=?1 WHERE id=1",
            [through.to_string()],
        )
        .map_err(invalid)?;
        tx.commit().map_err(invalid)
    }
}
