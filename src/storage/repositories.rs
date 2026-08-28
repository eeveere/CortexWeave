use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, Sqlite};

use crate::{
    CortexError, Result,
    domain::{
        Checkpoint, ContextPin, ContextSourceType, CortexEvent, Document, EmbeddingRecord,
        EventType, MemoryClaim, MemoryKind, MemoryOrigin, MemoryRecord, MemorySupersession,
        MemoryTrust, MemoryTrustReview, Session, SourceSegment, StoredChunk, SymbolKind, Task,
        TaskStatus, TemporalBounds, WorkingSetEntry, Workspace,
    },
};

use super::SqliteStorage;

#[derive(Debug, Clone)]
pub(crate) struct SemanticCandidate {
    pub chunk: StoredChunk,
    pub workspace_id: String,
    pub relative_path: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone)]
pub(crate) struct CodeCandidate {
    pub chunk: StoredChunk,
    pub workspace_id: String,
    pub relative_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StructuralRelation {
    Container,
    Neighbor,
    Related,
}

#[derive(Debug, Clone)]
pub(crate) struct StructuralCodeCandidate {
    pub candidate: TemporalCandidate,
    pub relation: StructuralRelation,
}

#[derive(Debug, Clone)]
pub(crate) struct LexicalCandidate {
    pub chunk: StoredChunk,
    pub workspace_id: String,
    pub relative_path: String,
    pub score: f32,
}

impl SqliteStorage {
    pub async fn insert_workspace(&self, workspace: &Workspace) -> Result<()> {
        sqlx::query(
            "INSERT INTO workspaces(id, root_path, name, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&workspace.id)
        .bind(&workspace.root_path)
        .bind(&workspace.name)
        .bind(workspace.created_at)
        .bind(workspace.updated_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        let rows = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, root_path, name, created_at, updated_at FROM workspaces ORDER BY name, id",
        )
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_workspace(&self, workspace_id: &str) -> Result<Option<Workspace>> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, root_path, name, created_at, updated_at FROM workspaces WHERE id = ?",
        )
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn get_workspace_by_root(&self, root_path: &str) -> Result<Option<Workspace>> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, root_path, name, created_at, updated_at FROM workspaces WHERE root_path = ?",
        )
        .bind(root_path)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn insert_workspace_if_absent(&self, workspace: &Workspace) -> Result<Workspace> {
        sqlx::query(
            "INSERT INTO workspaces(id, root_path, name, created_at, updated_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(root_path) DO NOTHING",
        )
        .bind(&workspace.id)
        .bind(&workspace.root_path)
        .bind(&workspace.name)
        .bind(workspace.created_at)
        .bind(workspace.updated_at)
        .execute(self.pool())
        .await?;
        self.get_workspace_by_root(&workspace.root_path)
            .await?
            .ok_or_else(|| {
                CortexError::Storage(sqlx::Error::Protocol(
                    "workspace insert did not produce a readable row".into(),
                ))
            })
    }

    pub async fn delete_workspace(&self, workspace_id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM workspaces WHERE id = ?")
            .bind(workspace_id)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn insert_session(&self, session: &Session) -> Result<()> {
        sqlx::query(
            "INSERT INTO sessions(id, workspace_id, started_at, ended_at, metadata_json) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&session.id)
        .bind(&session.workspace_id)
        .bind(session.started_at)
        .bind(session.ended_at)
        .bind(serde_json::to_string(&session.metadata)?)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn end_session(&self, session_id: &str, ended_at: DateTime<Utc>) -> Result<()> {
        let result = sqlx::query("UPDATE sessions SET ended_at = ? WHERE id = ?")
            .bind(ended_at)
            .bind(session_id)
            .execute(self.pool())
            .await?;
        require_one(result.rows_affected(), "session", session_id)
    }

    pub async fn get_session(&self, session_id: &str) -> Result<Option<Session>> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, workspace_id, started_at, ended_at, metadata_json FROM sessions WHERE id = ?",
        )
        .bind(session_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub(crate) async fn previous_session(
        &self,
        workspace_id: &str,
        before: DateTime<Utc>,
    ) -> Result<Option<Session>> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, workspace_id, started_at, ended_at, metadata_json FROM sessions WHERE workspace_id = ? AND started_at < ? ORDER BY started_at DESC, id DESC LIMIT 1",
        )
        .bind(workspace_id)
        .bind(before)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub(crate) async fn latest_active_session(
        &self,
        workspace_id: &str,
    ) -> Result<Option<Session>> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, workspace_id, started_at, ended_at, metadata_json FROM sessions WHERE workspace_id = ? AND ended_at IS NULL ORDER BY started_at DESC, id DESC LIMIT 1",
        )
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub(crate) async fn latest_ended_session(&self, workspace_id: &str) -> Result<Option<Session>> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, workspace_id, started_at, ended_at, metadata_json FROM sessions WHERE workspace_id = ? AND ended_at IS NOT NULL ORDER BY ended_at DESC, id DESC LIMIT 1",
        )
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn insert_task(&self, task: &Task) -> Result<()> {
        sqlx::query(
            "INSERT INTO tasks(id, workspace_id, session_id, title, status, details_json, created_at, updated_at, completed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&task.id)
        .bind(&task.workspace_id)
        .bind(&task.session_id)
        .bind(&task.title)
        .bind(task.status.as_str())
        .bind(serde_json::to_string(&task.details)?)
        .bind(task.created_at)
        .bind(task.updated_at)
        .bind(task.completed_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn update_task_status(
        &self,
        task_id: &str,
        status: TaskStatus,
        details: &Value,
    ) -> Result<()> {
        let now = Utc::now();
        let completed_at = matches!(
            status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        )
        .then_some(now);
        let result = sqlx::query(
            "UPDATE tasks SET status = ?, details_json = ?, updated_at = ?, completed_at = ? WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(serde_json::to_string(details)?)
        .bind(now)
        .bind(completed_at)
        .bind(task_id)
        .execute(self.pool())
        .await?;
        require_one(result.rows_affected(), "task", task_id)
    }

    pub async fn get_task(&self, task_id: &str) -> Result<Option<Task>> {
        let row = sqlx::query_as::<_, TaskRow>(
            "SELECT id, workspace_id, session_id, title, status, details_json, created_at, updated_at, completed_at FROM tasks WHERE id = ?",
        )
        .bind(task_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub(crate) async fn latest_active_task(
        &self,
        workspace_id: &str,
        session_id: Option<&str>,
    ) -> Result<Option<Task>> {
        self.latest_task_by_status(workspace_id, session_id, "status = 'active'")
            .await
    }

    pub(crate) async fn latest_incomplete_task(
        &self,
        workspace_id: &str,
        session_id: Option<&str>,
    ) -> Result<Option<Task>> {
        self.latest_task_by_status(workspace_id, session_id, "status IN ('pending', 'active')")
            .await
    }

    async fn latest_task_by_status(
        &self,
        workspace_id: &str,
        session_id: Option<&str>,
        status_predicate: &str,
    ) -> Result<Option<Task>> {
        let query = match session_id {
            Some(_) => format!(
                "SELECT id, workspace_id, session_id, title, status, details_json, created_at, updated_at, completed_at FROM tasks WHERE workspace_id = ? AND session_id = ? AND {status_predicate} ORDER BY updated_at DESC, id DESC LIMIT 1"
            ),
            None => format!(
                "SELECT id, workspace_id, session_id, title, status, details_json, created_at, updated_at, completed_at FROM tasks WHERE workspace_id = ? AND {status_predicate} ORDER BY updated_at DESC, id DESC LIMIT 1"
            ),
        };
        let row = match session_id {
            Some(session_id) => {
                sqlx::query_as::<_, TaskRow>(&query)
                    .bind(workspace_id)
                    .bind(session_id)
                    .fetch_optional(self.pool())
                    .await?
            }
            None => {
                sqlx::query_as::<_, TaskRow>(&query)
                    .bind(workspace_id)
                    .fetch_optional(self.pool())
                    .await?
            }
        };
        row.map(TryInto::try_into).transpose()
    }

    pub async fn insert_document(&self, document: &Document) -> Result<()> {
        insert_document(&mut *self.pool().acquire().await?, document).await
    }

    pub async fn find_document(
        &self,
        workspace_id: &str,
        relative_path: &str,
    ) -> Result<Option<Document>> {
        let row = sqlx::query_as::<_, DocumentRow>(
            "SELECT id, workspace_id, relative_path, language, analyzer_id, analyzer_version, segmentation_id, content_hash, size_bytes, modified_at_ns, indexed_at FROM documents WHERE workspace_id = ? AND relative_path = ?",
        )
        .bind(workspace_id)
        .bind(relative_path)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn list_documents(&self, workspace_id: &str) -> Result<Vec<Document>> {
        let rows = sqlx::query_as::<_, DocumentRow>(
            "SELECT id, workspace_id, relative_path, language, analyzer_id, analyzer_version, segmentation_id, content_hash, size_bytes, modified_at_ns, indexed_at FROM documents WHERE workspace_id = ? ORDER BY relative_path",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn delete_document(&self, workspace_id: &str, relative_path: &str) -> Result<bool> {
        let result =
            sqlx::query("DELETE FROM documents WHERE workspace_id = ? AND relative_path = ?")
                .bind(workspace_id)
                .bind(relative_path)
                .execute(self.pool())
                .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn insert_chunk(&self, chunk: &StoredChunk) -> Result<()> {
        insert_chunk(&mut *self.pool().acquire().await?, chunk).await
    }

    pub async fn list_chunks(&self, document_id: &str) -> Result<Vec<StoredChunk>> {
        let rows = sqlx::query_as::<_, ChunkRow>(
            "SELECT id, document_id, stable_key, language, symbol, qualified_symbol, symbol_kind, start_byte, end_byte, start_line, end_line, content, content_hash, metadata_json, created_at, updated_at FROM chunks WHERE document_id = ? ORDER BY start_byte",
        )
        .bind(document_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub(crate) async fn code_candidate(
        &self,
        workspace_id: &str,
        chunk_id: &str,
    ) -> Result<Option<CodeCandidate>> {
        let row = sqlx::query_as::<_, CodeCandidateRow>(
            "SELECT c.id, c.document_id, c.stable_key, c.language, c.symbol, c.qualified_symbol, c.symbol_kind, c.start_byte, c.end_byte, c.start_line, c.end_line, c.content, c.content_hash, c.metadata_json, c.created_at, c.updated_at, d.workspace_id, d.relative_path FROM chunks c JOIN documents d ON d.id = c.document_id WHERE d.workspace_id = ? AND c.id = ?",
        )
        .bind(workspace_id)
        .bind(chunk_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub(crate) async fn structural_code_candidates(
        &self,
        workspace_id: &str,
        chunk_id: &str,
        limit: usize,
    ) -> Result<Vec<StructuralCodeCandidate>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(target) = self.code_candidate(workspace_id, chunk_id).await? else {
            return Ok(Vec::new());
        };
        let chunks = self.list_chunks(&target.chunk.document_id).await?;
        let Some(target_index) = chunks.iter().position(|chunk| chunk.id == target.chunk.id) else {
            return Ok(Vec::new());
        };
        let target_chunk = &chunks[target_index];
        let parent_key = parent_stable_key(target_chunk);
        let parent = parent_key
            .and_then(|key| {
                chunks
                    .iter()
                    .filter(|chunk| logical_stable_key(chunk) == key)
                    .min_by_key(|chunk| {
                        let contains_target = chunk.start_byte <= target_chunk.start_byte
                            && chunk.end_byte >= target_chunk.end_byte;
                        (
                            !contains_target,
                            chunk.end_byte.saturating_sub(chunk.start_byte),
                            chunk.start_byte,
                            chunk.id.as_str(),
                        )
                    })
            })
            .or_else(|| enclosing_chunk(&chunks, target_chunk));

        let mut related = Vec::new();
        if let Some(parent) = parent {
            push_structural_candidate(&mut related, &target, parent, StructuralRelation::Container);
        }

        let mut siblings: Vec<_> = chunks
            .iter()
            .filter(|chunk| chunk.id != target_chunk.id)
            .filter(|chunk| parent_stable_key(chunk) == parent_key)
            .collect();
        siblings.sort_by(|left, right| {
            structural_order(left)
                .cmp(&structural_order(right))
                .then_with(|| left.id.cmp(&right.id))
        });
        let sibling_position = siblings.partition_point(|chunk| {
            structural_order(chunk) < structural_order(target_chunk)
                || (structural_order(chunk) == structural_order(target_chunk)
                    && chunk.id < target_chunk.id)
        });
        if let Some(previous) = sibling_position
            .checked_sub(1)
            .and_then(|index| siblings.get(index))
        {
            push_structural_candidate(
                &mut related,
                &target,
                previous,
                StructuralRelation::Neighbor,
            );
        }
        if let Some(next) = siblings.get(sibling_position) {
            push_structural_candidate(&mut related, &target, next, StructuralRelation::Neighbor);
        }

        let mut children: Vec<_> = chunks
            .iter()
            .filter(|chunk| parent_stable_key(chunk) == Some(logical_stable_key(target_chunk)))
            .collect();
        children.sort_by(|left, right| {
            structural_order(left)
                .cmp(&structural_order(right))
                .then_with(|| left.id.cmp(&right.id))
        });
        if let Some(child) = children.first() {
            push_structural_candidate(&mut related, &target, child, StructuralRelation::Related);
        } else if let Some(sibling) = siblings.iter().find(|sibling| {
            !related.iter().any(|candidate: &StructuralCodeCandidate| {
                candidate.candidate.source_id == sibling.id
            })
        }) {
            push_structural_candidate(&mut related, &target, sibling, StructuralRelation::Related);
        }

        related.truncate(limit);
        Ok(related)
    }

    pub async fn insert_embedding(&self, embedding: &EmbeddingRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO embeddings(chunk_id, model, dimension, vector, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&embedding.chunk_id)
        .bind(&embedding.model)
        .bind(embedding.dimension as i64)
        .bind(encode_vector(&embedding.vector))
        .bind(embedding.created_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn get_embedding(&self, chunk_id: &str) -> Result<Option<EmbeddingRecord>> {
        let row = sqlx::query_as::<_, EmbeddingRow>(
            "SELECT chunk_id, model, dimension, vector, created_at FROM embeddings WHERE chunk_id = ?",
        )
        .bind(chunk_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn document_embeddings_compatible(
        &self,
        document_id: &str,
        model: &str,
        expected_dimension: Option<usize>,
    ) -> Result<bool> {
        let (chunk_count, compatible_count, dimensions): (i64, i64, i64) = sqlx::query_as(
            "SELECT COUNT(c.id), COUNT(CASE WHEN e.model = ? AND e.dimension > 0 THEN 1 END), COUNT(DISTINCT CASE WHEN e.model = ? THEN e.dimension END) FROM chunks c LEFT JOIN embeddings e ON e.chunk_id = c.id WHERE c.document_id = ?",
        )
        .bind(model)
        .bind(model)
        .bind(document_id)
        .fetch_one(self.pool())
        .await?;
        if chunk_count != compatible_count || (chunk_count > 0 && dimensions != 1) {
            return Ok(false);
        }
        let Some(expected_dimension) = expected_dimension else {
            return Ok(true);
        };
        let mismatches: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chunks c JOIN embeddings e ON e.chunk_id = c.id WHERE c.document_id = ? AND e.dimension != ?",
        )
        .bind(document_id)
        .bind(i64::try_from(expected_dimension).map_err(|_| {
            CortexError::Embedding("embedding dimension exceeds SQLite integer range".into())
        })?)
        .fetch_one(self.pool())
        .await?;
        Ok(mismatches == 0)
    }

    pub(crate) async fn semantic_candidates(
        &self,
        workspace_id: &str,
        model: &str,
        dimension: usize,
    ) -> Result<Vec<SemanticCandidate>> {
        let rows = sqlx::query_as::<_, SemanticCandidateRow>(
            "SELECT c.id, c.document_id, c.stable_key, c.language, c.symbol, c.qualified_symbol, c.symbol_kind, c.start_byte, c.end_byte, c.start_line, c.end_line, c.content, c.content_hash, c.metadata_json, c.created_at, c.updated_at, d.workspace_id, d.relative_path, e.vector, e.dimension FROM embeddings e JOIN chunks c ON c.id = e.chunk_id JOIN documents d ON d.id = c.document_id WHERE d.workspace_id = ? AND e.model = ? AND e.dimension = ?",
        )
        .bind(workspace_id)
        .bind(model)
        .bind(i64::try_from(dimension).map_err(|_| CortexError::Embedding("embedding dimension exceeds SQLite integer range".into()))?)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub(crate) async fn lexical_candidates(
        &self,
        workspace_id: &str,
        match_query: &str,
        limit: usize,
    ) -> Result<Vec<LexicalCandidate>> {
        let rows = sqlx::query_as::<_, LexicalCandidateRow>(
            "SELECT c.id, c.document_id, c.stable_key, c.language, c.symbol, c.qualified_symbol, c.symbol_kind, c.start_byte, c.end_byte, c.start_line, c.end_line, c.content, c.content_hash, c.metadata_json, c.created_at, c.updated_at, d.workspace_id, d.relative_path, bm25(chunk_fts) AS rank FROM chunk_fts JOIN chunks c ON c.id = chunk_fts.chunk_id JOIN documents d ON d.id = c.document_id WHERE chunk_fts MATCH ? AND d.workspace_id = ? ORDER BY rank LIMIT ?",
        )
        .bind(match_query)
        .bind(workspace_id)
        .bind(i64::try_from(limit).map_err(|_| CortexError::Analysis("retrieval limit exceeds SQLite integer range".into()))?)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn insert_memory(&self, memory: &MemoryRecord) -> Result<()> {
        let claim_key = memory.claim.as_ref().map(|claim| claim.key.as_str());
        let claim_value = memory
            .claim
            .as_ref()
            .map(|claim| serde_json::to_string(&claim.value))
            .transpose()?;
        sqlx::query(
            "INSERT INTO memories(id, workspace_id, session_id, task_id, kind, content, metadata_json, origin, trust, source_segments_json, claim_key, claim_value_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&memory.id)
        .bind(&memory.workspace_id)
        .bind(&memory.session_id)
        .bind(&memory.task_id)
        .bind(memory.kind.as_str())
        .bind(&memory.content)
        .bind(serde_json::to_string(&memory.metadata_for_storage())?)
        .bind(memory.origin.as_str())
        .bind(memory.trust.as_str())
        .bind(serde_json::to_string(&memory.source_segments)?)
        .bind(claim_key)
        .bind(claim_value)
        .bind(memory.created_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn recent_memories(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        let rows = sqlx::query_as::<_, MemoryRow>(
            "SELECT id, workspace_id, session_id, task_id, kind, content, metadata_json, origin, trust, source_segments_json, claim_key, claim_value_json, created_at FROM memories WHERE workspace_id = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(workspace_id)
        .bind(memory_limit(limit)?)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn memory(
        &self,
        workspace_id: &str,
        memory_id: &str,
    ) -> Result<Option<MemoryRecord>> {
        let row = sqlx::query_as::<_, MemoryRow>(
            "SELECT id, workspace_id, session_id, task_id, kind, content, metadata_json, origin, trust, source_segments_json, claim_key, claim_value_json, created_at FROM memories WHERE workspace_id = ? AND id = ?",
        )
        .bind(workspace_id)
        .bind(memory_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub(crate) async fn resume_memories(
        &self,
        workspace_id: &str,
        task_id: Option<&str>,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        let Some((scope_column, scope_id)) = task_id
            .map(|id| ("m.task_id", id))
            .or_else(|| session_id.map(|id| ("m.session_id", id)))
        else {
            return Ok(Vec::new());
        };
        let query = format!(
            "SELECT m.id, m.workspace_id, m.session_id, m.task_id, m.kind, m.content, m.metadata_json, m.origin, m.trust, m.source_segments_json, m.claim_key, m.claim_value_json, m.created_at FROM memories m LEFT JOIN memory_supersession ms ON ms.superseded_memory_id = m.id WHERE m.workspace_id = ? AND {scope_column} = ? AND m.kind IN ('decision', 'failure') AND m.trust = 'trusted' AND ms.superseded_memory_id IS NULL ORDER BY m.created_at DESC, m.id DESC LIMIT ?"
        );
        let rows = sqlx::query_as::<_, MemoryRow>(&query)
            .bind(workspace_id)
            .bind(scope_id)
            .bind(memory_limit(limit)?)
            .fetch_all(self.pool())
            .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub(crate) async fn resume_memory(
        &self,
        workspace_id: &str,
        memory_id: &str,
    ) -> Result<Option<MemoryRecord>> {
        let row = sqlx::query_as::<_, MemoryRow>(
            "SELECT m.id, m.workspace_id, m.session_id, m.task_id, m.kind, m.content, m.metadata_json, m.origin, m.trust, m.source_segments_json, m.claim_key, m.claim_value_json, m.created_at FROM memories m LEFT JOIN memory_supersession ms ON ms.superseded_memory_id = m.id WHERE m.workspace_id = ? AND m.id = ? AND m.trust = 'trusted' AND ms.superseded_memory_id IS NULL",
        )
        .bind(workspace_id)
        .bind(memory_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn search_memories(
        &self,
        workspace_id: &str,
        match_query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRecord>> {
        let rows = sqlx::query_as::<_, MemoryRow>(
            "SELECT m.id, m.workspace_id, m.session_id, m.task_id, m.kind, m.content, m.metadata_json, m.origin, m.trust, m.source_segments_json, m.claim_key, m.claim_value_json, m.created_at FROM memory_fts JOIN memories m ON m.id = memory_fts.memory_id WHERE memory_fts MATCH ? AND m.workspace_id = ? ORDER BY bm25(memory_fts), m.id LIMIT ?",
        )
        .bind(match_query)
        .bind(workspace_id)
        .bind(memory_limit(limit)?)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn review_memory_trust(&self, review: &MemoryTrustReview) -> Result<()> {
        let mut transaction = self.pool().begin().await?;
        let result = sqlx::query(
            "UPDATE memories SET trust = ? WHERE id = ? AND workspace_id = ? AND origin = 'imported' AND trust = ?",
        )
        .bind(review.new_trust.as_str())
        .bind(&review.memory_id)
        .bind(&review.workspace_id)
        .bind(review.previous_trust.as_str())
        .execute(&mut *transaction)
        .await?;
        if result.rows_affected() != 1 {
            return Err(CortexError::Analysis(
                "imported memory trust changed before the review was applied".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO memory_trust_reviews(id, workspace_id, memory_id, previous_trust, new_trust, reviewed_by, reason, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&review.id)
        .bind(&review.workspace_id)
        .bind(&review.memory_id)
        .bind(review.previous_trust.as_str())
        .bind(review.new_trust.as_str())
        .bind(&review.reviewed_by)
        .bind(&review.reason)
        .bind(review.created_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn memory_trust_reviews(
        &self,
        workspace_id: &str,
        memory_id: &str,
    ) -> Result<Vec<MemoryTrustReview>> {
        let rows = sqlx::query_as::<_, MemoryTrustReviewRow>(
            "SELECT id, workspace_id, memory_id, previous_trust, new_trust, reviewed_by, reason, created_at FROM memory_trust_reviews WHERE workspace_id = ? AND memory_id = ? ORDER BY created_at, id",
        )
        .bind(workspace_id)
        .bind(memory_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn insert_event(&self, event: &CortexEvent) -> Result<()> {
        sqlx::query(
            "INSERT INTO events(id, workspace_id, session_id, task_id, event_type, payload_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&event.id)
        .bind(&event.workspace_id)
        .bind(&event.session_id)
        .bind(&event.task_id)
        .bind(event.event_type.storage_name())
        .bind(serde_json::to_string(&event.payload)?)
        .bind(event.created_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn recent_events(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<CortexEvent>> {
        let rows = sqlx::query_as::<_, EventRow>(
            "SELECT id, workspace_id, session_id, task_id, event_type, payload_json, created_at FROM events WHERE workspace_id = ? ORDER BY created_at DESC LIMIT ?",
        )
        .bind(workspace_id)
        .bind(memory_limit(limit)?)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub(crate) async fn resume_events(
        &self,
        workspace_id: &str,
        session_id: &str,
        started_at: DateTime<Utc>,
        ended_at: DateTime<Utc>,
        task_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CortexEvent>> {
        let rows = sqlx::query_as::<_, EventRow>(
            "SELECT id, workspace_id, session_id, task_id, event_type, payload_json, created_at FROM events WHERE workspace_id = ? AND created_at >= ? AND created_at <= ? AND (session_id IS NULL OR session_id = ?) AND (? IS NULL OR task_id IS NULL OR task_id = ?) ORDER BY created_at ASC, id ASC LIMIT ?",
        )
        .bind(workspace_id)
        .bind(started_at)
        .bind(ended_at)
        .bind(session_id)
        .bind(task_id)
        .bind(task_id)
        .bind(memory_limit(limit)?)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub(crate) async fn temporal_candidates(
        &self,
        workspace_id: &str,
        bounds: &TemporalBounds,
        source_types: &[ContextSourceType],
        limit: usize,
    ) -> Result<Vec<TemporalCandidate>> {
        let source_types: Vec<_> = source_types
            .iter()
            .map(ContextSourceType::storage_name)
            .collect();
        let mut query = sqlx::QueryBuilder::<Sqlite>::new(
            r#"SELECT source_id, source_type, session_id, task_id, content, path, symbol, language, start_byte, end_byte, created_at, modified_at, is_superseded
               FROM (
                   SELECT d.workspace_id, c.id AS source_id, 'code' AS source_type, NULL AS session_id, NULL AS task_id, c.content, d.relative_path AS path, COALESCE(c.qualified_symbol, c.symbol) AS symbol, c.language, c.start_byte, c.end_byte, c.created_at, c.updated_at AS modified_at, 0 AS is_superseded
                   FROM chunks c JOIN documents d ON d.id = c.document_id
                   UNION ALL
                   SELECT d.workspace_id, d.id, 'document', NULL, NULL, d.relative_path, d.relative_path, NULL, d.language, NULL, NULL, d.indexed_at, d.indexed_at, 0
                   FROM documents d
                   UNION ALL
                   SELECT m.workspace_id, m.id, 'memory', m.session_id, m.task_id, m.content, NULL, NULL, NULL, NULL, NULL, m.created_at, NULL, CASE WHEN ms.superseded_memory_id IS NULL THEN 0 ELSE 1 END
                   FROM memories m LEFT JOIN memory_supersession ms ON ms.superseded_memory_id = m.id
                   WHERE m.trust = 'trusted'
                   UNION ALL
                   SELECT e.workspace_id, e.id, 'event', e.session_id, e.task_id, e.payload_json, NULL, NULL, NULL, NULL, NULL, e.created_at, NULL, 0
                   FROM events e
                   UNION ALL
                   SELECT t.workspace_id, t.id, 'task_state', t.session_id, t.id, t.title, NULL, NULL, NULL, NULL, NULL, t.created_at, t.updated_at, 0
                   FROM tasks t
                   UNION ALL
                   SELECT s.workspace_id, s.id, 'session_state', s.id, NULL, s.metadata_json, NULL, NULL, NULL, NULL, NULL, s.started_at, s.ended_at, 0
                   FROM sessions s
               ) temporal
               WHERE workspace_id = "#,
        );
        query.push_bind(workspace_id);
        query.push(" AND ");
        query.push("(");
        query.push_bind(bounds.created_after);
        query.push(" IS NULL OR created_at >= ");
        query.push_bind(bounds.created_after);
        query.push(") AND (");
        query.push_bind(bounds.created_before);
        query.push(" IS NULL OR created_at <= ");
        query.push_bind(bounds.created_before);
        query.push(") AND (");
        query.push_bind(bounds.modified_after);
        query.push(" IS NULL OR modified_at >= ");
        query.push_bind(bounds.modified_after);
        query.push(") AND (");
        query.push_bind(bounds.modified_before);
        query.push(" IS NULL OR modified_at <= ");
        query.push_bind(bounds.modified_before);
        query.push(") AND (");
        query.push_bind(bounds.activity_after);
        query.push(" IS NULL OR COALESCE(modified_at, created_at) >= ");
        query.push_bind(bounds.activity_after);
        query.push(") AND (");
        query.push_bind(bounds.activity_before);
        query.push(" IS NULL OR COALESCE(modified_at, created_at) <= ");
        query.push_bind(bounds.activity_before);
        query.push(") AND (");
        query.push_bind(&bounds.scoped_session_id);
        query.push(" IS NULL OR source_type IN ('code', 'document') OR session_id = ");
        query.push_bind(&bounds.scoped_session_id);
        query.push(") AND (");
        query.push_bind(bounds.include_superseded);
        query.push(" = 1 OR is_superseded = 0)");
        if !source_types.is_empty() {
            query.push(" AND source_type IN (");
            let mut separated = query.separated(", ");
            for source_type in source_types {
                separated.push_bind(source_type);
            }
            separated.push_unseparated(")");
        }
        query.push(
            " ORDER BY CASE WHEN is_superseded = 1 THEN 0 WHEN source_type IN ('code', 'document', 'task_state') THEN 3 ELSE 1 END DESC, COALESCE(modified_at, created_at) DESC, source_type, source_id LIMIT ",
        );
        query.push_bind(i64::try_from(limit).map_err(|_| {
            CortexError::Analysis("temporal limit exceeds SQLite integer range".into())
        })?);
        let rows = query
            .build_query_as::<TemporalCandidateRow>()
            .fetch_all(self.pool())
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub(crate) async fn context_source_candidate(
        &self,
        workspace_id: &str,
        source_id: &str,
        source_type: &ContextSourceType,
    ) -> Result<Option<TemporalCandidate>> {
        let row = match source_type {
            ContextSourceType::Code => sqlx::query_as::<_, TemporalCandidateRow>(
                "SELECT c.id AS source_id, 'code' AS source_type, NULL AS session_id, NULL AS task_id, c.content, d.relative_path AS path, COALESCE(c.qualified_symbol, c.symbol) AS symbol, c.language, c.start_byte, c.end_byte, c.created_at, c.updated_at AS modified_at, 0 AS is_superseded FROM chunks c JOIN documents d ON d.id = c.document_id WHERE c.id = ? AND d.workspace_id = ?",
            )
            .bind(source_id)
            .bind(workspace_id)
            .fetch_optional(self.pool())
            .await?,
            ContextSourceType::Document => sqlx::query_as::<_, TemporalCandidateRow>(
                "SELECT id AS source_id, 'document' AS source_type, NULL AS session_id, NULL AS task_id, relative_path AS content, relative_path AS path, NULL AS symbol, language, NULL AS start_byte, NULL AS end_byte, indexed_at AS created_at, indexed_at AS modified_at, 0 AS is_superseded FROM documents WHERE id = ? AND workspace_id = ?",
            )
            .bind(source_id)
            .bind(workspace_id)
            .fetch_optional(self.pool())
            .await?,
            ContextSourceType::Memory => sqlx::query_as::<_, TemporalCandidateRow>(
                "SELECT m.id AS source_id, 'memory' AS source_type, m.session_id, m.task_id, m.content, NULL AS path, NULL AS symbol, NULL AS language, NULL AS start_byte, NULL AS end_byte, m.created_at, NULL AS modified_at, CASE WHEN ms.superseded_memory_id IS NULL THEN 0 ELSE 1 END AS is_superseded FROM memories m LEFT JOIN memory_supersession ms ON ms.superseded_memory_id = m.id WHERE m.id = ? AND m.workspace_id = ? AND m.trust = 'trusted'",
            )
            .bind(source_id)
            .bind(workspace_id)
            .fetch_optional(self.pool())
            .await?,
            ContextSourceType::Event => sqlx::query_as::<_, TemporalCandidateRow>(
                "SELECT id AS source_id, 'event' AS source_type, session_id, task_id, payload_json AS content, NULL AS path, NULL AS symbol, NULL AS language, NULL AS start_byte, NULL AS end_byte, created_at, NULL AS modified_at, 0 AS is_superseded FROM events WHERE id = ? AND workspace_id = ?",
            )
            .bind(source_id)
            .bind(workspace_id)
            .fetch_optional(self.pool())
            .await?,
            ContextSourceType::TaskState => sqlx::query_as::<_, TemporalCandidateRow>(
                "SELECT id AS source_id, 'task_state' AS source_type, session_id, id AS task_id, title AS content, NULL AS path, NULL AS symbol, NULL AS language, NULL AS start_byte, NULL AS end_byte, created_at, updated_at AS modified_at, 0 AS is_superseded FROM tasks WHERE id = ? AND workspace_id = ?",
            )
            .bind(source_id)
            .bind(workspace_id)
            .fetch_optional(self.pool())
            .await?,
            ContextSourceType::SessionState => sqlx::query_as::<_, TemporalCandidateRow>(
                "SELECT id AS source_id, 'session_state' AS source_type, id AS session_id, NULL AS task_id, metadata_json AS content, NULL AS path, NULL AS symbol, NULL AS language, NULL AS start_byte, NULL AS end_byte, started_at AS created_at, ended_at AS modified_at, 0 AS is_superseded FROM sessions WHERE id = ? AND workspace_id = ?",
            )
            .bind(source_id)
            .bind(workspace_id)
            .fetch_optional(self.pool())
            .await?,
            ContextSourceType::Other(_) => None,
        };
        Ok(row.map(Into::into))
    }

    pub async fn insert_working_set_entry(&self, entry: &WorkingSetEntry) -> Result<()> {
        sqlx::query(
            "INSERT INTO working_set_entries(id, workspace_id, session_id, task_id, source_id, source_type, activation_score, last_activated_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&entry.id)
        .bind(&entry.workspace_id)
        .bind(&entry.session_id)
        .bind(&entry.task_id)
        .bind(&entry.source_id)
        .bind(entry.source_type.storage_name())
        .bind(entry.activation_score)
        .bind(entry.last_activated_at)
        .bind(entry.created_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub(crate) async fn mutate_working_set_entry<F>(
        &self,
        candidate: WorkingSetEntry,
        update_score: F,
    ) -> Result<WorkingSetEntry>
    where
        F: Fn(Option<&WorkingSetEntry>) -> f32,
    {
        const MAX_ACTIVATION_RETRIES: usize = 16;

        for _ in 0..MAX_ACTIVATION_RETRIES {
            let row = sqlx::query_as::<_, WorkingSetEntryRow>(
                "SELECT id, workspace_id, session_id, task_id, source_id, source_type, activation_score, last_activated_at, created_at FROM working_set_entries WHERE session_id = ? AND task_id IS ? AND source_type = ? AND source_id = ?",
            )
            .bind(&candidate.session_id)
            .bind(&candidate.task_id)
            .bind(candidate.source_type.storage_name())
            .bind(&candidate.source_id)
            .fetch_optional(self.pool())
            .await?;
            let existing: Option<WorkingSetEntry> = row.map(Into::into);
            let activation_score = update_score(existing.as_ref());

            if let Some(mut entry) = existing {
                let previous_score = entry.activation_score;
                let previous_activated_at = entry.last_activated_at;
                entry.activation_score = activation_score;
                entry.last_activated_at = candidate.last_activated_at;
                let result = sqlx::query(
                    "UPDATE working_set_entries SET activation_score = ?, last_activated_at = ? WHERE id = ? AND activation_score = ? AND last_activated_at = ?",
                )
                .bind(entry.activation_score)
                .bind(entry.last_activated_at)
                .bind(&entry.id)
                .bind(previous_score)
                .bind(previous_activated_at)
                .execute(self.pool())
                .await?;
                if result.rows_affected() == 1 {
                    return Ok(entry);
                }
            } else {
                let mut entry = candidate.clone();
                entry.activation_score = activation_score;
                let result = sqlx::query(
                    "INSERT INTO working_set_entries(id, workspace_id, session_id, task_id, source_id, source_type, activation_score, last_activated_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
                )
                .bind(&entry.id)
                .bind(&entry.workspace_id)
                .bind(&entry.session_id)
                .bind(&entry.task_id)
                .bind(&entry.source_id)
                .bind(entry.source_type.storage_name())
                .bind(entry.activation_score)
                .bind(entry.last_activated_at)
                .bind(entry.created_at)
                .execute(self.pool())
                .await?;
                if result.rows_affected() == 1 {
                    return Ok(entry);
                }
            }
        }

        Err(CortexError::Analysis(
            "working-set activation changed too frequently to update".into(),
        ))
    }

    pub async fn working_set_entries(&self, session_id: &str) -> Result<Vec<WorkingSetEntry>> {
        let rows = sqlx::query_as::<_, WorkingSetEntryRow>(
            "SELECT id, workspace_id, session_id, task_id, source_id, source_type, activation_score, last_activated_at, created_at FROM working_set_entries WHERE session_id = ? ORDER BY last_activated_at DESC, id",
        )
        .bind(session_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub(crate) async fn delete_working_set_entries(&self, entry_ids: &[String]) -> Result<()> {
        if entry_ids.is_empty() {
            return Ok(());
        }
        let mut transaction = self.pool().begin().await?;
        for entry_id in entry_ids {
            sqlx::query("DELETE FROM working_set_entries WHERE id = ?")
                .bind(entry_id)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn insert_context_pin(&self, pin: &ContextPin) -> Result<()> {
        sqlx::query(
            "INSERT INTO context_pins(id, workspace_id, session_id, task_id, source_id, source_type, created_at) VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(&pin.id)
        .bind(&pin.workspace_id)
        .bind(&pin.session_id)
        .bind(&pin.task_id)
        .bind(&pin.source_id)
        .bind(pin.source_type.storage_name())
        .bind(pin.created_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn context_pins(&self, session_id: &str) -> Result<Vec<ContextPin>> {
        let rows = sqlx::query_as::<_, ContextPinRow>(
            "SELECT id, workspace_id, session_id, task_id, source_id, source_type, created_at FROM context_pins WHERE session_id = ? ORDER BY created_at DESC, id",
        )
        .bind(session_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub(crate) async fn context_pin(
        &self,
        session_id: &str,
        task_id: Option<&str>,
        source_id: &str,
        source_type: &ContextSourceType,
    ) -> Result<Option<ContextPin>> {
        let row = sqlx::query_as::<_, ContextPinRow>(
            "SELECT id, workspace_id, session_id, task_id, source_id, source_type, created_at FROM context_pins WHERE session_id = ? AND task_id IS ? AND source_id = ? AND source_type = ?",
        )
        .bind(session_id)
        .bind(task_id)
        .bind(source_id)
        .bind(source_type.storage_name())
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn delete_context_pin(&self, pin_id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM context_pins WHERE id = ?")
            .bind(pin_id)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn delete_context_pin_for_source(
        &self,
        session_id: &str,
        task_id: Option<&str>,
        source_id: &str,
        source_type: &ContextSourceType,
    ) -> Result<bool> {
        let result = sqlx::query(
            "DELETE FROM context_pins WHERE session_id = ? AND task_id IS ? AND source_id = ? AND source_type = ?",
        )
        .bind(session_id)
        .bind(task_id)
        .bind(source_id)
        .bind(source_type.storage_name())
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn context_source_exists(
        &self,
        workspace_id: &str,
        source_id: &str,
        source_type: &ContextSourceType,
    ) -> Result<bool> {
        let exists = match source_type {
            ContextSourceType::Code => {
                sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM chunks c JOIN documents d ON d.id = c.document_id WHERE c.id = ? AND d.workspace_id = ?)",
                )
                .bind(source_id)
                .bind(workspace_id)
                .fetch_one(self.pool())
                .await?
            }
            ContextSourceType::Document => {
                sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM documents WHERE id = ? AND workspace_id = ?)",
                )
                .bind(source_id)
                .bind(workspace_id)
                .fetch_one(self.pool())
                .await?
            }
            ContextSourceType::Memory => {
                sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM memories WHERE id = ? AND workspace_id = ? AND trust = 'trusted')",
                )
                .bind(source_id)
                .bind(workspace_id)
                .fetch_one(self.pool())
                .await?
            }
            ContextSourceType::Event => {
                sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM events WHERE id = ? AND workspace_id = ?)",
                )
                .bind(source_id)
                .bind(workspace_id)
                .fetch_one(self.pool())
                .await?
            }
            ContextSourceType::TaskState => {
                sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM tasks WHERE id = ? AND workspace_id = ?)",
                )
                .bind(source_id)
                .bind(workspace_id)
                .fetch_one(self.pool())
                .await?
            }
            ContextSourceType::SessionState => {
                sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ? AND workspace_id = ?)",
                )
                .bind(source_id)
                .bind(workspace_id)
                .fetch_one(self.pool())
                .await?
            }
            ContextSourceType::Other(_) => false,
        };
        Ok(exists)
    }

    pub(crate) async fn decision_memory_exists(
        &self,
        workspace_id: &str,
        memory_id: &str,
    ) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM memories WHERE id = ? AND workspace_id = ? AND kind = 'decision' AND trust = 'trusted')",
        )
        .bind(memory_id)
        .bind(workspace_id)
        .fetch_one(self.pool())
        .await
        .map_err(Into::into)
    }

    pub async fn insert_checkpoint(&self, checkpoint: &Checkpoint) -> Result<()> {
        sqlx::query(
            "INSERT INTO checkpoints(id, workspace_id, session_id, task_id, content, objective, completed_json, decision_ids_json, open_problems_json, related_paths_json, related_symbols_json, next_action, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&checkpoint.id)
        .bind(&checkpoint.workspace_id)
        .bind(&checkpoint.session_id)
        .bind(&checkpoint.task_id)
        .bind(&checkpoint.content)
        .bind(&checkpoint.objective)
        .bind(serde_json::to_string(&checkpoint.completed)?)
        .bind(serde_json::to_string(&checkpoint.decision_ids)?)
        .bind(serde_json::to_string(&checkpoint.open_problems)?)
        .bind(serde_json::to_string(&checkpoint.related_paths)?)
        .bind(serde_json::to_string(&checkpoint.related_symbols)?)
        .bind(&checkpoint.next_action)
        .bind(checkpoint.created_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn checkpoints(&self, workspace_id: &str) -> Result<Vec<Checkpoint>> {
        let rows = sqlx::query_as::<_, CheckpointRow>(
            "SELECT id, workspace_id, session_id, task_id, content, objective, completed_json, decision_ids_json, open_problems_json, related_paths_json, related_symbols_json, next_action, created_at FROM checkpoints WHERE workspace_id = ? ORDER BY created_at DESC, id",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn latest_checkpoint(&self, workspace_id: &str) -> Result<Option<Checkpoint>> {
        let row = sqlx::query_as::<_, CheckpointRow>(
            "SELECT id, workspace_id, session_id, task_id, content, objective, completed_json, decision_ids_json, open_problems_json, related_paths_json, related_symbols_json, next_action, created_at FROM checkpoints WHERE workspace_id = ? ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn latest_checkpoint_for_session(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Option<Checkpoint>> {
        let row = sqlx::query_as::<_, CheckpointRow>(
            "SELECT id, workspace_id, session_id, task_id, content, objective, completed_json, decision_ids_json, open_problems_json, related_paths_json, related_symbols_json, next_action, created_at FROM checkpoints WHERE workspace_id = ? AND session_id = ? ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(workspace_id)
        .bind(session_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub(crate) async fn latest_taskless_checkpoint_for_session(
        &self,
        workspace_id: &str,
        session_id: &str,
    ) -> Result<Option<Checkpoint>> {
        let row = sqlx::query_as::<_, CheckpointRow>(
            "SELECT id, workspace_id, session_id, task_id, content, objective, completed_json, decision_ids_json, open_problems_json, related_paths_json, related_symbols_json, next_action, created_at FROM checkpoints WHERE workspace_id = ? AND session_id = ? AND task_id IS NULL ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(workspace_id)
        .bind(session_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn latest_checkpoint_for_task(
        &self,
        workspace_id: &str,
        task_id: &str,
    ) -> Result<Option<Checkpoint>> {
        let row = sqlx::query_as::<_, CheckpointRow>(
            "SELECT id, workspace_id, session_id, task_id, content, objective, completed_json, decision_ids_json, open_problems_json, related_paths_json, related_symbols_json, next_action, created_at FROM checkpoints WHERE workspace_id = ? AND task_id = ? ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(workspace_id)
        .bind(task_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn insert_memory_supersession(
        &self,
        supersession: &MemorySupersession,
    ) -> Result<()> {
        let mut transaction = self.pool().begin().await?;
        let creates_cycle: i64 = sqlx::query_scalar(
            "WITH RECURSIVE chain(id) AS (\
                SELECT superseding_memory_id FROM memory_supersession \
                WHERE workspace_id = ? AND superseded_memory_id = ? \
                UNION \
                SELECT relation.superseding_memory_id FROM memory_supersession relation \
                JOIN chain ON relation.superseded_memory_id = chain.id \
                WHERE relation.workspace_id = ?\
            ) \
            SELECT EXISTS(SELECT 1 FROM chain WHERE id = ?)",
        )
        .bind(&supersession.workspace_id)
        .bind(&supersession.superseding_memory_id)
        .bind(&supersession.workspace_id)
        .bind(&supersession.superseded_memory_id)
        .fetch_one(&mut *transaction)
        .await?;
        if creates_cycle != 0 {
            return Err(CortexError::Analysis(
                "memory supersession cannot create a cycle".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO memory_supersession(workspace_id, superseded_memory_id, superseding_memory_id, reviewed_by, reason, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&supersession.workspace_id)
        .bind(&supersession.superseded_memory_id)
        .bind(&supersession.superseding_memory_id)
        .bind(&supersession.reviewed_by)
        .bind(&supersession.reason)
        .bind(supersession.created_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn memory_supersession(
        &self,
        superseded_memory_id: &str,
    ) -> Result<Option<MemorySupersession>> {
        let row = sqlx::query_as::<_, MemorySupersessionRow>(
            "SELECT workspace_id, superseded_memory_id, superseding_memory_id, reviewed_by, reason, created_at FROM memory_supersession WHERE superseded_memory_id = ?",
        )
        .bind(superseded_memory_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn persist_document_tree(
        &self,
        document: &Document,
        chunks: &[StoredChunk],
        embeddings: &[EmbeddingRecord],
    ) -> Result<()> {
        let mut transaction = self.pool().begin().await?;
        insert_document(&mut *transaction, document).await?;
        for chunk in chunks {
            insert_chunk(&mut *transaction, chunk).await?;
        }
        for embedding in embeddings {
            sqlx::query(
                "INSERT INTO embeddings(chunk_id, model, dimension, vector, created_at) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&embedding.chunk_id)
            .bind(&embedding.model)
            .bind(embedding.dimension as i64)
            .bind(encode_vector(&embedding.vector))
            .bind(embedding.created_at)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn apply_document_reconciliation(
        &self,
        document: &Document,
        chunks: &[StoredChunk],
        embeddings: &[EmbeddingRecord],
        removed_chunk_ids: &[String],
        reset_chunks: bool,
    ) -> Result<()> {
        let mut transaction = self.pool().begin().await?;
        if reset_chunks {
            sqlx::query("DELETE FROM chunks WHERE document_id = ?")
                .bind(&document.id)
                .execute(&mut *transaction)
                .await?;
        } else {
            for chunk_id in removed_chunk_ids {
                sqlx::query("DELETE FROM chunks WHERE id = ? AND document_id = ?")
                    .bind(chunk_id)
                    .bind(&document.id)
                    .execute(&mut *transaction)
                    .await?;
            }
        }
        sqlx::query(
            "INSERT INTO documents(id, workspace_id, relative_path, language, analyzer_id, analyzer_version, segmentation_id, content_hash, size_bytes, modified_at_ns, indexed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(workspace_id, relative_path) DO UPDATE SET language = excluded.language, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, segmentation_id = excluded.segmentation_id, content_hash = excluded.content_hash, size_bytes = excluded.size_bytes, modified_at_ns = excluded.modified_at_ns, indexed_at = excluded.indexed_at",
        )
        .bind(&document.id)
        .bind(&document.workspace_id)
        .bind(&document.relative_path)
        .bind(&document.language)
        .bind(&document.analyzer_id)
        .bind(&document.analyzer_version)
        .bind(&document.segmentation_id)
        .bind(&document.content_hash)
        .bind(document.size_bytes)
        .bind(document.modified_at_ns)
        .bind(document.indexed_at)
        .execute(&mut *transaction)
        .await?;
        for chunk in chunks {
            sqlx::query(
                "INSERT INTO chunks(id, document_id, stable_key, language, symbol, qualified_symbol, symbol_kind, start_byte, end_byte, start_line, end_line, content, content_hash, metadata_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(document_id, stable_key) DO UPDATE SET language = excluded.language, symbol = excluded.symbol, qualified_symbol = excluded.qualified_symbol, symbol_kind = excluded.symbol_kind, start_byte = excluded.start_byte, end_byte = excluded.end_byte, start_line = excluded.start_line, end_line = excluded.end_line, content = excluded.content, content_hash = excluded.content_hash, metadata_json = excluded.metadata_json, updated_at = excluded.updated_at",
            )
            .bind(&chunk.id)
            .bind(&chunk.document_id)
            .bind(&chunk.stable_key)
            .bind(&chunk.language)
            .bind(&chunk.symbol)
            .bind(&chunk.qualified_symbol)
            .bind(chunk.symbol_kind.as_ref().map(SymbolKind::storage_name))
            .bind(chunk.start_byte)
            .bind(chunk.end_byte)
            .bind(chunk.start_line)
            .bind(chunk.end_line)
            .bind(&chunk.content)
            .bind(&chunk.content_hash)
            .bind(serde_json::to_string(&chunk.metadata)?)
            .bind(chunk.created_at)
            .bind(chunk.updated_at)
            .execute(&mut *transaction)
            .await?;
        }
        for embedding in embeddings {
            sqlx::query(
                "INSERT INTO embeddings(chunk_id, model, dimension, vector, created_at) VALUES (?, ?, ?, ?, ?) ON CONFLICT(chunk_id) DO UPDATE SET model = excluded.model, dimension = excluded.dimension, vector = excluded.vector, created_at = excluded.created_at",
            )
            .bind(&embedding.chunk_id)
            .bind(&embedding.model)
            .bind(embedding.dimension as i64)
            .bind(encode_vector(&embedding.vector))
            .bind(embedding.created_at)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }
}

fn parent_stable_key(chunk: &StoredChunk) -> Option<&str> {
    chunk
        .metadata
        .get("parent_stable_key")
        .and_then(|value| value.as_str())
}

fn logical_stable_key(chunk: &StoredChunk) -> &str {
    chunk
        .metadata
        .get("parent_logical_stable_key")
        .and_then(|value| value.as_str())
        .unwrap_or(&chunk.stable_key)
}

fn structural_order(chunk: &StoredChunk) -> (bool, u64, i64) {
    let ordinal = chunk
        .metadata
        .get("ordinal_in_container")
        .and_then(|value| value.as_u64());
    (
        ordinal.is_none(),
        ordinal.unwrap_or_default(),
        chunk.start_byte,
    )
}

fn enclosing_chunk<'a>(chunks: &'a [StoredChunk], target: &StoredChunk) -> Option<&'a StoredChunk> {
    chunks
        .iter()
        .filter(|chunk| chunk.id != target.id)
        .filter(|chunk| chunk.start_byte <= target.start_byte && chunk.end_byte >= target.end_byte)
        .filter(|chunk| chunk.start_byte < target.start_byte || chunk.end_byte > target.end_byte)
        .min_by_key(|chunk| chunk.end_byte.saturating_sub(chunk.start_byte))
}

fn push_structural_candidate(
    candidates: &mut Vec<StructuralCodeCandidate>,
    target: &CodeCandidate,
    chunk: &StoredChunk,
    relation: StructuralRelation,
) {
    if chunk.id == target.chunk.id
        || candidates
            .iter()
            .any(|candidate| candidate.candidate.source_id == chunk.id)
    {
        return;
    }
    candidates.push(StructuralCodeCandidate {
        candidate: TemporalCandidate {
            source_id: chunk.id.clone(),
            source_type: ContextSourceType::Code,
            session_id: None,
            task_id: None,
            content: chunk.content.clone(),
            path: Some(target.relative_path.clone()),
            symbol: chunk
                .qualified_symbol
                .clone()
                .or_else(|| chunk.symbol.clone()),
            language: Some(chunk.language.clone()),
            source_segments: stored_source_segment(
                &target.relative_path,
                chunk.start_byte,
                chunk.end_byte,
            )
            .into_iter()
            .collect(),
            created_at: chunk.created_at,
            modified_at: Some(chunk.updated_at),
            superseded: false,
        },
        relation,
    });
}

fn stored_source_segment(path: &str, start_byte: i64, end_byte: i64) -> Option<SourceSegment> {
    let start_byte = u64::try_from(start_byte).ok()?;
    let end_byte = u64::try_from(end_byte).ok()?;
    (start_byte < end_byte).then(|| SourceSegment::new(path, start_byte, end_byte))
}

async fn insert_document<'e, E>(executor: E, document: &Document) -> Result<()>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "INSERT INTO documents(id, workspace_id, relative_path, language, analyzer_id, analyzer_version, segmentation_id, content_hash, size_bytes, modified_at_ns, indexed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&document.id)
    .bind(&document.workspace_id)
    .bind(&document.relative_path)
    .bind(&document.language)
    .bind(&document.analyzer_id)
    .bind(&document.analyzer_version)
    .bind(&document.segmentation_id)
    .bind(&document.content_hash)
    .bind(document.size_bytes)
    .bind(document.modified_at_ns)
    .bind(document.indexed_at)
    .execute(executor)
    .await?;
    Ok(())
}

async fn insert_chunk<'e, E>(executor: E, chunk: &StoredChunk) -> Result<()>
where
    E: sqlx::Executor<'e, Database = Sqlite>,
{
    sqlx::query(
        "INSERT INTO chunks(id, document_id, stable_key, language, symbol, qualified_symbol, symbol_kind, start_byte, end_byte, start_line, end_line, content, content_hash, metadata_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&chunk.id)
    .bind(&chunk.document_id)
    .bind(&chunk.stable_key)
    .bind(&chunk.language)
    .bind(&chunk.symbol)
    .bind(&chunk.qualified_symbol)
    .bind(chunk.symbol_kind.as_ref().map(SymbolKind::storage_name))
    .bind(chunk.start_byte)
    .bind(chunk.end_byte)
    .bind(chunk.start_line)
    .bind(chunk.end_line)
    .bind(&chunk.content)
    .bind(&chunk.content_hash)
    .bind(serde_json::to_string(&chunk.metadata)?)
    .bind(chunk.created_at)
    .bind(chunk.updated_at)
    .execute(executor)
    .await?;
    Ok(())
}

fn require_one(rows_affected: u64, kind: &str, id: &str) -> Result<()> {
    if rows_affected == 1 {
        Ok(())
    } else {
        Err(CortexError::NotFound(format!("{kind} {id}")))
    }
}

fn memory_limit(limit: usize) -> Result<i64> {
    i64::try_from(limit)
        .map_err(|_| CortexError::Analysis("memory limit exceeds SQLite integer range".into()))
}

pub(crate) fn encode_vector(vector: &[f32]) -> Vec<u8> {
    vector
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

pub(crate) fn decode_vector(bytes: &[u8], dimension: usize) -> Result<Vec<f32>> {
    if bytes.len() != dimension * size_of::<f32>() {
        return Err(CortexError::Storage(sqlx::Error::Decode(
            format!(
                "vector byte length {} does not match dimension {dimension}",
                bytes.len()
            )
            .into(),
        )));
    }
    let (chunks, remainder) = bytes.as_chunks::<{ size_of::<f32>() }>();
    debug_assert!(remainder.is_empty());
    Ok(chunks
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect())
}

#[derive(FromRow)]
struct WorkspaceRow {
    id: String,
    root_path: String,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<WorkspaceRow> for Workspace {
    fn from(row: WorkspaceRow) -> Self {
        Self {
            id: row.id,
            root_path: row.root_path,
            name: row.name,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(FromRow)]
struct SessionRow {
    id: String,
    workspace_id: String,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    metadata_json: String,
}

impl TryFrom<SessionRow> for Session {
    type Error = CortexError;

    fn try_from(row: SessionRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            started_at: row.started_at,
            ended_at: row.ended_at,
            metadata: serde_json::from_str(&row.metadata_json)?,
        })
    }
}

#[derive(FromRow)]
struct TaskRow {
    id: String,
    workspace_id: String,
    session_id: Option<String>,
    title: String,
    status: String,
    details_json: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

impl TryFrom<TaskRow> for Task {
    type Error = CortexError;

    fn try_from(row: TaskRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            session_id: row.session_id,
            title: row.title,
            status: TaskStatus::from_storage(&row.status),
            details: serde_json::from_str(&row.details_json)?,
            created_at: row.created_at,
            updated_at: row.updated_at,
            completed_at: row.completed_at,
        })
    }
}

#[derive(FromRow)]
struct DocumentRow {
    id: String,
    workspace_id: String,
    relative_path: String,
    language: String,
    analyzer_id: String,
    analyzer_version: String,
    segmentation_id: String,
    content_hash: String,
    size_bytes: i64,
    modified_at_ns: Option<i64>,
    indexed_at: DateTime<Utc>,
}

impl From<DocumentRow> for Document {
    fn from(row: DocumentRow) -> Self {
        Self {
            id: row.id,
            workspace_id: row.workspace_id,
            relative_path: row.relative_path,
            language: row.language,
            analyzer_id: row.analyzer_id,
            analyzer_version: row.analyzer_version,
            segmentation_id: row.segmentation_id,
            content_hash: row.content_hash,
            size_bytes: row.size_bytes,
            modified_at_ns: row.modified_at_ns,
            indexed_at: row.indexed_at,
        }
    }
}

#[derive(FromRow)]
struct ChunkRow {
    id: String,
    document_id: String,
    stable_key: String,
    language: String,
    symbol: Option<String>,
    qualified_symbol: Option<String>,
    symbol_kind: Option<String>,
    start_byte: i64,
    end_byte: i64,
    start_line: i64,
    end_line: i64,
    content: String,
    content_hash: String,
    metadata_json: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<ChunkRow> for StoredChunk {
    type Error = CortexError;
    fn try_from(row: ChunkRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            document_id: row.document_id,
            stable_key: row.stable_key,
            language: row.language,
            symbol: row.symbol,
            qualified_symbol: row.qualified_symbol,
            symbol_kind: row.symbol_kind.as_deref().map(SymbolKind::from_storage),
            start_byte: row.start_byte,
            end_byte: row.end_byte,
            start_line: row.start_line,
            end_line: row.end_line,
            content: row.content,
            content_hash: row.content_hash,
            metadata: serde_json::from_str(&row.metadata_json)?,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(FromRow)]
struct EmbeddingRow {
    chunk_id: String,
    model: String,
    dimension: i64,
    vector: Vec<u8>,
    created_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct SearchChunkRow {
    id: String,
    document_id: String,
    stable_key: String,
    language: String,
    symbol: Option<String>,
    qualified_symbol: Option<String>,
    symbol_kind: Option<String>,
    start_byte: i64,
    end_byte: i64,
    start_line: i64,
    end_line: i64,
    content: String,
    content_hash: String,
    metadata_json: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<SearchChunkRow> for StoredChunk {
    type Error = CortexError;

    fn try_from(row: SearchChunkRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            document_id: row.document_id,
            stable_key: row.stable_key,
            language: row.language,
            symbol: row.symbol,
            qualified_symbol: row.qualified_symbol,
            symbol_kind: row.symbol_kind.as_deref().map(SymbolKind::from_storage),
            start_byte: row.start_byte,
            end_byte: row.end_byte,
            start_line: row.start_line,
            end_line: row.end_line,
            content: row.content,
            content_hash: row.content_hash,
            metadata: serde_json::from_str(&row.metadata_json)?,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(FromRow)]
struct CodeCandidateRow {
    id: String,
    document_id: String,
    stable_key: String,
    language: String,
    symbol: Option<String>,
    qualified_symbol: Option<String>,
    symbol_kind: Option<String>,
    start_byte: i64,
    end_byte: i64,
    start_line: i64,
    end_line: i64,
    content: String,
    content_hash: String,
    metadata_json: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    workspace_id: String,
    relative_path: String,
}

impl TryFrom<CodeCandidateRow> for CodeCandidate {
    type Error = CortexError;

    fn try_from(row: CodeCandidateRow) -> Result<Self> {
        let chunk = StoredChunk::try_from(SearchChunkRow {
            id: row.id,
            document_id: row.document_id,
            stable_key: row.stable_key,
            language: row.language,
            symbol: row.symbol,
            qualified_symbol: row.qualified_symbol,
            symbol_kind: row.symbol_kind,
            start_byte: row.start_byte,
            end_byte: row.end_byte,
            start_line: row.start_line,
            end_line: row.end_line,
            content: row.content,
            content_hash: row.content_hash,
            metadata_json: row.metadata_json,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })?;
        Ok(Self {
            chunk,
            workspace_id: row.workspace_id,
            relative_path: row.relative_path,
        })
    }
}

#[derive(FromRow)]
struct SemanticCandidateRow {
    id: String,
    document_id: String,
    stable_key: String,
    language: String,
    symbol: Option<String>,
    qualified_symbol: Option<String>,
    symbol_kind: Option<String>,
    start_byte: i64,
    end_byte: i64,
    start_line: i64,
    end_line: i64,
    content: String,
    content_hash: String,
    metadata_json: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    workspace_id: String,
    relative_path: String,
    vector: Vec<u8>,
    dimension: i64,
}

impl TryFrom<SemanticCandidateRow> for SemanticCandidate {
    type Error = CortexError;

    fn try_from(row: SemanticCandidateRow) -> Result<Self> {
        let dimension = usize::try_from(row.dimension)
            .map_err(|_| CortexError::Embedding("negative embedding dimension".into()))?;
        let chunk = StoredChunk::try_from(SearchChunkRow {
            id: row.id,
            document_id: row.document_id,
            stable_key: row.stable_key,
            language: row.language,
            symbol: row.symbol,
            qualified_symbol: row.qualified_symbol,
            symbol_kind: row.symbol_kind,
            start_byte: row.start_byte,
            end_byte: row.end_byte,
            start_line: row.start_line,
            end_line: row.end_line,
            content: row.content,
            content_hash: row.content_hash,
            metadata_json: row.metadata_json,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })?;
        Ok(Self {
            chunk,
            workspace_id: row.workspace_id,
            relative_path: row.relative_path,
            vector: decode_vector(&row.vector, dimension)?,
        })
    }
}

#[derive(FromRow)]
struct LexicalCandidateRow {
    id: String,
    document_id: String,
    stable_key: String,
    language: String,
    symbol: Option<String>,
    qualified_symbol: Option<String>,
    symbol_kind: Option<String>,
    start_byte: i64,
    end_byte: i64,
    start_line: i64,
    end_line: i64,
    content: String,
    content_hash: String,
    metadata_json: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    workspace_id: String,
    relative_path: String,
    rank: f64,
}

impl TryFrom<LexicalCandidateRow> for LexicalCandidate {
    type Error = CortexError;

    fn try_from(row: LexicalCandidateRow) -> Result<Self> {
        let chunk = StoredChunk::try_from(SearchChunkRow {
            id: row.id,
            document_id: row.document_id,
            stable_key: row.stable_key,
            language: row.language,
            symbol: row.symbol,
            qualified_symbol: row.qualified_symbol,
            symbol_kind: row.symbol_kind,
            start_byte: row.start_byte,
            end_byte: row.end_byte,
            start_line: row.start_line,
            end_line: row.end_line,
            content: row.content,
            content_hash: row.content_hash,
            metadata_json: row.metadata_json,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })?;
        Ok(Self {
            chunk,
            workspace_id: row.workspace_id,
            relative_path: row.relative_path,
            score: (-row.rank).max(0.0) as f32,
        })
    }
}

impl TryFrom<EmbeddingRow> for EmbeddingRecord {
    type Error = CortexError;
    fn try_from(row: EmbeddingRow) -> Result<Self> {
        let dimension = usize::try_from(row.dimension)
            .map_err(|_| CortexError::Embedding("negative embedding dimension".into()))?;
        Ok(Self {
            chunk_id: row.chunk_id,
            model: row.model,
            dimension,
            vector: decode_vector(&row.vector, dimension)?,
            created_at: row.created_at,
        })
    }
}

#[derive(FromRow)]
struct MemoryRow {
    id: String,
    workspace_id: String,
    session_id: Option<String>,
    task_id: Option<String>,
    kind: String,
    content: String,
    metadata_json: String,
    origin: String,
    trust: String,
    source_segments_json: String,
    claim_key: Option<String>,
    claim_value_json: Option<String>,
    created_at: DateTime<Utc>,
}

impl TryFrom<MemoryRow> for MemoryRecord {
    type Error = CortexError;
    fn try_from(row: MemoryRow) -> Result<Self> {
        let metadata: Value = serde_json::from_str(&row.metadata_json)?;
        let source_segments: Vec<SourceSegment> = serde_json::from_str(&row.source_segments_json)?;
        let claim = match (row.claim_key, row.claim_value_json) {
            (Some(key), Some(value)) => Some(MemoryClaim {
                key,
                value: serde_json::from_str(&value)?,
            }),
            (None, None) => None,
            _ => {
                return Err(CortexError::Analysis(
                    "stored memory claim is incomplete".into(),
                ));
            }
        };
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            session_id: row.session_id,
            task_id: row.task_id,
            kind: MemoryKind::from_storage(&row.kind),
            content: row.content,
            related_paths: MemoryRecord::related_paths_from_metadata(&metadata),
            metadata,
            origin: MemoryOrigin::from_storage(&row.origin),
            trust: MemoryTrust::from_storage(&row.trust),
            source_segments,
            claim,
            created_at: row.created_at,
        })
    }
}

#[derive(FromRow)]
struct MemoryTrustReviewRow {
    id: String,
    workspace_id: String,
    memory_id: String,
    previous_trust: String,
    new_trust: String,
    reviewed_by: String,
    reason: String,
    created_at: DateTime<Utc>,
}

impl From<MemoryTrustReviewRow> for MemoryTrustReview {
    fn from(row: MemoryTrustReviewRow) -> Self {
        Self {
            id: row.id,
            workspace_id: row.workspace_id,
            memory_id: row.memory_id,
            previous_trust: MemoryTrust::from_storage(&row.previous_trust),
            new_trust: MemoryTrust::from_storage(&row.new_trust),
            reviewed_by: row.reviewed_by,
            reason: row.reason,
            created_at: row.created_at,
        }
    }
}

#[derive(FromRow)]
struct EventRow {
    id: String,
    workspace_id: String,
    session_id: Option<String>,
    task_id: Option<String>,
    event_type: String,
    payload_json: String,
    created_at: DateTime<Utc>,
}

impl TryFrom<EventRow> for CortexEvent {
    type Error = CortexError;
    fn try_from(row: EventRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            session_id: row.session_id,
            task_id: row.task_id,
            event_type: EventType::from_storage(&row.event_type),
            payload: serde_json::from_str(&row.payload_json)?,
            created_at: row.created_at,
        })
    }
}

#[derive(FromRow)]
struct WorkingSetEntryRow {
    id: String,
    workspace_id: String,
    session_id: String,
    task_id: Option<String>,
    source_id: String,
    source_type: String,
    activation_score: f32,
    last_activated_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

impl From<WorkingSetEntryRow> for WorkingSetEntry {
    fn from(row: WorkingSetEntryRow) -> Self {
        Self {
            id: row.id,
            workspace_id: row.workspace_id,
            session_id: row.session_id,
            task_id: row.task_id,
            source_id: row.source_id,
            source_type: ContextSourceType::from_storage(&row.source_type),
            activation_score: row.activation_score,
            last_activated_at: row.last_activated_at,
            created_at: row.created_at,
        }
    }
}

#[derive(FromRow)]
struct ContextPinRow {
    id: String,
    workspace_id: String,
    session_id: String,
    task_id: Option<String>,
    source_id: String,
    source_type: String,
    created_at: DateTime<Utc>,
}

impl From<ContextPinRow> for ContextPin {
    fn from(row: ContextPinRow) -> Self {
        Self {
            id: row.id,
            workspace_id: row.workspace_id,
            session_id: row.session_id,
            task_id: row.task_id,
            source_id: row.source_id,
            source_type: ContextSourceType::from_storage(&row.source_type),
            created_at: row.created_at,
        }
    }
}

#[derive(FromRow)]
struct CheckpointRow {
    id: String,
    workspace_id: String,
    session_id: String,
    task_id: Option<String>,
    content: String,
    objective: Option<String>,
    completed_json: String,
    decision_ids_json: String,
    open_problems_json: String,
    related_paths_json: String,
    related_symbols_json: String,
    next_action: Option<String>,
    created_at: DateTime<Utc>,
}

impl TryFrom<CheckpointRow> for Checkpoint {
    type Error = CortexError;

    fn try_from(row: CheckpointRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            session_id: row.session_id,
            task_id: row.task_id,
            content: row.content,
            objective: row.objective,
            completed: serde_json::from_str(&row.completed_json)?,
            decision_ids: serde_json::from_str(&row.decision_ids_json)?,
            open_problems: serde_json::from_str(&row.open_problems_json)?,
            related_paths: serde_json::from_str(&row.related_paths_json)?,
            related_symbols: serde_json::from_str(&row.related_symbols_json)?,
            next_action: row.next_action,
            created_at: row.created_at,
        })
    }
}

#[derive(FromRow)]
struct MemorySupersessionRow {
    workspace_id: String,
    superseded_memory_id: String,
    superseding_memory_id: String,
    reviewed_by: Option<String>,
    reason: Option<String>,
    created_at: DateTime<Utc>,
}

impl From<MemorySupersessionRow> for MemorySupersession {
    fn from(row: MemorySupersessionRow) -> Self {
        Self {
            workspace_id: row.workspace_id,
            superseded_memory_id: row.superseded_memory_id,
            superseding_memory_id: row.superseding_memory_id,
            reviewed_by: row.reviewed_by,
            reason: row.reason,
            created_at: row.created_at,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TemporalCandidate {
    pub source_id: String,
    pub source_type: ContextSourceType,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub content: String,
    pub path: Option<String>,
    pub symbol: Option<String>,
    pub language: Option<String>,
    pub source_segments: Vec<SourceSegment>,
    pub created_at: DateTime<Utc>,
    pub modified_at: Option<DateTime<Utc>>,
    pub superseded: bool,
}

#[derive(FromRow)]
struct TemporalCandidateRow {
    source_id: String,
    source_type: String,
    session_id: Option<String>,
    task_id: Option<String>,
    content: String,
    path: Option<String>,
    symbol: Option<String>,
    language: Option<String>,
    start_byte: Option<i64>,
    end_byte: Option<i64>,
    created_at: DateTime<Utc>,
    modified_at: Option<DateTime<Utc>>,
    is_superseded: bool,
}

impl From<TemporalCandidateRow> for TemporalCandidate {
    fn from(row: TemporalCandidateRow) -> Self {
        let source_segments = match (
            row.path.as_deref(),
            row.start_byte.and_then(|value| u64::try_from(value).ok()),
            row.end_byte.and_then(|value| u64::try_from(value).ok()),
        ) {
            (Some(path), Some(start_byte), Some(end_byte)) if start_byte < end_byte => {
                vec![SourceSegment::new(path, start_byte, end_byte)]
            }
            _ => Vec::new(),
        };
        Self {
            source_id: row.source_id,
            source_type: ContextSourceType::from_storage(&row.source_type),
            session_id: row.session_id,
            task_id: row.task_id,
            content: row.content,
            path: row.path,
            symbol: row.symbol,
            language: row.language,
            source_segments,
            created_at: row.created_at,
            modified_at: row.modified_at,
            superseded: row.is_superseded,
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn persists_across_restart_and_cascades_workspace_data() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("cortex.db");
        let workspace = Workspace::new("C:/project", "project");
        let document = Document::new(&workspace.id, "src/lib.rs");
        let chunk = StoredChunk::new(&document.id, "src/lib.rs::function:run", "fn run() {}");
        let embedding = EmbeddingRecord::new(&chunk.id, "test-model", vec![0.1, 0.2, 0.3]);

        {
            let storage = SqliteStorage::open(&path).await.unwrap();
            storage.insert_workspace(&workspace).await.unwrap();
            storage
                .persist_document_tree(
                    &document,
                    std::slice::from_ref(&chunk),
                    std::slice::from_ref(&embedding),
                )
                .await
                .unwrap();
        }
        {
            let storage = SqliteStorage::open(&path).await.unwrap();
            assert_eq!(
                storage.list_workspaces().await.unwrap(),
                vec![workspace.clone()]
            );
            assert_eq!(
                storage.list_chunks(&document.id).await.unwrap(),
                vec![chunk]
            );
            assert_eq!(
                storage.get_embedding(&embedding.chunk_id).await.unwrap(),
                Some(embedding)
            );
            storage.delete_workspace(&workspace.id).await.unwrap();
            assert!(storage.list_chunks(&document.id).await.unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn rejects_duplicate_stable_keys_and_rolls_back_tree() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/project", "project");
        storage.insert_workspace(&workspace).await.unwrap();
        let document = Document::new(&workspace.id, "src/lib.rs");
        let chunk = StoredChunk::new(&document.id, "same", "one");
        let mut duplicate = StoredChunk::new(&document.id, "same", "two");
        duplicate.id = uuid::Uuid::new_v4().to_string();

        assert!(
            storage
                .persist_document_tree(&document, &[chunk, duplicate], &[])
                .await
                .is_err()
        );
        assert!(
            storage
                .find_document(&workspace.id, "src/lib.rs")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn reconciliation_failure_rolls_back_all_document_changes() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/project", "project");
        storage.insert_workspace(&workspace).await.unwrap();
        let document = Document::new(&workspace.id, "src/lib.rs");
        let chunk = StoredChunk::new(&document.id, "function:run", "fn run() { 1 }");
        let embedding = EmbeddingRecord::new(&chunk.id, "model", vec![1.0, 0.0]);
        storage
            .persist_document_tree(
                &document,
                std::slice::from_ref(&chunk),
                std::slice::from_ref(&embedding),
            )
            .await
            .unwrap();

        let mut changed_document = document.clone();
        changed_document.content_hash = "changed".into();
        let mut changed_chunk = chunk.clone();
        changed_chunk.content = "fn run() { 2 }".into();
        changed_chunk.content_hash = "changed".into();
        let invalid_embedding = EmbeddingRecord::new("missing-chunk", "model", vec![0.0, 1.0]);
        assert!(
            storage
                .apply_document_reconciliation(
                    &changed_document,
                    &[changed_chunk],
                    &[invalid_embedding],
                    &[],
                    false,
                )
                .await
                .is_err()
        );

        let persisted = storage
            .find_document(&workspace.id, "src/lib.rs")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.content_hash, document.content_hash);
        assert_eq!(
            storage.list_chunks(&document.id).await.unwrap(),
            vec![chunk]
        );
        assert_eq!(
            storage.get_embedding(&embedding.chunk_id).await.unwrap(),
            Some(embedding)
        );
    }

    #[tokio::test]
    async fn enforces_workspace_consistency_for_session_associations() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let first = Workspace::new("C:/first", "first");
        let second = Workspace::new("C:/second", "second");
        storage.insert_workspace(&first).await.unwrap();
        storage.insert_workspace(&second).await.unwrap();
        let session = Session::new(&first.id, serde_json::json!({}));
        storage.insert_session(&session).await.unwrap();

        let task = Task::new(
            &second.id,
            Some(session.id),
            "invalid association",
            serde_json::json!({}),
        );
        assert!(storage.insert_task(&task).await.is_err());
    }

    #[tokio::test]
    async fn retains_memory_event_and_language_provenance() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/mixed", "mixed");
        storage.insert_workspace(&workspace).await.unwrap();
        let session = Session::new(&workspace.id, serde_json::json!({"client": "test"}));
        storage.insert_session(&session).await.unwrap();
        let mut task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "index",
            serde_json::json!({}),
        );
        task.status = TaskStatus::Active;
        storage.insert_task(&task).await.unwrap();

        let mut memory =
            MemoryRecord::new(&workspace.id, MemoryKind::Decision, "Use normalized chunks");
        memory.session_id = Some(session.id.clone());
        memory.task_id = Some(task.id.clone());
        storage.insert_memory(&memory).await.unwrap();
        let mut event = CortexEvent::new(
            &workspace.id,
            EventType::TaskStarted,
            serde_json::json!({"task": task.id}),
        );
        event.session_id = Some(session.id);
        event.task_id = Some(task.id);
        storage.insert_event(&event).await.unwrap();

        assert_eq!(
            storage.recent_memories(&workspace.id, 10).await.unwrap(),
            vec![memory]
        );
        assert_eq!(
            storage.recent_events(&workspace.id, 10).await.unwrap(),
            vec![event]
        );
    }

    #[tokio::test]
    async fn persists_context_state_with_workspace_provenance() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/context", "context");
        storage.insert_workspace(&workspace).await.unwrap();
        let session = Session::new(&workspace.id, serde_json::json!({}));
        storage.insert_session(&session).await.unwrap();
        let task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "assemble context",
            serde_json::json!({}),
        );
        storage.insert_task(&task).await.unwrap();

        let mut working_set = WorkingSetEntry::new(
            &workspace.id,
            &session.id,
            "chunk-1",
            ContextSourceType::Code,
            0.75,
        );
        working_set.task_id = Some(task.id.clone());
        storage
            .insert_working_set_entry(&working_set)
            .await
            .unwrap();

        let mut pin = ContextPin::new(
            &workspace.id,
            &session.id,
            "chunk-1",
            ContextSourceType::Code,
        );
        pin.task_id = Some(task.id.clone());
        storage.insert_context_pin(&pin).await.unwrap();

        let mut checkpoint = Checkpoint::new(&workspace.id, &session.id, "Phase 1 state");
        checkpoint.task_id = Some(task.id.clone());
        checkpoint.objective = Some("Add context persistence".into());
        checkpoint.completed = vec!["Compatibility audit".into()];
        checkpoint.related_paths = vec!["src/domain/context.rs".into()];
        storage.insert_checkpoint(&checkpoint).await.unwrap();

        let mut older = MemoryRecord::new(&workspace.id, MemoryKind::Decision, "Use L2");
        older.session_id = Some(session.id.clone());
        older.task_id = Some(task.id.clone());
        storage.insert_memory(&older).await.unwrap();
        let mut newer = MemoryRecord::new(&workspace.id, MemoryKind::Decision, "Use cosine");
        newer.session_id = Some(session.id.clone());
        newer.task_id = Some(task.id);
        storage.insert_memory(&newer).await.unwrap();
        let supersession = MemorySupersession::new(&workspace.id, &older.id, &newer.id);
        storage
            .insert_memory_supersession(&supersession)
            .await
            .unwrap();

        assert_eq!(
            storage.working_set_entries(&session.id).await.unwrap(),
            vec![working_set]
        );
        assert_eq!(storage.context_pins(&session.id).await.unwrap(), vec![pin]);
        assert_eq!(
            storage.checkpoints(&workspace.id).await.unwrap(),
            vec![checkpoint]
        );
        assert_eq!(
            storage.memory_supersession(&older.id).await.unwrap(),
            Some(supersession)
        );
    }

    #[tokio::test]
    async fn context_state_rejects_cross_workspace_provenance() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let first = Workspace::new("C:/first-context", "first-context");
        let second = Workspace::new("C:/second-context", "second-context");
        storage.insert_workspace(&first).await.unwrap();
        storage.insert_workspace(&second).await.unwrap();
        let session = Session::new(&first.id, serde_json::json!({}));
        storage.insert_session(&session).await.unwrap();

        let working_set = WorkingSetEntry::new(
            &second.id,
            &session.id,
            "chunk-1",
            ContextSourceType::Code,
            1.0,
        );
        assert!(
            storage
                .insert_working_set_entry(&working_set)
                .await
                .is_err()
        );

        let pin = ContextPin::new(&second.id, &session.id, "chunk-1", ContextSourceType::Code);
        assert!(storage.insert_context_pin(&pin).await.is_err());

        let checkpoint = Checkpoint::new(&second.id, &session.id, "invalid workspace");
        assert!(storage.insert_checkpoint(&checkpoint).await.is_err());

        let older = MemoryRecord::new(&first.id, MemoryKind::Decision, "First workspace");
        let newer = MemoryRecord::new(&second.id, MemoryKind::Decision, "Second workspace");
        storage.insert_memory(&older).await.unwrap();
        storage.insert_memory(&newer).await.unwrap();
        let supersession = MemorySupersession::new(&first.id, &older.id, &newer.id);
        assert!(
            storage
                .insert_memory_supersession(&supersession)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn memory_supersession_rejects_cycles() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/memory-cycles", "memory-cycles");
        storage.insert_workspace(&workspace).await.unwrap();
        let older = MemoryRecord::new(&workspace.id, MemoryKind::Decision, "Older decision");
        let newer = MemoryRecord::new(&workspace.id, MemoryKind::Decision, "Newer decision");
        storage.insert_memory(&older).await.unwrap();
        storage.insert_memory(&newer).await.unwrap();
        storage
            .insert_memory_supersession(&MemorySupersession::new(
                &workspace.id,
                &older.id,
                &newer.id,
            ))
            .await
            .unwrap();

        let error = storage
            .insert_memory_supersession(&MemorySupersession::new(
                &workspace.id,
                &newer.id,
                &older.id,
            ))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("cannot create a cycle"));
    }
}
