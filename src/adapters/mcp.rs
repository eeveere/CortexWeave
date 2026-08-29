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
    domain::{
        Checkpoint, ContextRequest, ContextSourceType, CortexEvent, EventType, MemoryKind,
        MemoryRecord, ResumeContextRequest, StructuralReadOptions,
    },
    indexing::{WorkspaceWatcher, WorkspaceWatcherHandle},
    workspace::WorkspaceSelector,
};

const PROTOCOL_VERSION: &str = "2025-06-18";
const MAX_MCP_FRAME_BYTES: usize = 1_048_576;
const MAX_TOOL_LIMIT: usize = 100;
const MAX_CONTEXT_TOKEN_BUDGET: usize = 65_536;
const MAX_MCP_GRAPH_NODES: usize = 100;
const MAX_MCP_GRAPH_EDGES: usize = 500;
const MAX_MCP_GRAPH_DEPTH: usize = 4;

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
            "semantic_context" => self.semantic_context(arguments).await,
            "resume_context" => self.resume_context(arguments).await,
            "working_set" => self.working_set(arguments).await,
            "context_pin" => self.context_pin(arguments).await,
            "context_unpin" => self.context_unpin(arguments).await,
            "checkpoint_create" => self.checkpoint_create(arguments).await,
            "checkpoint_latest" => self.checkpoint_latest(arguments).await,
            "semantic_get" => self.semantic_get(arguments).await,
            "graph_find" => self.graph_find(arguments).await,
            "graph_status" => self.graph_status(arguments).await,
            "graph_neighbors" => {
                self.graph_node_query(arguments, GraphNodeQuery::Neighbors)
                    .await
            }
            "graph_callers" => {
                self.graph_node_query(arguments, GraphNodeQuery::Callers)
                    .await
            }
            "graph_callees" => {
                self.graph_node_query(arguments, GraphNodeQuery::Callees)
                    .await
            }
            "graph_references" => {
                self.graph_node_query(arguments, GraphNodeQuery::References)
                    .await
            }
            "graph_implementations" => {
                self.graph_node_query(arguments, GraphNodeQuery::Implementations)
                    .await
            }
            "graph_tests" => {
                self.graph_node_query(arguments, GraphNodeQuery::Tests)
                    .await
            }
            "graph_dependencies" => {
                self.graph_node_query(arguments, GraphNodeQuery::Dependencies)
                    .await
            }
            "graph_dependents" => {
                self.graph_node_query(arguments, GraphNodeQuery::Dependents)
                    .await
            }
            "graph_impact_symbol" => self.graph_impact_symbol(arguments).await,
            "graph_impact_path" => self.graph_impact_path(arguments).await,
            "memory_record" => self.memory_record(arguments).await,
            "memory_search" => self.memory_search(arguments).await,
            "memory_recent" => self.memory_recent(arguments).await,
            "workspace_list" => self.workspace_list().await,
            "workspace_status" => self.workspace_status(arguments).await,
            "workspace_readiness" => self.workspace_readiness(arguments).await,
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

    async fn semantic_context(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let mut request = ContextRequest::new(&workspace.id);
        request.query = Some(required_string(args, "query")?.to_owned());
        request.session_id = optional_string(args, "session_id")?;
        request.task_id = optional_string(args, "task_id")?;
        request.token_budget = optional_context_budget(args, request.token_budget)?;
        request.include_code = optional_bool(args, "include_code", request.include_code)?;
        request.include_documents =
            optional_bool(args, "include_documents", request.include_documents)?;
        request.include_memories =
            optional_bool(args, "include_memories", request.include_memories)?;
        request.include_events = optional_bool(args, "include_events", request.include_events)?;
        request.path_scope = optional_strings(args, "path_scope")?;
        request.language_scope = optional_strings(args, "language_scope")?;
        request.include_explanation = optional_bool(args, "include_explanation", false)?;
        serialize_service(self.service.semantic_context(request).await)
    }

    async fn resume_context(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let mut request = ResumeContextRequest::new(&workspace.id);
        request.session_id = optional_string(args, "session_id")?;
        request.task_id = optional_string(args, "task_id")?;
        request.token_budget = optional_context_budget(args, request.token_budget)?;
        request.include_explanation = optional_bool(args, "include_explanation", false)?;
        serialize_service(self.service.resume_context(request).await)
    }

    async fn working_set(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        serialize_service(
            self.service
                .inspect_working_set(
                    &workspace.id,
                    required_string(args, "session_id")?,
                    optional_string(args, "task_id")?.as_deref(),
                )
                .await,
        )
    }

    async fn context_pin(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        serialize_service(
            self.service
                .pin_context(
                    &workspace.id,
                    required_string(args, "session_id")?,
                    optional_string(args, "task_id")?.as_deref(),
                    required_string(args, "source_id")?,
                    ContextSourceType::from_storage(required_string(args, "source_type")?),
                )
                .await,
        )
    }

    async fn context_unpin(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        serialize_service(
            self.service
                .unpin_context(
                    &workspace.id,
                    required_string(args, "session_id")?,
                    optional_string(args, "task_id")?.as_deref(),
                    required_string(args, "source_id")?,
                    ContextSourceType::from_storage(required_string(args, "source_type")?),
                )
                .await,
        )
    }

    async fn checkpoint_create(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let mut checkpoint = Checkpoint::new(
            &workspace.id,
            required_string(args, "session_id")?,
            required_string(args, "content")?,
        );
        checkpoint.task_id = optional_string(args, "task_id")?;
        checkpoint.objective = optional_string(args, "objective")?;
        checkpoint.completed = optional_strings(args, "completed")?;
        checkpoint.decision_ids = optional_strings(args, "decision_ids")?;
        checkpoint.open_problems = optional_strings(args, "open_problems")?;
        checkpoint.related_paths = optional_strings(args, "related_paths")?;
        checkpoint.related_symbols = optional_strings(args, "related_symbols")?;
        checkpoint.next_action = optional_string(args, "next_action")?;
        serialize_service(self.service.create_checkpoint(checkpoint).await)
    }

    async fn checkpoint_latest(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let checkpoint = match (
            optional_string(args, "session_id")?,
            optional_string(args, "task_id")?,
        ) {
            (Some(_session_id), Some(_)) => {
                return Err("checkpoint_latest accepts session_id or task_id, not both".into());
            }
            (Some(session_id), None) => {
                self.service
                    .latest_checkpoint_for_session(&workspace.id, &session_id)
                    .await
            }
            (None, Some(task_id)) => {
                self.service
                    .latest_checkpoint_for_task(&workspace.id, &task_id)
                    .await
            }
            (None, None) => self.service.latest_checkpoint(&workspace.id).await,
        };
        serialize_service(checkpoint)
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

    async fn graph_find(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let options = graph_read_options(args)?;
        serialize_service(
            self.service
                .graph_find_symbol(
                    &workspace.id,
                    required_string(args, "symbol_or_path")?,
                    &options,
                )
                .await,
        )
    }

    async fn graph_status(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        serialize_service(self.service.workspace_graph_status(&workspace.id).await)
    }

    async fn graph_node_query(
        &self,
        args: &Map<String, Value>,
        query: GraphNodeQuery,
    ) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let options = graph_read_options(args)?;
        let node_id = required_string(args, "node_id")?;
        let result = match query {
            GraphNodeQuery::Neighbors => {
                self.service
                    .graph_neighbors(&workspace.id, node_id, &options)
                    .await
            }
            GraphNodeQuery::Callers => {
                self.service
                    .graph_callers(&workspace.id, node_id, &options)
                    .await
            }
            GraphNodeQuery::Callees => {
                self.service
                    .graph_callees(&workspace.id, node_id, &options)
                    .await
            }
            GraphNodeQuery::References => {
                self.service
                    .graph_references(&workspace.id, node_id, &options)
                    .await
            }
            GraphNodeQuery::Implementations => {
                self.service
                    .graph_implementations(&workspace.id, node_id, &options)
                    .await
            }
            GraphNodeQuery::Tests => {
                self.service
                    .graph_tests(&workspace.id, node_id, &options)
                    .await
            }
            GraphNodeQuery::Dependencies => {
                self.service
                    .graph_dependencies(&workspace.id, node_id, &options)
                    .await
            }
            GraphNodeQuery::Dependents => {
                self.service
                    .graph_dependents(&workspace.id, node_id, &options)
                    .await
            }
        };
        serialize_service(result)
    }

    async fn graph_impact_symbol(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let options = graph_read_options(args)?;
        serialize_service(
            self.service
                .graph_impact_symbol(&workspace.id, required_string(args, "symbol")?, &options)
                .await,
        )
    }

    async fn graph_impact_path(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        let options = graph_read_options(args)?;
        serialize_service(
            self.service
                .graph_impact_path(&workspace.id, required_string(args, "path")?, &options)
                .await,
        )
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

    async fn workspace_readiness(&self, args: &Map<String, Value>) -> ToolResult {
        let workspace = self.resolve_workspace(args).await?;
        serialize_service(self.service.workspace_readiness(&workspace.id).await)
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

#[derive(Clone, Copy)]
enum GraphNodeQuery {
    Neighbors,
    Callers,
    Callees,
    References,
    Implementations,
    Tests,
    Dependencies,
    Dependents,
}

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
            "semantic_context",
            "Assemble one bounded, task-aware context packet from current code, memories, events, and session state. Use this first when asked to gather context or to answer from a context packet; it performs retrieval and selection in one call.",
            workspace_properties(json!({
                "query": string_schema(),
                "session_id": string_schema(),
                "task_id": string_schema(),
                "token_budget": context_budget_schema(),
                "include_code": { "type": "boolean" },
                "include_documents": { "type": "boolean" },
                "include_memories": { "type": "boolean" },
                "include_events": { "type": "boolean" },
                "path_scope": { "type": "array", "items": string_schema() },
                "language_scope": { "type": "array", "items": string_schema() }
                ,"include_explanation": { "type": "boolean" }
            })),
            &["query"],
        ),
        tool(
            "resume_context",
            "Reconstruct bounded, transcript-free context for the current or selected task.",
            workspace_properties(json!({
                "session_id": string_schema(),
                "task_id": string_schema(),
                "token_budget": context_budget_schema(),
                "include_explanation": { "type": "boolean" }
            })),
            &[],
        ),
        tool(
            "working_set",
            "Inspect the decayed working set and pins for a session.",
            workspace_properties(
                json!({ "session_id": string_schema(), "task_id": string_schema() }),
            ),
            &["session_id"],
        ),
        tool(
            "context_pin",
            "Pin a context source so it remains in the session working set.",
            workspace_properties(
                json!({ "session_id": string_schema(), "task_id": string_schema(), "source_id": string_schema(), "source_type": string_schema() }),
            ),
            &["session_id", "source_id", "source_type"],
        ),
        tool(
            "context_unpin",
            "Remove a context source pin from a session.",
            workspace_properties(
                json!({ "session_id": string_schema(), "task_id": string_schema(), "source_id": string_schema(), "source_type": string_schema() }),
            ),
            &["session_id", "source_id", "source_type"],
        ),
        tool(
            "checkpoint_create",
            "Persist explicit structured checkpoint state for an active session.",
            workspace_properties(json!({
                "session_id": string_schema(), "task_id": string_schema(), "content": string_schema(),
                "objective": string_schema(), "completed": { "type": "array", "items": string_schema() },
                "decision_ids": { "type": "array", "items": string_schema() },
                "open_problems": { "type": "array", "items": string_schema() },
                "related_paths": { "type": "array", "items": string_schema() },
                "related_symbols": { "type": "array", "items": string_schema() }, "next_action": string_schema()
            })),
            &["session_id", "content"],
        ),
        tool(
            "checkpoint_latest",
            "Read the latest checkpoint in a workspace, session, or task scope.",
            workspace_properties(
                json!({ "session_id": string_schema(), "task_id": string_schema() }),
            ),
            &[],
        ),
        tool(
            "semantic_get",
            "Get one indexed code item and its provenance by an exact chunk UUID returned by a prior CortexWeave search or context result. Do not use this for a filename, symbol name, or natural-language query.",
            workspace_properties(json!({ "chunk_id": string_schema() })),
            &["chunk_id"],
        ),
        tool(
            "graph_status",
            "Get graph revision, staleness, counts, unresolved relationships, and active analyzer capabilities for the resolved workspace.",
            workspace_properties(json!({})),
            &[],
        ),
        tool(
            "graph_find",
            "Find exact graph symbols or a source path. Use returned node IDs with graph relation tools. Reads require a current graph unless allow_stale is explicitly true.",
            graph_properties(json!({ "symbol_or_path": string_schema() })),
            &["symbol_or_path"],
        ),
        tool(
            "graph_neighbors",
            "Return bounded incoming and outgoing graph neighbors for one exact graph node ID.",
            graph_properties(json!({ "node_id": string_schema() })),
            &["node_id"],
        ),
        tool(
            "graph_callers",
            "Return bounded direct callers for one exact graph node ID.",
            graph_properties(json!({ "node_id": string_schema() })),
            &["node_id"],
        ),
        tool(
            "graph_callees",
            "Return bounded direct callees for one exact graph node ID.",
            graph_properties(json!({ "node_id": string_schema() })),
            &["node_id"],
        ),
        tool(
            "graph_references",
            "Return bounded direct references to one exact graph node ID.",
            graph_properties(json!({ "node_id": string_schema() })),
            &["node_id"],
        ),
        tool(
            "graph_implementations",
            "Return bounded implementations, inheritance, and overrides for one exact graph node ID.",
            graph_properties(json!({ "node_id": string_schema() })),
            &["node_id"],
        ),
        tool(
            "graph_tests",
            "Return bounded explicit tests directly associated with one exact graph node ID; these are likely direct-call associations, not proven coverage.",
            graph_properties(json!({ "node_id": string_schema() })),
            &["node_id"],
        ),
        tool(
            "graph_dependencies",
            "Return bounded direct import and dependency targets for one exact graph node ID.",
            graph_properties(json!({ "node_id": string_schema() })),
            &["node_id"],
        ),
        tool(
            "graph_dependents",
            "Return bounded direct import and dependency sources for one exact graph node ID.",
            graph_properties(json!({ "node_id": string_schema() })),
            &["node_id"],
        ),
        tool(
            "graph_impact_symbol",
            "Return a bounded reverse-dependency impact report for an exact symbol, including typed paths and confidence. This is graph reachability, not a git-diff prediction.",
            graph_properties(json!({ "symbol": string_schema() })),
            &["symbol"],
        ),
        tool(
            "graph_impact_path",
            "Return a bounded reverse-dependency impact report for declarations in one indexed relative path, including typed paths and confidence.",
            graph_properties(json!({ "path": string_schema() })),
            &["path"],
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
            "workspace_readiness",
            "Inspect analyzer coverage, generic fallback use, and explicit reindex cost for the resolved workspace without changing configuration or starting a reindex.",
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

fn graph_properties(properties: Value) -> Value {
    let mut properties = workspace_properties(properties);
    let properties = properties
        .as_object_mut()
        .expect("graph tool properties must be an object");
    properties.insert("allow_stale".into(), json!({ "type": "boolean" }));
    properties.insert(
        "max_nodes".into(),
        json!({ "type": "integer", "minimum": 1, "maximum": MAX_MCP_GRAPH_NODES }),
    );
    properties.insert(
        "max_edges".into(),
        json!({ "type": "integer", "minimum": 1, "maximum": MAX_MCP_GRAPH_EDGES }),
    );
    properties.insert(
        "max_depth".into(),
        json!({ "type": "integer", "minimum": 1, "maximum": MAX_MCP_GRAPH_DEPTH }),
    );
    properties.clone().into()
}

fn limit_schema() -> Value {
    json!({ "type": "integer", "minimum": 0, "maximum": MAX_TOOL_LIMIT })
}

fn context_budget_schema() -> Value {
    json!({ "type": "integer", "minimum": 0, "maximum": MAX_CONTEXT_TOKEN_BUDGET })
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

fn optional_context_budget(args: &Map<String, Value>, default: usize) -> Result<usize, String> {
    let Some(value) = args.get("token_budget") else {
        return Ok(default);
    };
    value
        .as_u64()
        .ok_or_else(|| "token_budget must be a non-negative integer".into())
        .and_then(|value| usize::try_from(value).map_err(|_| "token_budget is too large".into()))
        .and_then(|value| {
            (value <= MAX_CONTEXT_TOKEN_BUDGET)
                .then_some(value)
                .ok_or_else(|| format!("token_budget must be at most {MAX_CONTEXT_TOKEN_BUDGET}"))
        })
}

fn graph_read_options(args: &Map<String, Value>) -> Result<StructuralReadOptions, String> {
    let defaults = StructuralReadOptions::default();
    Ok(StructuralReadOptions {
        allow_stale: optional_bool(args, "allow_stale", false)?,
        max_nodes: optional_bounded_positive(
            args,
            "max_nodes",
            defaults.max_nodes,
            MAX_MCP_GRAPH_NODES,
        )?,
        max_edges: optional_bounded_positive(
            args,
            "max_edges",
            defaults.max_edges,
            MAX_MCP_GRAPH_EDGES,
        )?,
        max_depth: optional_bounded_positive(
            args,
            "max_depth",
            defaults.max_depth,
            MAX_MCP_GRAPH_DEPTH,
        )?,
    })
}

fn optional_bounded_positive(
    args: &Map<String, Value>,
    name: &str,
    default: usize,
    maximum: usize,
) -> Result<usize, String> {
    let Some(value) = args.get(name) else {
        return Ok(default);
    };
    value
        .as_u64()
        .ok_or_else(|| format!("{name} must be a positive integer"))
        .and_then(|value| usize::try_from(value).map_err(|_| format!("{name} is too large")))
        .and_then(|value| {
            (1..=maximum)
                .contains(&value)
                .then_some(value)
                .ok_or_else(|| format!("{name} must be between 1 and {maximum}"))
        })
}

fn optional_bool(args: &Map<String, Value>, name: &str, default: bool) -> Result<bool, String> {
    args.get(name)
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| format!("{name} must be a boolean when supplied"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
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
    use chrono::Utc;
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
            "semantic_context",
            "resume_context",
            "working_set",
            "context_pin",
            "context_unpin",
            "checkpoint_create",
            "checkpoint_latest",
            "semantic_get",
            "memory_record",
            "memory_search",
            "memory_recent",
            "workspace_list",
            "workspace_status",
            "workspace_reindex",
            "graph_status",
            "graph_find",
            "graph_callers",
            "graph_implementations",
            "graph_impact_symbol",
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
        let context = server
            .handle_json(json!({
                "jsonrpc": "2.0",
                "id": 5,
                "method": "tools/call",
                "params": {
                    "name": "semantic_context",
                    "arguments": {
                        "workspace_id": workspace_id,
                        "query": "Why BLAKE3?",
                        "token_budget": 256,
                        "include_code": false,
                        "include_documents": false,
                        "include_events": false,
                    },
                },
            }))
            .await
            .unwrap();
        assert_eq!(context["result"]["isError"], false);
        assert_eq!(context["result"]["structuredContent"]["token_budget"], 256);
        assert!(
            context["result"]["structuredContent"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["source_type"] == "memory")
        );
        let invalid = server
            .handle_json(json!({
                "jsonrpc": "2.0",
                "id": 6,
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
    async fn exposes_resume_and_checkpoint_tools_as_thin_service_calls() {
        let (server, workspace_id) = server().await;
        initialize(&server).await;
        let session = call_tool(
            &server,
            "session_start",
            json!({ "workspace_id": workspace_id }),
        )
        .await;
        let session_id = session["structuredContent"]["id"].as_str().unwrap();
        let checkpoint = call_tool(
            &server,
            "checkpoint_create",
            json!({ "workspace_id": workspace_id, "session_id": session_id, "content": "Continue context exposure." }),
        )
        .await;
        assert_eq!(checkpoint["isError"], false);
        let resume = call_tool(
            &server,
            "resume_context",
            json!({ "workspace_id": workspace_id, "token_budget": 256 }),
        )
        .await;
        assert_eq!(resume["isError"], false);
        assert_eq!(
            resume["structuredContent"]["selected_session"]["id"],
            session_id
        );
        let latest = call_tool(
            &server,
            "checkpoint_latest",
            json!({ "workspace_id": workspace_id, "session_id": session_id }),
        )
        .await;
        assert_eq!(latest["isError"], false);
        assert_eq!(
            latest["structuredContent"]["content"],
            "Continue context exposure."
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

    #[tokio::test]
    async fn graph_tools_are_bounded_current_by_default_and_explainable() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("workspace");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("architecture.test.ts"),
            "export interface Interface { execute(): void; }\n\
             export class ImplA implements Interface { execute(): void {} }\n\
             export class ImplB implements Interface { execute(): void {} }\n\
             export function service() { ImplA(); }\n\
             test('test_a', () => { ImplA(); });\n",
        )
        .unwrap();
        let service = Arc::new(
            CortexWeaveService::from_parts_with_embeddings(
                AppConfig::default(),
                SqliteStorage::in_memory().await.unwrap(),
                Arc::new(FixtureProvider),
            )
            .unwrap(),
        );
        let workspace = service
            .register_workspace(root.to_string_lossy(), "graph")
            .await
            .unwrap();
        service.workspace_reindex(&workspace.id).await.unwrap();
        let server = McpServer::new(Arc::clone(&service));
        initialize(&server).await;

        let graph_status = call_tool(
            &server,
            "graph_status",
            json!({ "workspace_id": workspace.id }),
        )
        .await;
        assert_eq!(graph_status["isError"], false);
        assert_eq!(graph_status["structuredContent"]["is_current"], true);
        assert!(graph_status["structuredContent"]["nodes"].as_u64().unwrap() >= 1);

        let found = call_tool(
            &server,
            "graph_find",
            json!({ "workspace_id": workspace.id, "symbol_or_path": "Interface" }),
        )
        .await;
        assert_eq!(found["isError"], false);
        assert_eq!(found["structuredContent"]["limits"]["max_nodes"], 50);
        let interface_id = found["structuredContent"]["seeds"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let implementations = call_tool(
            &server,
            "graph_implementations",
            json!({ "workspace_id": workspace.id, "node_id": interface_id, "max_nodes": 2 }),
        )
        .await;
        assert_eq!(implementations["isError"], false);
        assert_eq!(
            implementations["structuredContent"]["limits"]["max_nodes"],
            2
        );
        assert!(
            implementations["structuredContent"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["name"] == "ImplA")
        );

        let impact = call_tool(
            &server,
            "graph_impact_symbol",
            json!({ "workspace_id": workspace.id, "symbol": "Interface", "max_depth": 3 }),
        )
        .await;
        assert_eq!(impact["isError"], false);
        assert_eq!(impact["structuredContent"]["limits"]["max_depth"], 3);
        assert!(
            impact["structuredContent"]["impacts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["node"]["name"] == "service")
        );

        let invalid_bound = call_tool(
            &server,
            "graph_find",
            json!({ "workspace_id": workspace.id, "symbol_or_path": "Interface", "max_nodes": 101 }),
        )
        .await;
        assert_eq!(invalid_bound["isError"], true);
        assert!(
            invalid_bound["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("max_nodes must be between 1 and 100")
        );

        let revision = service
            .storage()
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        service
            .storage()
            .mark_graph_error(
                &workspace.id,
                revision.content_revision,
                "simulated graph failure",
                Utc::now(),
            )
            .await
            .unwrap();
        let current_only = call_tool(
            &server,
            "graph_find",
            json!({ "workspace_id": workspace.id, "symbol_or_path": "Interface" }),
        )
        .await;
        assert_eq!(current_only["isError"], true);
        let stale = call_tool(
            &server,
            "graph_find",
            json!({ "workspace_id": workspace.id, "symbol_or_path": "Interface", "allow_stale": true }),
        )
        .await;
        assert_eq!(stale["isError"], false);
        assert_eq!(
            stale["structuredContent"]["snapshot"]["graph_state"],
            "error"
        );
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
