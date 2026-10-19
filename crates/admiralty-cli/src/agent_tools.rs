use admiralty_runtime::agent_tools::{
    AgentClient, MAX_REQUEST_BYTES, ToolCall, serve_stdio, tool_catalog,
};
use clap::{Args, Subcommand};
use std::{
    fs::File,
    io::{self, Read},
    net::SocketAddr,
};

#[derive(Args)]
pub struct AgentToolsArgs {
    /// Explicit local authenticated agent API address.
    #[arg(long)]
    server: Option<SocketAddr>,
    /// Read the agent credential from a protected owner-only file instead of the environment.
    #[arg(long)]
    token_file: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: AgentToolsCommand,
}
#[derive(Subcommand)]
enum AgentToolsCommand {
    /// Print the same narrow tool schemas used by MCP without a server connection.
    List,
    /// Execute one JSON ToolCall from a bounded file.
    Call {
        #[arg(long)]
        file: std::path::PathBuf,
    },
    /// Serve MCP 2025-06-18 JSON-RPC on stdin/stdout.
    Mcp,
}
impl AgentToolsArgs {
    pub fn run(self) -> crate::CliResult<()> {
        if matches!(self.command, AgentToolsCommand::List) {
            return crate::print_json(tool_catalog());
        }
        let server = self
            .server
            .ok_or("--server is required for agent operations")?;
        let token = match self.token_file {
            Some(path) => admiralty_server::startup::read_private_token(&path)?,
            None => std::env::var("ADMIRALTY_AGENT_TOKEN")
                .map_err(|_| "--token-file or ADMIRALTY_AGENT_TOKEN is required")?,
        };
        let client = AgentClient::new(server, token)?;
        match self.command {
            AgentToolsCommand::List => unreachable!(),
            AgentToolsCommand::Call { file } => {
                let mut bytes = Vec::new();
                File::open(file)?
                    .take((MAX_REQUEST_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > MAX_REQUEST_BYTES {
                    return Err("agent request exceeds limit".into());
                }
                let call: ToolCall = serde_json::from_slice(&bytes)?;
                crate::print_json(client.call(&call)?)
            }
            AgentToolsCommand::Mcp => {
                serve_stdio(&client, io::stdin().lock(), io::stdout().lock())?;
                Ok(())
            }
        }
    }
}
