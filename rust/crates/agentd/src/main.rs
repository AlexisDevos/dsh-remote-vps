use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use dsh_core::{file_info, FileInfo, PathPolicyError, RootMap};
use dsh_protocol::{
    dsh::remote::v1::{
        exec_chunk::Payload as ExecPayload,
        remote_agent_server::{RemoteAgent, RemoteAgentServer},
        write_frame::Payload,
        CapabilitiesResponse, DirEntry, EditRequest, EditResponse, ExecChunk, ExecExit,
        ExecRequest, GetCapabilitiesRequest, GlobMatch, GlobRequest, GrepMatch, GrepRequest,
        HealthRequest, HealthResponse, ListRequest, NodeType, ReadChunk, ReadRequest, StatRequest,
        StatResponse, WriteFrame, WriteMode, WriteResponse,
    },
    PROTOCOL_VERSION,
};
use globset::Glob;
use regex::RegexBuilder;
use tempfile::NamedTempFile;
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, Mutex},
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status, Streaming};
use tracing::info;
use walkdir::WalkDir;

const READ_CHUNK_SIZE: usize = 64 * 1024;
const DEFAULT_MAX_READ_BYTES: u64 = 64 * 1024 * 1024;
const DEFAULT_MAX_LIST_ENTRIES: u32 = 100_000;
const MAX_EDIT_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_MAX_SEARCH_RESULTS: u32 = 10_000;
const MAX_SEARCH_RESULTS: u32 = 100_000;
const DEFAULT_MAX_GREP_FILE_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_EXEC_TIMEOUT_MS: u64 = 30 * 1000;
const MAX_EXEC_TIMEOUT_MS: u64 = 10 * 60 * 1000;
const DEFAULT_MAX_EXEC_OUTPUT: u64 = 8 * 1024 * 1024;
const MAX_EXEC_OUTPUT: u64 = 64 * 1024 * 1024;
const MAX_GRPC_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

#[derive(Debug, Parser)]
#[command(name = "dsh-agentd", version, about = "Rust daemon for dsh-remote-vps")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Serve the gRPC agent.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7443")]
        listen: String,
        /// Allowed roots in the form `id=/absolute/path`. May be repeated.
        #[arg(long = "root", value_name = "ID=PATH")]
        roots: Vec<String>,
        /// Commands allowed by Exec. No command is allowed unless explicitly listed.
        #[arg(long = "allow-exec", value_name = "COMMAND")]
        allow_exec: Vec<String>,
        /// Read a shared Bearer token from a root-readable file.
        #[arg(long, value_name = "FILE")]
        auth_token_file: Option<PathBuf>,
    },
}

#[derive(Clone, Debug)]
struct AgentService {
    roots: Arc<RootMap>,
    allow_exec: Arc<HashSet<String>>,
    write_locks: Arc<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>>,
    max_read_bytes: u64,
}

impl AgentService {
    fn with_options(root_specs: &[String], allow_exec: &[String]) -> Result<Self> {
        for command in allow_exec {
            if command.is_empty() || !Path::new(command).is_absolute() {
                bail!("allow-exec entries must be absolute executable paths: {command}");
            }
        }
        Ok(Self {
            roots: Arc::new(RootMap::from_specs(root_specs)?),
            allow_exec: Arc::new(allow_exec.iter().cloned().collect()),
            write_locks: Arc::new(Mutex::new(HashMap::new())),
            max_read_bytes: DEFAULT_MAX_READ_BYTES,
        })
    }

    #[allow(clippy::result_large_err)]
    fn resolve(&self, root_id: &str, path: &str) -> Result<std::path::PathBuf, Status> {
        self.roots
            .resolve(root_id, path)
            .map_err(status_from_path_error)
    }

    #[allow(clippy::result_large_err)]
    fn check_expected_version(
        path: &Path,
        mode: WriteMode,
        expected_version: &str,
    ) -> Result<Option<FileInfo>, Status> {
        let current = match file_info(path) {
            Ok(value) => Some(value),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(status_from_io(error)),
        };
        if let Some(current) = &current {
            if current.node_type != NodeType::File as i32 {
                return Err(Status::failed_precondition("target is not a regular file"));
            }
        }

        match mode {
            WriteMode::CreateIfAbsent if current.is_some() => {
                Err(Status::already_exists("target already exists"))
            }
            WriteMode::CreateIfAbsent => Ok(None),
            WriteMode::ReplaceIfVersion => {
                let current = current.ok_or_else(|| Status::not_found("target not found"))?;
                if expected_version.is_empty() || current.version != expected_version {
                    return Err(Status::aborted("stale target version"));
                }
                Ok(Some(current))
            }
            WriteMode::ReplaceAny => Ok(current),
            WriteMode::Unspecified => Err(Status::invalid_argument("write mode is required")),
        }
    }

    async fn path_lock(&self, path: &Path) -> Arc<Mutex<()>> {
        let mut locks = self.write_locks.lock().await;
        locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

#[tonic::async_trait]
impl RemoteAgent for AgentService {
    type ReadStream = ReceiverStream<Result<ReadChunk, Status>>;
    type ListStream = ReceiverStream<Result<DirEntry, Status>>;
    type GlobStream = ReceiverStream<Result<GlobMatch, Status>>;
    type GrepStream = ReceiverStream<Result<GrepMatch, Status>>;
    type ExecStream = ReceiverStream<Result<ExecChunk, Status>>;

    async fn get_capabilities(
        &self,
        request: Request<GetCapabilitiesRequest>,
    ) -> Result<Response<CapabilitiesResponse>, Status> {
        let client_version = request.into_inner().client_version;
        info!(%client_version, "capabilities requested");

        let mut features = vec![
            "health".to_owned(),
            "capabilities".to_owned(),
            "stat".to_owned(),
            "read-stream".to_owned(),
            "list-stream".to_owned(),
            "write-stream".to_owned(),
            "edit".to_owned(),
            "glob".to_owned(),
            "grep".to_owned(),
        ];
        if !self.allow_exec.is_empty() {
            features.push("exec".to_owned());
        }

        Ok(Response::new(CapabilitiesResponse {
            agent_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: PROTOCOL_VERSION.to_owned(),
            features,
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

    async fn stat(&self, request: Request<StatRequest>) -> Result<Response<StatResponse>, Status> {
        let request = request.into_inner();
        let path = self.resolve(&request.root_id, &request.path)?;
        let info = file_info(&path).map_err(status_from_io)?;
        Ok(Response::new(StatResponse {
            r#type: info.node_type,
            size: info.size,
            version: info.version,
        }))
    }

    async fn read(
        &self,
        request: Request<ReadRequest>,
    ) -> Result<Response<Self::ReadStream>, Status> {
        let request = request.into_inner();
        let path = self.resolve(&request.root_id, &request.path)?;
        let info = file_info(&path).map_err(status_from_io)?;
        if info.node_type != NodeType::File as i32 {
            return Err(Status::failed_precondition("target is not a regular file"));
        }
        let max_bytes = if request.max_bytes == 0 {
            self.max_read_bytes
        } else {
            request.max_bytes.min(self.max_read_bytes)
        };
        if info.size > max_bytes {
            return Err(Status::resource_exhausted("file exceeds read limit"));
        }

        let (sender, receiver) = mpsc::channel(8);
        let version = info.version;
        let total_size = info.size;
        tokio::spawn(async move {
            let mut file = match File::open(&path).await {
                Ok(file) => file,
                Err(error) => {
                    let _ = sender.send(Err(status_from_io(error))).await;
                    return;
                }
            };
            let mut buffer = vec![0_u8; READ_CHUNK_SIZE];
            let mut offset = 0_u64;
            loop {
                match file.read(&mut buffer).await {
                    Ok(0) => {
                        break;
                    }
                    Ok(size) => {
                        if offset.saturating_add(size as u64) > max_bytes {
                            let _ = sender
                                .send(Err(Status::resource_exhausted("file exceeds read limit")))
                                .await;
                            return;
                        }
                        let chunk = ReadChunk {
                            data: buffer[..size].to_vec(),
                            offset,
                            total_size,
                            version: version.clone(),
                            eof: false,
                        };
                        offset += size as u64;
                        if sender.send(Ok(chunk)).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(status_from_io(error))).await;
                        break;
                    }
                }
            }
            match file_info(&path) {
                Ok(after) if after.version != version => {
                    let _ = sender
                        .send(Err(Status::aborted("file changed during read")))
                        .await;
                }
                Err(error) => {
                    let _ = sender.send(Err(status_from_io(error))).await;
                }
                _ => {}
            }
            if let Ok(after) = file_info(&path) {
                if after.version == version {
                    let _ = sender
                        .send(Ok(ReadChunk {
                            data: Vec::new(),
                            offset,
                            total_size,
                            version: version.clone(),
                            eof: true,
                        }))
                        .await;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(receiver)))
    }

    async fn list(
        &self,
        request: Request<ListRequest>,
    ) -> Result<Response<Self::ListStream>, Status> {
        let request = request.into_inner();
        let path = self.resolve(&request.root_id, &request.path)?;
        let info = file_info(&path).map_err(status_from_io)?;
        if info.node_type != NodeType::Directory as i32 {
            return Err(Status::failed_precondition("target is not a directory"));
        }

        let max_entries = if request.max_entries == 0 {
            DEFAULT_MAX_LIST_ENTRIES
        } else {
            request.max_entries.min(DEFAULT_MAX_LIST_ENTRIES)
        } as usize;
        let mut directory = fs::read_dir(&path).await.map_err(status_from_io)?;
        let mut entries = Vec::new();
        while let Some(entry) = directory.next_entry().await.map_err(status_from_io)? {
            if entries.len() >= max_entries {
                return Err(Status::resource_exhausted("directory entry limit exceeded"));
            }
            let entry_path = entry.path();
            let entry_info = file_info(&entry_path).map_err(status_from_io)?;
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                r#type: entry_info.node_type,
                size: entry_info.size,
                version: entry_info.version,
            });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));

        let (sender, receiver) = mpsc::channel(32);
        tokio::spawn(async move {
            for entry in entries {
                if sender.send(Ok(entry)).await.is_err() {
                    break;
                }
            }
        });
        Ok(Response::new(ReceiverStream::new(receiver)))
    }

    async fn write(
        &self,
        request: Request<Streaming<WriteFrame>>,
    ) -> Result<Response<WriteResponse>, Status> {
        let mut stream = request.into_inner();
        let first = tokio::time::timeout(std::time::Duration::from_secs(60), stream.message())
            .await
            .map_err(|_| Status::deadline_exceeded("write stream idle"))??
            .ok_or_else(|| Status::invalid_argument("write stream is empty"))?;
        let start = match first.payload {
            Some(Payload::Start(start)) => start,
            Some(Payload::Data(_)) | None => {
                return Err(Status::invalid_argument("write must start with metadata"));
            }
        };
        let mode = WriteMode::try_from(start.mode)
            .map_err(|_| Status::invalid_argument("unknown write mode"))?;
        if start.declared_size > self.max_read_bytes {
            return Err(Status::resource_exhausted("write exceeds size limit"));
        }
        let provisional = self
            .roots
            .resolve_for_create(&start.root_id, &start.path)
            .map_err(status_from_path_error)?;
        if let Some(parent) = provisional.parent() {
            fs::create_dir_all(parent).await.map_err(status_from_io)?;
        }
        let path = self.resolve(&start.root_id, &start.path)?;
        let path_lock = self.path_lock(&path).await;
        let _write_guard = path_lock.lock().await;
        let before = Self::check_expected_version(&path, mode, &start.expected_version)?;
        let operation = if before.is_some() { "update" } else { "create" };
        let parent = path
            .parent()
            .ok_or_else(|| Status::invalid_argument("target has no parent"))?;
        let temporary = NamedTempFile::new_in(parent).map_err(status_from_io)?;
        let temporary_path = temporary.path().to_path_buf();
        let mut file = File::from_std(temporary.as_file().try_clone().map_err(status_from_io)?);
        let mut written = 0_u64;
        let write_result = match tokio::time::timeout(std::time::Duration::from_secs(300), async {
            loop {
                let frame =
                    tokio::time::timeout(std::time::Duration::from_secs(60), stream.message())
                        .await
                        .map_err(|_| Status::deadline_exceeded("write stream idle"))??;
                let Some(frame) = frame else {
                    break;
                };
                match frame.payload {
                    Some(Payload::Data(data)) => {
                        written = written.saturating_add(data.len() as u64);
                        if written > self.max_read_bytes {
                            return Err(Status::resource_exhausted("write exceeds size limit"));
                        }
                        file.write_all(&data).await.map_err(status_from_io)?;
                    }
                    Some(Payload::Start(_)) => {
                        return Err(Status::invalid_argument("write metadata may appear once"));
                    }
                    None => return Err(Status::invalid_argument("empty write frame")),
                }
            }
            if start.declared_size != 0 && start.declared_size != written {
                return Err(Status::invalid_argument(
                    "declared size does not match payload",
                ));
            }
            file.sync_all().await.map_err(status_from_io)?;
            Self::check_expected_version(&path, mode, &start.expected_version)?;
            let preserve_mode = file_mode(&path)?;
            if let Some(mode) = preserve_mode {
                set_file_mode(&temporary_path, mode).await?;
            }
            drop(file);
            fs::rename(&temporary_path, &path)
                .await
                .map_err(status_from_io)?;
            sync_parent(parent).await?;
            Ok::<(), Status>(())
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(Status::deadline_exceeded("write operation timed out")),
        };

        drop(temporary);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary_path).await;
            return Err(error);
        }
        let after = file_info(&path).map_err(status_from_io)?;
        info!(
            request_id = %start.request_id,
            root_id = %start.root_id,
            path = %start.path,
            operation,
            bytes = written,
            "write completed"
        );
        Ok(Response::new(WriteResponse {
            operation: operation.to_owned(),
            version: after.version,
            size: written,
        }))
    }

    async fn edit(&self, request: Request<EditRequest>) -> Result<Response<EditResponse>, Status> {
        let request = request.into_inner();
        if request.old_string.is_empty() {
            return Err(Status::invalid_argument("old_string must not be empty"));
        }
        let path = self.resolve(&request.root_id, &request.path)?;
        let path_lock = self.path_lock(&path).await;
        let _write_guard = path_lock.lock().await;
        let before_info = file_info(&path).map_err(status_from_io)?;
        if before_info.node_type != NodeType::File as i32 {
            return Err(Status::failed_precondition("target is not a regular file"));
        }
        if !request.expected_version.is_empty() && request.expected_version != before_info.version {
            return Err(Status::aborted("stale target version"));
        }
        let raw = fs::read(&path).await.map_err(status_from_io)?;
        if raw.len() > MAX_EDIT_BYTES {
            return Err(Status::resource_exhausted("edit target exceeds size limit"));
        }
        let text = String::from_utf8(raw)
            .map_err(|_| Status::failed_precondition("target is not UTF-8"))?;
        let crlf = text.contains("\r\n");
        let normalized = text.replace("\r\n", "\n");
        let old = request.old_string.replace("\r\n", "\n");
        let new = request.new_string.replace("\r\n", "\n");
        let occurrences = normalized.matches(&old).count();
        if occurrences == 0 {
            return Err(Status::not_found("old_string not found"));
        }
        if occurrences > 1 && !request.replace_all {
            return Err(Status::failed_precondition("old_string is ambiguous"));
        }
        let after_normalized = if request.replace_all {
            normalized.replace(&old, &new)
        } else {
            normalized.replacen(&old, &new, 1)
        };
        let after = if crlf {
            after_normalized.replace('\n', "\r\n")
        } else {
            after_normalized.clone()
        };
        atomic_replace(&path, after.as_bytes()).await?;
        let after_info = file_info(&path).map_err(status_from_io)?;
        info!(
            request_id = %request.request_id,
            root_id = %request.root_id,
            path = %request.path,
            occurrences,
            "edit completed"
        );

        Ok(Response::new(EditResponse {
            version: after_info.version,
            before: normalized,
            after: after_normalized,
        }))
    }

    async fn glob(
        &self,
        request: Request<GlobRequest>,
    ) -> Result<Response<Self::GlobStream>, Status> {
        let request = request.into_inner();
        if request.pattern.is_empty() {
            return Err(Status::invalid_argument("glob pattern must not be empty"));
        }
        let base_path = if request.base_path.is_empty() {
            "."
        } else {
            request.base_path.as_str()
        };
        let base = self.resolve(&request.root_id, base_path)?;
        let matcher = Glob::new(&request.pattern)
            .map_err(|error| Status::invalid_argument(format!("invalid glob pattern: {error}")))?
            .compile_matcher();
        let max_results = if request.max_results == 0 {
            DEFAULT_MAX_SEARCH_RESULTS
        } else {
            request.max_results.min(MAX_SEARCH_RESULTS)
        } as usize;
        let include_hidden = request.include_hidden;
        let entries = tokio::task::spawn_blocking(move || {
            collect_glob(base, matcher, max_results, include_hidden)
        })
        .await
        .map_err(|error| Status::internal(format!("glob worker failed: {error}")))?
        .map_err(Status::internal)?;
        Ok(Response::new(receiver_stream(entries)))
    }

    async fn grep(
        &self,
        request: Request<GrepRequest>,
    ) -> Result<Response<Self::GrepStream>, Status> {
        let request = request.into_inner();
        if request.pattern.is_empty() {
            return Err(Status::invalid_argument("grep pattern must not be empty"));
        }
        let base_path = if request.base_path.is_empty() {
            "."
        } else {
            request.base_path.as_str()
        };
        let base = self.resolve(&request.root_id, base_path)?;
        let regex = RegexBuilder::new(&request.pattern)
            .case_insensitive(!request.case_sensitive)
            .build()
            .map_err(|error| Status::invalid_argument(format!("invalid grep pattern: {error}")))?;
        let max_results = if request.max_results == 0 {
            DEFAULT_MAX_SEARCH_RESULTS
        } else {
            request.max_results.min(MAX_SEARCH_RESULTS)
        } as usize;
        let max_file_bytes = if request.max_file_bytes == 0 {
            DEFAULT_MAX_GREP_FILE_BYTES
        } else {
            request.max_file_bytes.min(DEFAULT_MAX_READ_BYTES)
        } as usize;
        let include_hidden = request.include_hidden;
        let entries = tokio::task::spawn_blocking(move || {
            collect_grep(base, regex, max_results, max_file_bytes, include_hidden)
        })
        .await
        .map_err(|error| Status::internal(format!("grep worker failed: {error}")))?
        .map_err(Status::internal)?;
        Ok(Response::new(receiver_stream(entries)))
    }

    async fn exec(
        &self,
        request: Request<ExecRequest>,
    ) -> Result<Response<Self::ExecStream>, Status> {
        let request = request.into_inner();
        let command = request
            .argv
            .first()
            .ok_or_else(|| Status::invalid_argument("argv must not be empty"))?;
        if request.argv.len() > 128
            || request.argv.iter().map(String::len).sum::<usize>() > 64 * 1024
        {
            return Err(Status::invalid_argument("argv is too large"));
        }
        let allowed = self.allow_exec.contains(command);
        if !allowed {
            return Err(Status::permission_denied(
                "command is not in the agent allowlist",
            ));
        }
        let cwd = self.resolve(
            &request.root_id,
            if request.cwd.is_empty() {
                "."
            } else {
                &request.cwd
            },
        )?;
        if !std::fs::metadata(&cwd).map_err(status_from_io)?.is_dir() {
            return Err(Status::failed_precondition("cwd is not a directory"));
        }
        let timeout_ms = if request.timeout_ms == 0 {
            DEFAULT_EXEC_TIMEOUT_MS
        } else {
            request.timeout_ms.min(MAX_EXEC_TIMEOUT_MS)
        };
        let max_output_bytes = if request.max_output_bytes == 0 {
            DEFAULT_MAX_EXEC_OUTPUT
        } else {
            request.max_output_bytes.min(MAX_EXEC_OUTPUT)
        };
        info!(
            request_id = %request.request_id,
            root_id = %request.root_id,
            command = %command,
            timeout_ms,
            max_output_bytes,
            "exec requested"
        );
        let (sender, receiver) = mpsc::channel(32);
        tokio::spawn(run_exec(
            request.argv,
            cwd,
            timeout_ms,
            max_output_bytes,
            sender,
        ));
        Ok(Response::new(ReceiverStream::new(receiver)))
    }
}

fn receiver_stream<T>(entries: Vec<T>) -> ReceiverStream<Result<T, Status>>
where
    T: Send + 'static,
{
    let (sender, receiver) = mpsc::channel(32);
    tokio::spawn(async move {
        for entry in entries {
            if sender.send(Ok(entry)).await.is_err() {
                break;
            }
        }
    });
    ReceiverStream::new(receiver)
}

fn is_hidden(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_string_lossy().starts_with('.'))
}

fn wire_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn collect_glob(
    base: PathBuf,
    matcher: globset::GlobMatcher,
    max_results: usize,
    include_hidden: bool,
) -> Result<Vec<GlobMatch>, String> {
    let mut matches = Vec::new();
    let walker = WalkDir::new(&base)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            include_hidden
                || entry.path() == base
                || !is_hidden(entry.path().strip_prefix(&base).unwrap_or(entry.path()))
        });
    for entry in walker.filter_map(Result::ok) {
        if entry.path() == base {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(&base)
            .map_err(|error| error.to_string())?;
        if !matcher.is_match(relative) {
            continue;
        }
        let info = file_info(entry.path()).map_err(|error| error.to_string())?;
        matches.push(GlobMatch {
            path: wire_path(&base, entry.path()),
            r#type: info.node_type,
            size: info.size,
            version: info.version,
        });
        if matches.len() >= max_results {
            break;
        }
    }
    matches.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(matches)
}

fn collect_grep(
    base: PathBuf,
    regex: regex::Regex,
    max_results: usize,
    max_file_bytes: usize,
    include_hidden: bool,
) -> Result<Vec<GrepMatch>, String> {
    let mut matches = Vec::new();
    let walker = WalkDir::new(&base)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            include_hidden
                || entry.path() == base
                || !is_hidden(entry.path().strip_prefix(&base).unwrap_or(entry.path()))
        });
    'entries: for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let metadata = match std::fs::metadata(entry.path()) {
            Ok(metadata) if metadata.len() <= max_file_bytes as u64 => metadata,
            _ => continue,
        };
        let file = match std::fs::File::open(entry.path()) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        if file
            .take(max_file_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > max_file_bytes
        {
            continue;
        }
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => continue,
        };
        for (line_index, line) in text.lines().enumerate() {
            for found in regex.find_iter(line) {
                let column = line[..found.start()].chars().count() as u64 + 1;
                let text = if line.len() > 16 * 1024 {
                    line[..line
                        .char_indices()
                        .take_while(|(index, _)| *index < 16 * 1024)
                        .last()
                        .map(|(index, character)| index + character.len_utf8())
                        .unwrap_or(0)]
                        .to_owned()
                } else {
                    line.to_owned()
                };
                matches.push(GrepMatch {
                    path: wire_path(&base, entry.path()),
                    line: line_index as u64 + 1,
                    column,
                    text,
                });
                if matches.len() >= max_results {
                    break 'entries;
                }
            }
        }
    }
    matches.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.line.cmp(&right.line))
            .then(left.column.cmp(&right.column))
    });
    Ok(matches)
}

struct ExecOutputState {
    total: AtomicU64,
    truncated: AtomicBool,
}

async fn pump_output<R>(
    mut reader: R,
    sender: tokio::sync::mpsc::UnboundedSender<ExecChunk>,
    stdout: bool,
    state: Arc<ExecOutputState>,
    limit: u64,
) where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = vec![0_u8; READ_CHUNK_SIZE];
    while let Ok(size) = reader.read(&mut buffer).await {
        if size == 0 {
            break;
        }
        let previous = state.total.fetch_add(size as u64, Ordering::AcqRel);
        if previous.saturating_add(size as u64) > limit {
            state.truncated.store(true, Ordering::Release);
        }
        if previous >= limit {
            // Keep draining the pipe after the output limit so the child
            // cannot block forever on a full stdout/stderr pipe.
            continue;
        }
        let allowed = (limit - previous).min(size as u64) as usize;
        if allowed == 0 {
            continue;
        }
        let payload = if stdout {
            ExecPayload::Stdout(buffer[..allowed].to_vec())
        } else {
            ExecPayload::Stderr(buffer[..allowed].to_vec())
        };
        if sender
            .send(ExecChunk {
                payload: Some(payload),
            })
            .is_err()
        {
            break;
        }
    }
}

async fn kill_child_group(
    child: &mut tokio::process::Child,
) -> std::io::Result<std::process::ExitStatus> {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // The child is started in its own process group. Killing the group
        // prevents shell-descendant processes from surviving a timeout.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
    child.wait().await
}

async fn run_exec(
    argv: Vec<String>,
    cwd: PathBuf,
    timeout_ms: u64,
    max_output_bytes: u64,
    sender: mpsc::Sender<Result<ExecChunk, Status>>,
) {
    let mut command = tokio::process::Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = sender.send(Err(status_from_io(error))).await;
            return;
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (output_sender, mut output_receiver) = tokio::sync::mpsc::unbounded_channel();
    let output_state = Arc::new(ExecOutputState {
        total: AtomicU64::new(0),
        truncated: AtomicBool::new(false),
    });
    let stdout_task = stdout.map(|reader| {
        tokio::spawn(pump_output(
            reader,
            output_sender.clone(),
            true,
            output_state.clone(),
            max_output_bytes,
        ))
    });
    let stderr_task = stderr.map(|reader| {
        tokio::spawn(pump_output(
            reader,
            output_sender.clone(),
            false,
            output_state.clone(),
            max_output_bytes,
        ))
    });
    drop(output_sender);

    let wait_result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
        tokio::select! {
            result = child.wait() => result.map(Some),
            _ = sender.closed() => Ok(None),
        }
    })
    .await;
    let (status, timed_out) = match wait_result {
        Ok(Ok(Some(status))) => (status, false),
        Ok(Ok(None)) => {
            let _ = kill_child_group(&mut child).await;
            if let Some(task) = stdout_task {
                let _ = task.await;
            }
            if let Some(task) = stderr_task {
                let _ = task.await;
            }
            return;
        }
        Ok(Err(error)) => {
            let _ = kill_child_group(&mut child).await;
            let _ = sender.send(Err(status_from_io(error))).await;
            return;
        }
        Err(_) => match kill_child_group(&mut child).await {
            Ok(status) => (status, true),
            Err(error) => {
                let _ = sender.send(Err(status_from_io(error))).await;
                return;
            }
        },
    };

    if let Some(task) = stdout_task {
        let _ = task.await;
    }
    if let Some(task) = stderr_task {
        let _ = task.await;
    }
    while let Some(chunk) = output_receiver.recv().await {
        if sender.send(Ok(chunk)).await.is_err() {
            return;
        }
    }

    let signal = exit_signal(&status);
    let exit_code = status.code().unwrap_or(-1);
    let truncated = output_state.truncated.load(Ordering::Acquire);
    let _ = sender
        .send(Ok(ExecChunk {
            payload: Some(ExecPayload::Exit(ExecExit {
                exit_code,
                signal,
                timed_out,
                truncated,
            })),
        }))
        .await;
}

fn exit_signal(status: &std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal().unwrap_or_default()
    }
    #[cfg(not(unix))]
    {
        let _ = status;
        0
    }
}

async fn atomic_replace(path: &Path, data: &[u8]) -> Result<(), Status> {
    let preserve_mode = file_mode(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| Status::invalid_argument("target has no parent"))?;
    let temporary = NamedTempFile::new_in(parent).map_err(status_from_io)?;
    let temporary_path = temporary.path().to_path_buf();
    let mut file = File::from_std(temporary.as_file().try_clone().map_err(status_from_io)?);
    let result = async {
        file.write_all(data).await.map_err(status_from_io)?;
        file.sync_all().await.map_err(status_from_io)?;
        if let Some(mode) = preserve_mode {
            set_file_mode(&temporary_path, mode).await?;
        }
        drop(file);
        fs::rename(&temporary_path, path)
            .await
            .map_err(status_from_io)?;
        sync_parent(parent).await
    }
    .await;
    drop(temporary);
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path).await;
    }
    result
}

#[allow(clippy::result_large_err)]
fn file_mode(path: &Path) -> Result<Option<u32>, Status> {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                Ok(Some(metadata.permissions().mode()))
            }
            #[cfg(not(unix))]
            {
                let _ = metadata;
                Ok(None)
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(status_from_io(error)),
    }
}

async fn set_file_mode(path: &Path, mode: u32) -> Result<(), Status> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .await
            .map_err(status_from_io)?
            .permissions();
        permissions.set_mode(mode);
        fs::set_permissions(path, permissions)
            .await
            .map_err(status_from_io)?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

async fn sync_parent(path: &Path) -> Result<(), Status> {
    #[cfg(unix)]
    {
        let directory = File::open(path).await.map_err(status_from_io)?;
        directory.sync_all().await.map_err(status_from_io)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn status_from_path_error(error: PathPolicyError) -> Status {
    match error {
        PathPolicyError::UnknownRoot(_)
        | PathPolicyError::AbsolutePath
        | PathPolicyError::NulByte => Status::invalid_argument(error.to_string()),
        PathPolicyError::ParentTraversal | PathPolicyError::OutsideRoot => {
            Status::permission_denied(error.to_string())
        }
        PathPolicyError::MissingFilename => Status::invalid_argument(error.to_string()),
        PathPolicyError::Io(error) => status_from_io(error),
    }
}

fn status_from_io(error: std::io::Error) -> Status {
    match error.kind() {
        std::io::ErrorKind::NotFound => Status::not_found(error.to_string()),
        std::io::ErrorKind::PermissionDenied => Status::permission_denied(error.to_string()),
        std::io::ErrorKind::AlreadyExists => Status::already_exists(error.to_string()),
        _ => Status::internal(error.to_string()),
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

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    info!("agent shutdown requested");
}

fn load_auth_token(path: Option<&Path>) -> Result<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let token = std::fs::read_to_string(path)
        .with_context(|| format!("read auth token file {}", path.display()))?
        .trim()
        .to_owned();
    if token.is_empty() || token.len() > 4096 || token.chars().any(char::is_whitespace) {
        bail!("auth token must be non-empty and contain no whitespace");
    }
    Ok(Some(token))
}

async fn serve(
    listen: String,
    roots: Vec<String>,
    allow_exec: Vec<String>,
    auth_token_file: Option<PathBuf>,
) -> Result<()> {
    let address: std::net::SocketAddr = listen.parse()?;
    if !address.ip().is_loopback() {
        anyhow::bail!(
            "refusing non-loopback bind {address}; use an SSH/Tailscale tunnel or add TLS first"
        );
    }
    let service =
        AgentService::with_options(&roots, &allow_exec).context("load configured roots")?;
    let auth_token =
        load_auth_token(auth_token_file.as_deref())?.map(|token| format!("Bearer {token}"));
    info!(%address, protocol = PROTOCOL_VERSION, roots = ?service.roots.ids().collect::<Vec<_>>(), "agent listening");

    let service = RemoteAgentServer::new(service)
        .max_decoding_message_size(MAX_GRPC_MESSAGE_SIZE)
        .max_encoding_message_size(MAX_GRPC_MESSAGE_SIZE);
    if let Some(expected) = auth_token {
        #[allow(clippy::result_large_err)]
        let interceptor = move |request: Request<()>| {
            let provided = request
                .metadata()
                .get("authorization")
                .and_then(|value| value.to_str().ok());
            if provided == Some(expected.as_str()) {
                Ok(request)
            } else {
                Err(Status::unauthenticated("missing or invalid bearer token"))
            }
        };
        Server::builder()
            .layer(tonic::service::interceptor(interceptor))
            .add_service(service)
            .serve_with_shutdown(address, shutdown_signal())
            .await?;
    } else {
        Server::builder()
            .add_service(service)
            .serve_with_shutdown(address, shutdown_signal())
            .await?;
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    match Cli::parse().command {
        Command::Serve {
            listen,
            roots,
            allow_exec,
            auth_token_file,
        } => serve(listen, roots, allow_exec, auth_token_file).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dsh_protocol::dsh::remote::v1::{
        exec_chunk::Payload as ExecPayload, remote_agent_client::RemoteAgentClient,
        remote_agent_server::RemoteAgent, write_frame::Payload, WriteStart,
    };
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;

    #[tokio::test]
    async fn health_returns_protocol_identity() {
        let service = AgentService::with_options(&[], &[]).expect("service");
        let response = service
            .health(Request::new(HealthRequest {
                request_id: "test-request".to_owned(),
            }))
            .await
            .expect("health response")
            .into_inner();

        assert_eq!(response.status, "ok");
        assert_eq!(response.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn exec_allowlist_requires_absolute_paths() {
        assert!(AgentService::with_options(&[], &["echo".to_owned()]).is_err());
    }

    #[tokio::test]
    async fn filesystem_round_trip_over_grpc() {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir(root.path().join("src")).expect("src directory");
        std::fs::write(root.path().join("src/lib.rs"), "worker adapter\n").expect("source file");
        std::fs::write(root.path().join(".hidden.rs"), "worker hidden\n").expect("hidden file");
        let service = AgentService::with_options(
            &[format!("project={}", root.path().display())],
            &["/bin/echo".to_owned()],
        )
        .expect("service");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let server = Server::builder()
            .add_service(RemoteAgentServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener));
        let server_task = tokio::spawn(server);

        let mut client = RemoteAgentClient::connect(format!("http://{address}"))
            .await
            .expect("client");
        let write_response = client
            .write(Request::new(tokio_stream::iter(vec![
                WriteFrame {
                    payload: Some(Payload::Start(WriteStart {
                        request_id: "write-1".to_owned(),
                        root_id: "project".to_owned(),
                        path: "hello.txt".to_owned(),
                        mode: WriteMode::CreateIfAbsent as i32,
                        expected_version: String::new(),
                        declared_size: 5,
                    })),
                },
                WriteFrame {
                    payload: Some(Payload::Data(b"hello".to_vec())),
                },
            ])))
            .await
            .expect("write")
            .into_inner();
        assert_eq!(write_response.operation, "create");

        let stat = client
            .stat(Request::new(StatRequest {
                request_id: "stat-1".to_owned(),
                root_id: "project".to_owned(),
                path: "hello.txt".to_owned(),
            }))
            .await
            .expect("stat")
            .into_inner();
        assert_eq!(stat.r#type, NodeType::File as i32);
        assert_eq!(stat.size, 5);

        let mut read_stream = client
            .read(Request::new(ReadRequest {
                request_id: "read-1".to_owned(),
                root_id: "project".to_owned(),
                path: "hello.txt".to_owned(),
                max_bytes: 1024,
            }))
            .await
            .expect("read")
            .into_inner();
        let mut content = Vec::new();
        while let Some(chunk) = read_stream.message().await.expect("read chunk") {
            content.extend_from_slice(&chunk.data);
            if chunk.eof {
                break;
            }
        }
        assert_eq!(content, b"hello");

        let edit = client
            .edit(Request::new(EditRequest {
                request_id: "edit-1".to_owned(),
                root_id: "project".to_owned(),
                path: "hello.txt".to_owned(),
                old_string: "hello".to_owned(),
                new_string: "world".to_owned(),
                replace_all: false,
                expected_version: stat.version,
            }))
            .await
            .expect("edit")
            .into_inner();
        assert_eq!(edit.after, "world");

        let mut glob_stream = client
            .glob(Request::new(GlobRequest {
                request_id: "glob-1".to_owned(),
                root_id: "project".to_owned(),
                base_path: ".".to_owned(),
                pattern: "**/*.rs".to_owned(),
                max_results: 20,
                include_hidden: false,
            }))
            .await
            .expect("glob")
            .into_inner();
        let mut glob_paths = Vec::new();
        while let Some(entry) = glob_stream.message().await.expect("glob entry") {
            glob_paths.push(entry.path);
        }
        assert_eq!(glob_paths, vec!["src/lib.rs"]);

        let mut grep_stream = client
            .grep(Request::new(GrepRequest {
                request_id: "grep-1".to_owned(),
                root_id: "project".to_owned(),
                base_path: ".".to_owned(),
                pattern: "worker".to_owned(),
                max_results: 20,
                max_file_bytes: 1024,
                case_sensitive: true,
                include_hidden: false,
            }))
            .await
            .expect("grep")
            .into_inner();
        let mut grep_paths = Vec::new();
        while let Some(entry) = grep_stream.message().await.expect("grep entry") {
            grep_paths.push(entry.path);
        }
        assert_eq!(grep_paths, vec!["src/lib.rs"]);

        let mut exec_stream = client
            .exec(Request::new(ExecRequest {
                request_id: "exec-1".to_owned(),
                root_id: "project".to_owned(),
                cwd: ".".to_owned(),
                argv: vec!["/bin/echo".to_owned(), "hello".to_owned()],
                timeout_ms: 1_000,
                max_output_bytes: 1024,
            }))
            .await
            .expect("exec")
            .into_inner();
        let mut stdout = Vec::new();
        let mut exit = None;
        while let Some(chunk) = exec_stream.message().await.expect("exec chunk") {
            match chunk.payload {
                Some(ExecPayload::Stdout(data)) => stdout.extend(data),
                Some(ExecPayload::Exit(status)) => exit = Some(status),
                _ => {}
            }
        }
        assert_eq!(stdout, b"hello\n");
        assert_eq!(exit.expect("exit").exit_code, 0);

        server_task.abort();
    }

    #[tokio::test]
    async fn exec_timeout_kills_process_group_and_reports_truncation() {
        let root = tempfile::tempdir().expect("root");
        let service = AgentService::with_options(
            &[format!("project={}", root.path().display())],
            &["/bin/sh".to_owned()],
        )
        .expect("service");
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
        let address = listener.local_addr().expect("address");
        let server = Server::builder()
            .add_service(RemoteAgentServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener));
        let server_task = tokio::spawn(server);
        let mut client = RemoteAgentClient::connect(format!("http://{address}"))
            .await
            .expect("client");

        let mut output_stream = client
            .exec(Request::new(ExecRequest {
                request_id: "exec-truncate".to_owned(),
                root_id: "project".to_owned(),
                cwd: ".".to_owned(),
                argv: vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "printf 123456789".to_owned(),
                ],
                timeout_ms: 1_000,
                max_output_bytes: 4,
            }))
            .await
            .expect("truncating exec")
            .into_inner();
        let mut output = Vec::new();
        let mut truncated = false;
        while let Some(chunk) = output_stream.message().await.expect("output chunk") {
            match chunk.payload {
                Some(ExecPayload::Stdout(data)) => output.extend(data),
                Some(ExecPayload::Exit(status)) => truncated = status.truncated,
                _ => {}
            }
        }
        assert_eq!(output, b"1234");
        assert!(truncated);

        let mut timeout_stream = client
            .exec(Request::new(ExecRequest {
                request_id: "exec-timeout".to_owned(),
                root_id: "project".to_owned(),
                cwd: ".".to_owned(),
                argv: vec!["/bin/sh".to_owned(), "-c".to_owned(), "sleep 30".to_owned()],
                timeout_ms: 100,
                max_output_bytes: 1024,
            }))
            .await
            .expect("timeout exec")
            .into_inner();
        let mut timed_out = false;
        while let Some(chunk) = timeout_stream.message().await.expect("timeout chunk") {
            if let Some(ExecPayload::Exit(status)) = chunk.payload {
                timed_out = status.timed_out;
            }
        }
        assert!(timed_out);
        server_task.abort();
    }
}
