use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use dsh_protocol::{
    dsh::remote::v1::{
        remote_agent_client::RemoteAgentClient, write_frame::Payload, CapabilitiesResponse,
        DirEntry, EditRequest, EditResponse, ExecChunk, ExecRequest, GetCapabilitiesRequest,
        GlobMatch, GlobRequest, GrepMatch, GrepRequest, HealthRequest, HealthResponse, ListRequest,
        ReadChunk, ReadRequest, StatRequest, StatResponse, WriteFrame, WriteMode, WriteResponse,
    },
    PROTOCOL_VERSION,
};
use tonic::{
    metadata::{Ascii, MetadataValue},
    transport::Channel,
    Request, Status,
};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);
const WRITE_CHUNK_SIZE: usize = 64 * 1024;
const MAX_GRPC_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

fn request_id(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Reusable async client for the Rust agent protocol.
#[derive(Debug)]
pub struct Controller {
    endpoint: String,
    client: RemoteAgentClient<Channel>,
    auth_token: Option<MetadataValue<Ascii>>,
}

impl Controller {
    pub async fn connect(endpoint: impl Into<String>) -> Result<Self> {
        Self::connect_with_token(endpoint, None).await
    }

    pub async fn connect_with_token(
        endpoint: impl Into<String>,
        token: Option<String>,
    ) -> Result<Self> {
        let endpoint = endpoint.into();
        let client = RemoteAgentClient::connect(endpoint.clone())
            .await?
            .max_decoding_message_size(MAX_GRPC_MESSAGE_SIZE)
            .max_encoding_message_size(MAX_GRPC_MESSAGE_SIZE);
        let auth_token = token
            .filter(|value| !value.trim().is_empty())
            .map(|value| MetadataValue::try_from(format!("Bearer {}", value.trim())))
            .transpose()?;
        let mut controller = Self {
            endpoint,
            client,
            auth_token,
        };
        controller.ensure_protocol().await?;
        Ok(controller)
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn request<T>(&self, message: T) -> Request<T> {
        let mut request = Request::new(message);
        if let Some(token) = &self.auth_token {
            request
                .metadata_mut()
                .insert("authorization", token.clone());
        }
        request
    }

    async fn ensure_protocol(&mut self) -> Result<(), Status> {
        let response = self
            .client
            .health(self.request(HealthRequest {
                request_id: request_id("protocol"),
            }))
            .await?
            .into_inner();
        let expected_major = PROTOCOL_VERSION.split('.').next().unwrap_or_default();
        let actual_major = response
            .protocol_version
            .split('.')
            .next()
            .unwrap_or_default();
        if actual_major != expected_major {
            return Err(Status::failed_precondition(format!(
                "incompatible protocol major: controller={PROTOCOL_VERSION} agent={}",
                response.protocol_version
            )));
        }
        Ok(())
    }

    pub async fn health(&mut self) -> Result<HealthResponse, Status> {
        Ok(self
            .client
            .health(self.request(HealthRequest {
                request_id: request_id("health"),
            }))
            .await?
            .into_inner())
    }

    pub async fn capabilities(&mut self) -> Result<CapabilitiesResponse, Status> {
        Ok(self
            .client
            .get_capabilities(self.request(GetCapabilitiesRequest {
                client_version: env!("CARGO_PKG_VERSION").to_owned(),
            }))
            .await?
            .into_inner())
    }

    pub async fn stat(&mut self, root_id: &str, path: &str) -> Result<StatResponse, Status> {
        Ok(self
            .client
            .stat(self.request(StatRequest {
                request_id: request_id("stat"),
                root_id: root_id.to_owned(),
                path: path.to_owned(),
            }))
            .await?
            .into_inner())
    }

    pub async fn read_bytes(
        &mut self,
        root_id: &str,
        path: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, Status> {
        let mut stream = self
            .client
            .read(self.request(ReadRequest {
                request_id: request_id("read"),
                root_id: root_id.to_owned(),
                path: path.to_owned(),
                max_bytes,
            }))
            .await?
            .into_inner();
        let mut output = Vec::new();
        let mut reached_eof = false;
        while let Some(chunk) = stream.message().await? {
            if chunk.offset != output.len() as u64 {
                return Err(Status::data_loss("read stream offset mismatch"));
            }
            output.extend_from_slice(&chunk.data);
            if chunk.eof {
                reached_eof = true;
                break;
            }
        }
        if !reached_eof {
            return Err(Status::data_loss("read stream ended without eof"));
        }
        Ok(output)
    }

    pub async fn read_chunks(
        &mut self,
        root_id: &str,
        path: &str,
        max_bytes: u64,
    ) -> Result<Vec<ReadChunk>, Status> {
        let mut stream = self
            .client
            .read(self.request(ReadRequest {
                request_id: request_id("read"),
                root_id: root_id.to_owned(),
                path: path.to_owned(),
                max_bytes,
            }))
            .await?
            .into_inner();
        let mut chunks = Vec::new();
        while let Some(chunk) = stream.message().await? {
            let eof = chunk.eof;
            chunks.push(chunk);
            if eof {
                break;
            }
        }
        Ok(chunks)
    }

    pub async fn list(&mut self, root_id: &str, path: &str) -> Result<Vec<DirEntry>, Status> {
        let mut stream = self
            .client
            .list(self.request(ListRequest {
                request_id: request_id("list"),
                root_id: root_id.to_owned(),
                path: path.to_owned(),
                max_entries: 0,
            }))
            .await?
            .into_inner();
        let mut entries = Vec::new();
        while let Some(entry) = stream.message().await? {
            entries.push(entry);
        }
        Ok(entries)
    }

    pub async fn write_bytes(
        &mut self,
        root_id: &str,
        path: &str,
        data: Vec<u8>,
        mode: WriteMode,
        expected_version: &str,
    ) -> Result<WriteResponse, Status> {
        let request = WriteFrame {
            payload: Some(Payload::Start(dsh_protocol::dsh::remote::v1::WriteStart {
                request_id: request_id("write"),
                root_id: root_id.to_owned(),
                path: path.to_owned(),
                mode: mode as i32,
                expected_version: expected_version.to_owned(),
                declared_size: data.len() as u64,
            })),
        };
        let mut frames = Vec::with_capacity(1 + data.len().div_ceil(WRITE_CHUNK_SIZE));
        frames.push(request);
        frames.extend(data.chunks(WRITE_CHUNK_SIZE).map(|chunk| WriteFrame {
            payload: Some(Payload::Data(chunk.to_vec())),
        }));
        Ok(self
            .client
            .write(self.request(tokio_stream::iter(frames)))
            .await?
            .into_inner())
    }

    pub async fn edit(
        &mut self,
        root_id: &str,
        path: &str,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
        expected_version: &str,
    ) -> Result<EditResponse, Status> {
        Ok(self
            .client
            .edit(self.request(EditRequest {
                request_id: request_id("edit"),
                root_id: root_id.to_owned(),
                path: path.to_owned(),
                old_string: old_string.to_owned(),
                new_string: new_string.to_owned(),
                replace_all,
                expected_version: expected_version.to_owned(),
            }))
            .await?
            .into_inner())
    }

    pub async fn glob(
        &mut self,
        root_id: &str,
        base_path: &str,
        pattern: &str,
        include_hidden: bool,
    ) -> Result<Vec<GlobMatch>, Status> {
        let mut stream = self
            .client
            .glob(self.request(GlobRequest {
                request_id: request_id("glob"),
                root_id: root_id.to_owned(),
                base_path: base_path.to_owned(),
                pattern: pattern.to_owned(),
                max_results: 0,
                include_hidden,
            }))
            .await?
            .into_inner();
        let mut matches = Vec::new();
        while let Some(entry) = stream.message().await? {
            matches.push(entry);
        }
        Ok(matches)
    }

    pub async fn grep(
        &mut self,
        root_id: &str,
        base_path: &str,
        pattern: &str,
        case_sensitive: bool,
        include_hidden: bool,
    ) -> Result<Vec<GrepMatch>, Status> {
        let mut stream = self
            .client
            .grep(self.request(GrepRequest {
                request_id: request_id("grep"),
                root_id: root_id.to_owned(),
                base_path: base_path.to_owned(),
                pattern: pattern.to_owned(),
                max_results: 0,
                max_file_bytes: 0,
                case_sensitive,
                include_hidden,
            }))
            .await?
            .into_inner();
        let mut matches = Vec::new();
        while let Some(entry) = stream.message().await? {
            matches.push(entry);
        }
        Ok(matches)
    }

    pub async fn exec(
        &mut self,
        root_id: &str,
        cwd: &str,
        argv: Vec<String>,
        timeout_ms: u64,
        max_output_bytes: u64,
    ) -> Result<Vec<ExecChunk>, Status> {
        let mut stream = self
            .client
            .exec(self.request(ExecRequest {
                request_id: request_id("exec"),
                root_id: root_id.to_owned(),
                cwd: cwd.to_owned(),
                argv,
                timeout_ms,
                max_output_bytes,
            }))
            .await?
            .into_inner();
        let mut chunks = Vec::new();
        while let Some(chunk) = stream.message().await? {
            let is_exit = matches!(
                chunk.payload,
                Some(dsh_protocol::dsh::remote::v1::exec_chunk::Payload::Exit(_))
            );
            chunks.push(chunk);
            if is_exit {
                break;
            }
        }
        Ok(chunks)
    }
}

#[cfg(test)]
mod tests {
    use super::request_id;

    #[test]
    fn request_ids_are_unique() {
        assert_ne!(request_id("test"), request_id("test"));
    }
}
