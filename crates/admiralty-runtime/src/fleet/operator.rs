use super::{
    decode, digest,
    enrollment::{Controller, Credentials, Inventory, private_directory},
    invalid, json,
    transport::{self, Request, Response, Spool},
};
use admiralty_core::nodes::{NodeId, NodeIdentity};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Args)]
pub struct NodeArgs {
    #[command(subcommand)]
    command: NodeCommand,
}

#[derive(Subcommand)]
enum NodeCommand {
    /// Adopt the controller-returned public identity for this matching key.
    Adopt {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        identity: PathBuf,
    },
    /// Create persistent keys and public enrollment request without network access.
    Init {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        inventory: PathBuf,
        #[arg(long, default_value = "localhost")]
        name: String,
    },
    /// Issue a scoped, expiring token into a new private file.
    Issue {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        scope: PathBuf,
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long, default_value_t = 300)]
        lifetime: u64,
    },
    /// Consume a locally supplied token and pin the node certificate.
    Enroll {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long)]
        request: PathBuf,
    },
    /// Revoke current generation, including existing connections.
    Revoke {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        node: String,
    },
    /// Replace a node certificate and print its new public identity.
    Rotate {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        node: String,
        #[arg(long)]
        request: PathBuf,
    },
    /// Receive observations/artifacts using mandatory mTLS. Does not execute work.
    Serve {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        identity: PathBuf,
        #[arg(long)]
        listen: SocketAddr,
    },
    /// Persist an observation in the local outbox.
    Enqueue {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        identity: PathBuf,
        #[arg(long)]
        scope: PathBuf,
        #[arg(long)]
        payload: PathBuf,
    },
    /// Deliver the outbox to an explicitly pinned controller.
    Flush {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        identity: PathBuf,
        #[arg(long)]
        controller: SocketAddr,
        #[arg(long)]
        controller_request: PathBuf,
        #[arg(long, default_value = "localhost")]
        name: String,
    },
    /// Authorize one exact inbound artifact under local custody.
    GrantTransfer {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
    },
    /// Send one resumable chunk to an explicitly pinned controller.
    Transfer {
        #[arg(long)]
        identity: PathBuf,
        #[arg(long)]
        controller: SocketAddr,
        #[arg(long)]
        controller_request: PathBuf,
        #[arg(long)]
        chunk: PathBuf,
        #[arg(long, default_value = "localhost")]
        name: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentRequest {
    identity: NodeIdentity,
    certificate: Vec<u8>,
    inventory: Inventory,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenFile {
    token_id: String,
    secret: String,
}

fn read(path: &Path) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > transport::MAX_FRAME as u64 {
        return Err(invalid("input must be a bounded regular file"));
    }
    let mut bytes = Vec::new();
    file.take(transport::MAX_FRAME as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > transport::MAX_FRAME {
        return Err(invalid("input exceeds frame bound"));
    }
    Ok(bytes)
}
fn now() -> io::Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(invalid)?
        .as_secs())
}
fn output(value: &impl Serialize) -> io::Result<()> {
    io::stdout().write_all(&json(value)?)
}
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("output parent required"))?;
    private_directory(parent)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::File::open(parent)?.sync_all()
}

pub fn run(args: NodeArgs) -> io::Result<()> {
    match args.command {
        NodeCommand::Adopt { state, identity } => {
            let identity: NodeIdentity = decode(&read(&identity)?)?;
            let path = state.join("enrollment.json");
            let mut request: EnrollmentRequest = decode(&read(&path)?)?;
            if identity.capability_digest != digest(&json(&request.inventory)?) {
                return Err(invalid("replacement inventory binding mismatch"));
            }
            Credentials::adopt(&state, identity.clone())?;
            request.identity = identity;
            let mut staged = tempfile::NamedTempFile::new_in(&state)?;
            staged.write_all(&json(&request)?)?;
            staged.as_file().sync_all()?;
            staged.persist(path).map_err(invalid)?;
            fs::File::open(state)?.sync_all()
        }
        NodeCommand::Init {
            state,
            inventory,
            name,
        } => {
            let inventory: Inventory = decode(&read(&inventory)?)?;
            inventory.validate()?;
            let credentials =
                Credentials::load_or_create(&state, &name, digest(&json(&inventory)?))?;
            if credentials.identity.capability_digest != digest(&json(&inventory)?) {
                return Err(invalid("inventory differs from persistent identity"));
            }
            let request = EnrollmentRequest {
                identity: credentials.identity,
                certificate: credentials.certificate,
                inventory,
            };
            let path = state.join("enrollment.json");
            let bytes = json(&request)?;
            if path.try_exists()? {
                if read(&path)? != bytes {
                    return Err(invalid("enrollment request changed"));
                }
            } else {
                write_new(&path, &bytes)?;
            }
            output(&request.identity)
        }
        NodeCommand::Issue {
            state,
            scope,
            token_file,
            lifetime,
        } => {
            let (token_id, secret) =
                Controller::open(&state)?.issue(decode(&read(&scope)?)?, now()?, lifetime)?;
            write_new(&token_file, &json(&TokenFile { token_id, secret })?)
        }
        NodeCommand::Enroll {
            state,
            token_file,
            request,
        } => {
            let token: TokenFile = decode(&read(&token_file)?)?;
            let request: EnrollmentRequest = decode(&read(&request)?)?;
            Controller::open(&state)?.enroll(
                &token.token_id,
                &token.secret,
                &request.identity,
                &request.certificate,
                &request.inventory,
                now()?,
            )?;
            output(&request.identity)
        }
        NodeCommand::Revoke { state, node } => {
            Controller::open(&state)?.revoke(&NodeId::new(node).map_err(invalid)?, now()?)
        }
        NodeCommand::Rotate {
            state,
            node,
            request,
        } => {
            let request: EnrollmentRequest = decode(&read(&request)?)?;
            output(&Controller::open(&state)?.rotate(
                &NodeId::new(node).map_err(invalid)?,
                &request.certificate,
                now()?,
            )?)
        }
        NodeCommand::Serve {
            state,
            identity,
            listen,
        } => {
            let credentials = Credentials::load(&identity)?;
            let mut controller = Controller::open(&state)?;
            let listener = TcpListener::bind(listen)?;
            for socket in listener.incoming() {
                let config = transport::server_config(&credentials, controller.certificates()?)?;
                if let Err(error) = transport::serve_once(socket?, config, &mut controller) {
                    eprintln!("node receipt rejected: {}", error.kind());
                }
            }
            Ok(())
        }
        NodeCommand::Enqueue {
            state,
            identity,
            scope,
            payload,
        } => {
            let credentials = Credentials::load(&identity)?;
            let observation = Spool::open(&state)?.append(
                credentials.identity,
                decode(&read(&scope)?)?,
                read(&payload)?,
            )?;
            output(&observation.sequence)
        }
        NodeCommand::Flush {
            state,
            identity,
            controller,
            controller_request,
            name,
        } => {
            let credentials = Credentials::load(&identity)?;
            let pinned: EnrollmentRequest = decode(&read(&controller_request)?)?;
            let config = transport::client_config(&credentials, pinned.certificate)?;
            let mut spool = Spool::open(&state)?;
            for observation in spool.pending()? {
                let mut stream = transport::connect(controller, name.clone(), config.clone())?;
                transport::write_frame(&mut stream, &Request::Observe(observation))?;
                match transport::read_frame(&mut stream)? {
                    Response::Ack(ack) => {
                        spool.acknowledge(&ack)?;
                        if ack.replay_from.is_some() {
                            return Err(invalid("controller requested earlier durable sequence"));
                        }
                    }
                    _ => return Err(invalid("unexpected node reply")),
                }
            }
            Ok(())
        }
        NodeCommand::GrantTransfer {
            state,
            request,
            manifest,
        } => {
            let request: EnrollmentRequest = decode(&read(&request)?)?;
            Controller::open(&state)?
                .grant_transfer(&request.certificate, &decode(&read(&manifest)?)?)
        }
        NodeCommand::Transfer {
            identity,
            controller,
            controller_request,
            chunk,
            name,
        } => {
            let credentials = Credentials::load(&identity)?;
            let pinned: EnrollmentRequest = decode(&read(&controller_request)?)?;
            let mut stream = transport::connect(
                controller,
                name,
                transport::client_config(&credentials, pinned.certificate)?,
            )?;
            transport::write_frame(&mut stream, &Request::Transfer(decode(&read(&chunk)?)?))?;
            match transport::read_frame(&mut stream)? {
                Response::Transfer(ack) => output(&ack),
                _ => Err(invalid("unexpected transfer reply")),
            }
        }
    }
}
