use std::io::Write;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use dsh_controller::Controller;
use dsh_protocol::dsh::remote::v1::{exec_chunk::Payload as ExecPayload, NodeType, WriteMode};

#[derive(Debug, Parser)]
#[command(
    name = "dsh-controller",
    version,
    about = "Rust controller for dsh-remote-vps"
)]
struct Cli {
    /// Read a Bearer token from a local file and attach it to every RPC.
    #[arg(long, global = true, value_name = "FILE")]
    auth_token_file: Option<std::path::PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Call the health endpoint of an agent.
    Health {
        #[arg(long, default_value = "http://127.0.0.1:7443")]
        endpoint: String,
    },
    /// Print the feature set advertised by an agent.
    Capabilities {
        #[arg(long, default_value = "http://127.0.0.1:7443")]
        endpoint: String,
    },
    /// Stat a path relative to a configured root.
    Stat {
        #[arg(long, default_value = "http://127.0.0.1:7443")]
        endpoint: String,
        #[arg(long, default_value = "project")]
        root: String,
        path: String,
    },
    /// Read raw bytes from a remote file to stdout.
    Read {
        #[arg(long, default_value = "http://127.0.0.1:7443")]
        endpoint: String,
        #[arg(long, default_value = "project")]
        root: String,
        path: String,
        #[arg(long, default_value_t = 64 * 1024 * 1024)]
        max_bytes: u64,
    },
    /// List a remote directory.
    List {
        #[arg(long, default_value = "http://127.0.0.1:7443")]
        endpoint: String,
        #[arg(long, default_value = "project")]
        root: String,
        path: String,
    },
    /// Write a local file atomically to a remote path.
    Write {
        #[arg(long, default_value = "http://127.0.0.1:7443")]
        endpoint: String,
        #[arg(long, default_value = "project")]
        root: String,
        path: String,
        #[arg(long, value_name = "FILE")]
        file: std::path::PathBuf,
        #[arg(long, default_value = "replace-any")]
        mode: String,
        #[arg(long, default_value = "")]
        expected_version: String,
    },
    /// Replace text in a remote file.
    Edit {
        #[arg(long, default_value = "http://127.0.0.1:7443")]
        endpoint: String,
        #[arg(long, default_value = "project")]
        root: String,
        path: String,
        #[arg(long)]
        old: String,
        #[arg(long)]
        new: String,
        #[arg(long)]
        replace_all: bool,
        #[arg(long, default_value = "")]
        expected_version: String,
    },
    /// Search paths using a glob pattern.
    Glob {
        #[arg(long, default_value = "http://127.0.0.1:7443")]
        endpoint: String,
        #[arg(long, default_value = "project")]
        root: String,
        #[arg(long, default_value = ".")]
        base: String,
        pattern: String,
        #[arg(long)]
        include_hidden: bool,
    },
    /// Search UTF-8 files using a Rust regular expression.
    Grep {
        #[arg(long, default_value = "http://127.0.0.1:7443")]
        endpoint: String,
        #[arg(long, default_value = "project")]
        root: String,
        #[arg(long, default_value = ".")]
        base: String,
        pattern: String,
        #[arg(long)]
        case_sensitive: bool,
        #[arg(long)]
        include_hidden: bool,
    },
    /// Execute an explicitly allowlisted argv without an implicit shell.
    Exec {
        #[arg(long, default_value = "http://127.0.0.1:7443")]
        endpoint: String,
        #[arg(long, default_value = "project")]
        root: String,
        #[arg(long, default_value = ".")]
        cwd: String,
        #[arg(long, default_value_t = 30_000)]
        timeout_ms: u64,
        #[arg(long, default_value_t = 8 * 1024 * 1024)]
        max_output_bytes: u64,
        #[arg(required = true, last = true)]
        argv: Vec<String>,
    },
}

fn parse_write_mode(mode: &str) -> Result<WriteMode> {
    match mode {
        "create-if-absent" => Ok(WriteMode::CreateIfAbsent),
        "replace-if-version" => Ok(WriteMode::ReplaceIfVersion),
        "replace-any" => Ok(WriteMode::ReplaceAny),
        _ => Err(anyhow!(
            "unknown write mode: {mode}; use create-if-absent, replace-if-version or replace-any"
        )),
    }
}

fn node_type(value: i32) -> &'static str {
    match NodeType::try_from(value).unwrap_or(NodeType::Unspecified) {
        NodeType::File => "file",
        NodeType::Directory => "directory",
        NodeType::Symlink => "symlink",
        NodeType::Other => "other",
        NodeType::Unspecified => "unspecified",
    }
}

async fn connect(endpoint: impl Into<String>, auth_token: &Option<String>) -> Result<Controller> {
    Controller::connect_with_token(endpoint, auth_token.clone()).await
}

async fn run(command: Command, auth_token: Option<String>) -> Result<()> {
    match command {
        Command::Health { endpoint } => {
            let mut controller = connect(endpoint.clone(), &auth_token).await?;
            let response = controller.health().await?;
            println!(
                "status={} agent_version={} protocol_version={} endpoint={}",
                response.status, response.agent_version, response.protocol_version, endpoint
            );
        }
        Command::Capabilities { endpoint } => {
            let mut controller = connect(endpoint, &auth_token).await?;
            let response = controller.capabilities().await?;
            println!(
                "agent_version={} protocol_version={}",
                response.agent_version, response.protocol_version
            );
            for feature in response.features {
                println!("feature={feature}");
            }
        }
        Command::Stat {
            endpoint,
            root,
            path,
        } => {
            let mut controller = connect(endpoint, &auth_token).await?;
            let response = controller.stat(&root, &path).await?;
            println!(
                "type={} size={} version={}",
                node_type(response.r#type),
                response.size,
                response.version
            );
        }
        Command::Read {
            endpoint,
            root,
            path,
            max_bytes,
        } => {
            let mut controller = connect(endpoint, &auth_token).await?;
            let bytes = controller.read_bytes(&root, &path, max_bytes).await?;
            std::io::stdout().write_all(&bytes)?;
        }
        Command::List {
            endpoint,
            root,
            path,
        } => {
            let mut controller = connect(endpoint, &auth_token).await?;
            for entry in controller.list(&root, &path).await? {
                println!(
                    "{}\t{}\t{}\t{}",
                    entry.name,
                    node_type(entry.r#type),
                    entry.size,
                    entry.version
                );
            }
        }
        Command::Write {
            endpoint,
            root,
            path,
            file,
            mode,
            expected_version,
        } => {
            let mut controller = connect(endpoint, &auth_token).await?;
            let data = tokio::fs::read(file).await?;
            let response = controller
                .write_bytes(
                    &root,
                    &path,
                    data,
                    parse_write_mode(&mode)?,
                    &expected_version,
                )
                .await?;
            println!(
                "operation={} size={} version={}",
                response.operation, response.size, response.version
            );
        }
        Command::Edit {
            endpoint,
            root,
            path,
            old,
            new,
            replace_all,
            expected_version,
        } => {
            let mut controller = connect(endpoint, &auth_token).await?;
            let response = controller
                .edit(&root, &path, &old, &new, replace_all, &expected_version)
                .await?;
            println!("version={}", response.version);
            println!("before={}", response.before);
            println!("after={}", response.after);
        }
        Command::Glob {
            endpoint,
            root,
            base,
            pattern,
            include_hidden,
        } => {
            let mut controller = connect(endpoint, &auth_token).await?;
            for entry in controller
                .glob(&root, &base, &pattern, include_hidden)
                .await?
            {
                println!(
                    "{}\t{}\t{}",
                    entry.path,
                    node_type(entry.r#type),
                    entry.size
                );
            }
        }
        Command::Grep {
            endpoint,
            root,
            base,
            pattern,
            case_sensitive,
            include_hidden,
        } => {
            let mut controller = connect(endpoint, &auth_token).await?;
            for entry in controller
                .grep(&root, &base, &pattern, case_sensitive, include_hidden)
                .await?
            {
                println!(
                    "{}:{}:{}:{}",
                    entry.path, entry.line, entry.column, entry.text
                );
            }
        }
        Command::Exec {
            endpoint,
            root,
            cwd,
            timeout_ms,
            max_output_bytes,
            argv,
        } => {
            let mut controller = connect(endpoint, &auth_token).await?;
            let mut failure = None;
            for chunk in controller
                .exec(&root, &cwd, argv, timeout_ms, max_output_bytes)
                .await?
            {
                match chunk.payload {
                    Some(ExecPayload::Stdout(data)) => std::io::stdout().write_all(&data)?,
                    Some(ExecPayload::Stderr(data)) => std::io::stderr().write_all(&data)?,
                    Some(ExecPayload::Exit(exit)) => {
                        eprintln!(
                            "exit_code={} signal={} timed_out={} truncated={}",
                            exit.exit_code, exit.signal, exit.timed_out, exit.truncated
                        );
                        if exit.exit_code != 0 || exit.signal != 0 || exit.timed_out {
                            failure = Some(format!(
                                "remote command failed: exit_code={} signal={} timed_out={}",
                                exit.exit_code, exit.signal, exit.timed_out
                            ));
                        }
                    }
                    None => {}
                }
            }
            if let Some(error) = failure {
                return Err(anyhow!(error));
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let auth_token = cli
        .auth_token_file
        .map(std::fs::read_to_string)
        .transpose()?
        .map(|value| value.trim().to_owned());
    run(cli.command, auth_token).await
}
