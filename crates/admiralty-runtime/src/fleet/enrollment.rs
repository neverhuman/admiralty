use super::{decode, digest, invalid, json};
use admiralty_core::nodes::{
    DecimalU64, EnrollmentEvent, EnrollmentToken, NodeId, NodeIdentity, NodeRegistry, NodeScope,
    reduce_enrollment_for,
};
use rcgen::{CertificateParams, ExtendedKeyUsagePurpose, KeyPair, PublicKeyData};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};
use uuid::Uuid;

const MAX_STATE: u64 = 16 * 1024 * 1024;

/// Private node key material. Deliberately has no Debug implementation.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub identity: NodeIdentity,
    pub certificate: Vec<u8>,
    pub(crate) key: Vec<u8>,
}

impl Credentials {
    /// Install the controller-returned public identity after rotation/re-enrollment.
    /// The private key and certificate must already match that exact identity.
    pub fn adopt(root: &Path, identity: NodeIdentity) -> io::Result<()> {
        private_directory(root)?;
        let lock = private_file(&root.join("identity.lock"))?;
        fs2::FileExt::lock_exclusive(&lock)?;
        let mut credentials = Self::load(root)?;
        credentials.identity = identity;
        credentials.validate()?;
        let mut staged = tempfile::NamedTempFile::new_in(root)?;
        staged.write_all(&json(&credentials)?)?;
        staged.as_file().sync_all()?;
        staged
            .persist(root.join("identity.json"))
            .map_err(invalid)?;
        File::open(root)?.sync_all()
    }
    pub fn load(root: &Path) -> io::Result<Self> {
        private_directory(root)?;
        let path = root.join("identity.json");
        if !path.try_exists()? {
            return Err(invalid("node identity has not been initialized"));
        }
        let file = private_file(&path)?;
        if file.metadata()?.len() > MAX_STATE {
            return Err(invalid("identity too large"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_STATE + 1).read_to_end(&mut bytes)?;
        let credentials: Self = decode(&bytes)?;
        credentials.validate()?;
        Ok(credentials)
    }
    pub fn generate(name: &str, capability_digest: String) -> io::Result<Self> {
        let key = KeyPair::generate().map_err(invalid)?;
        let mut parameters = CertificateParams::new(vec![name.to_owned()]).map_err(invalid)?;
        parameters.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ClientAuth,
            ExtendedKeyUsagePurpose::ServerAuth,
        ];
        let certificate = parameters
            .self_signed(&key)
            .map_err(invalid)?
            .der()
            .to_vec();
        Ok(Self {
            identity: NodeIdentity {
                node_id: NodeId::new(Uuid::new_v4().to_string()).map_err(invalid)?,
                boot_id: Uuid::new_v4().to_string(),
                public_key_digest: digest(&key.subject_public_key_info()),
                generation: DecimalU64::new(1),
                capability_digest,
            },
            certificate,
            key: key.serialize_der(),
        })
    }

    /// Create exactly once, or reload the same UUID/key/certificate on restart.
    pub fn load_or_create(root: &Path, name: &str, capability_digest: String) -> io::Result<Self> {
        private_directory(root)?;
        let lock = private_file(&root.join("identity.lock"))?;
        fs2::FileExt::lock_exclusive(&lock)?;
        let path = root.join("identity.json");
        if path.try_exists()? {
            return Self::load(root);
        }
        let credentials = Self::generate(name, capability_digest)?;
        let mut staged = tempfile::NamedTempFile::new_in(root)?;
        staged.write_all(&json(&credentials)?)?;
        staged.as_file().sync_all()?;
        staged.persist_noclobber(&path).map_err(invalid)?;
        File::open(root)?.sync_all()?;
        Ok(credentials)
    }

    pub fn validate(&self) -> io::Result<()> {
        let key = KeyPair::try_from(self.key.as_slice()).map_err(invalid)?;
        if digest(&key.subject_public_key_info()) != self.identity.public_key_digest
            || certificate_key_digest(&self.certificate)? != self.identity.public_key_digest
        {
            return Err(invalid("key/certificate identity mismatch"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    pub cpu_millis: u32,
    pub memory_bytes: DecimalU64,
    pub spool_bytes: DecimalU64,
    pub runners: Vec<String>,
}

impl Inventory {
    pub(crate) fn validate(&self) -> io::Result<()> {
        if self.cpu_millis == 0
            || self.memory_bytes.parse().map_err(invalid)? == 0
            || self.spool_bytes.parse().map_err(invalid)? == 0
            || self.runners.len() > 32
            || self.runners.iter().any(|r| {
                r.is_empty()
                    || r.len() > 64
                    || !r
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            })
        {
            return Err(invalid("invalid node capacity/runner inventory"));
        }
        Ok(())
    }
}

/// SQLite serializes enrollment, revocation and receipt turns across processes.
pub struct Controller {
    pub(crate) db: Connection,
}

impl Controller {
    pub fn open(root: &Path) -> io::Result<Self> {
        private_directory(root)?;
        private_file(&root.join("fleet.sqlite"))?;
        let db = Connection::open(root.join("fleet.sqlite")).map_err(invalid)?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(invalid)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS registry (id INTEGER PRIMARY KEY CHECK(id=1), state BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS credentials (node TEXT PRIMARY KEY, certificate BLOB NOT NULL, inventory BLOB NOT NULL);
            CREATE TABLE IF NOT EXISTS observations (stream TEXT NOT NULL, seq TEXT NOT NULL, payload BLOB NOT NULL, PRIMARY KEY(stream,seq));
            CREATE TABLE IF NOT EXISTS transfers (id TEXT PRIMARY KEY, manifest BLOB NOT NULL, data BLOB NOT NULL, complete INTEGER NOT NULL);") .map_err(invalid)?;
        db.execute(
            "INSERT OR IGNORE INTO registry VALUES(1,?1)",
            [json(&NodeRegistry::default())?],
        )
        .map_err(invalid)?;
        File::open(root)?.sync_all()?;
        Ok(Self { db })
    }

    pub fn registry(&self) -> io::Result<NodeRegistry> {
        read_registry(&self.db)
    }

    /// Return the secret only to the local operator; persist only its digest.
    pub fn issue(
        &mut self,
        scope: NodeScope,
        now: u64,
        lifetime: u64,
    ) -> io::Result<(String, String)> {
        if lifetime == 0 || lifetime > 3600 {
            return Err(invalid("enrollment lifetime must be 1..3600 seconds"));
        }
        let id = Uuid::new_v4().to_string();
        let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let token = EnrollmentToken {
            token_id: id.clone(),
            token_digest: digest(secret.as_bytes()),
            scope: scope.clone(),
            expires_tick: DecimalU64::new(
                now.checked_add(lifetime)
                    .ok_or_else(|| invalid("clock overflow"))?,
            ),
            consumed_binding: None,
        };
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(invalid)?;
        let next = reduce_enrollment_for(
            &read_registry(&tx)?,
            &EnrollmentEvent::TokenIssued {
                event_id: format!("issue-{id}"),
                token,
            },
            &scope,
            &scope.authority,
            Some(now),
        )
        .map_err(invalid)?;
        save_registry(&tx, &next)?;
        tx.commit().map_err(invalid)?;
        Ok((id, secret))
    }

    /// Bootstrap requires the scoped one-time secret and explicit certificate pin.
    pub fn enroll(
        &mut self,
        token_id: &str,
        secret: &str,
        identity: &NodeIdentity,
        certificate: &[u8],
        inventory: &Inventory,
        now: u64,
    ) -> io::Result<()> {
        inventory.validate()?;
        if identity.capability_digest != digest(&json(inventory)?)
            || certificate_key_digest(certificate)? != identity.public_key_digest
        {
            return Err(invalid("certificate/inventory binding mismatch"));
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(invalid)?;
        let state = read_registry(&tx)?;
        let token = state
            .tokens
            .get(token_id)
            .ok_or_else(|| invalid("unknown enrollment token"))?;
        if secret.len() != 64 || digest(secret.as_bytes()) != token.token_digest {
            return Err(invalid("invalid enrollment secret"));
        }
        // An identical already-consumed binding may retry after an ACK is lost.
        if let Some(binding) = &token.consumed_binding {
            let pinned: Option<(Vec<u8>, Vec<u8>)> = tx
                .query_row(
                    "SELECT certificate,inventory FROM credentials WHERE node=?1",
                    [identity.node_id.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(invalid)?;
            if binding.node_id == identity.node_id
                && state.nodes.get(&identity.node_id) == Some(identity)
                && pinned == Some((certificate.to_vec(), json(inventory)?))
            {
                return Ok(());
            }
            return Err(invalid("enrollment token already consumed"));
        }
        let event = EnrollmentEvent::Enrolled {
            event_id: format!("enroll-{token_id}"),
            token_id: token_id.into(),
            scope: token.scope.clone(),
            identity: identity.clone(),
        };
        let next = reduce_enrollment_for(
            &state,
            &event,
            &token.scope,
            &token.scope.authority,
            Some(now),
        )
        .map_err(invalid)?;
        tx.execute("INSERT INTO credentials VALUES(?1,?2,?3) ON CONFLICT(node) DO UPDATE SET certificate=excluded.certificate,inventory=excluded.inventory", params![identity.node_id.as_str(), certificate, json(inventory)?]).map_err(invalid)?;
        save_registry(&tx, &next)?;
        tx.commit().map_err(invalid)
    }

    pub fn revoke(&mut self, node: &NodeId, now: u64) -> io::Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(invalid)?;
        let state = read_registry(&tx)?;
        let identity = state
            .nodes
            .get(node)
            .ok_or_else(|| invalid("unknown node"))?;
        let scope = state
            .scopes
            .get(node)
            .ok_or_else(|| invalid("unknown scope"))?;
        let next = reduce_enrollment_for(
            &state,
            &EnrollmentEvent::Revoked {
                event_id: Uuid::new_v4().to_string(),
                node_id: node.clone(),
                expected_generation: identity.generation.clone(),
            },
            scope,
            &scope.authority,
            Some(now),
        )
        .map_err(invalid)?;
        save_registry(&tx, &next)?;
        tx.commit().map_err(invalid)
    }

    pub fn rotate(
        &mut self,
        node: &NodeId,
        certificate: &[u8],
        now: u64,
    ) -> io::Result<NodeIdentity> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(invalid)?;
        let state = read_registry(&tx)?;
        let identity = state
            .nodes
            .get(node)
            .ok_or_else(|| invalid("unknown node"))?;
        let scope = state
            .scopes
            .get(node)
            .ok_or_else(|| invalid("unknown scope"))?;
        let next = reduce_enrollment_for(
            &state,
            &EnrollmentEvent::Rotated {
                event_id: Uuid::new_v4().to_string(),
                node_id: node.clone(),
                expected_generation: identity.generation.clone(),
                new_public_key_digest: certificate_key_digest(certificate)?,
            },
            scope,
            &scope.authority,
            Some(now),
        )
        .map_err(invalid)?;
        tx.execute(
            "UPDATE credentials SET certificate=?2 WHERE node=?1",
            params![node.as_str(), certificate],
        )
        .map_err(invalid)?;
        let rotated = next
            .nodes
            .get(node)
            .ok_or_else(|| invalid("rotation lost node"))?
            .clone();
        save_registry(&tx, &next)?;
        tx.commit().map_err(invalid)?;
        Ok(rotated)
    }

    pub(crate) fn authorize(
        db: &Connection,
        certificate: &[u8],
        identity: &NodeIdentity,
        scope: &NodeScope,
    ) -> io::Result<()> {
        let state = read_registry(db)?;
        let pinned: Option<Vec<u8>> = db
            .query_row(
                "SELECT certificate FROM credentials WHERE node=?1",
                [identity.node_id.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(invalid)?;
        if state.nodes.get(&identity.node_id) != Some(identity)
            || state.scopes.get(&identity.node_id) != Some(scope)
            || pinned.as_deref() != Some(certificate)
        {
            return Err(invalid("revoked, stale or foreign node"));
        }
        Ok(())
    }

    pub fn certificates(&self) -> io::Result<Vec<Vec<u8>>> {
        let state = self.registry()?;
        let mut certificates = Vec::new();
        for node in state.nodes.keys() {
            certificates.push(
                self.db
                    .query_row(
                        "SELECT certificate FROM credentials WHERE node=?1",
                        [node.as_str()],
                        |r| r.get(0),
                    )
                    .map_err(invalid)?,
            );
        }
        Ok(certificates)
    }
}

fn read_registry(db: &Connection) -> io::Result<NodeRegistry> {
    let bytes: Vec<u8> = db
        .query_row("SELECT state FROM registry WHERE id=1", [], |r| r.get(0))
        .map_err(invalid)?;
    if bytes.len() as u64 > MAX_STATE {
        return Err(invalid("registry bound exceeded"));
    }
    decode(&bytes)
}

fn save_registry(db: &Connection, registry: &NodeRegistry) -> io::Result<()> {
    db.execute("UPDATE registry SET state=?1 WHERE id=1", [json(registry)?])
        .map_err(invalid)?;
    Ok(())
}

fn certificate_key_digest(bytes: &[u8]) -> io::Result<String> {
    if bytes.len() > 16 * 1024 {
        return Err(invalid("certificate too large"));
    }
    let (rest, cert) = x509_parser::parse_x509_certificate(bytes).map_err(invalid)?;
    if !rest.is_empty() {
        return Err(invalid("trailing certificate bytes"));
    }
    Ok(digest(cert.public_key().raw))
}

pub(crate) fn private_directory(root: &Path) -> io::Result<()> {
    if !root.try_exists()? {
        fs::DirBuilder::new().mode(0o700).create(root)?;
    }
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(invalid("fleet state must be an owned private directory"));
    }
    Ok(())
}

pub(crate) fn private_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(invalid("fleet state file is not private and singly linked"));
    }
    Ok(file)
}
