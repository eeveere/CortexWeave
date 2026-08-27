use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;

use crate::{
    CortexWeaveService,
    domain::{CortexEvent, EventType, MemoryKind, MemoryRecord},
    indexing::{WorkspaceWatcher, WorkspaceWatcherHandle},
    workspace::WorkspaceSelector,
};

const PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_MCP_FRAME_BYTES: usize = 1_048_576;
const MAX_TOOL_LIMIT: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceHint {
    RootPath(PathBuf),
    FileUri(String),
}

impl WorkspaceHint {
    fn selector(&self) -> WorkspaceSelector {
        match self {
            Self::RootPath(path) => WorkspaceSelector::RootPath(path.clone()),
            Self::FileUri(uri) => WorkspaceSelector::FileUri(uri.clone()),
        }
    }
}

pub struct McpServer {
    service: Arc<CortexWeaveService>,
    workspace_hint: Option<WorkspaceHint>,
    initialized: AtomicBool,
    watcher_state: Arc<Mutex<BTreeMap<String, String>>>,
}

impl McpServer {
    pub fn new(service: Arc<CortexWeaveService>) -> Self {
        Self::with_workspace_hint(service, None)
    }

    pub fn with_workspace_hint(
        service: Arc<CortexWeaveService>,
        workspace_hint: Option<WorkspaceHint>,
    ) -> Self {
        Self {
            service,
            workspace_hint,
            initialized: AtomicBool::new(false),
            watcher_state: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub async fn serve_stdio(&self) -> crate::Result<()> {
        let mut input = BufReader::new(tokio::io::stdin());
        let mut output = tokio::io::stdout();
        let mut frame = Vec::new();
        let mut watcher_startup: Option<JoinHandle<Vec<WorkspaceWatcherHandle>>> = None;
        while let Some(line) = read_mcp_frame(&mut input, &mut frame).await? {
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Value>(&line) {
                Ok(request) => {
                    let initialize =
                        request.get("method").and_then(Value::as_str) == Some("initialize");
                    let response = self.handle_json(request).await;
                    if initialize && watcher_startup.is_none() {
                        watcher_startup = Some(tokio::spawn(Self::start_workspace_watchers_for(
                            Arc::clone(&self.service),
                            Arc::clone(&self.watcher_state),
                        )));
                    }
                    response
                }
                Err(error) => Some(protocol_error(Value::Null, -32700, error.to_string())),
            };
            if let Some(response) = response {
                let encoded = serde_json::to_string(&response)?;
                output.write_all(encoded.as_bytes()).await?;
                output.write_all(b"\n").await?;
                output.flush().await?;
            }
        }
        if let Some(startup) = watcher_startup
            && let Ok(watchers) = startup.await
        {
            for watcher in watchers {
                watcher.shutdown().await;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    async fn start_workspace_watchers(&self) -> Vec<WorkspaceWatcherHandle> {
        Self::start_workspace_watchers_for(
            Arc::clone(&self.service),
            Arc::clone(&self.watcher_state),
        )
        .await
    }

    async fn start_workspace_watchers_for(
        service: Arc<CortexWeaveService>,
        watcher_state: Arc<Mutex<BTreeMap<String, String>>>,
    ) -> Vec<WorkspaceWatcherHandle> {
        let mut watchers = Vec::new();
        let workspaces = match service.list_workspaces().await {
            Ok(workspaces) => workspaces,
            Err(error) => {
                tracing::error!(%error, "failed to list workspaces for MCP watchers");
                return watchers;
            }
        };
        for workspace in workspaces {
            let workspace_id = workspace.id.clone();
            watcher_state
                .lock()
                .expect("watcher state lock poisoned")
                .insert(workspace_id.clone(), "starting".into());
            match WorkspaceWatcher::start(
                workspace,
                service.indexing_handle(),
                service.storage_handle(),
                std::time::Duration::from_millis(service.config().indexing.debounce_ms),
                1_024,
            )
            .await
            {
                Ok(watcher) => {
                    watcher_state
                        .lock()
                        .expect("watcher state lock poisoned")
                        .insert(workspace_id, "ready".into());
                    watchers.push(watcher);
                }
                Err(error) => {
                    let message = error.to_string();
                    watcher_state
                        .lock()
                        .expect("watcher state lock poisoned")
                        .insert(workspace_id, format!("failed: {message}"));
                    tracing::warn!(%error, "failed to start workspace watcher for MCP");
                }
            }
        }
        watchers
    }

    pub async fn handle_json(&self, request: Value) -> Option<Value> {
        let Some(object) = request.as_object() else {
            return Some(protocol_error(
                Value::Null,
                -32600,
                "request must be an object",
            ));
        };
        let has_id = object.contains_key("id");
        let id = object.get("id").cloned().unwrap_or(Value::Null);
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return has_id.then(|| protocol_error(id, -32600, "jsonrpc must be 2.0"));
        }
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            return has_id.then(|| protocol_error(id, -32600, "method must be a string"));
        };
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        let response = match method {
            "initialize" => {
                self.initialized.store(true, Ordering::Release);
                Ok(initialize_result(&params))
            }
            "notifications/initialized" => return None,
            "ping" => Ok(json!({})),
            "tools/list" if self.initialized.load(Ordering::Acquire) => {
                Ok(json!({ "tools": tool_definitions() }))
            }
            "tools/call" if self.initialized.load(Ordering::Acquire) => {
                self.call_tool(&params).await
            }
            "tools/list" | "tools/call" => Err((-32002, "initialize must complete first".into())),
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        has_id.then(|| match response {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => protocol_error(id, code, message),
        })
    }

    async fn call_tool(&self, params: &Value) -> Result<Value, (i64, String)> {
        let params = object(params).map_err(invalid_params)?;
        let name = required_string(params, "name").map_err(invalid_params)?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let arguments = object(&arguments).map_err(invalid_params)?;
        let result = match name {
            "semantic_search" => self.semantic_search(arguments).await,
            "semantic_get" => self.semantic_get(arguments).await,
            "memory_record" => self.memory_record(arguments).await,
            "memory_search" => self.memory_search(arguments).await,
            "memory_recent" => self.memory_recent(arguments).await,
            "workspace_list" => self.workspace_list().await,
            "workspace_status" => self.workspace_status(arguments).await,
            "workspace_reindex" => self.workspace_reindex(arguments).await,
            "session_start" => self.session_start(arguments).await,
            "session_end" => self.session_end(arguments).await,
            "event_record" => self.event_record(arguments).await,
            _ => return Err((-32601, format!("unknown tool: {name}"))),
        };
        Ok(match result {
            Ok(value) => tool_success(value),
            Err(message) => tool_error(message),
        })
    }

    async fn semantic_search(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let query = required_string(args, "query")?;
        let limit = optional_limit(args, self.service.config().retrieval.default_k)?;
        serialize_service(
            self.service
                .semantic_search(&workspace.id, query, limit)
                .await,
        )
    }

    async fn semantic_get(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let chunk_id = required_string(args, "chunk_id")?;
        let item = self
            .service
            .get_item(&workspace.id, chunk_id)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("code item not found: {chunk_id}"))?;
        json_value(item)
    }

    async fn memory_record(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let kind = parse_memory_kind(required_string(args, "kind")?)?;
        let mut memory = MemoryRecord::new(&workspace.id, kind, required_string(args, "content")?);
        memory.session_id = optional_string(args, "session_id")?;
        memory.task_id = optional_string(args, "task_id")?;
        memory.related_paths = optional_strings(args, "related_paths")?;
        memory.metadata = args.get("metadata").cloned().unwrap_or_else(|| json!({}));
        serialize_service(self.service.record_memory(memory).await)
    }

    async fn memory_search(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let query = required_string(args, "query")?;
        let limit = optional_limit(args, self.service.config().retrieval.default_k)?;
        serialize_service(
            self.service
                .search_memories(&workspace.id, query, limit)
                .await,
        )
    }

    async fn memory_recent(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let limit = optional_limit(args, self.service.config().retrieval.default_k)?;
        serialize_service(self.service.recent_memories(&workspace.id, limit).await)
    }

    async fn workspace_list(&self) -> ToolResult {
        let catalog = self
            .service
            .workspace_catalog(self.default_workspace_selector())
            .await
            .map_err(|error| error.to_string())?;
        let default_id = catalog
            .default_hint_match
            .as_ref()
            .map(|workspace| workspace.id.as_str());
        let workspaces = catalog
            .workspaces
            .iter()
            .map(|workspace| {
                json!({
                    "id": workspace.id,
                    "name": workspace.name,
                    "root_path": workspace.root_path,
                    "default_hint_match": default_id == Some(workspace.id.as_str()),
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "workspaces": workspaces,
            "default_hint_error": catalog.default_hint_error,
        }))
    }

    async fn workspace_status(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let mut status = serde_json::to_value(
            self.service
                .workspace_status(&workspace.id)
                .await
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        status["watcher"] = json!(
            self.watcher_state
                .lock()
                .expect("watcher state lock poisoned")
                .get(&workspace.id)
                .cloned()
                .unwrap_or_else(|| "not_started".into())
        );
        Ok(status)
    }

    async fn workspace_reindex(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        serialize_service(self.service.workspace_reindex(&workspace.id).await)
    }

    async fn session_start(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        serialize_service(
            self.service
                .start_session(
                    &workspace.id,
                    args.get("metadata").cloned().unwrap_or_else(|| json!({})),
                )
                .await,
        )
    }

    async fn session_end(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        serialize_service(
            self.service
                .end_session_in_workspace(&workspace.id, required_string(args, "session_id")?)
                .await,
        )
    }

    async fn event_record(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let mut event = CortexEvent::new(
            &workspace.id,
            EventType::from_storage(required_string(args, "event_type")?),
            args.get("payload").cloned().unwrap_or_else(|| json!({})),
        );
        event.session_id = optional_string(args, "session_id")?;
        event.task_id = optional_string(args, "task_id")?;
        serialize_service(self.service.record_event(event).await)
    }

    async fn resolve_workspace(
        &self,
        args: &Map<String, Value>,
    ) -> Result<crate::domain::Workspace, String> {
        let workspace_id = optional_string(args, "workspace_id")?;
        let workspace =
            optional_string(args, "workspace")?.map(|value| parse_workspace_selector(&value));
        self.service
            .resolve_workspace_selection(workspace_id, workspace, self.default_workspace_selector())
            .await
            .map_err(|error| error.to_string())
    }

    fn default_workspace_selector(&self) -> Option<WorkspaceSelector> {
        self.workspace_hint.as_ref().map(WorkspaceHint::selector)
    }
}

async fn read_mcp_frame(
    input: &mut BufReader<tokio::io::Stdin>,
    frame: &mut Vec<u8>,
) -> crate::Result<Option<String>> {
    frame.clear();
    loop {
        let (consumed, complete) = {
            let available = input.fill_buf().await?;
            if available.is_empty() {
                if frame.is_empty() {
                    return Ok(None);
                }
                return String::from_utf8(frame.clone())
                    .map(Some)
                    .map_err(|error| crate::CortexError::Analysis(error.to_string()));
            }
            let complete = available.iter().position(|byte| *byte == b'\n');
            let consumed = complete.map_or(available.len(), |index| index + 1);
            if frame.len().saturating_add(consumed) > MAX_MCP_FRAME_BYTES {
                return Err(crate::CortexError::Analysis(
                    "MCP frame exceeds maximum size".into(),
                ));
            }
            frame.extend_from_slice(&available[..consumed]);
            (consumed, complete.is_some())
        };
        input.consume(consumed);
        if complete {
            while matches!(frame.last(), Some(b'\n' | b'\r')) {
                frame.pop();
            }
            return String::from_utf8(frame.clone())
                .map(Some)
                .map_err(|error| crate::CortexError::Analysis(error.to_string()));
        }
    }
}

type ToolResult = std::result::Result<Value, String>;

fn initialize_result(_params: &Value) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "cortexweave", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "semantic_search",
            "Search indexed code semantically. Select a workspace by legacy UUID, human-friendly selector, configured default, or unambiguous singleton.",
            workspace_properties(json!({ "query": string_schema(), "limit": limit_schema() })),
            &["query"],
        ),
        tool(
            "semantic_get",
            "Get one indexed code item and its provenance from the resolved workspace.",
            workspace_properties(json!({ "chunk_id": string_schema() })),
            &["chunk_id"],
        ),
        tool(
            "memory_record",
            "Record explicit durable memory in the resolved workspace.",
            workspace_properties(
                json!({ "kind": { "type": "string", "enum": ["decision", "observation", "failure", "solution", "todo", "note", "checkpoint"] }, "content": string_schema(), "session_id": string_schema(), "task_id": string_schema(), "related_paths": { "type": "array", "items": string_schema() }, "metadata": {} }),
            ),
            &["kind", "content"],
        ),
        tool(
            "memory_search",
            "Search explicit memory in the resolved workspace.",
            workspace_properties(json!({ "query": string_schema(), "limit": limit_schema() })),
            &["query"],
        ),
        tool(
            "memory_recent",
            "Retrieve recent explicit memory from the resolved workspace.",
            workspace_properties(json!({ "limit": limit_schema() })),
            &[],
        ),
        tool(
            "workspace_list",
            "List registered workspace UUIDs, names, roots, and the configured default-hint match. Use this before retrying an ambiguous or unknown selector.",
            json!({}),
            &[],
        ),
        tool(
            "workspace_status",
            "Get indexing status for the resolved workspace.",
            workspace_properties(json!({})),
            &[],
        ),
        tool(
            "workspace_reindex",
            "Reconcile the resolved workspace against its current filesystem state.",
            workspace_properties(json!({})),
            &[],
        ),
        tool(
            "session_start",
            "Start a session in the resolved workspace.",
            workspace_properties(json!({ "metadata": {} })),
            &[],
        ),
        tool(
            "session_end",
            "End a session in the resolved workspace.",
            workspace_properties(json!({ "session_id": string_schema() })),
            &["session_id"],
        ),
        tool(
            "event_record",
            "Record an external operational event in the resolved workspace.",
            workspace_properties(
                json!({ "event_type": string_schema(), "payload": {}, "session_id": string_schema(), "task_id": string_schema() }),
            ),
            &["event_type"],
        ),
    ]
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

fn string_schema() -> Value {
    json!({ "type": "string" })
}

fn workspace_properties(mut properties: Value) -> Value {
    let properties = properties
        .as_object_mut()
        .expect("workspace tool properties must be an object");
    properties.insert(
        "workspace_id".into(),
        json!({
            "type": "string",
            "description": "Legacy exact workspace UUID. If workspace is also supplied, both must resolve to the same registration."
        }),
    );
    properties.insert(
        "workspace".into(),
        json!({
            "type": "string",
            "description": "Workspace UUID, unique exact name, absolute root or subdirectory path, or file URI. Omit when the configured hint or only registered workspace is correct."
        }),
    );
    Value::Object(properties.clone())
}

fn limit_schema() -> Value {
    json!({ "type": "integer", "minimum": 0, "maximum": MAX_TOOL_LIMIT })
}

fn tool_success(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": value,
        "isError": false,
    })
}

fn tool_error(message: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

fn protocol_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    })
}

fn object(value: &Value) -> Result<&Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| "arguments must be an object".into())
}

fn required_string<'a>(args: &'a Map<String, Value>, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{name} must be a non-empty string"))
}

fn optional_string(args: &Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    args.get(name)
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("{name} must be a non-empty string when supplied"))
        })
        .transpose()
}

fn optional_strings(args: &Map<String, Value>, name: &str) -> Result<Vec<String>, String> {
    let Some(values) = args.get(name) else {
        return Ok(Vec::new());
    };
    values
        .as_array()
        .ok_or_else(|| format!("{name} must be an array of strings"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("{name} must contain non-empty strings"))
        })
        .collect()
}

fn parse_workspace_selector(value: &str) -> WorkspaceSelector {
    if value
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
    {
        return WorkspaceSelector::FileUri(value.to_owned());
    }
    if uuid::Uuid::parse_str(value).is_ok() {
        return WorkspaceSelector::Id(value.to_owned());
    }
    if looks_like_absolute_path(value) {
        return WorkspaceSelector::RootPath(PathBuf::from(value));
    }
    WorkspaceSelector::Name(value.to_owned())
}

fn looks_like_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    PathBuf::from(value).is_absolute()
        || value.starts_with('/')
        || value.starts_with(r"\\")
        || (bytes.get(1) == Some(&b':')
            && bytes.first().is_some_and(u8::is_ascii_alphabetic)
            && matches!(bytes.get(2), Some(b'/' | b'\\')))
}

fn optional_limit(args: &Map<String, Value>, default: usize) -> Result<usize, String> {
    let Some(value) = args.get("limit") else {
        return Ok(default);
    };
    value
        .as_u64()
        .ok_or_else(|| "limit must be a non-negative integer".into())
        .and_then(|value| usize::try_from(value).map_err(|_| "limit is too large".into()))
        .and_then(|value| {
            (value <= MAX_TOOL_LIMIT)
                .then_some(value)
                .ok_or_else(|| format!("limit must be at most {MAX_TOOL_LIMIT}"))
        })
}

fn parse_memory_kind(value: &str) -> Result<MemoryKind, String> {
    match value {
        "decision" => Ok(MemoryKind::Decision),
        "observation" => Ok(MemoryKind::Observation),
        "failure" => Ok(MemoryKind::Failure),
        "solution" => Ok(MemoryKind::Solution),
        "todo" => Ok(MemoryKind::Todo),
        "note" => Ok(MemoryKind::Note),
        "checkpoint" => Ok(MemoryKind::Checkpoint),
        _ => Err(format!("unsupported memory kind: {value}")),
    }
}

fn json_value(value: impl Serialize) -> ToolResult {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn serialize_service(value: crate::Result<impl Serialize>) -> ToolResult {
    value
        .map_err(|error| error.to_string())
        .and_then(json_value)
}

fn invalid_params(message: String) -> (i64, String) {
    (-32602, message)
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc, time::Duration};

    use async_trait::async_trait;
    use serde_json::json;
    use tempfile::tempdir;
    use tokio::time::sleep;

    use super::*;
    use crate::{AppConfig, Result, embedding::EmbeddingProvider, storage::SqliteStorage};

    struct FixtureProvider;

    #[async_trait]
    impl EmbeddingProvider for FixtureProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(texts
                .iter()
                .map(|text| match_marker(&text.to_ascii_lowercase()))
                .collect())
        }

        fn model_name(&self) -> &str {
            "mcp-fixture"
        }
    }

    async fn server() -> (McpServer, String) {
        let service = Arc::new(
            CortexWeaveService::from_parts(
                AppConfig::default(),
                SqliteStorage::in_memory().await.unwrap(),
            )
            .unwrap(),
        );
        let root = std::env::current_dir().unwrap();
        let workspace = service
            .register_workspace(root.to_string_lossy(), "mcp")
            .await
            .unwrap();
        (McpServer::new(service), workspace.id)
    }

    #[tokio::test]
    async fn supports_initialize_and_advertises_required_tools() {
        let (server, _) = server().await;
        let initialized = server
            .handle_json(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": PROTOCOL_VERSION } }))
            .await
            .unwrap();
        assert_eq!(
            initialized["result"]["capabilities"]["tools"]["listChanged"],
            false
        );
        let listed = server
            .handle_json(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .await
            .unwrap();
        let names: Vec<_> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        for required in [
            "semantic_search",
            "semantic_get",
            "memory_record",
            "memory_search",
            "memory_recent",
            "workspace_list",
            "workspace_status",
            "workspace_reindex",
        ] {
            assert!(names.contains(&required));
        }
    }

    #[tokio::test]
    async fn calls_memory_tools_and_reports_tool_errors_to_the_client() {
        let (server, workspace_id) = server().await;
        initialize(&server).await;
        let recorded = server
            .handle_json(json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "memory_record",
                    "arguments": {
                        "workspace_id": workspace_id,
                        "kind": "decision",
                        "content": "Use BLAKE3 for deterministic change detection.",
                    },
                },
            }))
            .await
            .unwrap();
        assert_eq!(recorded["result"]["isError"], false);
        let searched = server
            .handle_json(json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": {
                    "name": "memory_search",
                    "arguments": { "workspace_id": workspace_id, "query": "Why BLAKE3?" },
                },
            }))
            .await
            .unwrap();
        assert_eq!(searched["result"]["isError"], false);
        assert_eq!(
            searched["result"]["structuredContent"][0]["kind"],
            "decision"
        );
        let invalid = server
            .handle_json(json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": { "name": "memory_record", "arguments": { "workspace_id": workspace_id } },
            }))
            .await
            .unwrap();
        assert_eq!(invalid["result"]["isError"], true);

        let singleton = call_tool(&server, "workspace_status", json!({})).await;
        assert_eq!(singleton["isError"], false);
        assert_eq!(
            singleton["structuredContent"]["workspace"]["id"],
            workspace_id
        );
    }

    #[tokio::test]
    async fn resolves_hints_names_paths_uris_and_conflicts_before_tool_execution() {
        let directory = tempdir().unwrap();
        let first_root = directory.path().join("first");
        let first_subdirectory = first_root.join("src");
        let second_root = directory.path().join("second");
        let unique_root = directory.path().join("unique");
        fs::create_dir_all(&first_subdirectory).unwrap();
        fs::create_dir_all(&second_root).unwrap();
        fs::create_dir_all(&unique_root).unwrap();
        let service = Arc::new(
            CortexWeaveService::from_parts(
                AppConfig::default(),
                SqliteStorage::in_memory().await.unwrap(),
            )
            .unwrap(),
        );
        let first = service
            .register_workspace(first_root.to_string_lossy(), "duplicate")
            .await
            .unwrap();
        let second = service
            .register_workspace(second_root.to_string_lossy(), "duplicate")
            .await
            .unwrap();
        let unique = service
            .register_workspace(unique_root.to_string_lossy(), "unique")
            .await
            .unwrap();
        let server = McpServer::with_workspace_hint(
            Arc::clone(&service),
            Some(WorkspaceHint::RootPath(first_subdirectory)),
        );
        initialize(&server).await;

        let listed = call_tool(&server, "workspace_list", json!({})).await;
        assert_eq!(listed["isError"], false);
        let entries = listed["structuredContent"]["workspaces"]
            .as_array()
            .unwrap();
        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .any(|entry| { entry["id"] == first.id && entry["default_hint_match"] == true })
        );

        let hinted = call_tool(&server, "workspace_status", json!({})).await;
        assert_eq!(hinted["structuredContent"]["workspace"]["id"], first.id);

        let legacy = call_tool(
            &server,
            "workspace_status",
            json!({ "workspace_id": second.id }),
        )
        .await;
        assert_eq!(legacy["structuredContent"]["workspace"]["id"], second.id);

        let by_name = call_tool(
            &server,
            "workspace_status",
            json!({ "workspace": "unique" }),
        )
        .await;
        assert_eq!(by_name["structuredContent"]["workspace"]["id"], unique.id);

        let by_path = call_tool(
            &server,
            "workspace_status",
            json!({ "workspace": second_root.to_string_lossy() }),
        )
        .await;
        assert_eq!(by_path["structuredContent"]["workspace"]["id"], second.id);

        let uri = url::Url::from_directory_path(&unique_root)
            .unwrap()
            .to_string();
        let by_uri = call_tool(&server, "workspace_status", json!({ "workspace": uri })).await;
        assert_eq!(by_uri["structuredContent"]["workspace"]["id"], unique.id);

        let ambiguous = call_tool(
            &server,
            "workspace_status",
            json!({ "workspace": "duplicate" }),
        )
        .await;
        assert_eq!(ambiguous["isError"], true);
        assert!(
            ambiguous["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(&first.id)
        );
        assert!(
            ambiguous["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(&second.id)
        );

        let conflict = call_tool(
            &server,
            "memory_record",
            json!({
                "workspace_id": first.id,
                "workspace": unique_root.to_string_lossy(),
                "kind": "note",
                "content": "must not be written",
            }),
        )
        .await;
        assert_eq!(conflict["isError"], true);

        let unknown = call_tool(
            &server,
            "workspace_status",
            json!({ "workspace": "does-not-exist" }),
        )
        .await;
        assert_eq!(unknown["isError"], true);
        assert!(
            unknown["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("list registered workspaces")
        );

        let recorded = call_tool(
            &server,
            "memory_record",
            json!({ "kind": "note", "content": "resolved by adapter hint" }),
        )
        .await;
        assert_eq!(recorded["isError"], false);
        assert_eq!(recorded["structuredContent"]["workspace_id"], first.id);

        let recent = call_tool(&server, "memory_recent", json!({})).await;
        assert_eq!(recent["structuredContent"].as_array().unwrap().len(), 1);
        assert_eq!(
            recent["structuredContent"][0]["content"],
            "resolved by adapter hint"
        );

        let session = call_tool(&server, "session_start", json!({})).await;
        assert_eq!(session["isError"], false);
        let session_id = session["structuredContent"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let cross_workspace_end = call_tool(
            &server,
            "session_end",
            json!({ "workspace_id": unique.id, "session_id": session_id }),
        )
        .await;
        assert_eq!(cross_workspace_end["isError"], true);
        let ended = call_tool(&server, "session_end", json!({ "session_id": session_id })).await;
        assert_eq!(ended["isError"], false);

        let broken_hint = McpServer::with_workspace_hint(
            service,
            Some(WorkspaceHint::FileUri(
                "file:///definitely/not/a/registered/workspace".into(),
            )),
        );
        initialize(&broken_hint).await;
        let discoverable = call_tool(&broken_hint, "workspace_list", json!({})).await;
        assert_eq!(discoverable["isError"], false);
        assert_eq!(
            discoverable["structuredContent"]["workspaces"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        assert!(discoverable["structuredContent"]["default_hint_error"].is_string());
    }

    #[tokio::test]
    async fn mcp_mixed_language_workflow_observes_watcher_freshness_and_memory() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("workspace");
        fs::create_dir_all(&root).unwrap();
        let rust = root.join("lib.rs");
        let python = root.join("cache.py");
        let typescript = root.join("retry.ts");
        fs::write(&rust, "pub fn rust_marker() {}\n").unwrap();
        fs::write(&python, "class PythonMarker:\n    pass\n").unwrap();
        fs::write(&typescript, "export function typescriptMarker() {}\n").unwrap();
        let service = Arc::new(
            CortexWeaveService::from_parts_with_embeddings(
                AppConfig::default(),
                SqliteStorage::in_memory().await.unwrap(),
                Arc::new(FixtureProvider),
            )
            .unwrap(),
        );
        let workspace = service
            .register_workspace(root.to_string_lossy(), "mixed")
            .await
            .unwrap();
        let server = McpServer::new(Arc::clone(&service));
        initialize(&server).await;
        let watchers = server.start_workspace_watchers().await;
        assert_eq!(watchers.len(), 1);

        for (query, expected_path) in [
            ("rust_marker", "lib.rs"),
            ("pythonmarker", "cache.py"),
            ("typescriptmarker", "retry.ts"),
        ] {
            let result = call_tool(
                &server,
                "semantic_search",
                json!({ "workspace_id": workspace.id, "query": query }),
            )
            .await;
            assert_eq!(result["isError"], false);
            assert_eq!(result["structuredContent"][0]["path"], expected_path);
        }

        fs::write(&rust, "pub fn rust_marker_fresh() {}\n").unwrap();
        fs::write(&python, "class PythonMarkerFresh:\n    pass\n").unwrap();
        fs::write(&typescript, "export function typescriptMarkerFresh() {}\n").unwrap();
        for (query, fresh_marker) in [
            ("rust_marker", "rust_marker_fresh"),
            ("pythonmarker", "PythonMarkerFresh"),
            ("typescriptmarker", "typescriptMarkerFresh"),
        ] {
            wait_for_fresh_content(&server, &workspace.id, query, fresh_marker).await;
        }

        let session = call_tool(
            &server,
            "session_start",
            json!({ "workspace_id": workspace.id, "metadata": { "agent": "fixture" } }),
        )
        .await;
        let session_id = session["structuredContent"]["id"].as_str().unwrap();
        let decision = call_tool(
            &server,
            "memory_record",
            json!({
                "workspace_id": workspace.id,
                "session_id": session_id,
                "kind": "decision",
                "content": "Use BLAKE3 for deterministic change detection.",
            }),
        )
        .await;
        assert_eq!(decision["isError"], false);
        let ended = call_tool(&server, "session_end", json!({ "session_id": session_id })).await;
        assert_eq!(ended["isError"], false);

        let restarted = McpServer::new(service);
        initialize(&restarted).await;
        let new_session = call_tool(
            &restarted,
            "session_start",
            json!({ "workspace_id": workspace.id }),
        )
        .await;
        assert_eq!(new_session["isError"], false);
        let recovered = call_tool(
            &restarted,
            "memory_search",
            json!({ "workspace_id": workspace.id, "query": "Why are we using BLAKE3?" }),
        )
        .await;
        assert_eq!(recovered["isError"], false);
        assert_eq!(
            recovered["structuredContent"][0]["content"],
            "Use BLAKE3 for deterministic change detection."
        );

        for watcher in watchers {
            watcher.shutdown().await;
        }
    }

    async fn call_tool(server: &McpServer, name: &str, arguments: Value) -> Value {
        server
            .handle_json(json!({
                "jsonrpc": "2.0",
                "id": 99,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            }))
            .await
            .unwrap()["result"]
            .clone()
    }

    async fn initialize(server: &McpServer) {
        let response = server
            .handle_json(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "unsupported-version" },
            }))
            .await
            .unwrap();
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    async fn wait_for_fresh_content(
        server: &McpServer,
        workspace_id: &str,
        query: &str,
        fresh_marker: &str,
    ) {
        for _ in 0..30 {
            let result = call_tool(
                server,
                "semantic_search",
                json!({ "workspace_id": workspace_id, "query": query }),
            )
            .await;
            let fresh = result["structuredContent"]
                .as_array()
                .is_some_and(|results| {
                    results.iter().any(|result| {
                        result["content"]
                            .as_str()
                            .is_some_and(|content| content.contains(fresh_marker))
                    })
                });
            if fresh {
                return;
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!("watcher did not index fresh content containing {fresh_marker}");
    }

    fn match_marker(text: &str) -> Vec<f32> {
        if text.contains("rust_marker") {
            vec![1.0, 0.0, 0.0]
        } else if text.contains("pythonmarker") {
            vec![0.0, 1.0, 0.0]
        } else if text.contains("typescriptmarker") {
            vec![0.0, 0.0, 1.0]
        } else {
            vec![1.0, 1.0, 1.0]
        }
    }
}
