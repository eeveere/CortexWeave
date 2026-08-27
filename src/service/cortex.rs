use std::{path::Path, sync::Arc};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    AppConfig, CortexError, Result,
    domain::{
        Checkpoint, ContextCandidatePool, ContextPin, ContextRequest, ContextSourceType,
        CortexEvent, MemoryRecord, Session, Task, TaskStatus, TemporalContextItem, TemporalQuery,
        WorkingSetEntry, WorkingSetSnapshot, Workspace,
    },
    embedding::{
        EmbeddingLimits, EmbeddingProvider, OpenAiCompatibleEmbeddingProvider, TokenCountAccuracy,
    },
    indexing::{IndexingService, WorkspaceReindexOutcome},
    instrumentation::{InstrumentationSnapshot, RuntimeMetrics, WorkspaceResolutionKind, snapshot},
    parsing::AnalyzerRegistry,
    retrieval::{RetrievalResult, RetrievalService},
    service::ContextService,
    storage::SqliteStorage,
    workspace::{PathIdentity, WorkspaceSelector},
};

const MAX_COLLECTION_LIMIT: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceStatus {
    pub workspace: Workspace,
    pub documents_indexed: usize,
    pub chunks_indexed: usize,
    pub last_indexed_at: Option<DateTime<Utc>>,
    pub embedding_model: String,
    pub embedding_space: String,
    pub embedding_limits: EmbeddingLimits,
    pub token_counter: String,
    pub token_counter_accuracy: TokenCountAccuracy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceCatalog {
    pub workspaces: Vec<Workspace>,
    pub default_hint_match: Option<Workspace>,
    pub default_hint_error: Option<String>,
}

pub struct CortexWeaveService {
    config: Arc<AppConfig>,
    storage: Arc<SqliteStorage>,
    analyzers: Arc<AnalyzerRegistry>,
    embeddings: Arc<dyn EmbeddingProvider>,
    indexing: Arc<IndexingService>,
    retrieval: Arc<RetrievalService>,
    context: Arc<ContextService>,
    metrics: Arc<RuntimeMetrics>,
}

impl CortexWeaveService {
    pub async fn open(config: AppConfig) -> Result<Self> {
        let storage = SqliteStorage::open(&config.database.path).await?;
        Self::from_parts(config, storage)
    }

    pub fn from_parts(config: AppConfig, storage: SqliteStorage) -> Result<Self> {
        let embeddings = Arc::new(OpenAiCompatibleEmbeddingProvider::new(&config.embedding)?);
        Self::from_parts_with_embeddings(config, storage, embeddings)
    }

    pub fn from_parts_with_embeddings(
        config: AppConfig,
        storage: SqliteStorage,
        embeddings: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self> {
        config.validate()?;
        let storage = Arc::new(storage);
        let analyzers = Arc::new(AnalyzerRegistry::configured(
            &config.languages,
            &config.indexing.generic_chunks,
        ));
        let metrics = Arc::new(RuntimeMetrics::default());
        let indexing = Arc::new(IndexingService::with_config(
            Arc::clone(&storage),
            Arc::clone(&embeddings),
            Arc::clone(&analyzers),
            &config.indexing,
            Arc::clone(&metrics),
        ));
        let retrieval = Arc::new(RetrievalService::with_metrics(
            Arc::clone(&storage),
            Arc::clone(&embeddings),
            config.retrieval.semantic_weight,
            config.retrieval.lexical_weight,
            Arc::clone(&metrics),
        )?);
        let context = Arc::new(ContextService::new(
            Arc::clone(&storage),
            Arc::clone(&retrieval),
            config.working_set.clone(),
            config.temporal.clone(),
            config.context.clone(),
        )?);
        Ok(Self {
            config: Arc::new(config),
            storage,
            analyzers,
            embeddings,
            indexing,
            retrieval,
            context,
            metrics,
        })
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn storage(&self) -> &SqliteStorage {
        &self.storage
    }

    pub fn analyzers(&self) -> &AnalyzerRegistry {
        &self.analyzers
    }

    pub fn embeddings(&self) -> &dyn EmbeddingProvider {
        self.embeddings.as_ref()
    }

    pub fn indexing(&self) -> &IndexingService {
        &self.indexing
    }

    pub fn indexing_handle(&self) -> Arc<IndexingService> {
        Arc::clone(&self.indexing)
    }

    pub fn retrieval(&self) -> &RetrievalService {
        &self.retrieval
    }

    pub fn context(&self) -> &ContextService {
        &self.context
    }

    pub fn storage_handle(&self) -> Arc<SqliteStorage> {
        Arc::clone(&self.storage)
    }

    pub async fn register_workspace(
        &self,
        root_path: impl Into<String>,
        name: impl Into<String>,
    ) -> Result<Workspace> {
        let root_path = root_path.into();
        let name = name.into();
        if root_path.trim().is_empty() || name.trim().is_empty() {
            return Err(CortexError::Analysis(
                "workspace root path and name cannot be empty".into(),
            ));
        }
        let root_identity = PathIdentity::existing_directory(Path::new(&root_path))?;
        for existing in self.storage.list_workspaces().await? {
            let existing_identity = PathIdentity::from_path(Path::new(&existing.root_path))?;
            if existing_identity.comparison_key() == root_identity.comparison_key() {
                return Ok(existing);
            }
        }
        let workspace = Workspace::new(root_identity.display_path().to_string_lossy(), name);
        self.storage.insert_workspace_if_absent(&workspace).await
    }

    pub async fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        self.storage.list_workspaces().await
    }

    pub async fn resolve_workspace(
        &self,
        selector: WorkspaceSelector,
        default_hint: Option<WorkspaceSelector>,
    ) -> Result<Workspace> {
        let workspaces = self.storage.list_workspaces().await?;
        let kind = workspace_resolution_kind(&selector, default_hint.as_ref(), workspaces.len());
        let result = resolve_workspace_with_default(&workspaces, selector, default_hint);
        self.metrics.record_workspace_resolution(kind, &result);
        result
    }

    pub async fn resolve_workspace_selection(
        &self,
        workspace_id: Option<String>,
        workspace: Option<WorkspaceSelector>,
        default_hint: Option<WorkspaceSelector>,
    ) -> Result<Workspace> {
        let workspaces = self.storage.list_workspaces().await?;
        let kind = workspace_selection_kind(
            workspace_id.as_deref(),
            workspace.as_ref(),
            default_hint.as_ref(),
            workspaces.len(),
        );
        let result = match (workspace_id, workspace) {
            (Some(id), Some(selector)) => {
                let by_id = resolve_explicit_workspace(&workspaces, WorkspaceSelector::Id(id))?;
                let by_selector = resolve_explicit_workspace(&workspaces, selector)?;
                if by_id.id != by_selector.id {
                    return Err(workspace_ambiguity(
                        "workspace_id and workspace select different registrations",
                        &[by_id, by_selector],
                    ));
                }
                Ok(by_id)
            }
            (Some(id), None) => resolve_explicit_workspace(&workspaces, WorkspaceSelector::Id(id)),
            (None, Some(selector)) => resolve_explicit_workspace(&workspaces, selector),
            (None, None) => resolve_workspace_with_default(
                &workspaces,
                WorkspaceSelector::Default,
                default_hint,
            ),
        };
        self.metrics.record_workspace_resolution(kind, &result);
        result
    }

    pub async fn workspace_catalog(
        &self,
        default_hint: Option<WorkspaceSelector>,
    ) -> Result<WorkspaceCatalog> {
        let workspaces = self.storage.list_workspaces().await?;
        let (default_hint_match, default_hint_error) = match default_hint {
            Some(WorkspaceSelector::Default) | None => (None, None),
            Some(hint) => match resolve_explicit_workspace(&workspaces, hint) {
                Ok(workspace) => (Some(workspace), None),
                Err(error) => (None, Some(error.to_string())),
            },
        };
        Ok(WorkspaceCatalog {
            workspaces,
            default_hint_match,
            default_hint_error,
        })
    }

    pub async fn workspace_status(&self, workspace_id: &str) -> Result<WorkspaceStatus> {
        let workspace = self.require_workspace(workspace_id).await?;
        let documents = self.storage.list_documents(workspace_id).await?;
        let mut chunks_indexed = 0;
        for document in &documents {
            chunks_indexed += self.storage.list_chunks(&document.id).await?.len();
        }
        Ok(WorkspaceStatus {
            workspace,
            documents_indexed: documents.len(),
            chunks_indexed,
            last_indexed_at: documents.iter().map(|document| document.indexed_at).max(),
            embedding_model: self.embeddings.request_model_name().to_owned(),
            embedding_space: self.embeddings.embedding_space_id().to_owned(),
            embedding_limits: self.embeddings.limits(),
            token_counter: self.embeddings.token_counter_id().to_owned(),
            token_counter_accuracy: self.embeddings.token_counter_accuracy(),
        })
    }

    pub async fn instrumentation(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<InstrumentationSnapshot> {
        if let Some(workspace_id) = workspace_id {
            self.require_workspace(workspace_id).await?;
        }
        snapshot(
            &self.storage,
            &self.metrics,
            workspace_id,
            self.embeddings.as_ref(),
        )
        .await
    }

    pub async fn workspace_reindex(&self, workspace_id: &str) -> Result<WorkspaceReindexOutcome> {
        let workspace = self.require_workspace(workspace_id).await?;
        self.indexing.reindex_workspace(&workspace).await
    }

    pub async fn semantic_search(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        self.retrieval
            .semantic_search(workspace_id, query, limit)
            .await
    }

    pub async fn lexical_search(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        self.retrieval
            .lexical_search(workspace_id, query, limit)
            .await
    }

    pub async fn hybrid_search(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RetrievalResult>> {
        self.retrieval
            .hybrid_search(workspace_id, query, limit)
            .await
    }

    pub async fn get_item(
        &self,
        workspace_id: &str,
        chunk_id: &str,
    ) -> Result<Option<RetrievalResult>> {
        self.retrieval.get_item(workspace_id, chunk_id).await
    }

    pub async fn temporal_retrieval(
        &self,
        query: TemporalQuery,
    ) -> Result<Vec<TemporalContextItem>> {
        self.context.temporal_retrieval(query).await
    }

    pub async fn build_candidate_pool(
        &self,
        request: ContextRequest,
    ) -> Result<ContextCandidatePool> {
        self.context.build_candidate_pool(request).await
    }

    pub async fn activate_context_source(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
        source_id: &str,
        source_type: ContextSourceType,
    ) -> Result<WorkingSetEntry> {
        self.context
            .activate_source(workspace_id, session_id, task_id, source_id, source_type)
            .await
    }

    pub async fn inspect_working_set(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
    ) -> Result<WorkingSetSnapshot> {
        self.context
            .inspect_working_set(workspace_id, session_id, task_id)
            .await
    }

    pub async fn pin_context(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
        source_id: &str,
        source_type: ContextSourceType,
    ) -> Result<ContextPin> {
        self.context
            .pin_context(workspace_id, session_id, task_id, source_id, source_type)
            .await
    }

    pub async fn unpin_context(
        &self,
        workspace_id: &str,
        session_id: &str,
        task_id: Option<&str>,
        source_id: &str,
        source_type: ContextSourceType,
    ) -> Result<bool> {
        self.context
            .unpin_context(workspace_id, session_id, task_id, source_id, source_type)
            .await
    }

    pub async fn create_checkpoint(&self, checkpoint: Checkpoint) -> Result<Checkpoint> {
        self.context.create_checkpoint(checkpoint).await
    }

    pub async fn latest_checkpoint(&self, workspace_id: &str) -> Result<Option<Checkpoint>> {
        self.context.latest_checkpoint(workspace_id).await
    }

    pub async fn latest_checkpoint_for_session(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Option<Checkpoint>> {
        self.context
            .latest_checkpoint_for_session(workspace_id, session_id)
            .await
    }

    pub async fn latest_checkpoint_for_task(
        &self,
        workspace_id: &str,
        task_id: &str,
    ) -> Result<Option<Checkpoint>> {
        self.context
            .latest_checkpoint_for_task(workspace_id, task_id)
            .await
    }

    pub async fn start_session(&self, workspace_id: &str, metadata: Value) -> Result<Session> {
        self.require_workspace(workspace_id).await?;
        let session = Session::new(workspace_id, metadata);
        self.storage.insert_session(&session).await?;
        Ok(session)
    }

    pub async fn end_session(&self, session_id: &str) -> Result<Session> {
        self.end_session_scoped(None, session_id).await
    }

    pub async fn end_session_in_workspace(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Session> {
        self.end_session_scoped(Some(workspace_id), session_id)
            .await
    }

    async fn end_session_scoped(
        &self,
        expected_workspace_id: Option<&str>,
        session_id: &str,
    ) -> Result<Session> {
        if let Some(workspace_id) = expected_workspace_id {
            self.require_workspace(workspace_id).await?;
        }
        let session = self
            .storage
            .get_session(session_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))?;
        if expected_workspace_id.is_some_and(|workspace_id| session.workspace_id != workspace_id) {
            return Err(CortexError::Analysis(
                "session belongs to a different workspace".into(),
            ));
        }
        if session.ended_at.is_some() {
            return Err(CortexError::Analysis(format!(
                "session {session_id} is already ended"
            )));
        }
        self.storage.end_session(session_id, Utc::now()).await?;
        self.storage
            .get_session(session_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))
    }

    pub async fn start_task(
        &self,
        workspace_id: &str,
        session_id: Option<String>,
        title: impl Into<String>,
        details: Value,
    ) -> Result<Task> {
        self.require_workspace(workspace_id).await?;
        let title = title.into();
        if title.trim().is_empty() {
            return Err(CortexError::Analysis("task title cannot be empty".into()));
        }
        if let Some(session_id) = &session_id {
            let session = self
                .storage
                .get_session(session_id)
                .await?
                .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))?;
            if session.workspace_id != workspace_id {
                return Err(CortexError::Analysis(
                    "task session belongs to a different workspace".into(),
                ));
            }
            if session.ended_at.is_some() {
                return Err(CortexError::Analysis(
                    "cannot start a task on an ended session".into(),
                ));
            }
        }
        let mut task = Task::new(workspace_id, session_id, title, details);
        task.status = TaskStatus::Active;
        self.storage.insert_task(&task).await?;
        Ok(task)
    }

    pub async fn update_task(
        &self,
        task_id: &str,
        status: TaskStatus,
        details: Value,
    ) -> Result<Task> {
        let current = self
            .storage
            .get_task(task_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))?;
        if matches!(
            current.status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        ) {
            return Err(CortexError::Analysis(format!(
                "cannot transition terminal task {task_id}"
            )));
        }
        self.storage
            .update_task_status(task_id, status, &details)
            .await?;
        self.storage
            .get_task(task_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))
    }

    pub async fn complete_task(&self, task_id: &str, details: Value) -> Result<Task> {
        self.update_task(task_id, TaskStatus::Completed, details)
            .await
    }

    pub async fn record_memory(&self, mut memory: MemoryRecord) -> Result<MemoryRecord> {
        if memory.content.trim().is_empty() {
            return Err(CortexError::Analysis(
                "memory content cannot be empty".into(),
            ));
        }
        self.validate_provenance(&memory.workspace_id, &memory.session_id, &memory.task_id)
            .await?;
        memory.metadata = memory.metadata_for_storage();
        self.storage.insert_memory(&memory).await?;
        Ok(memory)
    }

    pub async fn search_memories(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        let limit = limit.min(MAX_COLLECTION_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(match_query) = memory_fts_query(query) else {
            return Ok(Vec::new());
        };
        self.storage
            .search_memories(workspace_id, &match_query, limit)
            .await
    }

    pub async fn recent_memories(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        let limit = limit.min(MAX_COLLECTION_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.storage.recent_memories(workspace_id, limit).await
    }

    pub async fn record_event(&self, event: CortexEvent) -> Result<CortexEvent> {
        self.validate_provenance(&event.workspace_id, &event.session_id, &event.task_id)
            .await?;
        self.storage.insert_event(&event).await?;
        Ok(event)
    }

    pub async fn recent_events(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<CortexEvent>> {
        let limit = limit.min(MAX_COLLECTION_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.storage.recent_events(workspace_id, limit).await
    }

    async fn require_workspace(&self, workspace_id: &str) -> Result<Workspace> {
        self.storage
            .get_workspace(workspace_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("workspace {workspace_id}")))
    }

    async fn validate_provenance(
        &self,
        workspace_id: &str,
        session_id: &Option<String>,
        task_id: &Option<String>,
    ) -> Result<()> {
        self.require_workspace(workspace_id).await?;
        let session = match session_id {
            Some(session_id) => Some(
                self.storage
                    .get_session(session_id)
                    .await?
                    .ok_or_else(|| CortexError::NotFound(format!("session {session_id}")))?,
            ),
            None => None,
        };
        let task = match task_id {
            Some(task_id) => Some(
                self.storage
                    .get_task(task_id)
                    .await?
                    .ok_or_else(|| CortexError::NotFound(format!("task {task_id}")))?,
            ),
            None => None,
        };
        if session
            .as_ref()
            .is_some_and(|session| session.workspace_id != workspace_id)
            || task
                .as_ref()
                .is_some_and(|task| task.workspace_id != workspace_id)
        {
            return Err(CortexError::Analysis(
                "provenance belongs to a different workspace".into(),
            ));
        }
        if let (Some(session), Some(task)) = (session, task)
            && task.session_id.as_deref() != Some(&session.id)
        {
            return Err(CortexError::Analysis(
                "task and session provenance do not match".into(),
            ));
        }
        Ok(())
    }
}

fn workspace_resolution_kind(
    selector: &WorkspaceSelector,
    default_hint: Option<&WorkspaceSelector>,
    workspace_count: usize,
) -> WorkspaceResolutionKind {
    match selector {
        WorkspaceSelector::Id(_) => WorkspaceResolutionKind::Id,
        WorkspaceSelector::Name(_) => WorkspaceResolutionKind::Name,
        WorkspaceSelector::RootPath(_) | WorkspaceSelector::FileUri(_) => {
            WorkspaceResolutionKind::Path
        }
        WorkspaceSelector::Default if default_hint.is_none() && workspace_count == 1 => {
            WorkspaceResolutionKind::Singleton
        }
        WorkspaceSelector::Default => WorkspaceResolutionKind::Default,
    }
}

fn workspace_selection_kind(
    workspace_id: Option<&str>,
    workspace: Option<&WorkspaceSelector>,
    default_hint: Option<&WorkspaceSelector>,
    workspace_count: usize,
) -> WorkspaceResolutionKind {
    if workspace_id.is_some() {
        WorkspaceResolutionKind::Id
    } else if let Some(selector) = workspace {
        workspace_resolution_kind(selector, None, workspace_count)
    } else if default_hint.is_none() && workspace_count == 1 {
        WorkspaceResolutionKind::Singleton
    } else {
        WorkspaceResolutionKind::Default
    }
}

fn resolve_workspace_with_default(
    workspaces: &[Workspace],
    selector: WorkspaceSelector,
    default_hint: Option<WorkspaceSelector>,
) -> Result<Workspace> {
    match selector {
        WorkspaceSelector::Default => match default_hint {
            Some(WorkspaceSelector::Default) | None => resolve_default_workspace(workspaces),
            Some(hint) => resolve_explicit_workspace(workspaces, hint),
        },
        explicit => resolve_explicit_workspace(workspaces, explicit),
    }
}

fn resolve_default_workspace(workspaces: &[Workspace]) -> Result<Workspace> {
    match workspaces {
        [workspace] => Ok(workspace.clone()),
        [] => Err(workspace_not_found(
            "no workspace selector was supplied and none are registered",
        )),
        candidates => Err(workspace_ambiguity(
            "no workspace selector was supplied",
            candidates,
        )),
    }
}

fn resolve_explicit_workspace(
    workspaces: &[Workspace],
    selector: WorkspaceSelector,
) -> Result<Workspace> {
    match selector {
        WorkspaceSelector::Id(id) => workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .cloned()
            .ok_or_else(|| workspace_not_found(&format!("workspace ID {id:?} was not found"))),
        WorkspaceSelector::Name(name) => resolve_workspace_name(workspaces, &name),
        WorkspaceSelector::RootPath(path) => {
            let identity = PathIdentity::from_path(&path)?;
            resolve_workspace_path(workspaces, &identity, &path.display().to_string())
        }
        WorkspaceSelector::FileUri(uri) => {
            let identity = PathIdentity::from_file_uri(&uri)?;
            resolve_workspace_path(workspaces, &identity, &uri)
        }
        WorkspaceSelector::Default => resolve_default_workspace(workspaces),
    }
}

fn resolve_workspace_name(workspaces: &[Workspace], name: &str) -> Result<Workspace> {
    let exact: Vec<_> = workspaces
        .iter()
        .filter(|workspace| workspace.name == name)
        .cloned()
        .collect();
    match exact.as_slice() {
        [workspace] => return Ok(workspace.clone()),
        [] => {}
        candidates => {
            return Err(workspace_ambiguity(
                &format!("workspace name {name:?} matches multiple registrations"),
                candidates,
            ));
        }
    }

    let insensitive: Vec<_> = workspaces
        .iter()
        .filter(|workspace| workspace.name.eq_ignore_ascii_case(name))
        .cloned()
        .collect();
    match insensitive.as_slice() {
        [workspace] => Ok(workspace.clone()),
        [] => Err(workspace_not_found(&format!(
            "workspace name {name:?} was not found"
        ))),
        candidates => Err(workspace_ambiguity(
            &format!("workspace name {name:?} is ambiguous ignoring case"),
            candidates,
        )),
    }
}

fn resolve_workspace_path(
    workspaces: &[Workspace],
    selector: &PathIdentity,
    selector_display: &str,
) -> Result<Workspace> {
    let mut matches = Vec::new();
    for workspace in workspaces {
        let root = PathIdentity::from_path(Path::new(&workspace.root_path))?;
        if root.contains(selector) {
            matches.push((root.comparison_key().len(), workspace.clone()));
        }
    }
    let Some(longest) = matches.iter().map(|(length, _)| *length).max() else {
        return Err(workspace_not_found(&format!(
            "workspace path {selector_display:?} is not inside a registered root"
        )));
    };
    let candidates: Vec<_> = matches
        .into_iter()
        .filter(|(length, _)| *length == longest)
        .map(|(_, workspace)| workspace)
        .collect();
    match candidates.as_slice() {
        [workspace] => Ok(workspace.clone()),
        _ => Err(workspace_ambiguity(
            &format!("workspace path {selector_display:?} matches duplicate roots"),
            &candidates,
        )),
    }
}

fn workspace_not_found(reason: &str) -> CortexError {
    CortexError::NotFound(format!(
        "{reason}; list registered workspaces or register the intended root, then select it by UUID, exact name, root path, or file URI"
    ))
}

fn workspace_ambiguity(reason: &str, candidates: &[Workspace]) -> CortexError {
    let candidates = candidates
        .iter()
        .map(|workspace| {
            format!(
                "id={} name={:?} root={:?}",
                workspace.id, workspace.name, workspace.root_path
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    CortexError::Ambiguous(format!(
        "{reason}; choose one explicitly. candidates: {candidates}"
    ))
}

fn memory_fts_query(query: &str) -> Option<String> {
    let terms: Vec<_> = query
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != ':'
        })
        .filter(|term| !term.is_empty())
        .map(|term| format!("\"{}\"", term.replace('"', "")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        domain::{Checkpoint, EventType, MemoryKind, Workspace},
        embedding::{TokenCountAccuracy, provider::MockEmbeddingProvider},
    };

    fn resolver_service(storage: SqliteStorage) -> CortexWeaveService {
        CortexWeaveService::from_parts_with_embeddings(
            AppConfig::default(),
            storage,
            Arc::new(MockEmbeddingProvider::new("resolver", 4)),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn service_can_start_without_an_adapter() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
        service.storage().health_check().await.unwrap();
        assert_eq!(
            service
                .analyzers()
                .for_path(std::path::Path::new("x.unknown"))
                .analyzer_id(),
            "generic"
        );
    }

    #[tokio::test]
    async fn facade_exposes_structured_checkpoint_operations() {
        let service = resolver_service(SqliteStorage::in_memory().await.unwrap());
        let workspace = Workspace::new("C:/checkpoint-facade", "checkpoint-facade");
        service
            .storage()
            .insert_workspace(&workspace)
            .await
            .unwrap();
        let session = service
            .start_session(&workspace.id, json!({}))
            .await
            .unwrap();
        let task = service
            .start_task(
                &workspace.id,
                Some(session.id.clone()),
                "checkpoint facade",
                json!({}),
            )
            .await
            .unwrap();
        let mut checkpoint = Checkpoint::new(&workspace.id, &session.id, "checkpoint state");
        checkpoint.task_id = Some(task.id.clone());
        checkpoint.next_action = Some("continue work".into());

        let checkpoint = service.create_checkpoint(checkpoint).await.unwrap();
        assert_eq!(
            service.latest_checkpoint(&workspace.id).await.unwrap(),
            Some(checkpoint.clone())
        );
        assert_eq!(
            service
                .latest_checkpoint_for_session(&workspace.id, &session.id)
                .await
                .unwrap(),
            Some(checkpoint.clone())
        );
        assert_eq!(
            service
                .latest_checkpoint_for_task(&workspace.id, &task.id)
                .await
                .unwrap(),
            Some(checkpoint)
        );
    }

    #[tokio::test]
    async fn workspace_resolution_reports_an_actionable_empty_registry() {
        let service = resolver_service(SqliteStorage::in_memory().await.unwrap());

        let error = service
            .resolve_workspace(WorkspaceSelector::Default, None)
            .await
            .unwrap_err();

        assert!(matches!(error, CortexError::NotFound(_)));
        let message = error.to_string();
        assert!(message.contains("none are registered"));
        assert!(message.contains("register the intended root"));
    }

    #[tokio::test]
    async fn workspace_registration_is_idempotent_by_canonical_root() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let service = resolver_service(SqliteStorage::in_memory().await.unwrap());

        let first = service
            .register_workspace(root.to_string_lossy(), "original")
            .await
            .unwrap();
        let alternate_spelling = root.to_string_lossy().replace('\\', "/");
        let second = service
            .register_workspace(alternate_spelling, "replacement")
            .await
            .unwrap();

        assert_eq!(second.id, first.id);
        assert_eq!(second.name, "original");
        assert_eq!(
            service.list_workspaces().await.unwrap(),
            vec![first.clone()]
        );
        assert_eq!(
            service
                .resolve_workspace(WorkspaceSelector::Default, None)
                .await
                .unwrap(),
            first
        );
    }

    #[tokio::test]
    async fn explicit_workspace_selectors_outrank_default_hints() {
        let directory = tempdir().unwrap();
        let first_root = directory.path().join("first");
        let second_root = directory.path().join("second");
        fs::create_dir_all(&first_root).unwrap();
        fs::create_dir_all(&second_root).unwrap();
        let service = resolver_service(SqliteStorage::in_memory().await.unwrap());
        let first = service
            .register_workspace(first_root.to_string_lossy(), "first")
            .await
            .unwrap();
        let second = service
            .register_workspace(second_root.to_string_lossy(), "second")
            .await
            .unwrap();

        let explicit = service
            .resolve_workspace(
                WorkspaceSelector::Id(first.id.clone()),
                Some(WorkspaceSelector::Name(second.name.clone())),
            )
            .await
            .unwrap();
        let hinted = service
            .resolve_workspace(
                WorkspaceSelector::Default,
                Some(WorkspaceSelector::Name(second.name.clone())),
            )
            .await
            .unwrap();

        assert_eq!(explicit, first);
        assert_eq!(hinted, second);
        let error = service
            .resolve_workspace(WorkspaceSelector::Default, None)
            .await
            .unwrap_err();
        assert!(matches!(error, CortexError::Ambiguous(_)));
        assert!(error.to_string().contains(&explicit.id));
        assert!(error.to_string().contains(&hinted.id));

        let consistent = service
            .resolve_workspace_selection(
                Some(explicit.id.clone()),
                Some(WorkspaceSelector::Name(explicit.name.clone())),
                Some(WorkspaceSelector::Name(hinted.name.clone())),
            )
            .await
            .unwrap();
        assert_eq!(consistent, explicit);

        let conflict = service
            .resolve_workspace_selection(
                Some(explicit.id.clone()),
                Some(WorkspaceSelector::Name(hinted.name.clone())),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(conflict, CortexError::Ambiguous(_)));
        assert!(conflict.to_string().contains(&explicit.id));
        assert!(conflict.to_string().contains(&hinted.id));

        let catalog = service
            .workspace_catalog(Some(WorkspaceSelector::RootPath(second_root)))
            .await
            .unwrap();
        assert_eq!(catalog.workspaces.len(), 2);
        assert_eq!(catalog.default_hint_match, Some(hinted));
        assert_eq!(catalog.default_hint_error, None);
    }

    #[tokio::test]
    async fn workspace_names_require_a_unique_exact_or_case_insensitive_match() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let first = Workspace::new("C:/resolver/one", "Alpha");
        let second = Workspace::new("C:/resolver/two", "ALPHA");
        storage.insert_workspace(&first).await.unwrap();
        storage.insert_workspace(&second).await.unwrap();
        let service = resolver_service(storage);

        assert_eq!(
            service
                .resolve_workspace(WorkspaceSelector::Name("Alpha".into()), None)
                .await
                .unwrap(),
            first
        );
        let case_error = service
            .resolve_workspace(WorkspaceSelector::Name("alpha".into()), None)
            .await
            .unwrap_err();
        assert!(matches!(case_error, CortexError::Ambiguous(_)));
        assert!(case_error.to_string().contains(&first.id));
        assert!(case_error.to_string().contains(&second.id));

        let third = Workspace::new("C:/resolver/three", "duplicate");
        let fourth = Workspace::new("C:/resolver/four", "duplicate");
        service.storage().insert_workspace(&third).await.unwrap();
        service.storage().insert_workspace(&fourth).await.unwrap();
        let duplicate_error = service
            .resolve_workspace(WorkspaceSelector::Name("duplicate".into()), None)
            .await
            .unwrap_err();
        assert!(matches!(duplicate_error, CortexError::Ambiguous(_)));
        assert!(duplicate_error.to_string().contains(&third.root_path));
        assert!(duplicate_error.to_string().contains(&fourth.root_path));
    }

    #[tokio::test]
    async fn path_resolution_uses_the_deepest_registered_root_and_file_uris() {
        let directory = tempdir().unwrap();
        let outer_root = directory.path().join("outer workspace");
        let inner_root = outer_root.join("packages").join("inner workspace");
        let source = inner_root.join("src");
        fs::create_dir_all(&source).unwrap();
        let service = resolver_service(SqliteStorage::in_memory().await.unwrap());
        let outer = service
            .register_workspace(outer_root.to_string_lossy(), "outer")
            .await
            .unwrap();
        let inner = service
            .register_workspace(inner_root.to_string_lossy(), "inner")
            .await
            .unwrap();

        let from_root = service
            .resolve_workspace(WorkspaceSelector::RootPath(outer_root.clone()), None)
            .await
            .unwrap();
        let from_child = service
            .resolve_workspace(
                WorkspaceSelector::RootPath(source.join("not-created-yet.rs")),
                None,
            )
            .await
            .unwrap();
        let source_uri = url::Url::from_directory_path(&source).unwrap().to_string();
        let from_uri = service
            .resolve_workspace(WorkspaceSelector::FileUri(source_uri), None)
            .await
            .unwrap();

        assert_eq!(from_root, outer);
        assert_eq!(from_child, inner);
        assert_eq!(from_uri, inner);

        let outside = directory.path().join("outside").join("missing.rs");
        let error = service
            .resolve_workspace(WorkspaceSelector::RootPath(outside), None)
            .await
            .unwrap_err();
        assert!(matches!(error, CortexError::NotFound(_)));
        assert!(error.to_string().contains("not inside a registered root"));
    }

    #[tokio::test]
    async fn records_searches_and_lists_explicit_memory() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/memory", "memory");
        storage.insert_workspace(&workspace).await.unwrap();
        let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
        let mut decision = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Decision,
            "Use BLAKE3 for deterministic change detection.",
        );
        decision.related_paths = vec!["src/indexing/reconciler.rs".into()];
        decision.metadata = json!({ "author": "agent" });
        let saved = service.record_memory(decision).await.unwrap();

        let search = service
            .search_memories(&workspace.id, "Why are we using BLAKE3?", 5)
            .await
            .unwrap();
        assert_eq!(search, vec![saved.clone()]);
        assert_eq!(search[0].related_paths, ["src/indexing/reconciler.rs"]);
        assert_eq!(search[0].metadata["author"], "agent");
        assert_eq!(
            service.recent_memories(&workspace.id, 5).await.unwrap(),
            vec![saved]
        );
        assert!(
            service
                .search_memories(&workspace.id, "", 5)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            service
                .recent_memories(&workspace.id, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn memory_search_survives_a_service_restart() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("cortexweave.sqlite");
        let workspace = Workspace::new("C:/restart-memory", "restart-memory");

        {
            let storage = SqliteStorage::open(&database).await.unwrap();
            storage.insert_workspace(&workspace).await.unwrap();
            let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
            let memory = MemoryRecord::new(
                &workspace.id,
                MemoryKind::Decision,
                "Use BLAKE3 for deterministic change detection.",
            );
            service.record_memory(memory).await.unwrap();
        }

        let restarted = CortexWeaveService::from_parts(
            AppConfig::default(),
            SqliteStorage::open(&database).await.unwrap(),
        )
        .unwrap();
        let results = restarted
            .search_memories(&workspace.id, "Why are we using BLAKE3?", 5)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, MemoryKind::Decision);
        assert_eq!(
            results[0].content,
            "Use BLAKE3 for deterministic change detection."
        );
    }

    #[tokio::test]
    async fn ending_a_session_checks_its_workspace_when_one_is_resolved() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let first = Workspace::new("C:/sessions/first", "first");
        let second = Workspace::new("C:/sessions/second", "second");
        storage.insert_workspace(&first).await.unwrap();
        storage.insert_workspace(&second).await.unwrap();
        let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
        let session = service.start_session(&first.id, json!({})).await.unwrap();

        assert!(
            service
                .end_session_in_workspace(&second.id, &session.id)
                .await
                .is_err()
        );
        assert!(
            service
                .end_session_in_workspace(&first.id, &session.id)
                .await
                .unwrap()
                .ended_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn records_harness_events_without_automatic_interpretation() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/events", "events");
        storage.insert_workspace(&workspace).await.unwrap();
        let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
        let types = [
            EventType::ExternalToolStarted,
            EventType::ExternalToolFinished,
            EventType::CompilerResult,
            EventType::TestResult,
            EventType::TaskUpdated,
        ];

        for event_type in types {
            service
                .record_event(CortexEvent::new(
                    &workspace.id,
                    event_type,
                    json!({ "ok": true }),
                ))
                .await
                .unwrap();
        }

        let events = service.recent_events(&workspace.id, 10).await.unwrap();
        assert_eq!(events.len(), 5);
        assert!(events.iter().all(|event| event.payload["ok"] == true));
        assert!(
            service
                .recent_events(&workspace.id, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn service_rejects_incoherent_provenance_and_terminal_lifecycle_changes() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let first = Workspace::new("C:/first", "first");
        let second = Workspace::new("C:/second", "second");
        storage.insert_workspace(&first).await.unwrap();
        storage.insert_workspace(&second).await.unwrap();
        let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
        let first_session = service.start_session(&first.id, json!({})).await.unwrap();
        let second_session = service.start_session(&second.id, json!({})).await.unwrap();
        let task = service
            .start_task(&first.id, Some(first_session.id.clone()), "work", json!({}))
            .await
            .unwrap();

        let mut memory = MemoryRecord::new(&first.id, MemoryKind::Note, "mismatch");
        memory.session_id = Some(second_session.id.clone());
        memory.task_id = Some(task.id.clone());
        assert!(service.record_memory(memory).await.is_err());

        service.complete_task(&task.id, json!({})).await.unwrap();
        assert!(
            service
                .update_task(&task.id, TaskStatus::Active, json!({}))
                .await
                .is_err()
        );
        service.end_session(&first_session.id).await.unwrap();
        assert!(service.end_session(&first_session.id).await.is_err());
        assert!(
            service
                .start_task(&first.id, Some(first_session.id), "late", json!({}))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn dedicated_harness_can_use_the_facade_without_an_adapter() {
        let directory = tempdir().unwrap();
        let source_root = directory.path().join("workspace");
        fs::create_dir_all(source_root.join("src")).unwrap();
        fs::write(
            source_root.join("src/lib.rs"),
            "pub trait EmbeddingProvider { fn embed(&self); }\n",
        )
        .unwrap();
        let provider: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new("harness-test", 8));
        let service = CortexWeaveService::from_parts_with_embeddings(
            AppConfig::default(),
            SqliteStorage::in_memory().await.unwrap(),
            provider,
        )
        .unwrap();

        let workspace = service
            .register_workspace(source_root.to_string_lossy(), "harness")
            .await
            .unwrap();
        assert_eq!(
            service.list_workspaces().await.unwrap(),
            std::slice::from_ref(&workspace)
        );

        let session = service
            .start_session(&workspace.id, json!({ "client": "native-harness" }))
            .await
            .unwrap();
        let task = service
            .start_task(
                &workspace.id,
                Some(session.id.clone()),
                "inspect embeddings",
                json!({}),
            )
            .await
            .unwrap();
        assert_eq!(task.status, TaskStatus::Active);

        let reindex = service.workspace_reindex(&workspace.id).await.unwrap();
        assert_eq!(reindex.files_seen, 1);
        let status = service.workspace_status(&workspace.id).await.unwrap();
        assert_eq!(status.documents_indexed, 1);
        assert!(status.chunks_indexed >= 1);
        assert_eq!(status.embedding_model, "harness-test");

        let semantic = service
            .semantic_search(&workspace.id, "embedding interface", 5)
            .await
            .unwrap();
        let lexical = service
            .lexical_search(&workspace.id, "EmbeddingProvider", 5)
            .await
            .unwrap();
        assert!(!semantic.is_empty());
        let item = service
            .get_item(&workspace.id, &lexical[0].chunk_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(item.path, "src/lib.rs");
        assert_eq!(item.scores.semantic, None);

        let mut memory = MemoryRecord::new(
            &workspace.id,
            MemoryKind::Observation,
            "EmbeddingProvider is defined in src/lib.rs.",
        );
        memory.session_id = Some(session.id.clone());
        memory.task_id = Some(task.id.clone());
        memory.related_paths = vec!["src/lib.rs".into()];
        service.record_memory(memory).await.unwrap();
        assert_eq!(
            service
                .search_memories(&workspace.id, "Where is EmbeddingProvider?", 5)
                .await
                .unwrap()[0]
                .related_paths,
            ["src/lib.rs"]
        );

        let mut event = CortexEvent::new(
            &workspace.id,
            EventType::TaskUpdated,
            json!({ "phase": "verified" }),
        );
        event.session_id = Some(session.id.clone());
        event.task_id = Some(task.id.clone());
        service.record_event(event).await.unwrap();
        assert_eq!(
            service.recent_events(&workspace.id, 5).await.unwrap().len(),
            1
        );

        let completed = service
            .complete_task(&task.id, json!({ "result": "found" }))
            .await
            .unwrap();
        assert_eq!(completed.status, TaskStatus::Completed);
        assert!(completed.completed_at.is_some());
        assert!(
            service
                .end_session(&session.id)
                .await
                .unwrap()
                .ended_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn instrumentation_combines_persisted_and_runtime_activity() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join("lib.rs"),
            "fn indexed() -> i32 { 1 }\n",
        )
        .unwrap();
        fs::write(directory.path().join("notes.unknown"), "fallback text\n").unwrap();
        let provider: Arc<dyn EmbeddingProvider> =
            Arc::new(MockEmbeddingProvider::new("metrics-model", 8));
        let service = CortexWeaveService::from_parts_with_embeddings(
            AppConfig::default(),
            SqliteStorage::in_memory().await.unwrap(),
            provider,
        )
        .unwrap();
        let workspace = service
            .register_workspace(directory.path().to_string_lossy(), "metrics")
            .await
            .unwrap();
        service.workspace_reindex(&workspace.id).await.unwrap();
        service
            .semantic_search(&workspace.id, "indexed", 5)
            .await
            .unwrap();
        service
            .record_memory(MemoryRecord::new(
                &workspace.id,
                MemoryKind::Note,
                "instrumentation memory",
            ))
            .await
            .unwrap();
        service
            .record_event(CortexEvent::new(
                &workspace.id,
                EventType::TaskUpdated,
                json!({}),
            ))
            .await
            .unwrap();
        service
            .resolve_workspace(WorkspaceSelector::Id(workspace.id.clone()), None)
            .await
            .unwrap();
        assert!(
            service
                .resolve_workspace(WorkspaceSelector::Name("not-registered".into()), None)
                .await
                .is_err()
        );

        let snapshot = service.instrumentation(Some(&workspace.id)).await.unwrap();
        assert_eq!(snapshot.documents, 2);
        assert!(snapshot.chunks >= 2);
        assert!(snapshot.chunks_by_language.contains_key("rust"));
        assert!(snapshot.chunks_by_language.contains_key("text"));
        assert_eq!(snapshot.analyzer_usage["tree-sitter-rust"], 1);
        assert_eq!(snapshot.fallback_usage, 1);
        assert_eq!(snapshot.memories, 1);
        assert_eq!(snapshot.events, 1);
        assert_eq!(snapshot.pending_jobs, 0);
        assert_eq!(snapshot.failed_jobs, 0);
        assert!(snapshot.embedding_calls >= 3);
        assert!(snapshot.embedding_batches >= 3);
        assert!(snapshot.embedding_latency.samples >= 3);
        assert_eq!(snapshot.search_latency.samples, 1);
        assert_eq!(snapshot.analyzer_latency.samples, 2);
        assert!(snapshot.chunks_added >= 2);
        assert!(snapshot.last_indexed_time.is_some());
        assert_eq!(snapshot.embedding_model, "metrics-model");
        assert_eq!(snapshot.embedding_dimension, Some(8));
        assert_eq!(
            snapshot.token_counter_accuracy,
            TokenCountAccuracy::ConservativeEstimate
        );
        assert_eq!(snapshot.workspace_resolutions_by_id, 1);
        assert_eq!(snapshot.workspace_resolution_misses, 1);
        assert_eq!(snapshot.runtime_metrics_scope, "process");
    }
}
