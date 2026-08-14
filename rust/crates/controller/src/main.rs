use anyhow::Result;
use clap::{Parser, Subcommand};
use dsh_protocol::dsh::remote::v1::{remote_agent_client::RemoteAgentClient, HealthRequest};
use tonic::{transport::Channel, Request};

#[derive(Debug, Parser)]
#[command(
    name = "dsh-controller",
    version,
    about = "Rust controller for dsh-remote-vps"
)]
struct Cli {
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
}

async fn health(endpoint: String) -> Result<()> {
    let mut client = RemoteAgentClient::connect(endpoint.clone()).await?;
    let response = client
        .health(Request::new(HealthRequest {
            request_id: format!("controller-{}", std::process::id()),
        }))
        .await?
        .into_inner();

    println!(
        "status={} agent_version={} protocol_version={} endpoint={}",
        response.status, response.agent_version, response.protocol_version, endpoint
    );

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Health { endpoint } => health(endpoint).await,
    }
}

#[allow(dead_code)]
fn _channel_type_is_kept_explicit(_: Channel) {}
