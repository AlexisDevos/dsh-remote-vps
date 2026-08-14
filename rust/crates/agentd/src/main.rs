use anyhow::Result;
use clap::{Parser, Subcommand};
use dsh_protocol::{
    dsh::remote::v1::{
        remote_agent_server::{RemoteAgent, RemoteAgentServer},
        CapabilitiesResponse, GetCapabilitiesRequest, HealthRequest, HealthResponse,
    },
    PROTOCOL_VERSION,
};
use tonic::{transport::Server, Request, Response, Status};
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "dsh-agentd", version, about = "Rust daemon for dsh-remote-vps")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Serve the initial gRPC health/capabilities contract.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7443")]
        listen: String,
    },
}

#[derive(Debug, Default)]
struct AgentService;

#[tonic::async_trait]
impl RemoteAgent for AgentService {
    async fn get_capabilities(
        &self,
        request: Request<GetCapabilitiesRequest>,
    ) -> Result<Response<CapabilitiesResponse>, Status> {
        let client_version = request.into_inner().client_version;
        info!(%client_version, "capabilities requested");

        Ok(Response::new(CapabilitiesResponse {
            agent_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
            features: vec!["health".to_owned(), "capabilities".to_owned()],
        }))
    }

    async fn health(
        &self,
        request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let request_id = request.into_inner().request_id;
        info!(%request_id, "health requested");

        Ok(Response::new(HealthResponse {
            status: "ok".to_owned(),
            agent_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
        }))
    }
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dsh_agentd=info".into()),
        )
        .init();
}

async fn serve(listen: String) -> Result<()> {
    let address = listen.parse()?;
    info!(%address, protocol = PROTOCOL_VERSION, "agent listening");

    Server::builder()
        .add_service(RemoteAgentServer::new(AgentService))
        .serve(address)
        .await?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    match Cli::parse().command {
        Command::Serve { listen } => serve(listen).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_protocol::dsh::remote::v1::remote_agent_server::RemoteAgent;

    #[tokio::test]
    async fn health_returns_protocol_identity() {
        let response = AgentService
            .health(Request::new(HealthRequest {
                request_id: "test-request".to_owned(),
            }))
            .await
            .expect("health response")
            .into_inner();

        assert_eq!(response.status, "ok");
        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
    }
}
