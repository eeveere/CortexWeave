use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, QueryBuilder, Row, Sqlite, Transaction};

use crate::{
    CortexError, Result,
    domain::{
        Checkpoint, ConsolidationInputIdentity, ConsolidationInputMember, ContextPin,
        ContextSourceType, CortexEvent, Document, EmbeddingRecord, Episode, EpisodeEvent,
        EpisodeEventAssociationRequest, EpisodeListRequest, EpisodeStatus, EpisodeTerminalRequest,
        EventType, Experience, ExperienceAssessment, ExperienceAttempt, ExperienceCodeSnapshot,
        ExperienceEvidenceLink, ExperienceGraphSnapshot, ExperienceRecord,
        GraphAnalysisExpectation, GraphAnalysisState, GraphEdge, GraphEdgeType, GraphNode,
        GraphNodeType, GraphProjectionManifest, GraphRelationshipFact, GraphRepairGeneration,
        GraphRepairMode, GraphRepairState, GraphState, MAX_EPISODE_EVENTS, MemoryClaim, MemoryKind,
        MemoryOrigin, MemoryRecord, MemorySupersession, MemoryTrust, MemoryTrustReview,
        RelationshipTargetKind, Session, SourceSegment, StoredChunk, SymbolKind, Task, TaskStatus,
        TemporalBounds, UnresolvedRelationship, WorkingSetEntry, Workspace, WorkspaceGraphRevision,
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

#[derive(Debug, Clone, Default)]
pub(crate) struct ExperienceSearchCandidates {
    pub exact_signature: Vec<String>,
    pub compatible_components: Vec<String>,
    pub lexical: Vec<String>,
    pub path: Vec<String>,
    pub graph_stable_key: Vec<String>,
    pub recent: Vec<String>,
}

pub(crate) struct ExperienceCandidateQuery<'a> {
    pub workspace_id: &'a str,
    pub exact_failure_key: Option<&'a str>,
    pub components: &'a BTreeMap<String, String>,
    pub lexical_query: Option<&'a str>,
    pub path: Option<&'a str>,
    pub graph_stable_key: Option<&'a str>,
    pub limit: usize,
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
pub(crate) struct GraphRelationshipIdentity {
    pub source_document_id: String,
    pub relationship_key: String,
}

#[derive(Debug, Clone)]
pub(crate) struct UnresolvedGraphProjection {
    pub relationship: UnresolvedRelationship,
    pub candidate_node_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct GraphReconciliationBatch {
    pub workspace_id: String,
    pub target_content_revision: i64,
    pub expected_graph_updated_at: Option<DateTime<Utc>>,
    pub update_started_at: Option<DateTime<Utc>>,
    pub repair_generation_id: Option<String>,
    pub delete_relative_path: Option<String>,
    pub source_document_id: Option<String>,
    pub nodes: Vec<GraphNode>,
    pub facts: Vec<GraphRelationshipFact>,
    pub analysis_state: Option<GraphAnalysisState>,
    pub expected_analysis: Vec<GraphAnalysisExpectation>,
    pub affected_relationships: Vec<GraphRelationshipIdentity>,
    pub edges: Vec<GraphEdge>,
    pub unresolved: Vec<UnresolvedGraphProjection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GraphReconciliationStatus {
    Current,
    Stale,
    Superseded,
}

struct EpisodeMutation {
    workspace_id: String,
    episode_id: String,
    expected_version: u64,
    request_key: String,
    operation: &'static str,
    request_hash: String,
    event_ids: Option<Vec<String>>,
    terminal_status: Option<EpisodeStatus>,
    occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GraphRepairAcquire {
    Acquired(GraphRepairGeneration),
    InProgress(GraphRepairGeneration),
}

#[derive(Debug, FromRow)]
struct GraphAnalysisSnapshotRow {
    document_id: String,
    document_analyzer_id: String,
    document_analyzer_version: String,
    document_content_revision: i64,
    state_content_revision: Option<i64>,
    state_analyzer_id: Option<String>,
    state_analyzer_version: Option<String>,
    state_structure_version: Option<String>,
    state_last_error: Option<String>,
    manifest_content_revision: Option<i64>,
    manifest_analyzer_id: Option<String>,
    manifest_analyzer_version: Option<String>,
    manifest_structure_version: Option<String>,
    manifest_node_count: Option<i64>,
    manifest_fact_count: Option<i64>,
    manifest_edge_count: Option<i64>,
    manifest_unresolved_count: Option<i64>,
    physical_node_count: i64,
    physical_fact_count: i64,
    physical_edge_count: i64,
    physical_unresolved_count: i64,
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
            "SELECT id, workspace_id, relative_path, language, analyzer_id, analyzer_version, segmentation_id, content_revision, content_hash, size_bytes, modified_at_ns, indexed_at FROM documents WHERE workspace_id = ? AND relative_path = ?",
        )
        .bind(workspace_id)
        .bind(relative_path)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn list_documents(&self, workspace_id: &str) -> Result<Vec<Document>> {
        let rows = sqlx::query_as::<_, DocumentRow>(
            "SELECT id, workspace_id, relative_path, language, analyzer_id, analyzer_version, segmentation_id, content_revision, content_hash, size_bytes, modified_at_ns, indexed_at FROM documents WHERE workspace_id = ? ORDER BY relative_path",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn delete_document(&self, workspace_id: &str, relative_path: &str) -> Result<bool> {
        let mut transaction = self.pool().begin().await?;
        let result =
            sqlx::query("DELETE FROM documents WHERE workspace_id = ? AND relative_path = ?")
                .bind(workspace_id)
                .bind(relative_path)
                .execute(&mut *transaction)
                .await?;
        let deleted = result.rows_affected() == 1;
        if deleted {
            bump_content_revision(&mut transaction, workspace_id, Utc::now()).await?;
        }
        transaction.commit().await?;
        Ok(deleted)
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

    pub(crate) async fn workspace_chunk_count(&self, workspace_id: &str) -> Result<usize> {
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM chunks c JOIN documents d ON d.id = c.document_id WHERE d.workspace_id = ?",
        )
        .bind(workspace_id)
        .fetch_one(self.pool())
        .await?;
        Ok(count as usize)
    }

    pub(crate) async fn workspace_graph_counts(
        &self,
        workspace_id: &str,
    ) -> Result<(usize, usize, usize)> {
        let (nodes,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM graph_nodes WHERE workspace_id = ?")
                .bind(workspace_id)
                .fetch_one(self.pool())
                .await?;
        let (edges,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM graph_edges WHERE workspace_id = ?")
                .bind(workspace_id)
                .fetch_one(self.pool())
                .await?;
        let (unresolved,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM unresolved_relationships WHERE workspace_id = ?")
                .bind(workspace_id)
                .fetch_one(self.pool())
                .await?;
        Ok((nodes as usize, edges as usize, unresolved as usize))
    }

    pub(crate) async fn workspace_graph_counts_by_language(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<(String, usize, usize, usize)>> {
        let rows: Vec<(String, i64, i64, i64)> = sqlx::query_as(
            "SELECT language, SUM(nodes), SUM(edges), SUM(unresolved_relationships) FROM (\
                SELECT COALESCE(n.language, d.language, 'graph') AS language, COUNT(*) AS nodes, 0 AS edges, 0 AS unresolved_relationships \
                FROM graph_nodes n LEFT JOIN documents d ON d.id = n.document_id AND d.workspace_id = n.workspace_id \
                WHERE n.workspace_id = ? GROUP BY COALESCE(n.language, d.language, 'graph') \
                UNION ALL \
                SELECT d.language AS language, 0 AS nodes, COUNT(*) AS edges, 0 AS unresolved_relationships \
                FROM graph_edges e JOIN documents d ON d.id = e.source_document_id AND d.workspace_id = e.workspace_id \
                WHERE e.workspace_id = ? GROUP BY d.language \
                UNION ALL \
                SELECT d.language AS language, 0 AS nodes, 0 AS edges, COUNT(*) AS unresolved_relationships \
                FROM unresolved_relationships u JOIN documents d ON d.id = u.source_document_id AND d.workspace_id = u.workspace_id \
                WHERE u.workspace_id = ? GROUP BY d.language\
             ) GROUP BY language ORDER BY language",
        )
        .bind(workspace_id)
        .bind(workspace_id)
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await?;
        Ok(rows
            .into_iter()
            .map(|(language, nodes, edges, unresolved)| {
                (
                    language,
                    nodes as usize,
                    edges as usize,
                    unresolved as usize,
                )
            })
            .collect())
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
        let mut transaction = self.pool().begin().await?;
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
        .execute(&mut *transaction)
        .await?;
        // Upgrade fixtures deliberately exercise pre-v0.5 schemas. Production
        // opens run every migration first; only those legacy test schemas lack
        // this frontier table and therefore retain no false ordering claim.
        if historical_frontier_schema_available(&mut transaction).await? {
            insert_historical_write_order(
                &mut transaction,
                &event.workspace_id,
                "event",
                &event.id,
            )
            .await?;
        }
        transaction.commit().await?;
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

    pub async fn event(&self, workspace_id: &str, event_id: &str) -> Result<Option<CortexEvent>> {
        let row = sqlx::query_as::<_, EventRow>(
            "SELECT id, workspace_id, session_id, task_id, event_type, payload_json, created_at FROM events WHERE workspace_id = ? AND id = ?",
        )
        .bind(workspace_id)
        .bind(event_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    /// Persists one fully-formed immutable historical record. This is a
    /// storage primitive for the later consolidator; it neither extracts an
    /// interpretation nor changes source events.
    #[allow(dead_code)] // Phase 5's internal consolidator will become the production caller.
    pub(crate) async fn insert_experience(
        &self,
        record: &ExperienceRecord,
    ) -> Result<ExperienceRecord> {
        self.insert_experience_checked(record, None, None).await
    }

    /// Consolidation acceptance rechecks the exact episode membership frontier
    /// while its immutable record is inserted in the same SQLite transaction.
    pub(crate) async fn insert_consolidated_experience(
        &self,
        record: &ExperienceRecord,
        expected_episode_version: u64,
        input_identity: &ConsolidationInputIdentity,
    ) -> Result<ExperienceRecord> {
        self.insert_experience_checked(record, Some(expected_episode_version), Some(input_identity))
            .await
    }

    async fn insert_experience_checked(
        &self,
        record: &ExperienceRecord,
        expected_episode_version: Option<u64>,
        expected_input_identity: Option<&ConsolidationInputIdentity>,
    ) -> Result<ExperienceRecord> {
        validate_experience_record(record)?;
        let experience = &record.experience;
        // A consolidation reads its immutable input before becoming a writer.
        // SQLite's deferred transactions can both read and then deadlock while
        // upgrading; take the single writer slot up front so concurrent service
        // instances serialize and the loser performs the idempotent lookup.
        let mut transaction = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let episode = episode_in_transaction(
            &mut transaction,
            &experience.workspace_id,
            &experience.episode_id,
        )
        .await?
        .ok_or_else(|| CortexError::NotFound(format!("episode {}", experience.episode_id)))?;
        if let Some(expected_episode_version) = expected_episode_version
            && episode.version != expected_episode_version
        {
            return Err(CortexError::Conflict(
                "episode changed after consolidation preview".into(),
            ));
        }
        if let Some(expected_input_identity) = expected_input_identity {
            validate_consolidation_input_in_transaction(
                &mut transaction,
                experience,
                expected_input_identity,
            )
            .await?;
        }
        if !matches!(
            episode.status,
            EpisodeStatus::Closed | EpisodeStatus::Abandoned
        ) {
            return Err(CortexError::Analysis(
                "an experience requires a closed or explicitly abandoned episode".into(),
            ));
        }
        if episode.session_id != experience.session_id || episode.task_id != experience.task_id {
            return Err(CortexError::Analysis(
                "experience scope must exactly match its episode".into(),
            ));
        }
        if episode.status == EpisodeStatus::Abandoned
            && !matches!(
                experience.outcome,
                crate::domain::ExperienceOutcome::Failure
                    | crate::domain::ExperienceOutcome::Abandoned
            )
        {
            return Err(CortexError::Analysis(
                "an abandoned episode may support only a failed or abandoned experience".into(),
            ));
        }
        if episode.status == EpisodeStatus::Closed
            && experience.outcome == crate::domain::ExperienceOutcome::Abandoned
        {
            return Err(CortexError::Analysis(
                "an abandoned experience requires an explicitly abandoned episode".into(),
            ));
        }
        if let Some(existing) = experience_by_fingerprint_in_transaction(
            &mut transaction,
            &experience.workspace_id,
            &experience.consolidation_fingerprint,
        )
        .await?
        {
            if existing.proposal_hash != experience.proposal_hash {
                return Err(CortexError::Conflict(
                    "consolidation fingerprint already has a different material proposal".into(),
                ));
            }
            let existing = experience_record_in_transaction(
                &mut transaction,
                &experience.workspace_id,
                &existing.id,
            )
            .await?
            .ok_or_else(|| {
                CortexError::Storage(sqlx::Error::Decode(
                    "experience disappeared during idempotent lookup".into(),
                ))
            })?;
            transaction.commit().await?;
            return Ok(existing);
        }

        insert_experience_row(&mut transaction, experience).await?;
        for link in &record.evidence {
            insert_experience_evidence(
                &mut transaction,
                &experience.workspace_id,
                &experience.id,
                link,
            )
            .await?;
        }
        for observation in &experience.verification.observations {
            insert_experience_verification(
                &mut transaction,
                &experience.workspace_id,
                &experience.id,
                observation,
            )
            .await?;
        }
        for attempt in &record.attempts {
            insert_experience_attempt(&mut transaction, attempt).await?;
        }
        for snapshot in &record.code_snapshots {
            insert_experience_code_snapshot(
                &mut transaction,
                &experience.workspace_id,
                &experience.id,
                snapshot,
            )
            .await?;
        }
        for snapshot in &record.graph_snapshots {
            insert_experience_graph_snapshot(
                &mut transaction,
                &experience.workspace_id,
                &experience.id,
                snapshot,
            )
            .await?;
        }
        for (ordinal, basis) in experience.evidence_strength.bases.iter().enumerate() {
            sqlx::query("INSERT INTO experience_strength_bases(workspace_id, experience_id, ordinal, basis) VALUES (?, ?, ?, ?)")
                .bind(&experience.workspace_id).bind(&experience.id).bind(ordinal_to_i64(ordinal as u64)?).bind(basis.as_str())
                .execute(&mut *transaction).await?;
        }
        let acceptance_order = insert_historical_write_order(
            &mut transaction,
            &experience.workspace_id,
            "experience_acceptance",
            &experience.id,
        )
        .await?;
        sqlx::query(
            "INSERT INTO experience_seals(workspace_id, experience_id, acceptance_order) VALUES (?, ?, ?)",
        )
        .bind(&experience.workspace_id)
        .bind(&experience.id)
        .bind(acceptance_order)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(record.clone())
    }

    /// Returns true only when SQLite durably observed every Event after the
    /// Experience acceptance frontier. Legacy rows have no provable frontier
    /// and therefore fail closed.
    pub(crate) async fn events_ingressed_after_experience(
        &self,
        workspace_id: &str,
        experience_id: &str,
        event_ids: &[String],
    ) -> Result<bool> {
        if event_ids.is_empty() {
            return Ok(false);
        }
        let acceptance_order = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT acceptance_order FROM experience_seals WHERE workspace_id = ? AND experience_id = ?",
        )
        .bind(workspace_id)
        .bind(experience_id)
        .fetch_optional(self.pool())
        .await?
        .flatten();
        let Some(acceptance_order) = acceptance_order else {
            return Ok(false);
        };

        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT sequence FROM historical_write_order WHERE workspace_id = ",
        );
        query
            .push_bind(workspace_id)
            .push(" AND entity_kind = 'event' AND entity_id IN (");
        {
            let mut separated = query.separated(", ");
            for event_id in event_ids {
                separated.push_bind(event_id);
            }
        }
        query.push(")");
        let orders = query
            .build_query_scalar::<i64>()
            .fetch_all(self.pool())
            .await?;
        Ok(orders.len() == event_ids.len()
            && orders
                .into_iter()
                .all(|event_order| event_order > acceptance_order))
    }

    pub async fn experience(
        &self,
        workspace_id: &str,
        experience_id: &str,
    ) -> Result<Option<ExperienceRecord>> {
        let mut transaction = self.pool().begin().await?;
        let result =
            experience_record_in_transaction(&mut transaction, workspace_id, experience_id).await?;
        transaction.commit().await?;
        Ok(result)
    }

    /// Returns bounded identifiers from independent deterministic candidate
    /// sources. The service performs all semantic filtering and scoring after
    /// loading immutable records.
    pub(crate) async fn experience_search_candidates(
        &self,
        query: ExperienceCandidateQuery<'_>,
    ) -> Result<ExperienceSearchCandidates> {
        let limit = memory_limit(query.limit)?;
        let exact_signature = match query.exact_failure_key {
            Some(key) => experience_search_ids(self, query.workspace_id,
                "SELECT id FROM experiences WHERE workspace_id = ? AND failure_key = ? ORDER BY created_at DESC, id DESC LIMIT ?",
                key,
                limit,
            )
            .await?,
            None => Vec::new(),
        };
        let mut compatible_components = Vec::new();
        for (name, value) in query.components {
            let token = format!("{name}:{value}");
            compatible_components.extend(
                experience_search_ids(self, query.workspace_id,
                    "SELECT id FROM experiences WHERE workspace_id = ? AND instr(failure_components, ?) > 0 ORDER BY created_at DESC, id DESC LIMIT ?",
                    &token,
                    limit,
                )
                .await?,
            );
        }
        let lexical = match query.lexical_query {
            Some(value) if !value.is_empty() => experience_search_ids(self, query.workspace_id,
                "SELECT experience.id FROM experience_fts fts JOIN experiences experience ON experience.id = fts.experience_id WHERE experience.workspace_id = ? AND experience_fts MATCH ? ORDER BY bm25(experience_fts), experience.created_at DESC, experience.id DESC LIMIT ?",
                value,
                limit,
            )
            .await?,
            _ => Vec::new(),
        };
        let path_ids = match query.path {
            Some(path) => sqlx::query_scalar::<_, String>(
                "SELECT DISTINCT experience.id FROM experiences experience LEFT JOIN experience_code_snapshots snapshot ON snapshot.workspace_id = experience.workspace_id AND snapshot.experience_id = experience.id WHERE experience.workspace_id = ? AND (experience.failure_path = ? OR snapshot.relative_path = ?) ORDER BY experience.created_at DESC, experience.id DESC LIMIT ?",
            )
            .bind(query.workspace_id)
            .bind(path)
            .bind(path)
            .bind(limit)
            .fetch_all(self.pool())
            .await?,
            None => Vec::new(),
        };
        let graph_ids = match query.graph_stable_key {
            Some(key) => experience_search_ids(self, query.workspace_id,
                "SELECT DISTINCT experience.id FROM experiences experience JOIN experience_graph_snapshots snapshot ON snapshot.workspace_id = experience.workspace_id AND snapshot.experience_id = experience.id WHERE experience.workspace_id = ? AND snapshot.node_stable_key = ? ORDER BY experience.created_at DESC, experience.id DESC LIMIT ?",
                key,
                limit,
            )
            .await?,
            None => Vec::new(),
        };
        let recent = sqlx::query_scalar::<_, String>(
            "SELECT id FROM experiences WHERE workspace_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(query.workspace_id)
        .bind(limit)
        .fetch_all(self.pool())
        .await?;
        Ok(ExperienceSearchCandidates {
            exact_signature,
            compatible_components,
            lexical,
            path: path_ids,
            graph_stable_key: graph_ids,
            recent,
        })
    }

    #[cfg(test)]
    pub(crate) async fn append_experience_assessment(
        &self,
        assessment: &ExperienceAssessment,
    ) -> Result<()> {
        validate_assessment(assessment)?;
        let mut transaction = self.pool().begin().await?;
        sqlx::query("INSERT INTO experience_assessments(id, workspace_id, experience_id, kind, actor, reason, replacement_experience_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&assessment.id).bind(&assessment.workspace_id).bind(&assessment.experience_id)
            .bind(assessment.kind.as_str()).bind(&assessment.actor).bind(&assessment.reason)
            .bind(&assessment.replacement_experience_id).bind(assessment.created_at)
            .execute(&mut *transaction).await?;
        for (ordinal, event_id) in assessment.evidence_event_ids.iter().enumerate() {
            sqlx::query("INSERT INTO experience_assessment_evidence(workspace_id, assessment_id, ordinal, event_id) VALUES (?, ?, ?, ?)")
                .bind(&assessment.workspace_id).bind(&assessment.id).bind(ordinal_to_i64(ordinal as u64)?).bind(event_id)
                .execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Appends a public reviewed assessment exactly once for its caller key.
    /// The unique index remains the concurrency backstop; a sequential retry
    /// returns the original immutable assessment instead of a new review row.
    pub(crate) async fn append_reviewed_experience_assessment(
        &self,
        assessment: &ExperienceAssessment,
        request_key: &str,
        request_hash: &str,
    ) -> Result<ExperienceAssessment> {
        validate_assessment(assessment)?;
        let mut transaction = self.pool().begin().await?;
        let existing = sqlx::query_as::<_, (String, String)>(
            "SELECT id, request_hash FROM experience_assessments WHERE workspace_id = ? AND experience_id = ? AND request_key = ?",
        )
        .bind(&assessment.workspace_id)
        .bind(&assessment.experience_id)
        .bind(request_key)
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some((id, existing_hash)) = existing {
            if existing_hash != request_hash {
                return Err(CortexError::Conflict(
                    "experience assessment request key was reused with different review content"
                        .into(),
                ));
            }
            transaction.commit().await?;
            return self
                .experience_assessment_by_id(&assessment.workspace_id, &id)
                .await?
                .ok_or_else(|| CortexError::Storage(sqlx::Error::RowNotFound));
        }
        sqlx::query("INSERT INTO experience_assessments(id, workspace_id, experience_id, kind, actor, reason, replacement_experience_id, created_at, request_key, request_hash) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&assessment.id).bind(&assessment.workspace_id).bind(&assessment.experience_id)
            .bind(assessment.kind.as_str()).bind(&assessment.actor).bind(&assessment.reason)
            .bind(&assessment.replacement_experience_id).bind(assessment.created_at)
            .bind(request_key).bind(request_hash)
            .execute(&mut *transaction).await?;
        for (ordinal, event_id) in assessment.evidence_event_ids.iter().enumerate() {
            sqlx::query("INSERT INTO experience_assessment_evidence(workspace_id, assessment_id, ordinal, event_id) VALUES (?, ?, ?, ?)")
                .bind(&assessment.workspace_id).bind(&assessment.id).bind(ordinal_to_i64(ordinal as u64)?).bind(event_id)
                .execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        Ok(assessment.clone())
    }

    async fn experience_assessment_by_id(
        &self,
        workspace_id: &str,
        assessment_id: &str,
    ) -> Result<Option<ExperienceAssessment>> {
        let row = sqlx::query_as::<_, ExperienceAssessmentRow>(
            "SELECT id, workspace_id, experience_id, kind, actor, reason, replacement_experience_id, created_at FROM experience_assessments WHERE workspace_id = ? AND id = ?",
        )
        .bind(workspace_id)
        .bind(assessment_id)
        .fetch_optional(self.pool())
        .await?;
        let Some(row) = row else { return Ok(None) };
        let evidence_event_ids = sqlx::query_scalar::<_, String>(
            "SELECT event_id FROM experience_assessment_evidence WHERE workspace_id = ? AND assessment_id = ? ORDER BY ordinal ASC",
        )
        .bind(workspace_id)
        .bind(assessment_id)
        .fetch_all(self.pool())
        .await?;
        Ok(Some(ExperienceAssessment {
            id: row.id,
            workspace_id: row.workspace_id,
            experience_id: row.experience_id,
            kind: crate::domain::ExperienceAssessmentKind::from_storage(&row.kind),
            actor: row.actor,
            reason: row.reason,
            replacement_experience_id: row.replacement_experience_id,
            evidence_event_ids,
            created_at: row.created_at,
        }))
    }

    pub async fn experience_assessments(
        &self,
        workspace_id: &str,
        experience_id: &str,
    ) -> Result<Vec<ExperienceAssessment>> {
        Ok(self
            .experience_assessment_page(
                workspace_id,
                experience_id,
                None,
                crate::domain::MAX_EXPERIENCE_ASSESSMENT_PAGE_LIMIT,
            )
            .await?
            .assessments)
    }

    pub async fn experience_assessment_page(
        &self,
        workspace_id: &str,
        experience_id: &str,
        after: Option<&crate::domain::ExperienceAssessmentCursor>,
        limit: usize,
    ) -> Result<crate::domain::ExperienceAssessmentPage> {
        if limit == 0 || limit > crate::domain::MAX_EXPERIENCE_ASSESSMENT_PAGE_LIMIT {
            return Err(CortexError::Analysis(format!(
                "experience assessment page limit must be between 1 and {}",
                crate::domain::MAX_EXPERIENCE_ASSESSMENT_PAGE_LIMIT
            )));
        }
        let sql = if after.is_some() {
            "SELECT id, workspace_id, experience_id, kind, actor, reason, replacement_experience_id, created_at FROM experience_assessments WHERE workspace_id = ? AND experience_id = ? AND (created_at < ? OR (created_at = ? AND id < ?)) ORDER BY created_at DESC, id DESC LIMIT ?"
        } else {
            "SELECT id, workspace_id, experience_id, kind, actor, reason, replacement_experience_id, created_at FROM experience_assessments WHERE workspace_id = ? AND experience_id = ? ORDER BY created_at DESC, id DESC LIMIT ?"
        };
        let mut query = sqlx::query_as::<_, ExperienceAssessmentRow>(sql)
            .bind(workspace_id)
            .bind(experience_id);
        if let Some(after) = after {
            query = query
                .bind(after.created_at)
                .bind(after.created_at)
                .bind(&after.id);
        }
        let rows = query
            .bind(i64::try_from(limit + 1).expect("bounded limit"))
            .fetch_all(self.pool())
            .await?;
        let has_more = rows.len() > limit;
        let rows = rows.into_iter().take(limit).collect::<Vec<_>>();
        let ids: Vec<_> = rows.iter().map(|row| row.id.as_str()).collect();
        let mut evidence_by_assessment: BTreeMap<String, Vec<String>> = BTreeMap::new();
        if !ids.is_empty() {
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT assessment_id, event_id FROM experience_assessment_evidence WHERE workspace_id = ",
            );
            query
                .push_bind(workspace_id)
                .push(" AND assessment_id IN (");
            {
                let mut separated = query.separated(", ");
                for id in &ids {
                    separated.push_bind(id);
                }
            }
            query.push(") ORDER BY assessment_id ASC, ordinal ASC");
            for row in query.build().fetch_all(self.pool()).await? {
                evidence_by_assessment
                    .entry(row.try_get("assessment_id")?)
                    .or_default()
                    .push(row.try_get("event_id")?);
            }
        }
        let assessments = rows
            .into_iter()
            .map(|row| ExperienceAssessment {
                evidence_event_ids: evidence_by_assessment.remove(&row.id).unwrap_or_default(),
                id: row.id,
                workspace_id: row.workspace_id,
                experience_id: row.experience_id,
                kind: crate::domain::ExperienceAssessmentKind::from_storage(&row.kind),
                actor: row.actor,
                reason: row.reason,
                replacement_experience_id: row.replacement_experience_id,
                created_at: row.created_at,
            })
            .collect::<Vec<_>>();
        let next_cursor = has_more.then(|| {
            let last = assessments
                .last()
                .expect("non-empty page with a next cursor");
            crate::domain::ExperienceAssessmentCursor {
                created_at: last.created_at,
                id: last.id.clone(),
            }
        });
        Ok(crate::domain::ExperienceAssessmentPage {
            assessments,
            next_cursor,
        })
    }

    /// Projects lifecycle in SQLite without materializing the unbounded
    /// append-only assessment ledger. Negative assessments remain terminal for
    /// ordinary context eligibility even when they are older than a display page.
    pub(crate) async fn experience_lifecycle(
        &self,
        workspace_id: &str,
        experience_id: &str,
    ) -> Result<crate::domain::ExperienceLifecycle> {
        let lifecycle = sqlx::query_scalar::<_, String>(
            "SELECT CASE
                WHEN EXISTS (SELECT 1 FROM experience_assessments WHERE workspace_id = ? AND experience_id = ? AND kind = 'superseded') THEN 'superseded'
                WHEN EXISTS (SELECT 1 FROM experience_assessments WHERE workspace_id = ? AND experience_id = ? AND kind = 'refuted') THEN 'refuted'
                WHEN EXISTS (SELECT 1 FROM experience_assessments WHERE workspace_id = ? AND experience_id = ? AND kind = 'disputed') THEN 'disputed'
                ELSE 'active' END",
        )
        .bind(workspace_id)
        .bind(experience_id)
        .bind(workspace_id)
        .bind(experience_id)
        .bind(workspace_id)
        .bind(experience_id)
        .fetch_one(self.pool())
        .await?;
        Ok(match lifecycle.as_str() {
            "superseded" => crate::domain::ExperienceLifecycle::Superseded,
            "refuted" => crate::domain::ExperienceLifecycle::Refuted,
            "disputed" => crate::domain::ExperienceLifecycle::Disputed,
            _ => crate::domain::ExperienceLifecycle::Active,
        })
    }

    pub async fn insert_episode(&self, episode: &Episode) -> Result<()> {
        sqlx::query(
            "INSERT INTO episodes(id, workspace_id, session_id, task_id, episode_type, status, title, created_by, version, started_at, ended_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&episode.id)
        .bind(&episode.workspace_id)
        .bind(&episode.session_id)
        .bind(&episode.task_id)
        .bind(episode.episode_type.as_str())
        .bind(episode.status.as_str())
        .bind(&episode.title)
        .bind(episode.created_by.as_str())
        .bind(episode_version(episode.version)?)
        .bind(episode.started_at)
        .bind(episode.ended_at)
        .bind(episode.created_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn episode(&self, workspace_id: &str, episode_id: &str) -> Result<Option<Episode>> {
        let row = sqlx::query_as::<_, EpisodeRow>(
            "SELECT id, workspace_id, session_id, task_id, episode_type, status, title, created_by, version, started_at, ended_at, created_at FROM episodes WHERE workspace_id = ? AND id = ?",
        )
        .bind(workspace_id)
        .bind(episode_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn list_episodes(&self, request: &EpisodeListRequest) -> Result<Vec<Episode>> {
        let rows = sqlx::query_as::<_, EpisodeRow>(
            "SELECT id, workspace_id, session_id, task_id, episode_type, status, title, created_by, version, started_at, ended_at, created_at FROM episodes WHERE workspace_id = ? AND (? IS NULL OR session_id = ?) AND (? IS NULL OR task_id = ?) ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(&request.workspace_id)
        .bind(&request.session_id)
        .bind(&request.session_id)
        .bind(&request.task_id)
        .bind(&request.task_id)
        .bind(memory_limit(request.limit)?)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn episode_events(
        &self,
        workspace_id: &str,
        episode_id: &str,
        limit: usize,
    ) -> Result<Vec<EpisodeEvent>> {
        let rows = sqlx::query_as::<_, EpisodeEventRow>(
            "SELECT workspace_id, episode_id, event_id, ordinal, associated_at FROM episode_events WHERE workspace_id = ? AND episode_id = ? ORDER BY ordinal ASC LIMIT ?",
        )
        .bind(workspace_id)
        .bind(episode_id)
        .bind(memory_limit(limit)?)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn associate_episode_events(
        &self,
        request: &EpisodeEventAssociationRequest,
        request_hash: &str,
        associated_at: DateTime<Utc>,
    ) -> Result<Episode> {
        self.mutate_episode(EpisodeMutation {
            workspace_id: request.workspace_id.clone(),
            episode_id: request.episode_id.clone(),
            expected_version: request.expected_version,
            request_key: request.request_key.clone(),
            operation: "add_events",
            request_hash: request_hash.to_owned(),
            event_ids: Some(request.event_ids.clone()),
            terminal_status: None,
            occurred_at: associated_at,
        })
        .await
    }

    pub async fn transition_episode(
        &self,
        request: &EpisodeTerminalRequest,
        status: EpisodeStatus,
        request_hash: &str,
        ended_at: DateTime<Utc>,
    ) -> Result<Episode> {
        debug_assert!(status.is_terminal());
        let operation = match status {
            EpisodeStatus::Closed => "close",
            EpisodeStatus::Abandoned => "abandon",
            EpisodeStatus::Open | EpisodeStatus::Invalid => {
                return Err(CortexError::Analysis(
                    "only close or abandon is supported by the v0.5 episode facade".into(),
                ));
            }
        };
        self.mutate_episode(EpisodeMutation {
            workspace_id: request.workspace_id.clone(),
            episode_id: request.episode_id.clone(),
            expected_version: request.expected_version,
            request_key: request.request_key.clone(),
            operation,
            request_hash: request_hash.to_owned(),
            event_ids: None,
            terminal_status: Some(status),
            occurred_at: ended_at,
        })
        .await
    }

    async fn mutate_episode(&self, mutation: EpisodeMutation) -> Result<Episode> {
        let workspace_id = mutation.workspace_id.as_str();
        let episode_id = mutation.episode_id.as_str();
        let request_key = mutation.request_key.as_str();
        let operation = mutation.operation;
        let request_hash = mutation.request_hash.as_str();
        let event_ids = mutation.event_ids.as_deref();
        let terminal_status = mutation.terminal_status;
        let occurred_at = mutation.occurred_at;
        let expected_version = episode_version(mutation.expected_version)?;
        let resulting_version = expected_version.checked_add(1).ok_or_else(|| {
            CortexError::Conflict("episode version cannot exceed SQLite integer range".into())
        })?;
        let mut transaction = self.pool().begin().await?;

        if let Some(existing) =
            episode_mutation_request(&mut transaction, workspace_id, episode_id, request_key)
                .await?
        {
            if existing.operation != operation || existing.request_hash != request_hash {
                return Err(CortexError::Conflict(format!(
                    "episode mutation request key {request_key} was already used with different content"
                )));
            }
            let episode = episode_in_transaction(&mut transaction, workspace_id, episode_id)
                .await?
                .ok_or_else(|| CortexError::NotFound(format!("episode {episode_id}")))?;
            if episode.version < episode_result_version(existing.resulting_version)? {
                return Err(CortexError::Storage(sqlx::Error::Decode(
                    "episode mutation result version exceeds episode version".into(),
                )));
            }
            transaction.commit().await?;
            return Ok(episode);
        }

        let reservation = sqlx::query(
            "INSERT INTO episode_mutation_requests(workspace_id, episode_id, operation, request_key, request_hash, resulting_version, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(workspace_id)
        .bind(episode_id)
        .bind(operation)
        .bind(request_key)
        .bind(request_hash)
        .bind(resulting_version)
        .bind(occurred_at)
        .execute(&mut *transaction)
        .await;
        if reservation.is_err() {
            if let Some(existing) =
                episode_mutation_request(&mut transaction, workspace_id, episode_id, request_key)
                    .await?
            {
                if existing.operation == operation && existing.request_hash == request_hash {
                    let episode =
                        episode_in_transaction(&mut transaction, workspace_id, episode_id)
                            .await?
                            .ok_or_else(|| {
                                CortexError::NotFound(format!("episode {episode_id}"))
                            })?;
                    if episode.version < episode_result_version(existing.resulting_version)? {
                        return Err(CortexError::Storage(sqlx::Error::Decode(
                            "episode mutation result version exceeds episode version".into(),
                        )));
                    }
                    transaction.commit().await?;
                    return Ok(episode);
                }
                return Err(CortexError::Conflict(format!(
                    "episode mutation request key {request_key} was already used with different content"
                )));
            }
            return Err(reservation.expect_err("checked reservation error").into());
        }

        let updated = match terminal_status {
            Some(status) => sqlx::query(
                "UPDATE episodes SET status = ?, ended_at = ?, version = ? WHERE workspace_id = ? AND id = ? AND status = 'open' AND version = ?",
            )
            .bind(status.as_str())
            .bind(occurred_at)
            .bind(resulting_version)
            .bind(workspace_id)
            .bind(episode_id)
            .bind(expected_version)
            .execute(&mut *transaction)
            .await?,
            None => sqlx::query(
                "UPDATE episodes SET version = ? WHERE workspace_id = ? AND id = ? AND status = 'open' AND version = ?",
            )
            .bind(resulting_version)
            .bind(workspace_id)
            .bind(episode_id)
            .bind(expected_version)
            .execute(&mut *transaction)
            .await?,
        };
        if updated.rows_affected() != 1 {
            return episode_mutation_conflict(
                &mut transaction,
                workspace_id,
                episode_id,
                expected_version,
            )
            .await;
        }

        if let Some(event_ids) = event_ids {
            let ordinal_start: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM episode_events WHERE workspace_id = ? AND episode_id = ?",
            )
            .bind(workspace_id)
            .bind(episode_id)
            .fetch_one(&mut *transaction)
            .await?;
            let requested = i64::try_from(event_ids.len()).map_err(|_| {
                CortexError::Analysis("episode event count exceeds SQLite integer range".into())
            })?;
            let total = ordinal_start.checked_add(requested).ok_or_else(|| {
                CortexError::Analysis("episode event count exceeds SQLite integer range".into())
            })?;
            if total > i64::try_from(MAX_EPISODE_EVENTS).expect("episode limit fits i64") {
                return Err(CortexError::Analysis(format!(
                    "episode membership cannot exceed {MAX_EPISODE_EVENTS} events"
                )));
            }
            for (index, event_id) in event_ids.iter().enumerate() {
                let ordinal = ordinal_start
                    .checked_add(i64::try_from(index).map_err(|_| {
                        CortexError::Conflict(
                            "episode event ordinal exceeds SQLite integer range".into(),
                        )
                    })?)
                    .ok_or_else(|| {
                        CortexError::Conflict(
                            "episode event ordinal exceeds SQLite integer range".into(),
                        )
                    })?;
                let insertion = sqlx::query(
                    "INSERT INTO episode_events(workspace_id, episode_id, event_id, ordinal, associated_at) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(workspace_id)
                .bind(episode_id)
                .bind(event_id)
                .bind(ordinal)
                .bind(occurred_at)
                .execute(&mut *transaction)
                .await;
                if let Err(error) = insertion {
                    if error
                        .as_database_error()
                        .is_some_and(|database| database.is_unique_violation())
                    {
                        return Err(CortexError::Conflict(format!(
                            "event {event_id} already has a primary episode membership"
                        )));
                    }
                    return Err(error.into());
                }
            }
        }

        let episode = episode_in_transaction(&mut transaction, workspace_id, episode_id)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("episode {episode_id}")))?;
        transaction.commit().await?;
        Ok(episode)
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
            ContextSourceType::Experience => None,
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
            ContextSourceType::Experience => false,
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

    pub async fn workspace_graph_revision(
        &self,
        workspace_id: &str,
    ) -> Result<Option<WorkspaceGraphRevision>> {
        let row = sqlx::query_as::<_, WorkspaceGraphRevisionRow>(
            "SELECT workspace_id, content_revision, graph_content_revision, graph_schema_version, graph_state, graph_update_started_at, failed_graph_target_revision, last_graph_error, updated_at FROM workspace_graph_revisions WHERE workspace_id = ?",
        )
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn mark_graph_updating(
        &self,
        workspace_id: &str,
        started_at: DateTime<Utc>,
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE workspace_graph_revisions SET graph_state = 'updating', graph_update_started_at = ?, failed_graph_target_revision = NULL, last_graph_error = NULL, updated_at = ? WHERE workspace_id = ?",
        )
        .bind(started_at)
        .bind(started_at)
        .bind(workspace_id)
        .execute(self.pool())
        .await?;
        require_one(
            result.rows_affected(),
            "workspace graph revision",
            workspace_id,
        )
    }

    pub(crate) async fn mark_graph_updating_if_current(
        &self,
        workspace_id: &str,
        target_revision: i64,
        expected_graph_updated_at: DateTime<Utc>,
        started_at: DateTime<Utc>,
        repair_generation_id: Option<&str>,
    ) -> Result<bool> {
        let result = if let Some(generation_id) = repair_generation_id {
            sqlx::query(
                "UPDATE workspace_graph_revisions SET graph_state = 'updating', graph_update_started_at = ?, updated_at = ? WHERE workspace_id = ? AND content_revision = ? AND updated_at = ? AND EXISTS (SELECT 1 FROM workspace_graph_repairs repair WHERE repair.workspace_id = ? AND repair.generation_id = ? AND repair.target_content_revision = ? AND repair.state = 'active' AND repair.lease_expires_at > ?)",
            )
            .bind(started_at)
            .bind(started_at)
            .bind(workspace_id)
            .bind(target_revision)
            .bind(expected_graph_updated_at)
            .bind(workspace_id)
            .bind(generation_id)
            .bind(target_revision)
            .bind(started_at)
            .execute(self.pool())
            .await?
        } else {
            sqlx::query(
                "UPDATE workspace_graph_revisions SET graph_state = 'updating', graph_update_started_at = ?, updated_at = ? WHERE workspace_id = ? AND content_revision = ? AND updated_at = ? AND NOT EXISTS (SELECT 1 FROM workspace_graph_repairs repair WHERE repair.workspace_id = ? AND repair.state IN ('active', 'failed', 'interrupted'))",
            )
            .bind(started_at)
            .bind(started_at)
            .bind(workspace_id)
            .bind(target_revision)
            .bind(expected_graph_updated_at)
            .bind(workspace_id)
            .execute(self.pool())
            .await?
        };
        Ok(result.rows_affected() == 1)
    }

    pub async fn mark_graph_error(
        &self,
        workspace_id: &str,
        target_revision: i64,
        error: &str,
        updated_at: DateTime<Utc>,
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE workspace_graph_revisions SET graph_state = 'error', graph_update_started_at = NULL, failed_graph_target_revision = ?, last_graph_error = ?, updated_at = ? WHERE workspace_id = ?",
        )
        .bind(target_revision)
        .bind(error)
        .bind(updated_at)
        .bind(workspace_id)
        .execute(self.pool())
        .await?;
        require_one(
            result.rows_affected(),
            "workspace graph revision",
            workspace_id,
        )
    }

    pub(crate) async fn mark_graph_error_if_snapshot(
        &self,
        workspace_id: &str,
        target_revision: i64,
        expected_graph_updated_at: DateTime<Utc>,
        repair_generation_id: Option<&str>,
        error: &str,
        updated_at: DateTime<Utc>,
    ) -> Result<bool> {
        let result = if let Some(generation_id) = repair_generation_id {
            sqlx::query(
                "UPDATE workspace_graph_revisions SET graph_state = 'error', graph_update_started_at = NULL, failed_graph_target_revision = ?, last_graph_error = ?, updated_at = ? WHERE workspace_id = ? AND content_revision = ? AND updated_at = ? AND EXISTS (SELECT 1 FROM workspace_graph_repairs repair WHERE repair.workspace_id = ? AND repair.generation_id = ? AND repair.target_content_revision = ? AND repair.state = 'active' AND repair.lease_expires_at > ?)",
            )
            .bind(target_revision)
            .bind(error)
            .bind(updated_at)
            .bind(workspace_id)
            .bind(target_revision)
            .bind(expected_graph_updated_at)
            .bind(workspace_id)
            .bind(generation_id)
            .bind(target_revision)
            .bind(updated_at)
            .execute(self.pool())
            .await?
        } else {
            sqlx::query(
                "UPDATE workspace_graph_revisions SET graph_state = 'error', graph_update_started_at = NULL, failed_graph_target_revision = ?, last_graph_error = ?, updated_at = ? WHERE workspace_id = ? AND content_revision = ? AND updated_at = ? AND NOT EXISTS (SELECT 1 FROM workspace_graph_repairs repair WHERE repair.workspace_id = ? AND repair.state IN ('active', 'failed', 'interrupted'))",
            )
            .bind(target_revision)
            .bind(error)
            .bind(updated_at)
            .bind(workspace_id)
            .bind(target_revision)
            .bind(expected_graph_updated_at)
            .bind(workspace_id)
            .execute(self.pool())
            .await?
        };
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn mark_graph_error_for_update(
        &self,
        workspace_id: &str,
        target_revision: i64,
        update_started_at: DateTime<Utc>,
        error: &str,
        updated_at: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE workspace_graph_revisions SET graph_state = 'error', graph_update_started_at = NULL, failed_graph_target_revision = ?, last_graph_error = ?, updated_at = ? WHERE workspace_id = ? AND content_revision = ? AND graph_state = 'updating' AND graph_update_started_at = ?",
        )
        .bind(target_revision)
        .bind(error)
        .bind(updated_at)
        .bind(workspace_id)
        .bind(target_revision)
        .bind(update_started_at)
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn clear_graph_update_if_owned(
        &self,
        workspace_id: &str,
        target_revision: i64,
        update_started_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE workspace_graph_revisions SET graph_state = 'stale', graph_update_started_at = NULL, updated_at = ? WHERE workspace_id = ? AND content_revision = ? AND graph_state = 'updating' AND graph_update_started_at = ?",
        )
        .bind(updated_at)
        .bind(workspace_id)
        .bind(target_revision)
        .bind(update_started_at)
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn acknowledge_graph_revision(
        &self,
        workspace_id: &str,
        target_revision: i64,
        updated_at: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE workspace_graph_revisions SET graph_content_revision = ?, graph_state = CASE WHEN content_revision = ? THEN 'current' ELSE 'stale' END, graph_update_started_at = NULL, failed_graph_target_revision = CASE WHEN content_revision = ? THEN NULL ELSE failed_graph_target_revision END, last_graph_error = CASE WHEN content_revision = ? THEN NULL ELSE last_graph_error END, updated_at = ? WHERE workspace_id = ? AND content_revision = ? AND graph_content_revision <= ?",
        )
        .bind(target_revision)
        .bind(target_revision)
        .bind(target_revision)
        .bind(target_revision)
        .bind(updated_at)
        .bind(workspace_id)
        .bind(target_revision)
        .bind(target_revision)
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn workspace_graph_repair(
        &self,
        workspace_id: &str,
    ) -> Result<Option<GraphRepairGeneration>> {
        let row = sqlx::query_as::<_, GraphRepairGenerationRow>(
            "SELECT workspace_id, generation_id, mode, target_content_revision, state, started_at, lease_expires_at, updated_at, completed_at, documents_considered, documents_repaired, documents_failed, last_error FROM workspace_graph_repairs WHERE workspace_id = ?",
        )
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub(crate) async fn acquire_graph_repair(
        &self,
        generation: &GraphRepairGeneration,
        now: DateTime<Utc>,
    ) -> Result<GraphRepairAcquire> {
        let mut transaction = self.pool().begin().await?;
        sqlx::query(
            "UPDATE workspace_graph_repairs SET state = 'interrupted', updated_at = ?, last_error = COALESCE(last_error, 'graph repair lease expired') WHERE workspace_id = ? AND state = 'active' AND lease_expires_at <= ?",
        )
        .bind(now)
        .bind(&generation.workspace_id)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
        let inserted = sqlx::query(
            "INSERT INTO workspace_graph_repairs(workspace_id, generation_id, mode, target_content_revision, state, started_at, lease_expires_at, updated_at, completed_at, documents_considered, documents_repaired, documents_failed, last_error) VALUES (?, ?, ?, ?, 'active', ?, ?, ?, NULL, ?, 0, 0, NULL) ON CONFLICT(workspace_id) DO UPDATE SET generation_id = excluded.generation_id, mode = excluded.mode, target_content_revision = excluded.target_content_revision, state = 'active', started_at = excluded.started_at, lease_expires_at = excluded.lease_expires_at, updated_at = excluded.updated_at, completed_at = NULL, documents_considered = excluded.documents_considered, documents_repaired = 0, documents_failed = 0, last_error = NULL WHERE workspace_graph_repairs.state IN ('completed', 'failed', 'interrupted')",
        )
        .bind(&generation.workspace_id)
        .bind(&generation.generation_id)
        .bind(generation.mode.storage_name())
        .bind(generation.target_content_revision)
        .bind(generation.started_at)
        .bind(generation.lease_expires_at)
        .bind(generation.updated_at)
        .bind(i64::try_from(generation.documents_considered).map_err(|_| {
            CortexError::Analysis("graph repair document count exceeds SQLite integer range".into())
        })?)
        .execute(&mut *transaction)
        .await?;
        if inserted.rows_affected() == 1 {
            transaction.commit().await?;
            return Ok(GraphRepairAcquire::Acquired(generation.clone()));
        }
        let current = sqlx::query_as::<_, GraphRepairGenerationRow>(
            "SELECT workspace_id, generation_id, mode, target_content_revision, state, started_at, lease_expires_at, updated_at, completed_at, documents_considered, documents_repaired, documents_failed, last_error FROM workspace_graph_repairs WHERE workspace_id = ?",
        )
        .bind(&generation.workspace_id)
        .fetch_one(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(GraphRepairAcquire::InProgress(current.try_into()?))
    }

    pub(crate) async fn record_graph_repair_progress(
        &self,
        workspace_id: &str,
        generation_id: &str,
        documents_repaired: usize,
        lease_expires_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> Result<bool> {
        if lease_expires_at <= updated_at {
            return Err(CortexError::Analysis(
                "graph repair lease renewal must expire after its update time".into(),
            ));
        }
        let result = sqlx::query(
            "UPDATE workspace_graph_repairs SET documents_repaired = ?, lease_expires_at = ?, updated_at = ? WHERE workspace_id = ? AND generation_id = ? AND state = 'active' AND lease_expires_at > ?",
        )
        .bind(i64::try_from(documents_repaired).map_err(|_| {
            CortexError::Analysis("graph repair document count exceeds SQLite integer range".into())
        })?)
        .bind(lease_expires_at)
        .bind(updated_at)
        .bind(workspace_id)
        .bind(generation_id)
        .bind(updated_at)
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn fail_graph_repair(
        &self,
        workspace_id: &str,
        generation_id: &str,
        documents_repaired: usize,
        error: &str,
        updated_at: DateTime<Utc>,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE workspace_graph_repairs SET state = 'failed', documents_repaired = ?, documents_failed = documents_failed + 1, updated_at = ?, last_error = ? WHERE workspace_id = ? AND generation_id = ? AND state = 'active' AND lease_expires_at > ?",
        )
        .bind(i64::try_from(documents_repaired).map_err(|_| {
            CortexError::Analysis("graph repair document count exceeds SQLite integer range".into())
        })?)
        .bind(updated_at)
        .bind(error)
        .bind(workspace_id)
        .bind(generation_id)
        .bind(updated_at)
        .execute(self.pool())
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub(crate) async fn complete_graph_repair(
        &self,
        workspace_id: &str,
        generation_id: &str,
        target_revision: i64,
        expected_analysis: &[GraphAnalysisExpectation],
        updated_at: DateTime<Utc>,
    ) -> Result<bool> {
        let mut transaction = self.pool().begin().await?;
        let owned: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM workspace_graph_repairs WHERE workspace_id = ? AND generation_id = ? AND target_content_revision = ? AND state = 'active' AND lease_expires_at > ?",
        )
        .bind(workspace_id)
        .bind(generation_id)
        .bind(target_revision)
        .bind(updated_at)
        .fetch_one(&mut *transaction)
        .await?;
        if owned != 1 {
            transaction.rollback().await?;
            return Ok(false);
        }
        let rows = graph_analysis_snapshot_rows(&mut transaction, workspace_id).await?;
        if !analysis_expectations_match(&rows, expected_analysis) {
            transaction.rollback().await?;
            return Ok(false);
        }
        let acknowledged = sqlx::query(
            "UPDATE workspace_graph_revisions SET graph_content_revision = ?, graph_state = 'current', graph_update_started_at = NULL, failed_graph_target_revision = NULL, last_graph_error = NULL, updated_at = ? WHERE workspace_id = ? AND content_revision = ?",
        )
        .bind(target_revision)
        .bind(updated_at)
        .bind(workspace_id)
        .bind(target_revision)
        .execute(&mut *transaction)
        .await?;
        if acknowledged.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(false);
        }
        let completed = sqlx::query(
            "UPDATE workspace_graph_repairs SET state = 'completed', lease_expires_at = ?, updated_at = ?, completed_at = ?, last_error = NULL WHERE workspace_id = ? AND generation_id = ? AND state = 'active' AND lease_expires_at > ?",
        )
        .bind(updated_at)
        .bind(updated_at)
        .bind(updated_at)
        .bind(workspace_id)
        .bind(generation_id)
        .bind(updated_at)
        .execute(&mut *transaction)
        .await?;
        if completed.rows_affected() != 1 {
            transaction.rollback().await?;
            return Ok(false);
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn graph_projection_manifest(
        &self,
        document_id: &str,
    ) -> Result<Option<GraphProjectionManifest>> {
        let row = sqlx::query_as::<_, GraphProjectionManifestRow>(
            "SELECT document_id, workspace_id, content_revision, analyzer_id, analyzer_version, structure_version, node_count, fact_count, edge_count, unresolved_count, projected_at FROM graph_document_projections WHERE document_id = ?",
        )
        .bind(document_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn graph_analysis_state(
        &self,
        document_id: &str,
    ) -> Result<Option<GraphAnalysisState>> {
        let row = sqlx::query_as::<_, GraphAnalysisStateRow>(
            "SELECT document_id, workspace_id, content_revision, analyzer_id, analyzer_version, structure_version, last_error, analyzed_at FROM graph_document_states WHERE document_id = ?",
        )
        .bind(document_id)
        .fetch_optional(self.pool())
        .await?;
        Ok(row.map(Into::into))
    }

    pub async fn upsert_graph_analysis_state(&self, state: &GraphAnalysisState) -> Result<()> {
        sqlx::query(
            "INSERT INTO graph_document_states(document_id, workspace_id, content_revision, analyzer_id, analyzer_version, structure_version, last_error, analyzed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(document_id) DO UPDATE SET workspace_id = excluded.workspace_id, content_revision = excluded.content_revision, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, structure_version = excluded.structure_version, last_error = excluded.last_error, analyzed_at = excluded.analyzed_at",
        )
        .bind(&state.document_id)
        .bind(&state.workspace_id)
        .bind(state.content_revision)
        .bind(&state.analyzer_id)
        .bind(&state.analyzer_version)
        .bind(&state.structure_version)
        .bind(&state.last_error)
        .bind(state.analyzed_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn graph_node_by_stable_key(
        &self,
        workspace_id: &str,
        stable_key: &str,
    ) -> Result<Option<GraphNode>> {
        let row = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, workspace_id, node_type, stable_key, language, name, qualified_name, document_id, chunk_id, source_path, source_start_byte, source_end_byte, analyzer_id, analyzer_version, structure_version, content_revision, metadata_json, created_at, updated_at FROM graph_nodes WHERE workspace_id = ? AND stable_key = ?",
        )
        .bind(workspace_id)
        .bind(stable_key)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn graph_nodes(&self, workspace_id: &str) -> Result<Vec<GraphNode>> {
        let rows = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, workspace_id, node_type, stable_key, language, name, qualified_name, document_id, chunk_id, source_path, source_start_byte, source_end_byte, analyzer_id, analyzer_version, structure_version, content_revision, metadata_json, created_at, updated_at FROM graph_nodes WHERE workspace_id = ? ORDER BY stable_key",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub(crate) async fn graph_nodes_matching(
        &self,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<GraphNode>> {
        if limit == 0 || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let query = query.trim().replace('\\', "/");
        let rows = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, workspace_id, node_type, stable_key, language, name, qualified_name, document_id, chunk_id, source_path, source_start_byte, source_end_byte, analyzer_id, analyzer_version, structure_version, content_revision, metadata_json, created_at, updated_at FROM graph_nodes WHERE workspace_id = ? AND (stable_key = ? OR qualified_name = ? OR name = ? OR source_path = ?) ORDER BY CASE WHEN stable_key = ? THEN 0 WHEN qualified_name = ? THEN 1 WHEN source_path = ? THEN 2 ELSE 3 END, stable_key LIMIT ?",
        )
        .bind(workspace_id)
        .bind(&query)
        .bind(&query)
        .bind(&query)
        .bind(&query)
        .bind(&query)
        .bind(&query)
        .bind(&query)
        .bind(memory_limit(limit)?)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub(crate) async fn graph_nodes_for_chunk(
        &self,
        workspace_id: &str,
        chunk_id: &str,
        limit: usize,
    ) -> Result<Vec<GraphNode>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT n.id, n.workspace_id, n.node_type, n.stable_key, n.language, n.name, n.qualified_name, n.document_id, n.chunk_id, n.source_path, n.source_start_byte, n.source_end_byte, n.analyzer_id, n.analyzer_version, n.structure_version, n.content_revision, n.metadata_json, n.created_at, n.updated_at FROM graph_nodes n JOIN chunks c ON c.document_id = n.document_id JOIN documents d ON d.id = c.document_id AND d.workspace_id = n.workspace_id WHERE n.workspace_id = ? AND c.id = ? AND n.source_start_byte IS NOT NULL AND n.source_end_byte IS NOT NULL AND n.source_start_byte <= c.end_byte AND n.source_end_byte >= c.start_byte ORDER BY CASE WHEN n.node_type = 'file' THEN 1 ELSE 0 END, (n.source_end_byte - n.source_start_byte), n.stable_key LIMIT ?",
        )
        .bind(workspace_id)
        .bind(chunk_id)
        .bind(memory_limit(limit)?)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn graph_node_by_id(
        &self,
        workspace_id: &str,
        node_id: &str,
    ) -> Result<Option<GraphNode>> {
        let row = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, workspace_id, node_type, stable_key, language, name, qualified_name, document_id, chunk_id, source_path, source_start_byte, source_end_byte, analyzer_id, analyzer_version, structure_version, content_revision, metadata_json, created_at, updated_at FROM graph_nodes WHERE workspace_id = ? AND id = ?",
        )
        .bind(workspace_id)
        .bind(node_id)
        .fetch_optional(self.pool())
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn graph_nodes_by_qualified_name(
        &self,
        workspace_id: &str,
        qualified_name: &str,
        language: Option<&str>,
    ) -> Result<Vec<GraphNode>> {
        let rows = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, workspace_id, node_type, stable_key, language, name, qualified_name, document_id, chunk_id, source_path, source_start_byte, source_end_byte, analyzer_id, analyzer_version, structure_version, content_revision, metadata_json, created_at, updated_at FROM graph_nodes WHERE workspace_id = ? AND qualified_name = ? AND (? IS NULL OR language = ?) ORDER BY stable_key",
        )
        .bind(workspace_id)
        .bind(qualified_name)
        .bind(language)
        .bind(language)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn graph_nodes_by_name(
        &self,
        workspace_id: &str,
        name: &str,
        language: Option<&str>,
    ) -> Result<Vec<GraphNode>> {
        let rows = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, workspace_id, node_type, stable_key, language, name, qualified_name, document_id, chunk_id, source_path, source_start_byte, source_end_byte, analyzer_id, analyzer_version, structure_version, content_revision, metadata_json, created_at, updated_at FROM graph_nodes WHERE workspace_id = ? AND name = ? AND (? IS NULL OR language = ?) ORDER BY stable_key",
        )
        .bind(workspace_id)
        .bind(name)
        .bind(language)
        .bind(language)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn graph_nodes_for_document(
        &self,
        workspace_id: &str,
        document_id: &str,
    ) -> Result<Vec<GraphNode>> {
        let rows = sqlx::query_as::<_, GraphNodeRow>(
            "SELECT id, workspace_id, node_type, stable_key, language, name, qualified_name, document_id, chunk_id, source_path, source_start_byte, source_end_byte, analyzer_id, analyzer_version, structure_version, content_revision, metadata_json, created_at, updated_at FROM graph_nodes WHERE workspace_id = ? AND document_id = ? ORDER BY stable_key",
        )
        .bind(workspace_id)
        .bind(document_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn delete_graph_node(&self, workspace_id: &str, node_id: &str) -> Result<bool> {
        let result = sqlx::query("DELETE FROM graph_nodes WHERE workspace_id = ? AND id = ?")
            .bind(workspace_id)
            .bind(node_id)
            .execute(self.pool())
            .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn upsert_graph_node(&self, node: &GraphNode) -> Result<GraphNode> {
        let (source_path, source_start_byte, source_end_byte) =
            graph_source_segment_bindings(&node.source_segment)?;
        let mut transaction = self.pool().begin().await?;
        let conflicting_owner: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM graph_nodes WHERE workspace_id = ? AND stable_key = ? AND document_id IS NOT ?",
        )
        .bind(&node.workspace_id)
        .bind(&node.stable_key)
        .bind(&node.document_id)
        .fetch_one(&mut *transaction)
        .await?;
        if conflicting_owner != 0 {
            return Err(CortexError::Analysis(format!(
                "graph node stable key {} is already owned by another document",
                node.stable_key
            )));
        }
        sqlx::query(
            "INSERT INTO graph_nodes(id, workspace_id, node_type, stable_key, language, name, qualified_name, document_id, chunk_id, source_path, source_start_byte, source_end_byte, analyzer_id, analyzer_version, structure_version, content_revision, metadata_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(workspace_id, stable_key) DO UPDATE SET node_type = excluded.node_type, language = excluded.language, name = excluded.name, qualified_name = excluded.qualified_name, document_id = excluded.document_id, chunk_id = excluded.chunk_id, source_path = excluded.source_path, source_start_byte = excluded.source_start_byte, source_end_byte = excluded.source_end_byte, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, structure_version = excluded.structure_version, content_revision = excluded.content_revision, metadata_json = excluded.metadata_json, updated_at = excluded.updated_at",
        )
        .bind(&node.id)
        .bind(&node.workspace_id)
        .bind(node.node_type.storage_name())
        .bind(&node.stable_key)
        .bind(&node.language)
        .bind(&node.name)
        .bind(&node.qualified_name)
        .bind(&node.document_id)
        .bind(&node.chunk_id)
        .bind(source_path)
        .bind(source_start_byte)
        .bind(source_end_byte)
        .bind(&node.analyzer_id)
        .bind(&node.analyzer_version)
        .bind(&node.structure_version)
        .bind(node.content_revision)
        .bind(serde_json::to_string(&node.metadata)?)
        .bind(node.created_at)
        .bind(node.updated_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        self.graph_node_by_stable_key(&node.workspace_id, &node.stable_key)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("graph node {}", node.stable_key)))
    }

    pub async fn reconcile_graph_nodes_for_document(
        &self,
        workspace_id: &str,
        document_id: &str,
        nodes: &[GraphNode],
    ) -> Result<Vec<GraphNode>> {
        let mut stable_keys = HashSet::with_capacity(nodes.len());
        for node in nodes {
            if node.workspace_id != workspace_id || node.document_id.as_deref() != Some(document_id)
            {
                return Err(CortexError::Analysis(
                    "graph node reconciliation crossed a workspace or document boundary".into(),
                ));
            }
            if !stable_keys.insert(node.stable_key.as_str()) {
                return Err(CortexError::Analysis(format!(
                    "duplicate graph node stable key {}",
                    node.stable_key
                )));
            }
        }

        let mut transaction = self.pool().begin().await?;
        let previous_keys: Vec<String> = sqlx::query_scalar(
            "SELECT stable_key FROM graph_nodes WHERE workspace_id = ? AND document_id = ?",
        )
        .bind(workspace_id)
        .bind(document_id)
        .fetch_all(&mut *transaction)
        .await?;

        for node in nodes {
            let conflicting_owner: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM graph_nodes WHERE workspace_id = ? AND stable_key = ? AND document_id IS NOT ?",
            )
            .bind(workspace_id)
            .bind(&node.stable_key)
            .bind(&node.document_id)
            .fetch_one(&mut *transaction)
            .await?;
            if conflicting_owner != 0 {
                return Err(CortexError::Analysis(format!(
                    "graph node stable key {} is already owned by another document",
                    node.stable_key
                )));
            }
            let (source_path, source_start_byte, source_end_byte) =
                graph_source_segment_bindings(&node.source_segment)?;
            sqlx::query(
                "INSERT INTO graph_nodes(id, workspace_id, node_type, stable_key, language, name, qualified_name, document_id, chunk_id, source_path, source_start_byte, source_end_byte, analyzer_id, analyzer_version, structure_version, content_revision, metadata_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(workspace_id, stable_key) DO UPDATE SET node_type = excluded.node_type, language = excluded.language, name = excluded.name, qualified_name = excluded.qualified_name, document_id = excluded.document_id, chunk_id = excluded.chunk_id, source_path = excluded.source_path, source_start_byte = excluded.source_start_byte, source_end_byte = excluded.source_end_byte, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, structure_version = excluded.structure_version, content_revision = excluded.content_revision, metadata_json = excluded.metadata_json, updated_at = excluded.updated_at",
            )
            .bind(&node.id)
            .bind(&node.workspace_id)
            .bind(node.node_type.storage_name())
            .bind(&node.stable_key)
            .bind(&node.language)
            .bind(&node.name)
            .bind(&node.qualified_name)
            .bind(&node.document_id)
            .bind(&node.chunk_id)
            .bind(source_path)
            .bind(source_start_byte)
            .bind(source_end_byte)
            .bind(&node.analyzer_id)
            .bind(&node.analyzer_version)
            .bind(&node.structure_version)
            .bind(node.content_revision)
            .bind(serde_json::to_string(&node.metadata)?)
            .bind(node.created_at)
            .bind(node.updated_at)
            .execute(&mut *transaction)
            .await?;
        }

        for previous_key in previous_keys {
            if !stable_keys.contains(previous_key.as_str()) {
                sqlx::query(
                    "DELETE FROM graph_nodes WHERE workspace_id = ? AND document_id = ? AND stable_key = ?",
                )
                .bind(workspace_id)
                .bind(document_id)
                .bind(previous_key)
                .execute(&mut *transaction)
                .await?;
            }
        }
        transaction.commit().await?;
        self.graph_nodes_for_document(workspace_id, document_id)
            .await
    }

    pub async fn graph_edges(&self, workspace_id: &str) -> Result<Vec<GraphEdge>> {
        let rows = sqlx::query_as::<_, GraphEdgeRow>(
            "SELECT id, workspace_id, relationship_key, relationship_fact_id, from_node, to_node, edge_type, confidence, analyzer_id, analyzer_version, structure_version, source_document_id, source_path, source_start_byte, source_end_byte, content_revision, metadata_json, created_at FROM graph_edges WHERE workspace_id = ? ORDER BY relationship_key",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub(crate) async fn graph_edges_for_node(
        &self,
        workspace_id: &str,
        node_id: &str,
        incoming: bool,
        edge_types: &[GraphEdgeType],
        limit: usize,
    ) -> Result<Vec<GraphEdge>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let endpoint = if incoming { "to_node" } else { "from_node" };
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT id, workspace_id, relationship_key, relationship_fact_id, from_node, to_node, edge_type, confidence, analyzer_id, analyzer_version, structure_version, source_document_id, source_path, source_start_byte, source_end_byte, content_revision, metadata_json, created_at FROM graph_edges WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(format!(" AND {endpoint} = "));
        query.push_bind(node_id);
        if !edge_types.is_empty() {
            query.push(" AND edge_type IN (");
            let mut separated = query.separated(", ");
            for edge_type in edge_types {
                separated.push_bind(edge_type.storage_name());
            }
            separated.push_unseparated(")");
        }
        query.push(" ORDER BY edge_type, relationship_key LIMIT ");
        query.push_bind(memory_limit(limit)?);
        let rows = query
            .build_query_as::<GraphEdgeRow>()
            .fetch_all(self.pool())
            .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub(crate) async fn code_candidates_for_graph_node(
        &self,
        workspace_id: &str,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<CodeCandidate>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let rows = sqlx::query_as::<_, CodeCandidateRow>(
            "SELECT c.id, c.document_id, c.stable_key, c.language, c.symbol, c.qualified_symbol, c.symbol_kind, c.start_byte, c.end_byte, c.start_line, c.end_line, c.content, c.content_hash, c.metadata_json, c.created_at, c.updated_at, d.workspace_id, d.relative_path FROM graph_nodes n JOIN documents d ON d.id = n.document_id AND d.workspace_id = n.workspace_id JOIN chunks c ON c.document_id = n.document_id WHERE n.workspace_id = ? AND n.id = ? ORDER BY CASE WHEN n.source_start_byte IS NOT NULL AND n.source_end_byte IS NOT NULL AND c.start_byte <= n.source_end_byte AND c.end_byte >= n.source_start_byte THEN 0 ELSE 1 END, ABS(c.start_byte - COALESCE(n.source_start_byte, c.start_byte)), (c.end_byte - c.start_byte), c.start_byte, c.id LIMIT ?",
        )
        .bind(workspace_id)
        .bind(node_id)
        .bind(memory_limit(limit)?)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn upsert_graph_edge(&self, edge: &GraphEdge) -> Result<GraphEdge> {
        let (source_path, source_start_byte, source_end_byte) =
            graph_source_segment_bindings(&edge.source_segment)?;
        sqlx::query(
            "INSERT INTO graph_edges(id, workspace_id, relationship_key, relationship_fact_id, from_node, to_node, edge_type, confidence, analyzer_id, analyzer_version, structure_version, source_document_id, source_path, source_start_byte, source_end_byte, content_revision, metadata_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(workspace_id, relationship_key) DO UPDATE SET relationship_fact_id = excluded.relationship_fact_id, from_node = excluded.from_node, to_node = excluded.to_node, edge_type = excluded.edge_type, confidence = excluded.confidence, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, structure_version = excluded.structure_version, source_document_id = excluded.source_document_id, source_path = excluded.source_path, source_start_byte = excluded.source_start_byte, source_end_byte = excluded.source_end_byte, content_revision = excluded.content_revision, metadata_json = excluded.metadata_json",
        )
        .bind(&edge.id)
        .bind(&edge.workspace_id)
        .bind(&edge.relationship_key)
        .bind(&edge.relationship_fact_id)
        .bind(&edge.from_node)
        .bind(&edge.to_node)
        .bind(edge.edge_type.storage_name())
        .bind(edge.confidence)
        .bind(&edge.analyzer_id)
        .bind(&edge.analyzer_version)
        .bind(&edge.structure_version)
        .bind(&edge.source_document_id)
        .bind(source_path)
        .bind(source_start_byte)
        .bind(source_end_byte)
        .bind(edge.content_revision)
        .bind(serde_json::to_string(&edge.metadata)?)
        .bind(edge.created_at)
        .execute(self.pool())
        .await?;
        self.graph_edges(&edge.workspace_id)
            .await?
            .into_iter()
            .find(|stored| stored.relationship_key == edge.relationship_key)
            .ok_or_else(|| CortexError::NotFound(format!("graph edge {}", edge.relationship_key)))
    }

    pub async fn graph_relationship_facts(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<GraphRelationshipFact>> {
        let rows = sqlx::query_as::<_, UnresolvedRelationshipRow>(
            "SELECT id, workspace_id, source_document_id, relationship_key, from_node, from_stable_key, edge_type, target_kind, target_value, confidence, analyzer_id, analyzer_version, structure_version, source_path, source_start_byte, source_end_byte, content_revision, metadata_json, created_at, updated_at FROM graph_relationship_facts WHERE workspace_id = ? ORDER BY source_document_id, relationship_key",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn upsert_graph_relationship_fact(
        &self,
        fact: &GraphRelationshipFact,
    ) -> Result<GraphRelationshipFact> {
        let (source_path, source_start_byte, source_end_byte) =
            graph_source_segment_bindings(&fact.source_segment)?;
        sqlx::query(
            "INSERT INTO graph_relationship_facts(id, workspace_id, source_document_id, relationship_key, from_node, from_stable_key, edge_type, target_kind, target_value, confidence, analyzer_id, analyzer_version, structure_version, source_path, source_start_byte, source_end_byte, content_revision, metadata_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(workspace_id, source_document_id, relationship_key) DO UPDATE SET from_node = excluded.from_node, from_stable_key = excluded.from_stable_key, edge_type = excluded.edge_type, target_kind = excluded.target_kind, target_value = excluded.target_value, confidence = excluded.confidence, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, structure_version = excluded.structure_version, source_path = excluded.source_path, source_start_byte = excluded.source_start_byte, source_end_byte = excluded.source_end_byte, content_revision = excluded.content_revision, metadata_json = excluded.metadata_json, updated_at = excluded.updated_at",
        )
        .bind(&fact.id)
        .bind(&fact.workspace_id)
        .bind(&fact.source_document_id)
        .bind(&fact.relationship_key)
        .bind(&fact.from_node)
        .bind(&fact.from_stable_key)
        .bind(fact.edge_type.storage_name())
        .bind(fact.target_kind.storage_name())
        .bind(&fact.target_value)
        .bind(fact.confidence)
        .bind(&fact.analyzer_id)
        .bind(&fact.analyzer_version)
        .bind(&fact.structure_version)
        .bind(source_path)
        .bind(source_start_byte)
        .bind(source_end_byte)
        .bind(fact.content_revision)
        .bind(serde_json::to_string(&fact.metadata)?)
        .bind(fact.created_at)
        .bind(fact.updated_at)
        .execute(self.pool())
        .await?;
        self.graph_relationship_facts(&fact.workspace_id)
            .await?
            .into_iter()
            .find(|stored| {
                stored.source_document_id == fact.source_document_id
                    && stored.relationship_key == fact.relationship_key
            })
            .ok_or_else(|| {
                CortexError::NotFound(format!("graph relationship fact {}", fact.relationship_key))
            })
    }

    pub async fn unresolved_relationships(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<UnresolvedRelationship>> {
        let rows = sqlx::query_as::<_, UnresolvedRelationshipRow>(
            "SELECT id, workspace_id, source_document_id, relationship_key, from_node, from_stable_key, edge_type, target_kind, target_value, confidence, analyzer_id, analyzer_version, structure_version, source_path, source_start_byte, source_end_byte, content_revision, metadata_json, created_at, updated_at FROM unresolved_relationships WHERE workspace_id = ? ORDER BY source_document_id, relationship_key",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn upsert_unresolved_relationship(
        &self,
        relationship: &UnresolvedRelationship,
    ) -> Result<UnresolvedRelationship> {
        let (source_path, source_start_byte, source_end_byte) =
            graph_source_segment_bindings(&relationship.source_segment)?;
        sqlx::query(
            "INSERT INTO unresolved_relationships(id, workspace_id, source_document_id, relationship_key, from_node, from_stable_key, edge_type, target_kind, target_value, confidence, analyzer_id, analyzer_version, structure_version, source_path, source_start_byte, source_end_byte, content_revision, metadata_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(workspace_id, source_document_id, relationship_key) DO UPDATE SET from_node = excluded.from_node, from_stable_key = excluded.from_stable_key, edge_type = excluded.edge_type, target_kind = excluded.target_kind, target_value = excluded.target_value, confidence = excluded.confidence, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, structure_version = excluded.structure_version, source_path = excluded.source_path, source_start_byte = excluded.source_start_byte, source_end_byte = excluded.source_end_byte, content_revision = excluded.content_revision, metadata_json = excluded.metadata_json, updated_at = excluded.updated_at",
        )
        .bind(&relationship.id)
        .bind(&relationship.workspace_id)
        .bind(&relationship.source_document_id)
        .bind(&relationship.relationship_key)
        .bind(&relationship.from_node)
        .bind(&relationship.from_stable_key)
        .bind(relationship.edge_type.storage_name())
        .bind(relationship.target_kind.storage_name())
        .bind(&relationship.target_value)
        .bind(relationship.confidence)
        .bind(&relationship.analyzer_id)
        .bind(&relationship.analyzer_version)
        .bind(&relationship.structure_version)
        .bind(source_path)
        .bind(source_start_byte)
        .bind(source_end_byte)
        .bind(relationship.content_revision)
        .bind(serde_json::to_string(&relationship.metadata)?)
        .bind(relationship.created_at)
        .bind(relationship.updated_at)
        .execute(self.pool())
        .await?;
        self.unresolved_relationships(&relationship.workspace_id)
            .await?
            .into_iter()
            .find(|stored| {
                stored.source_document_id == relationship.source_document_id
                    && stored.relationship_key == relationship.relationship_key
            })
            .ok_or_else(|| {
                CortexError::NotFound(format!(
                    "unresolved relationship {}",
                    relationship.relationship_key
                ))
            })
    }

    pub async fn insert_unresolved_relationship_candidate(
        &self,
        relationship_id: &str,
        workspace_id: &str,
        candidate_node_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO unresolved_relationship_candidates(unresolved_relationship_id, workspace_id, candidate_node_id, created_at) VALUES (?, ?, ?, ?) ON CONFLICT(unresolved_relationship_id, candidate_node_id) DO NOTHING",
        )
        .bind(relationship_id)
        .bind(workspace_id)
        .bind(candidate_node_id)
        .bind(created_at)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub(crate) async fn apply_graph_reconciliation(
        &self,
        batch: &GraphReconciliationBatch,
        updated_at: DateTime<Utc>,
    ) -> Result<GraphReconciliationStatus> {
        validate_graph_reconciliation_batch(batch)?;
        let mut transaction = self.pool().begin().await?;
        let (current_revision, graph_state, graph_update_started_at, graph_updated_at): (
            i64,
            String,
            Option<DateTime<Utc>>,
            DateTime<Utc>,
        ) = sqlx::query_as(
            "SELECT content_revision, graph_state, graph_update_started_at, updated_at FROM workspace_graph_revisions WHERE workspace_id = ?",
        )
        .bind(&batch.workspace_id)
        .fetch_one(&mut *transaction)
        .await?;
        let expected_revision = if batch.delete_relative_path.is_some() {
            batch
                .target_content_revision
                .checked_sub(1)
                .ok_or_else(|| {
                    CortexError::Analysis("graph deletion target revision must be positive".into())
                })?
        } else {
            batch.target_content_revision
        };
        if current_revision != expected_revision {
            transaction.rollback().await?;
            return Ok(GraphReconciliationStatus::Superseded);
        }
        let owns_snapshot = match (
            batch.delete_relative_path.as_ref(),
            batch.expected_graph_updated_at,
            batch.update_started_at,
        ) {
            (Some(_), Some(expected), None) => graph_updated_at == expected,
            (None, None, Some(started_at)) => {
                graph_state == "updating" && graph_update_started_at == Some(started_at)
            }
            _ => false,
        };
        if !owns_snapshot {
            transaction.rollback().await?;
            return Ok(GraphReconciliationStatus::Superseded);
        }
        if let Some(generation_id) = &batch.repair_generation_id {
            let repair_owned: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM workspace_graph_repairs WHERE workspace_id = ? AND generation_id = ? AND target_content_revision = ? AND state = 'active' AND lease_expires_at > ?",
            )
            .bind(&batch.workspace_id)
            .bind(generation_id)
            .bind(batch.target_content_revision)
            .bind(updated_at)
            .fetch_one(&mut *transaction)
            .await?;
            if repair_owned != 1 {
                transaction.rollback().await?;
                return Ok(GraphReconciliationStatus::Superseded);
            }
        } else {
            let blocking_repair: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM workspace_graph_repairs WHERE workspace_id = ? AND state IN ('active', 'failed', 'interrupted')",
            )
            .bind(&batch.workspace_id)
            .fetch_one(&mut *transaction)
            .await?;
            if blocking_repair != 0 {
                transaction.rollback().await?;
                return Ok(GraphReconciliationStatus::Superseded);
            }
        }

        if let Some(state) = &batch.analysis_state {
            let source_is_current: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM documents WHERE id = ? AND workspace_id = ? AND content_revision = ? AND analyzer_id = ? AND analyzer_version = ?",
            )
            .bind(&state.document_id)
            .bind(&state.workspace_id)
            .bind(state.content_revision)
            .bind(&state.analyzer_id)
            .bind(&state.analyzer_version)
            .fetch_one(&mut *transaction)
            .await?;
            if source_is_current != 1 {
                transaction.rollback().await?;
                return Ok(GraphReconciliationStatus::Superseded);
            }
        }

        if let Some(relative_path) = &batch.delete_relative_path {
            let deleted =
                sqlx::query("DELETE FROM documents WHERE workspace_id = ? AND relative_path = ?")
                    .bind(&batch.workspace_id)
                    .bind(relative_path)
                    .execute(&mut *transaction)
                    .await?;
            if deleted.rows_affected() != 1 {
                transaction.rollback().await?;
                return Ok(GraphReconciliationStatus::Superseded);
            }
            let revision =
                bump_content_revision(&mut transaction, &batch.workspace_id, updated_at).await?;
            if revision != batch.target_content_revision {
                return Err(CortexError::Analysis(format!(
                    "graph deletion produced content revision {revision}, expected {}",
                    batch.target_content_revision
                )));
            }
        }

        if let Some(document_id) = &batch.source_document_id {
            let previous_node_keys: Vec<String> = sqlx::query_scalar(
                "SELECT stable_key FROM graph_nodes WHERE workspace_id = ? AND document_id = ?",
            )
            .bind(&batch.workspace_id)
            .bind(document_id)
            .fetch_all(&mut *transaction)
            .await?;
            let next_node_keys: HashSet<_> = batch
                .nodes
                .iter()
                .map(|node| node.stable_key.as_str())
                .collect();

            for node in &batch.nodes {
                let conflicting_owner: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM graph_nodes WHERE workspace_id = ? AND stable_key = ? AND document_id IS NOT ?",
                )
                .bind(&batch.workspace_id)
                .bind(&node.stable_key)
                .bind(document_id)
                .fetch_one(&mut *transaction)
                .await?;
                if conflicting_owner != 0 {
                    return Err(CortexError::Analysis(format!(
                        "graph node stable key {} is already owned by another document",
                        node.stable_key
                    )));
                }
                let (source_path, source_start_byte, source_end_byte) =
                    graph_source_segment_bindings(&node.source_segment)?;
                sqlx::query(
                    "INSERT INTO graph_nodes(id, workspace_id, node_type, stable_key, language, name, qualified_name, document_id, chunk_id, source_path, source_start_byte, source_end_byte, analyzer_id, analyzer_version, structure_version, content_revision, metadata_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(workspace_id, stable_key) DO UPDATE SET node_type = excluded.node_type, language = excluded.language, name = excluded.name, qualified_name = excluded.qualified_name, document_id = excluded.document_id, chunk_id = excluded.chunk_id, source_path = excluded.source_path, source_start_byte = excluded.source_start_byte, source_end_byte = excluded.source_end_byte, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, structure_version = excluded.structure_version, content_revision = excluded.content_revision, metadata_json = excluded.metadata_json, updated_at = excluded.updated_at",
                )
                .bind(&node.id)
                .bind(&node.workspace_id)
                .bind(node.node_type.storage_name())
                .bind(&node.stable_key)
                .bind(&node.language)
                .bind(&node.name)
                .bind(&node.qualified_name)
                .bind(&node.document_id)
                .bind(&node.chunk_id)
                .bind(source_path)
                .bind(source_start_byte)
                .bind(source_end_byte)
                .bind(&node.analyzer_id)
                .bind(&node.analyzer_version)
                .bind(&node.structure_version)
                .bind(node.content_revision)
                .bind(serde_json::to_string(&node.metadata)?)
                .bind(node.created_at)
                .bind(node.updated_at)
                .execute(&mut *transaction)
                .await?;
            }

            let previous_fact_keys: Vec<String> = sqlx::query_scalar(
                "SELECT relationship_key FROM graph_relationship_facts WHERE workspace_id = ? AND source_document_id = ?",
            )
            .bind(&batch.workspace_id)
            .bind(document_id)
            .fetch_all(&mut *transaction)
            .await?;
            let next_fact_keys: HashSet<_> = batch
                .facts
                .iter()
                .map(|fact| fact.relationship_key.as_str())
                .collect();
            for fact in &batch.facts {
                let (source_path, source_start_byte, source_end_byte) =
                    graph_source_segment_bindings(&fact.source_segment)?;
                sqlx::query(
                    "INSERT INTO graph_relationship_facts(id, workspace_id, source_document_id, relationship_key, from_node, from_stable_key, edge_type, target_kind, target_value, confidence, analyzer_id, analyzer_version, structure_version, source_path, source_start_byte, source_end_byte, content_revision, metadata_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(workspace_id, source_document_id, relationship_key) DO UPDATE SET from_node = excluded.from_node, from_stable_key = excluded.from_stable_key, edge_type = excluded.edge_type, target_kind = excluded.target_kind, target_value = excluded.target_value, confidence = excluded.confidence, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, structure_version = excluded.structure_version, source_path = excluded.source_path, source_start_byte = excluded.source_start_byte, source_end_byte = excluded.source_end_byte, content_revision = excluded.content_revision, metadata_json = excluded.metadata_json, updated_at = excluded.updated_at",
                )
                .bind(&fact.id)
                .bind(&fact.workspace_id)
                .bind(&fact.source_document_id)
                .bind(&fact.relationship_key)
                .bind(&fact.from_node)
                .bind(&fact.from_stable_key)
                .bind(fact.edge_type.storage_name())
                .bind(fact.target_kind.storage_name())
                .bind(&fact.target_value)
                .bind(fact.confidence)
                .bind(&fact.analyzer_id)
                .bind(&fact.analyzer_version)
                .bind(&fact.structure_version)
                .bind(source_path)
                .bind(source_start_byte)
                .bind(source_end_byte)
                .bind(fact.content_revision)
                .bind(serde_json::to_string(&fact.metadata)?)
                .bind(fact.created_at)
                .bind(fact.updated_at)
                .execute(&mut *transaction)
                .await?;
            }
            for previous_key in previous_fact_keys {
                if !next_fact_keys.contains(previous_key.as_str()) {
                    sqlx::query(
                        "DELETE FROM graph_relationship_facts WHERE workspace_id = ? AND source_document_id = ? AND relationship_key = ?",
                    )
                    .bind(&batch.workspace_id)
                    .bind(document_id)
                    .bind(previous_key)
                    .execute(&mut *transaction)
                    .await?;
                }
            }
            for previous_key in previous_node_keys {
                if !next_node_keys.contains(previous_key.as_str()) {
                    sqlx::query(
                        "DELETE FROM graph_nodes WHERE workspace_id = ? AND document_id = ? AND stable_key = ?",
                    )
                    .bind(&batch.workspace_id)
                    .bind(document_id)
                    .bind(previous_key)
                    .execute(&mut *transaction)
                    .await?;
                }
            }
        }

        for identity in &batch.affected_relationships {
            sqlx::query(
                "DELETE FROM graph_edges WHERE workspace_id = ? AND source_document_id = ? AND relationship_key = ?",
            )
            .bind(&batch.workspace_id)
            .bind(&identity.source_document_id)
            .bind(&identity.relationship_key)
            .execute(&mut *transaction)
            .await?;
            sqlx::query(
                "DELETE FROM unresolved_relationships WHERE workspace_id = ? AND source_document_id = ? AND relationship_key = ?",
            )
            .bind(&batch.workspace_id)
            .bind(&identity.source_document_id)
            .bind(&identity.relationship_key)
            .execute(&mut *transaction)
            .await?;
        }

        for edge in &batch.edges {
            let (source_path, source_start_byte, source_end_byte) =
                graph_source_segment_bindings(&edge.source_segment)?;
            sqlx::query(
                "INSERT INTO graph_edges(id, workspace_id, relationship_key, relationship_fact_id, from_node, to_node, edge_type, confidence, analyzer_id, analyzer_version, structure_version, source_document_id, source_path, source_start_byte, source_end_byte, content_revision, metadata_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&edge.id)
            .bind(&edge.workspace_id)
            .bind(&edge.relationship_key)
            .bind(&edge.relationship_fact_id)
            .bind(&edge.from_node)
            .bind(&edge.to_node)
            .bind(edge.edge_type.storage_name())
            .bind(edge.confidence)
            .bind(&edge.analyzer_id)
            .bind(&edge.analyzer_version)
            .bind(&edge.structure_version)
            .bind(&edge.source_document_id)
            .bind(source_path)
            .bind(source_start_byte)
            .bind(source_end_byte)
            .bind(edge.content_revision)
            .bind(serde_json::to_string(&edge.metadata)?)
            .bind(edge.created_at)
            .execute(&mut *transaction)
            .await?;
        }
        for projection in &batch.unresolved {
            let relationship = &projection.relationship;
            let (source_path, source_start_byte, source_end_byte) =
                graph_source_segment_bindings(&relationship.source_segment)?;
            sqlx::query(
                "INSERT INTO unresolved_relationships(id, workspace_id, source_document_id, relationship_key, from_node, from_stable_key, edge_type, target_kind, target_value, confidence, analyzer_id, analyzer_version, structure_version, source_path, source_start_byte, source_end_byte, content_revision, metadata_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&relationship.id)
            .bind(&relationship.workspace_id)
            .bind(&relationship.source_document_id)
            .bind(&relationship.relationship_key)
            .bind(&relationship.from_node)
            .bind(&relationship.from_stable_key)
            .bind(relationship.edge_type.storage_name())
            .bind(relationship.target_kind.storage_name())
            .bind(&relationship.target_value)
            .bind(relationship.confidence)
            .bind(&relationship.analyzer_id)
            .bind(&relationship.analyzer_version)
            .bind(&relationship.structure_version)
            .bind(source_path)
            .bind(source_start_byte)
            .bind(source_end_byte)
            .bind(relationship.content_revision)
            .bind(serde_json::to_string(&relationship.metadata)?)
            .bind(relationship.created_at)
            .bind(relationship.updated_at)
            .execute(&mut *transaction)
            .await?;
            for candidate_node_id in &projection.candidate_node_ids {
                sqlx::query(
                    "INSERT INTO unresolved_relationship_candidates(unresolved_relationship_id, workspace_id, candidate_node_id, created_at) VALUES (?, ?, ?, ?)",
                )
                .bind(&relationship.id)
                .bind(&batch.workspace_id)
                .bind(candidate_node_id)
                .bind(updated_at)
                .execute(&mut *transaction)
                .await?;
            }
        }

        if let Some(state) = &batch.analysis_state {
            sqlx::query(
                "INSERT INTO graph_document_states(document_id, workspace_id, content_revision, analyzer_id, analyzer_version, structure_version, last_error, analyzed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(document_id) DO UPDATE SET workspace_id = excluded.workspace_id, content_revision = excluded.content_revision, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, structure_version = excluded.structure_version, last_error = excluded.last_error, analyzed_at = excluded.analyzed_at",
            )
            .bind(&state.document_id)
            .bind(&state.workspace_id)
            .bind(state.content_revision)
            .bind(&state.analyzer_id)
            .bind(&state.analyzer_version)
            .bind(&state.structure_version)
            .bind(&state.last_error)
            .bind(state.analyzed_at)
            .execute(&mut *transaction)
            .await?;
        }

        sqlx::query(
            "INSERT INTO graph_document_projections(document_id, workspace_id, content_revision, analyzer_id, analyzer_version, structure_version, node_count, fact_count, edge_count, unresolved_count, projected_at) SELECT s.document_id, s.workspace_id, s.content_revision, s.analyzer_id, s.analyzer_version, s.structure_version, (SELECT COUNT(*) FROM graph_nodes nodes WHERE nodes.workspace_id = s.workspace_id AND nodes.document_id = s.document_id), (SELECT COUNT(*) FROM graph_relationship_facts facts WHERE facts.workspace_id = s.workspace_id AND facts.source_document_id = s.document_id), (SELECT COUNT(*) FROM graph_edges edges WHERE edges.workspace_id = s.workspace_id AND edges.source_document_id = s.document_id), (SELECT COUNT(*) FROM unresolved_relationships unresolved WHERE unresolved.workspace_id = s.workspace_id AND unresolved.source_document_id = s.document_id), s.analyzed_at FROM graph_document_states s WHERE s.workspace_id = ? ON CONFLICT(document_id) DO UPDATE SET workspace_id = excluded.workspace_id, content_revision = excluded.content_revision, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, structure_version = excluded.structure_version, node_count = excluded.node_count, fact_count = excluded.fact_count, edge_count = excluded.edge_count, unresolved_count = excluded.unresolved_count, projected_at = excluded.projected_at",
        )
        .bind(&batch.workspace_id)
        .execute(&mut *transaction)
        .await?;

        let analysis_rows =
            graph_analysis_snapshot_rows(&mut transaction, &batch.workspace_id).await?;
        let expectations_match =
            analysis_expectations_match(&analysis_rows, &batch.expected_analysis);
        let status = if expectations_match && batch.repair_generation_id.is_none() {
            let updated = sqlx::query(
                "UPDATE workspace_graph_revisions SET graph_content_revision = ?, graph_state = 'current', graph_update_started_at = NULL, failed_graph_target_revision = NULL, last_graph_error = NULL, updated_at = ? WHERE workspace_id = ? AND content_revision = ? AND graph_content_revision <= ?",
            )
            .bind(batch.target_content_revision)
            .bind(updated_at)
            .bind(&batch.workspace_id)
            .bind(batch.target_content_revision)
            .bind(batch.target_content_revision)
            .execute(&mut *transaction)
            .await?;
            if updated.rows_affected() != 1 {
                transaction.rollback().await?;
                return Ok(GraphReconciliationStatus::Superseded);
            }
            GraphReconciliationStatus::Current
        } else {
            sqlx::query(
                "UPDATE workspace_graph_revisions SET graph_state = 'stale', graph_update_started_at = NULL, updated_at = ? WHERE workspace_id = ? AND content_revision = ?",
            )
            .bind(updated_at)
            .bind(&batch.workspace_id)
            .bind(batch.target_content_revision)
            .execute(&mut *transaction)
            .await?;
            GraphReconciliationStatus::Stale
        };
        transaction.commit().await?;
        Ok(status)
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
        content_index_changed: bool,
    ) -> Result<i64> {
        let mut transaction = self.pool().begin().await?;
        let content_revision = if content_index_changed {
            bump_content_revision(
                &mut transaction,
                &document.workspace_id,
                document.indexed_at,
            )
            .await?
        } else {
            document.content_revision
        };
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
            "INSERT INTO documents(id, workspace_id, relative_path, language, analyzer_id, analyzer_version, segmentation_id, content_revision, content_hash, size_bytes, modified_at_ns, indexed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(workspace_id, relative_path) DO UPDATE SET language = excluded.language, analyzer_id = excluded.analyzer_id, analyzer_version = excluded.analyzer_version, segmentation_id = excluded.segmentation_id, content_revision = excluded.content_revision, content_hash = excluded.content_hash, size_bytes = excluded.size_bytes, modified_at_ns = excluded.modified_at_ns, indexed_at = excluded.indexed_at",
        )
        .bind(&document.id)
        .bind(&document.workspace_id)
        .bind(&document.relative_path)
        .bind(&document.language)
        .bind(&document.analyzer_id)
        .bind(&document.analyzer_version)
        .bind(&document.segmentation_id)
        .bind(content_revision)
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
        Ok(content_revision)
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

fn validate_graph_reconciliation_batch(batch: &GraphReconciliationBatch) -> Result<()> {
    if batch.workspace_id.trim().is_empty() || batch.target_content_revision < 0 {
        return Err(CortexError::Analysis(
            "graph reconciliation requires a workspace and non-negative target revision".into(),
        ));
    }
    if batch.delete_relative_path.is_some() && batch.source_document_id.is_some() {
        return Err(CortexError::Analysis(
            "graph reconciliation cannot update and delete a source document together".into(),
        ));
    }
    match (
        batch.delete_relative_path.as_ref(),
        batch.expected_graph_updated_at,
        batch.update_started_at,
    ) {
        (Some(_), Some(_), None) | (None, None, Some(_)) => {}
        _ => {
            return Err(CortexError::Analysis(
                "graph reconciliation has an invalid snapshot ownership token".into(),
            ));
        }
    }
    let mut node_keys = HashSet::new();
    for node in &batch.nodes {
        if node.workspace_id != batch.workspace_id
            || node.document_id.as_deref() != batch.source_document_id.as_deref()
            || !node_keys.insert(node.stable_key.as_str())
        {
            return Err(CortexError::Analysis(
                "graph reconciliation contains invalid or duplicate source nodes".into(),
            ));
        }
    }
    let mut fact_keys = HashSet::new();
    for fact in &batch.facts {
        if fact.workspace_id != batch.workspace_id
            || Some(fact.source_document_id.as_str()) != batch.source_document_id.as_deref()
            || !fact_keys.insert(fact.relationship_key.as_str())
        {
            return Err(CortexError::Analysis(
                "graph reconciliation contains invalid or duplicate source facts".into(),
            ));
        }
    }
    if let Some(state) = &batch.analysis_state
        && (state.workspace_id != batch.workspace_id
            || Some(state.document_id.as_str()) != batch.source_document_id.as_deref())
    {
        return Err(CortexError::Analysis(
            "graph analysis state does not match its reconciliation target".into(),
        ));
    }
    let mut expected_documents = HashSet::new();
    for expectation in &batch.expected_analysis {
        if expectation.document_id.trim().is_empty()
            || expectation.analyzer_id.trim().is_empty()
            || expectation.analyzer_version.trim().is_empty()
            || expectation.structure_version.trim().is_empty()
            || !expected_documents.insert(expectation.document_id.as_str())
        {
            return Err(CortexError::Analysis(
                "graph reconciliation contains invalid analysis expectations".into(),
            ));
        }
    }

    let mut affected = HashSet::new();
    for identity in &batch.affected_relationships {
        if !affected.insert((
            identity.source_document_id.as_str(),
            identity.relationship_key.as_str(),
        )) {
            return Err(CortexError::Analysis(
                "graph reconciliation contains duplicate affected relationships".into(),
            ));
        }
    }
    let mut projected = HashSet::new();
    let mut edge_ids = HashSet::new();
    for edge in &batch.edges {
        let Some(source_document_id) = edge.source_document_id.as_deref() else {
            return Err(CortexError::Analysis(
                "resolved graph projections require source ownership".into(),
            ));
        };
        let identity = (source_document_id, edge.relationship_key.as_str());
        if edge.workspace_id != batch.workspace_id
            || !affected.contains(&identity)
            || !projected.insert(identity)
            || !edge_ids.insert(edge.id.as_str())
        {
            return Err(CortexError::Analysis(
                "resolved graph projections do not match the affected relationship set".into(),
            ));
        }
    }
    let mut unresolved_ids = HashSet::new();
    for projection in &batch.unresolved {
        let relationship = &projection.relationship;
        let identity = (
            relationship.source_document_id.as_str(),
            relationship.relationship_key.as_str(),
        );
        let mut candidate_ids = HashSet::new();
        if relationship.workspace_id != batch.workspace_id
            || !affected.contains(&identity)
            || !projected.insert(identity)
            || !unresolved_ids.insert(relationship.id.as_str())
            || projection
                .candidate_node_ids
                .iter()
                .any(|id| !candidate_ids.insert(id.as_str()))
        {
            return Err(CortexError::Analysis(
                "unresolved graph projections do not match the affected relationship set".into(),
            ));
        }
    }
    Ok(())
}

async fn graph_analysis_snapshot_rows(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
) -> Result<Vec<GraphAnalysisSnapshotRow>> {
    Ok(sqlx::query_as::<_, GraphAnalysisSnapshotRow>(
        "SELECT d.id AS document_id, d.analyzer_id AS document_analyzer_id, d.analyzer_version AS document_analyzer_version, d.content_revision AS document_content_revision, s.content_revision AS state_content_revision, s.analyzer_id AS state_analyzer_id, s.analyzer_version AS state_analyzer_version, s.structure_version AS state_structure_version, s.last_error AS state_last_error, p.content_revision AS manifest_content_revision, p.analyzer_id AS manifest_analyzer_id, p.analyzer_version AS manifest_analyzer_version, p.structure_version AS manifest_structure_version, p.node_count AS manifest_node_count, p.fact_count AS manifest_fact_count, p.edge_count AS manifest_edge_count, p.unresolved_count AS manifest_unresolved_count, (SELECT COUNT(*) FROM graph_nodes nodes WHERE nodes.workspace_id = d.workspace_id AND nodes.document_id = d.id) AS physical_node_count, (SELECT COUNT(*) FROM graph_relationship_facts facts WHERE facts.workspace_id = d.workspace_id AND facts.source_document_id = d.id) AS physical_fact_count, (SELECT COUNT(*) FROM graph_edges edges WHERE edges.workspace_id = d.workspace_id AND edges.source_document_id = d.id) AS physical_edge_count, (SELECT COUNT(*) FROM unresolved_relationships unresolved WHERE unresolved.workspace_id = d.workspace_id AND unresolved.source_document_id = d.id) AS physical_unresolved_count FROM documents d LEFT JOIN graph_document_states s ON s.document_id = d.id AND s.workspace_id = d.workspace_id LEFT JOIN graph_document_projections p ON p.document_id = d.id AND p.workspace_id = d.workspace_id WHERE d.workspace_id = ?",
    )
    .bind(workspace_id)
    .fetch_all(&mut **transaction)
    .await?)
}

fn analysis_expectations_match(
    rows: &[GraphAnalysisSnapshotRow],
    expected_analysis: &[GraphAnalysisExpectation],
) -> bool {
    let analysis_by_document: HashMap<_, _> = rows
        .iter()
        .map(|row| (row.document_id.as_str(), row))
        .collect();
    analysis_by_document.len() == expected_analysis.len()
        && expected_analysis.iter().all(|expectation| {
            analysis_by_document
                .get(expectation.document_id.as_str())
                .is_some_and(|row| {
                    row.document_analyzer_id == expectation.analyzer_id
                        && row.document_analyzer_version == expectation.analyzer_version
                        && row.state_content_revision == Some(row.document_content_revision)
                        && row.state_analyzer_id.as_deref()
                            == Some(expectation.analyzer_id.as_str())
                        && row.state_analyzer_version.as_deref()
                            == Some(expectation.analyzer_version.as_str())
                        && row.state_structure_version.as_deref()
                            == Some(expectation.structure_version.as_str())
                        && row.state_last_error.is_none()
                        && row.manifest_content_revision == Some(row.document_content_revision)
                        && row.manifest_analyzer_id.as_deref()
                            == Some(expectation.analyzer_id.as_str())
                        && row.manifest_analyzer_version.as_deref()
                            == Some(expectation.analyzer_version.as_str())
                        && row.manifest_structure_version.as_deref()
                            == Some(expectation.structure_version.as_str())
                        && row.manifest_node_count == Some(row.physical_node_count)
                        && row.manifest_fact_count == Some(row.physical_fact_count)
                        && row.manifest_edge_count == Some(row.physical_edge_count)
                        && row.manifest_unresolved_count == Some(row.physical_unresolved_count)
                })
        })
}

fn graph_source_segment_bindings(
    segment: &Option<SourceSegment>,
) -> Result<(Option<String>, Option<i64>, Option<i64>)> {
    let Some(segment) = segment else {
        return Ok((None, None, None));
    };
    let start_byte = i64::try_from(segment.start_byte).map_err(|_| {
        CortexError::Analysis("graph source segment start exceeds SQLite range".into())
    })?;
    let end_byte = i64::try_from(segment.end_byte).map_err(|_| {
        CortexError::Analysis("graph source segment end exceeds SQLite range".into())
    })?;
    if end_byte < start_byte {
        return Err(CortexError::Analysis(
            "graph source segment ends before it starts".into(),
        ));
    }
    Ok((
        Some(segment.source.clone()),
        Some(start_byte),
        Some(end_byte),
    ))
}

fn graph_source_segment_from_columns(
    source_path: Option<String>,
    source_start_byte: Option<i64>,
    source_end_byte: Option<i64>,
) -> Result<Option<SourceSegment>> {
    match (source_path, source_start_byte, source_end_byte) {
        (None, None, None) => Ok(None),
        (Some(source), Some(start_byte), Some(end_byte)) => {
            let start_byte = u64::try_from(start_byte)
                .map_err(|_| CortexError::Analysis("negative graph source segment start".into()))?;
            let end_byte = u64::try_from(end_byte)
                .map_err(|_| CortexError::Analysis("negative graph source segment end".into()))?;
            if end_byte < start_byte {
                return Err(CortexError::Analysis(
                    "graph source segment ends before it starts".into(),
                ));
            }
            Ok(Some(SourceSegment::new(source, start_byte, end_byte)))
        }
        _ => Err(CortexError::Analysis(
            "incomplete graph source segment columns".into(),
        )),
    }
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

async fn bump_content_revision(
    transaction: &mut sqlx::Transaction<'_, Sqlite>,
    workspace_id: &str,
    updated_at: DateTime<Utc>,
) -> Result<i64> {
    let revision = sqlx::query_scalar::<_, i64>(
        "UPDATE workspace_graph_revisions SET content_revision = content_revision + 1, graph_state = 'stale', graph_update_started_at = NULL, failed_graph_target_revision = NULL, last_graph_error = NULL, updated_at = ? WHERE workspace_id = ? RETURNING content_revision",
    )
    .bind(updated_at)
    .bind(workspace_id)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(revision)
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
    content_revision: i64,
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
            content_revision: row.content_revision,
            content_hash: row.content_hash,
            size_bytes: row.size_bytes,
            modified_at_ns: row.modified_at_ns,
            indexed_at: row.indexed_at,
        }
    }
}

#[derive(FromRow)]
struct WorkspaceGraphRevisionRow {
    workspace_id: String,
    content_revision: i64,
    graph_content_revision: i64,
    graph_schema_version: i64,
    graph_state: String,
    graph_update_started_at: Option<DateTime<Utc>>,
    failed_graph_target_revision: Option<i64>,
    last_graph_error: Option<String>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<WorkspaceGraphRevisionRow> for WorkspaceGraphRevision {
    type Error = CortexError;

    fn try_from(row: WorkspaceGraphRevisionRow) -> Result<Self> {
        if row.content_revision < 0
            || row.graph_content_revision < 0
            || row.graph_content_revision > row.content_revision
            || row.graph_schema_version <= 0
        {
            return Err(CortexError::Analysis(
                "invalid persisted workspace graph revision".into(),
            ));
        }
        Ok(Self {
            workspace_id: row.workspace_id,
            content_revision: row.content_revision,
            graph_content_revision: row.graph_content_revision,
            graph_schema_version: row.graph_schema_version,
            graph_state: GraphState::from_storage(&row.graph_state),
            graph_update_started_at: row.graph_update_started_at,
            failed_graph_target_revision: row.failed_graph_target_revision,
            last_graph_error: row.last_graph_error,
            updated_at: row.updated_at,
        })
    }
}

#[derive(FromRow)]
struct GraphAnalysisStateRow {
    document_id: String,
    workspace_id: String,
    content_revision: i64,
    analyzer_id: String,
    analyzer_version: String,
    structure_version: String,
    last_error: Option<String>,
    analyzed_at: DateTime<Utc>,
}

impl From<GraphAnalysisStateRow> for GraphAnalysisState {
    fn from(row: GraphAnalysisStateRow) -> Self {
        Self {
            document_id: row.document_id,
            workspace_id: row.workspace_id,
            content_revision: row.content_revision,
            analyzer_id: row.analyzer_id,
            analyzer_version: row.analyzer_version,
            structure_version: row.structure_version,
            last_error: row.last_error,
            analyzed_at: row.analyzed_at,
        }
    }
}

#[derive(FromRow)]
struct GraphRepairGenerationRow {
    workspace_id: String,
    generation_id: String,
    mode: String,
    target_content_revision: i64,
    state: String,
    started_at: DateTime<Utc>,
    lease_expires_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    documents_considered: i64,
    documents_repaired: i64,
    documents_failed: i64,
    last_error: Option<String>,
}

impl TryFrom<GraphRepairGenerationRow> for GraphRepairGeneration {
    type Error = CortexError;

    fn try_from(row: GraphRepairGenerationRow) -> Result<Self> {
        Ok(Self {
            workspace_id: row.workspace_id,
            generation_id: row.generation_id,
            mode: GraphRepairMode::from_storage(&row.mode),
            target_content_revision: row.target_content_revision,
            state: GraphRepairState::from_storage(&row.state),
            started_at: row.started_at,
            lease_expires_at: row.lease_expires_at,
            updated_at: row.updated_at,
            completed_at: row.completed_at,
            documents_considered: usize::try_from(row.documents_considered).map_err(|_| {
                CortexError::Analysis("invalid persisted graph repair document count".into())
            })?,
            documents_repaired: usize::try_from(row.documents_repaired).map_err(|_| {
                CortexError::Analysis("invalid persisted graph repair document count".into())
            })?,
            documents_failed: usize::try_from(row.documents_failed).map_err(|_| {
                CortexError::Analysis("invalid persisted graph repair document count".into())
            })?,
            last_error: row.last_error,
        })
    }
}

#[derive(FromRow)]
struct GraphProjectionManifestRow {
    document_id: String,
    workspace_id: String,
    content_revision: i64,
    analyzer_id: String,
    analyzer_version: String,
    structure_version: String,
    node_count: i64,
    fact_count: i64,
    edge_count: i64,
    unresolved_count: i64,
    projected_at: DateTime<Utc>,
}

impl TryFrom<GraphProjectionManifestRow> for GraphProjectionManifest {
    type Error = CortexError;

    fn try_from(row: GraphProjectionManifestRow) -> Result<Self> {
        Ok(Self {
            document_id: row.document_id,
            workspace_id: row.workspace_id,
            content_revision: row.content_revision,
            analyzer_id: row.analyzer_id,
            analyzer_version: row.analyzer_version,
            structure_version: row.structure_version,
            node_count: usize::try_from(row.node_count).map_err(|_| {
                CortexError::Analysis("invalid persisted graph projection count".into())
            })?,
            fact_count: usize::try_from(row.fact_count).map_err(|_| {
                CortexError::Analysis("invalid persisted graph projection count".into())
            })?,
            edge_count: usize::try_from(row.edge_count).map_err(|_| {
                CortexError::Analysis("invalid persisted graph projection count".into())
            })?,
            unresolved_count: usize::try_from(row.unresolved_count).map_err(|_| {
                CortexError::Analysis("invalid persisted graph projection count".into())
            })?,
            projected_at: row.projected_at,
        })
    }
}

#[derive(FromRow)]
struct GraphNodeRow {
    id: String,
    workspace_id: String,
    node_type: String,
    stable_key: String,
    language: Option<String>,
    name: String,
    qualified_name: Option<String>,
    document_id: Option<String>,
    chunk_id: Option<String>,
    source_path: Option<String>,
    source_start_byte: Option<i64>,
    source_end_byte: Option<i64>,
    analyzer_id: String,
    analyzer_version: String,
    structure_version: String,
    content_revision: i64,
    metadata_json: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<GraphNodeRow> for GraphNode {
    type Error = CortexError;

    fn try_from(row: GraphNodeRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            node_type: GraphNodeType::from_storage(&row.node_type),
            stable_key: row.stable_key,
            language: row.language,
            name: row.name,
            qualified_name: row.qualified_name,
            document_id: row.document_id,
            chunk_id: row.chunk_id,
            source_segment: graph_source_segment_from_columns(
                row.source_path,
                row.source_start_byte,
                row.source_end_byte,
            )?,
            analyzer_id: row.analyzer_id,
            analyzer_version: row.analyzer_version,
            structure_version: row.structure_version,
            content_revision: row.content_revision,
            metadata: serde_json::from_str(&row.metadata_json)?,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

#[derive(FromRow)]
struct GraphEdgeRow {
    id: String,
    workspace_id: String,
    relationship_key: String,
    relationship_fact_id: Option<String>,
    from_node: String,
    to_node: String,
    edge_type: String,
    confidence: f32,
    analyzer_id: String,
    analyzer_version: String,
    structure_version: String,
    source_document_id: Option<String>,
    source_path: Option<String>,
    source_start_byte: Option<i64>,
    source_end_byte: Option<i64>,
    content_revision: i64,
    metadata_json: String,
    created_at: DateTime<Utc>,
}

impl TryFrom<GraphEdgeRow> for GraphEdge {
    type Error = CortexError;

    fn try_from(row: GraphEdgeRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            relationship_key: row.relationship_key,
            relationship_fact_id: row.relationship_fact_id,
            from_node: row.from_node,
            to_node: row.to_node,
            edge_type: GraphEdgeType::from_storage(&row.edge_type),
            confidence: row.confidence,
            analyzer_id: row.analyzer_id,
            analyzer_version: row.analyzer_version,
            structure_version: row.structure_version,
            source_document_id: row.source_document_id,
            source_segment: graph_source_segment_from_columns(
                row.source_path,
                row.source_start_byte,
                row.source_end_byte,
            )?,
            content_revision: row.content_revision,
            metadata: serde_json::from_str(&row.metadata_json)?,
            created_at: row.created_at,
        })
    }
}

#[derive(FromRow)]
struct UnresolvedRelationshipRow {
    id: String,
    workspace_id: String,
    source_document_id: String,
    relationship_key: String,
    from_node: Option<String>,
    from_stable_key: String,
    edge_type: String,
    target_kind: String,
    target_value: String,
    confidence: f32,
    analyzer_id: String,
    analyzer_version: String,
    structure_version: String,
    source_path: Option<String>,
    source_start_byte: Option<i64>,
    source_end_byte: Option<i64>,
    content_revision: i64,
    metadata_json: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<UnresolvedRelationshipRow> for UnresolvedRelationship {
    type Error = CortexError;

    fn try_from(row: UnresolvedRelationshipRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            source_document_id: row.source_document_id,
            relationship_key: row.relationship_key,
            from_node: row.from_node,
            from_stable_key: row.from_stable_key,
            edge_type: GraphEdgeType::from_storage(&row.edge_type),
            target_kind: RelationshipTargetKind::from_storage(&row.target_kind),
            target_value: row.target_value,
            confidence: row.confidence,
            analyzer_id: row.analyzer_id,
            analyzer_version: row.analyzer_version,
            structure_version: row.structure_version,
            source_segment: graph_source_segment_from_columns(
                row.source_path,
                row.source_start_byte,
                row.source_end_byte,
            )?,
            content_revision: row.content_revision,
            metadata: serde_json::from_str(&row.metadata_json)?,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
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

async fn experience_search_ids(
    storage: &SqliteStorage,
    workspace_id: &str,
    sql: &str,
    value: &str,
    limit: i64,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar::<_, String>(sql)
        .bind(workspace_id)
        .bind(value)
        .bind(limit)
        .fetch_all(storage.pool())
        .await?)
}

#[derive(FromRow)]
struct ExperienceRow {
    id: String,
    workspace_id: String,
    session_id: String,
    task_id: Option<String>,
    episode_id: String,
    failure_signature_json: Option<String>,
    outcome: String,
    verification_status: String,
    verification_reasons_json: String,
    evidence_strength: String,
    summary: String,
    extractor_id: String,
    extractor_version: String,
    summary_renderer_version: String,
    canonicalization_version: String,
    consolidation_fingerprint: String,
    proposal_hash: String,
    created_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct ExperienceVerificationRow {
    ordinal: i64,
    status: String,
    kind: String,
    subject_kind: String,
    subject_value: String,
    evidence_event_id: String,
    rule_id: String,
    rule_version: String,
}

#[derive(FromRow)]
struct ExperienceAttemptRow {
    id: String,
    workspace_id: String,
    experience_id: String,
    ordinal: i64,
    result: String,
    change_evidence_ordinals_json: String,
    following_verification_ordinal: Option<i64>,
}

#[derive(FromRow)]
struct ExperienceEvidenceRow {
    ordinal: i64,
    relation: String,
    event_id: String,
}

#[derive(FromRow)]
struct ExperienceCodeSnapshotRow {
    ordinal: i64,
    source_event_id: String,
    relative_path: String,
    workspace_content_revision: i64,
    document_content_revision: i64,
    document_content_hash: String,
    content: String,
    chunk_stable_key: Option<String>,
    chunk_content_hash: Option<String>,
    symbol_logical_key: Option<String>,
    symbol_label: Option<String>,
    source_start_byte: Option<i64>,
    source_end_byte: Option<i64>,
}

#[derive(FromRow)]
struct ExperienceGraphSnapshotRow {
    ordinal: i64,
    code_snapshot_ordinal: i64,
    graph_content_revision: i64,
    graph_schema_version: i64,
    graph_state: String,
    analyzer_id: String,
    analyzer_version: String,
    structure_version: String,
    node_stable_key: String,
    node_type: String,
    resolution_provenance_json: String,
}

#[derive(FromRow)]
struct ExperienceAssessmentRow {
    id: String,
    workspace_id: String,
    experience_id: String,
    kind: String,
    actor: String,
    reason: String,
    replacement_experience_id: Option<String>,
    created_at: DateTime<Utc>,
}

async fn experience_by_fingerprint_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    fingerprint: &str,
) -> Result<Option<ExperienceRow>> {
    Ok(sqlx::query_as::<_, ExperienceRow>(
        "SELECT id, workspace_id, session_id, task_id, episode_id, failure_signature_json, outcome, verification_status, verification_reasons_json, evidence_strength, summary, extractor_id, extractor_version, summary_renderer_version, canonicalization_version, consolidation_fingerprint, proposal_hash, created_at FROM experiences WHERE workspace_id = ? AND consolidation_fingerprint = ?",
    ).bind(workspace_id).bind(fingerprint).fetch_optional(&mut **transaction).await?)
}

async fn experience_record_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    experience_id: &str,
) -> Result<Option<ExperienceRecord>> {
    let Some(row) = sqlx::query_as::<_, ExperienceRow>(
        "SELECT id, workspace_id, session_id, task_id, episode_id, failure_signature_json, outcome, verification_status, verification_reasons_json, evidence_strength, summary, extractor_id, extractor_version, summary_renderer_version, canonicalization_version, consolidation_fingerprint, proposal_hash, created_at FROM experiences WHERE workspace_id = ? AND id = ?",
    ).bind(workspace_id).bind(experience_id).fetch_optional(&mut **transaction).await? else { return Ok(None); };
    let bases = sqlx::query_scalar::<_, String>("SELECT basis FROM experience_strength_bases WHERE workspace_id = ? AND experience_id = ? ORDER BY ordinal ASC")
        .bind(workspace_id).bind(experience_id).fetch_all(&mut **transaction).await?
        .into_iter().map(|value| crate::domain::EvidenceBasis::from_storage(&value)).collect();
    let failure_signature = row
        .failure_signature_json
        .map(|value| serde_json::from_str(&value))
        .transpose()?;
    let observations = sqlx::query_as::<_, ExperienceVerificationRow>(
        "SELECT ordinal, status, kind, subject_kind, subject_value, evidence_event_id, rule_id, rule_version FROM experience_verifications WHERE workspace_id = ? AND experience_id = ? ORDER BY ordinal ASC",
    ).bind(workspace_id).bind(experience_id).fetch_all(&mut **transaction).await?.into_iter().map(|observation| Ok(crate::domain::ExperienceVerificationObservation {
        ordinal: ordinal_from_i64(observation.ordinal)?,
        status: crate::domain::VerificationStatus::from_storage(&observation.status),
        kind: crate::domain::VerificationKind::from_storage(&observation.kind),
        subject: crate::domain::VerificationSubject { kind: verification_subject_kind(&observation.subject_kind)?, value: observation.subject_value },
        evidence_event_id: observation.evidence_event_id, rule_id: observation.rule_id, rule_version: observation.rule_version,
    })).collect::<Result<Vec<_>>>()?;
    let experience = Experience {
        id: row.id,
        workspace_id: row.workspace_id,
        session_id: row.session_id,
        task_id: row.task_id,
        episode_id: row.episode_id,
        failure_signature,
        outcome: crate::domain::ExperienceOutcome::from_storage(&row.outcome),
        verification: crate::domain::ExperienceVerification {
            status: crate::domain::VerificationStatus::from_storage(&row.verification_status),
            observations,
            reasons: serde_json::from_str(&row.verification_reasons_json)?,
        },
        summary: row.summary,
        evidence_strength: crate::domain::EvidenceStrengthAssessment {
            strength: crate::domain::EvidenceStrength::from_storage(&row.evidence_strength),
            bases,
        },
        extractor_id: row.extractor_id,
        extractor_version: row.extractor_version,
        summary_renderer_version: row.summary_renderer_version,
        canonicalization_version: row.canonicalization_version,
        consolidation_fingerprint: row.consolidation_fingerprint,
        proposal_hash: row.proposal_hash,
        created_at: row.created_at,
    };
    let attempts = sqlx::query_as::<_, ExperienceAttemptRow>("SELECT id, workspace_id, experience_id, ordinal, result, change_evidence_ordinals_json, following_verification_ordinal FROM experience_attempts WHERE workspace_id = ? AND experience_id = ? ORDER BY ordinal ASC")
        .bind(workspace_id).bind(experience_id).fetch_all(&mut **transaction).await?.into_iter().map(|row| Ok(ExperienceAttempt {
            id: row.id, workspace_id: row.workspace_id, experience_id: row.experience_id, ordinal: ordinal_from_i64(row.ordinal)?, result: crate::domain::AttemptResult::from_storage(&row.result),
            change_evidence_ordinals: serde_json::from_str(&row.change_evidence_ordinals_json)?, following_verification_ordinal: row.following_verification_ordinal.map(ordinal_from_i64).transpose()?,
        })).collect::<Result<Vec<_>>>()?;
    let evidence = sqlx::query_as::<_, ExperienceEvidenceRow>("SELECT ordinal, relation, event_id FROM experience_evidence WHERE workspace_id = ? AND experience_id = ? ORDER BY ordinal ASC")
        .bind(workspace_id).bind(experience_id).fetch_all(&mut **transaction).await?.into_iter().map(|row| Ok(ExperienceEvidenceLink { ordinal: ordinal_from_i64(row.ordinal)?, relation: crate::domain::ExperienceEvidenceRelation::from_storage(&row.relation), event_id: row.event_id })).collect::<Result<Vec<_>>>()?;
    let code_snapshots = sqlx::query_as::<_, ExperienceCodeSnapshotRow>("SELECT ordinal, source_event_id, relative_path, workspace_content_revision, document_content_revision, document_content_hash, content, chunk_stable_key, chunk_content_hash, symbol_logical_key, symbol_label, source_start_byte, source_end_byte FROM experience_code_snapshots WHERE workspace_id = ? AND experience_id = ? ORDER BY ordinal ASC")
        .bind(workspace_id).bind(experience_id).fetch_all(&mut **transaction).await?.into_iter().map(|row| Ok(ExperienceCodeSnapshot { ordinal: ordinal_from_i64(row.ordinal)?, source_event_id: row.source_event_id, relative_path: row.relative_path, workspace_content_revision: row.workspace_content_revision, document_content_revision: row.document_content_revision, document_content_hash: row.document_content_hash, content: row.content, chunk_stable_key: row.chunk_stable_key, chunk_content_hash: row.chunk_content_hash, symbol_logical_key: row.symbol_logical_key, symbol_label: row.symbol_label, source_start_byte: row.source_start_byte, source_end_byte: row.source_end_byte })).collect::<Result<Vec<_>>>()?;
    let graph_snapshots = sqlx::query_as::<_, ExperienceGraphSnapshotRow>("SELECT ordinal, code_snapshot_ordinal, graph_content_revision, graph_schema_version, graph_state, analyzer_id, analyzer_version, structure_version, node_stable_key, node_type, resolution_provenance_json FROM experience_graph_snapshots WHERE workspace_id = ? AND experience_id = ? ORDER BY ordinal ASC")
        .bind(workspace_id).bind(experience_id).fetch_all(&mut **transaction).await?.into_iter().map(|row| Ok(ExperienceGraphSnapshot { ordinal: ordinal_from_i64(row.ordinal)?, code_snapshot_ordinal: ordinal_from_i64(row.code_snapshot_ordinal)?, graph_content_revision: row.graph_content_revision, graph_schema_version: row.graph_schema_version, graph_state: crate::domain::GraphState::from_storage(&row.graph_state), analyzer_id: row.analyzer_id, analyzer_version: row.analyzer_version, structure_version: row.structure_version, node_stable_key: row.node_stable_key, node_type: crate::domain::GraphNodeType::from_storage(&row.node_type), resolution_provenance: serde_json::from_str(&row.resolution_provenance_json)? })).collect::<Result<Vec<_>>>()?;
    Ok(Some(ExperienceRecord {
        experience,
        attempts,
        evidence,
        code_snapshots,
        graph_snapshots,
    }))
}

async fn insert_historical_write_order(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    entity_kind: &str,
    entity_id: &str,
) -> Result<i64> {
    let result = sqlx::query(
        "INSERT INTO historical_write_order(workspace_id, entity_kind, entity_id) VALUES (?, ?, ?)",
    )
    .bind(workspace_id)
    .bind(entity_kind)
    .bind(entity_id)
    .execute(&mut **transaction)
    .await?;
    Ok(result.last_insert_rowid())
}

async fn historical_frontier_schema_available(
    transaction: &mut Transaction<'_, Sqlite>,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'historical_write_order'",
    )
    .fetch_one(&mut **transaction)
    .await?
        != 0)
}

async fn insert_experience_row(
    transaction: &mut Transaction<'_, Sqlite>,
    experience: &Experience,
) -> Result<()> {
    let signature_json = experience
        .failure_signature
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let (key, components, path, symbol) = match &experience.failure_signature {
        Some(signature) => (
            Some(signature.normalized_key.clone()),
            signature
                .components
                .iter()
                .map(|(key, value)| format!("{key}:{value}"))
                .collect::<Vec<_>>()
                .join(" "),
            signature.scope.path.clone(),
            signature.scope.symbol_key.clone(),
        ),
        None => (None, String::new(), None, None),
    };
    sqlx::query("INSERT INTO experiences(id, workspace_id, session_id, task_id, episode_id, failure_signature_json, failure_key, failure_components, failure_path, failure_symbol_key, outcome, verification_status, verification_reasons_json, evidence_strength, summary, extractor_id, extractor_version, summary_renderer_version, canonicalization_version, consolidation_fingerprint, proposal_hash, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(&experience.id).bind(&experience.workspace_id).bind(&experience.session_id).bind(&experience.task_id).bind(&experience.episode_id)
        .bind(signature_json).bind(key).bind(components).bind(path).bind(symbol).bind(experience.outcome.as_str())
        .bind(experience.verification.status.as_str())
        .bind(serde_json::to_string(&experience.verification.reasons)?).bind(experience.evidence_strength.strength.as_str())
        .bind(&experience.summary).bind(&experience.extractor_id).bind(&experience.extractor_version).bind(&experience.summary_renderer_version).bind(&experience.canonicalization_version)
        .bind(&experience.consolidation_fingerprint).bind(&experience.proposal_hash).bind(experience.created_at)
        .execute(&mut **transaction).await?;
    Ok(())
}

async fn insert_experience_verification(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    experience_id: &str,
    observation: &crate::domain::ExperienceVerificationObservation,
) -> Result<()> {
    sqlx::query("INSERT INTO experience_verifications(workspace_id, experience_id, ordinal, status, kind, subject_kind, subject_value, evidence_event_id, rule_id, rule_version) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(workspace_id).bind(experience_id).bind(ordinal_from_u64(observation.ordinal)?)
        .bind(observation.status.as_str()).bind(observation.kind.as_str())
        .bind(verification_subject_kind_name(observation.subject.kind)).bind(&observation.subject.value)
        .bind(&observation.evidence_event_id).bind(&observation.rule_id).bind(&observation.rule_version)
        .execute(&mut **transaction).await?;
    Ok(())
}

async fn insert_experience_attempt(
    transaction: &mut Transaction<'_, Sqlite>,
    attempt: &ExperienceAttempt,
) -> Result<()> {
    sqlx::query("INSERT INTO experience_attempts(id, workspace_id, experience_id, ordinal, result, change_evidence_ordinals_json, following_verification_ordinal) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(&attempt.id).bind(&attempt.workspace_id).bind(&attempt.experience_id).bind(ordinal_from_u64(attempt.ordinal)?)
        .bind(attempt.result.as_str()).bind(serde_json::to_string(&attempt.change_evidence_ordinals)?).bind(attempt.following_verification_ordinal.map(ordinal_from_u64).transpose()?)
        .execute(&mut **transaction).await?;
    Ok(())
}

async fn insert_experience_evidence(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    experience_id: &str,
    link: &ExperienceEvidenceLink,
) -> Result<()> {
    sqlx::query("INSERT INTO experience_evidence(workspace_id, experience_id, ordinal, relation, event_id) VALUES (?, ?, ?, ?, ?)")
        .bind(workspace_id).bind(experience_id).bind(ordinal_from_u64(link.ordinal)?).bind(link.relation.as_str()).bind(&link.event_id).execute(&mut **transaction).await?;
    Ok(())
}
async fn insert_experience_code_snapshot(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    experience_id: &str,
    snapshot: &ExperienceCodeSnapshot,
) -> Result<()> {
    sqlx::query("INSERT INTO experience_code_snapshots(workspace_id, experience_id, ordinal, source_event_id, relative_path, workspace_content_revision, document_content_revision, document_content_hash, content, chunk_stable_key, chunk_content_hash, symbol_logical_key, symbol_label, source_start_byte, source_end_byte) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(workspace_id).bind(experience_id).bind(ordinal_from_u64(snapshot.ordinal)?).bind(&snapshot.source_event_id).bind(&snapshot.relative_path).bind(snapshot.workspace_content_revision).bind(snapshot.document_content_revision).bind(&snapshot.document_content_hash).bind(&snapshot.content).bind(&snapshot.chunk_stable_key).bind(&snapshot.chunk_content_hash).bind(&snapshot.symbol_logical_key).bind(&snapshot.symbol_label).bind(snapshot.source_start_byte).bind(snapshot.source_end_byte).execute(&mut **transaction).await?;
    Ok(())
}
async fn insert_experience_graph_snapshot(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    experience_id: &str,
    snapshot: &ExperienceGraphSnapshot,
) -> Result<()> {
    sqlx::query("INSERT INTO experience_graph_snapshots(workspace_id, experience_id, ordinal, code_snapshot_ordinal, graph_content_revision, graph_schema_version, graph_state, analyzer_id, analyzer_version, structure_version, node_stable_key, node_type, resolution_provenance_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(workspace_id).bind(experience_id).bind(ordinal_from_u64(snapshot.ordinal)?).bind(ordinal_from_u64(snapshot.code_snapshot_ordinal)?).bind(snapshot.graph_content_revision).bind(snapshot.graph_schema_version).bind(snapshot.graph_state.storage_name()).bind(&snapshot.analyzer_id).bind(&snapshot.analyzer_version).bind(&snapshot.structure_version).bind(&snapshot.node_stable_key).bind(snapshot.node_type.storage_name()).bind(serde_json::to_string(&snapshot.resolution_provenance)?).execute(&mut **transaction).await?;
    Ok(())
}

fn validate_experience_record(record: &ExperienceRecord) -> Result<()> {
    let experience = &record.experience;
    for (label, value) in [
        ("experience ID", &experience.id),
        ("workspace ID", &experience.workspace_id),
        ("session ID", &experience.session_id),
        ("episode ID", &experience.episode_id),
        ("extractor ID", &experience.extractor_id),
        ("extractor version", &experience.extractor_version),
    ] {
        if value.trim().is_empty() || value.len() > 256 {
            return Err(CortexError::Analysis(format!(
                "{label} must be non-empty and bounded"
            )));
        }
    }
    if !valid_blake3_hash(&experience.consolidation_fingerprint)
        || !valid_blake3_hash(&experience.proposal_hash)
    {
        return Err(CortexError::Analysis(
            "experience fingerprints must be lowercase BLAKE3 hashes".into(),
        ));
    }
    if experience.summary.trim().is_empty() || experience.summary.len() > 4096 {
        return Err(CortexError::Analysis(
            "experience summary must be non-empty and bounded".into(),
        ));
    }
    if experience.summary_renderer_version != crate::domain::EXPERIENCE_SUMMARY_RENDERER_VERSION {
        return Err(CortexError::Analysis(
            "experience uses an unsupported deterministic summary renderer version".into(),
        ));
    }
    if experience.canonicalization_version != crate::domain::EXPERIENCE_CANONICALIZATION_VERSION {
        return Err(CortexError::Analysis(
            "experience uses an unsupported canonicalization version".into(),
        ));
    }
    if experience.summary
        != crate::domain::render_summary(
            experience.failure_signature.as_ref(),
            experience.outcome,
            &experience.verification,
            record.attempts.len(),
        )
    {
        return Err(CortexError::Analysis(
            "experience summary does not match the deterministic renderer".into(),
        ));
    }
    if let Some(signature) = &experience.failure_signature {
        if signature.scope.workspace_id != experience.workspace_id {
            return Err(CortexError::Analysis(
                "failure signature workspace does not match experience workspace".into(),
            ));
        }
        let rebuilt = crate::domain::FailureSignature::new(
            signature.domain,
            signature.identity_capability,
            signature.components.clone(),
            signature.normalizer_id.clone(),
            signature.normalizer_version.clone(),
            signature.scope.clone(),
        )?;
        if &rebuilt != signature {
            return Err(CortexError::Analysis(
                "failure signature does not match its canonical normalized fields".into(),
            ));
        }
    }
    if record.attempts.is_empty() || record.evidence.is_empty() {
        return Err(CortexError::Analysis(
            "an experience requires at least one attempt and one evidence link".into(),
        ));
    }
    if record.attempts.len() > crate::domain::MAX_EXPERIENCE_ATTEMPTS
        || record.evidence.len() > crate::domain::MAX_EXPERIENCE_EVIDENCE
        || record.code_snapshots.len() > crate::domain::MAX_EXPERIENCE_SNAPSHOTS
        || record.graph_snapshots.len() > crate::domain::MAX_EXPERIENCE_SNAPSHOTS
    {
        return Err(CortexError::Analysis(
            "experience relation limit exceeded".into(),
        ));
    }
    validate_verification_semantics(experience, &record.evidence)?;
    validate_ordinals(record.attempts.iter().map(|value| value.ordinal), "attempt")?;
    validate_ordinals(
        record.evidence.iter().map(|value| value.ordinal),
        "evidence",
    )?;
    validate_ordinals(
        record.code_snapshots.iter().map(|value| value.ordinal),
        "code snapshot",
    )?;
    validate_ordinals(
        record.graph_snapshots.iter().map(|value| value.ordinal),
        "graph snapshot",
    )?;
    let unique_event_ids: HashSet<_> = record
        .evidence
        .iter()
        .map(|link| link.event_id.as_str())
        .collect();
    if unique_event_ids.len() != record.evidence.len() {
        return Err(CortexError::Analysis(
            "each event may have only one unambiguous relation in an experience".into(),
        ));
    }
    let evidence_ordinals: HashSet<_> = record.evidence.iter().map(|value| value.ordinal).collect();
    let mut attempt_ids = HashSet::new();
    let mut claimed_changes = HashSet::new();
    let mut claimed_verifications = HashSet::new();
    for attempt in &record.attempts {
        if attempt.id.trim().is_empty()
            || attempt.id.len() > 256
            || !attempt_ids.insert(attempt.id.as_str())
            || attempt.workspace_id != experience.workspace_id
            || attempt.experience_id != experience.id
        {
            return Err(CortexError::Analysis(
                "attempt identity and ownership must be unique and match the experience".into(),
            ));
        }
        if attempt.change_evidence_ordinals.is_empty() {
            return Err(CortexError::Analysis(
                "every attempt requires change or tool evidence".into(),
            ));
        }
        for ordinal in &attempt.change_evidence_ordinals {
            if !claimed_changes.insert(*ordinal) {
                return Err(CortexError::Analysis(
                    "attempt change evidence must belong to exactly one attempt".into(),
                ));
            }
        }
        if attempt
            .following_verification_ordinal
            .is_some_and(|ordinal| !claimed_verifications.insert(ordinal))
        {
            return Err(CortexError::Analysis(
                "following verification evidence must belong to exactly one attempt".into(),
            ));
        }
        if attempt
            .change_evidence_ordinals
            .iter()
            .any(|ordinal| !evidence_ordinals.contains(ordinal))
            || attempt
                .following_verification_ordinal
                .is_some_and(|ordinal| !evidence_ordinals.contains(&ordinal))
        {
            return Err(CortexError::Analysis(
                "attempt references an absent evidence link".into(),
            ));
        }
        if attempt.change_evidence_ordinals.iter().any(|ordinal| {
            record.evidence.get(*ordinal as usize).is_none_or(|link| {
                link.relation != crate::domain::ExperienceEvidenceRelation::AttemptChange
            })
        }) {
            return Err(CortexError::Analysis(
                "attempt change ordinals must reference attempt_change evidence".into(),
            ));
        }
        if attempt
            .following_verification_ordinal
            .is_some_and(|ordinal| {
                record.evidence.get(ordinal as usize).is_none_or(|link| {
                    link.relation != crate::domain::ExperienceEvidenceRelation::AttemptVerification
                })
            })
        {
            return Err(CortexError::Analysis(
                "following verification must reference attempt_verification evidence".into(),
            ));
        }
        if attempt.result != crate::domain::AttemptResult::Inconclusive
            && attempt.following_verification_ordinal.is_none()
        {
            return Err(CortexError::Analysis(
                "a conclusive attempt result requires following verification evidence".into(),
            ));
        }
        // The following relation proves only the ordered event link.  Earlier
        // attempt results are intentionally absent from the terminal
        // observations when a later attempt establishes a new frontier.  The
        // consolidator owns decoded result classification; persistence must
        // not collapse historical attempt results into the terminal claim.
    }
    for link in &record.evidence {
        match link.relation {
            crate::domain::ExperienceEvidenceRelation::AttemptChange
                if !claimed_changes.contains(&link.ordinal) =>
            {
                return Err(CortexError::Analysis(
                    "attempt_change evidence must belong to exactly one attempt".into(),
                ));
            }
            crate::domain::ExperienceEvidenceRelation::AttemptVerification
                if !claimed_verifications.contains(&link.ordinal) =>
            {
                return Err(CortexError::Analysis(
                    "attempt_verification evidence must follow exactly one attempt".into(),
                ));
            }
            crate::domain::ExperienceEvidenceRelation::TerminalVerification
                if !experience
                    .verification
                    .observations
                    .iter()
                    .any(|observation| observation.evidence_event_id == link.event_id) =>
            {
                return Err(CortexError::Analysis(
                    "terminal_verification evidence requires a scoped observation".into(),
                ));
            }
            _ => {}
        }
    }
    for snapshot in &record.code_snapshots {
        if !workspace_relative_experience_path(&snapshot.relative_path)
            || snapshot.workspace_content_revision < 0
            || snapshot.document_content_revision < 0
            || !valid_blake3_hash(&snapshot.document_content_hash)
            || blake3::hash(snapshot.content.as_bytes()).to_hex().as_str()
                != snapshot.document_content_hash
            || snapshot.content.len() > 65_536
            || snapshot
                .chunk_stable_key
                .as_deref()
                .is_some_and(|value| !bounded_experience_key(value, 512))
            || snapshot
                .chunk_content_hash
                .as_deref()
                .is_some_and(|value| !valid_blake3_hash(value))
            || snapshot.chunk_stable_key.is_some() != snapshot.chunk_content_hash.is_some()
            || snapshot
                .symbol_logical_key
                .as_deref()
                .is_some_and(|value| !bounded_experience_key(value, 512))
            || snapshot
                .symbol_label
                .as_deref()
                .is_some_and(|value| !bounded_experience_key(value, 512))
            || snapshot.source_start_byte.is_some() != snapshot.source_end_byte.is_some()
            || snapshot
                .source_start_byte
                .zip(snapshot.source_end_byte)
                .is_some_and(|(start, end)| {
                    start < 0 || end < start || end as usize > snapshot.content.len()
                })
            || !record.evidence.iter().any(|link| {
                link.event_id == snapshot.source_event_id
                    && link.relation == crate::domain::ExperienceEvidenceRelation::AttemptChange
            })
        {
            return Err(CortexError::Analysis(
                "code snapshots require self-consistent bounded historical content".into(),
            ));
        }
    }
    for snapshot in &record.graph_snapshots {
        let code_snapshot = record
            .code_snapshots
            .get(snapshot.code_snapshot_ordinal as usize);
        if code_snapshot.is_none_or(|code| {
            code.ordinal != snapshot.code_snapshot_ordinal
                || code.workspace_content_revision != snapshot.graph_content_revision
        }) || snapshot.graph_content_revision < 0
            || snapshot.graph_schema_version < 0
            || !bounded_experience_key(&snapshot.analyzer_id, 256)
            || !bounded_experience_key(&snapshot.analyzer_version, 256)
            || !bounded_experience_key(&snapshot.structure_version, 256)
            || !bounded_experience_key(&snapshot.node_stable_key, 512)
            || serde_json::to_vec(&snapshot.resolution_provenance)?.len() > 65_536
        {
            return Err(CortexError::Analysis(
                "graph snapshots require bounded stable historical material".into(),
            ));
        }
    }
    Ok(())
}

fn validate_verification_semantics(
    experience: &Experience,
    evidence: &[ExperienceEvidenceLink],
) -> Result<()> {
    let verification = &experience.verification;
    if verification.observations.len() > 16
        || verification.reasons.len() > 16
        || verification
            .reasons
            .iter()
            .any(|reason| reason.trim().is_empty() || reason.len() > 4096)
    {
        return Err(CortexError::Analysis(
            "verification observations and reasons must be bounded".into(),
        ));
    }
    validate_ordinals(
        verification
            .observations
            .iter()
            .map(|observation| observation.ordinal),
        "verification observation",
    )?;
    let mut passed = 0;
    let mut failed = 0;
    let mut accepted = 0;
    let mut positive_subjects = BTreeSet::new();
    let mut failed_subjects = BTreeSet::new();
    for observation in &verification.observations {
        match observation.status {
            crate::domain::VerificationStatus::VerifiedPassed => {
                passed += 1;
                positive_subjects
                    .insert((observation.subject.kind, observation.subject.value.as_str()));
            }
            crate::domain::VerificationStatus::VerifiedFailed => {
                failed += 1;
                failed_subjects
                    .insert((observation.subject.kind, observation.subject.value.as_str()));
            }
            crate::domain::VerificationStatus::ExplicitlyAccepted => {
                accepted += 1;
                positive_subjects
                    .insert((observation.subject.kind, observation.subject.value.as_str()));
            }
            crate::domain::VerificationStatus::Conflicting
            | crate::domain::VerificationStatus::Missing
            | crate::domain::VerificationStatus::Unsupported => {
                return Err(CortexError::Analysis(
                    "individual verification observations must be passed, failed, or explicitly accepted".into(),
                ));
            }
        }
        if observation.kind == crate::domain::VerificationKind::None
            || (observation.status == crate::domain::VerificationStatus::ExplicitlyAccepted
                && observation.kind != crate::domain::VerificationKind::UserAcceptance)
            || (observation.status != crate::domain::VerificationStatus::ExplicitlyAccepted
                && observation.kind == crate::domain::VerificationKind::UserAcceptance)
        {
            return Err(CortexError::Analysis(
                "verification status and kind are incompatible".into(),
            ));
        }
        if !stable_experience_component(&observation.subject.value, 512)
            || !stable_experience_component(&observation.rule_id, 256)
            || !stable_experience_component(&observation.rule_version, 256)
            || (observation.subject.kind == crate::domain::VerificationSubjectKind::Path
                && !workspace_relative_experience_path(&observation.subject.value))
        {
            return Err(CortexError::Analysis(
                "verification observations require subject and rule identity".into(),
            ));
        }
        let Some(link) = evidence
            .iter()
            .find(|link| link.event_id == observation.evidence_event_id)
        else {
            return Err(CortexError::Analysis(
                "verification observation must reference exact experience evidence".into(),
            ));
        };
        if !matches!(
            link.relation,
            crate::domain::ExperienceEvidenceRelation::AttemptVerification
                | crate::domain::ExperienceEvidenceRelation::TerminalVerification
        ) {
            return Err(CortexError::Analysis(
                "verification observation uses an invalid evidence relation".into(),
            ));
        }
    }
    if verification.status == crate::domain::VerificationStatus::Conflicting
        && (failed == 0 || passed + accepted == 0)
    {
        return Err(CortexError::Analysis(
            "conflicting verification requires both positive and failed observations".into(),
        ));
    }
    let outcome_valid = match experience.outcome {
        crate::domain::ExperienceOutcome::Success => {
            failed == 0
                && ((verification.status == crate::domain::VerificationStatus::VerifiedPassed
                    && passed > 0)
                    || (verification.status
                        == crate::domain::VerificationStatus::ExplicitlyAccepted
                        && accepted > 0))
        }
        crate::domain::ExperienceOutcome::Failure => {
            failed > 0
                && passed + accepted == 0
                && verification.status == crate::domain::VerificationStatus::VerifiedFailed
        }
        crate::domain::ExperienceOutcome::PartialSuccess => {
            failed > 0
                && passed + accepted > 0
                && verification.status == crate::domain::VerificationStatus::Conflicting
                && positive_subjects
                    .iter()
                    .any(|subject| failed_subjects.iter().any(|failed| failed != subject))
        }
        crate::domain::ExperienceOutcome::Inconclusive => matches!(
            verification.status,
            crate::domain::VerificationStatus::Conflicting
                | crate::domain::VerificationStatus::Missing
                | crate::domain::VerificationStatus::Unsupported
        ),
        crate::domain::ExperienceOutcome::Abandoned => matches!(
            verification.status,
            crate::domain::VerificationStatus::Missing
                | crate::domain::VerificationStatus::Unsupported
        ),
    };
    if !outcome_valid {
        return Err(CortexError::Analysis(
            "experience outcome is not supported by its scoped verification observations".into(),
        ));
    }
    let unique_bases: BTreeSet<_> = experience.evidence_strength.bases.iter().copied().collect();
    if unique_bases.len() != experience.evidence_strength.bases.len()
        || experience.evidence_strength.bases.is_empty()
    {
        return Err(CortexError::Analysis(
            "evidence strength bases must be non-empty and unique".into(),
        ));
    }
    if matches!(
        experience.outcome,
        crate::domain::ExperienceOutcome::Success
            | crate::domain::ExperienceOutcome::Failure
            | crate::domain::ExperienceOutcome::PartialSuccess
    ) && !matches!(
        experience.evidence_strength.strength,
        crate::domain::EvidenceStrength::Strong | crate::domain::EvidenceStrength::Moderate
    ) {
        return Err(CortexError::Analysis(
            "a conclusive outcome requires at least moderate structured evidence".into(),
        ));
    }
    if matches!(
        experience.evidence_strength.strength,
        crate::domain::EvidenceStrength::Strong | crate::domain::EvidenceStrength::Moderate
    ) && !unique_bases.iter().any(|basis| {
        matches!(
            basis,
            crate::domain::EvidenceBasis::DeterministicVerifier
                | crate::domain::EvidenceBasis::RepeatedDeterministicEvidence
                | crate::domain::EvidenceBasis::ExplicitUserAcceptance
                | crate::domain::EvidenceBasis::ExplicitHarnessAssertion
        )
    }) {
        return Err(CortexError::Analysis(
            "moderate or strong evidence requires an explicit authoritative basis".into(),
        ));
    }
    if accepted > 0 && !unique_bases.contains(&crate::domain::EvidenceBasis::ExplicitUserAcceptance)
    {
        return Err(CortexError::Analysis(
            "explicit acceptance requires an explicit user acceptance evidence basis".into(),
        ));
    }
    if passed + failed > 0
        && !unique_bases.iter().any(|basis| {
            matches!(
                basis,
                crate::domain::EvidenceBasis::DeterministicVerifier
                    | crate::domain::EvidenceBasis::RepeatedDeterministicEvidence
                    | crate::domain::EvidenceBasis::ExplicitHarnessAssertion
            )
        })
    {
        return Err(CortexError::Analysis(
            "verifier observations require a deterministic or harness evidence basis".into(),
        ));
    }
    if unique_bases.contains(&crate::domain::EvidenceBasis::RepeatedDeterministicEvidence)
        && passed + failed < 2
    {
        return Err(CortexError::Analysis(
            "repeated deterministic evidence requires at least two verifier observations".into(),
        ));
    }
    if matches!(
        verification.status,
        crate::domain::VerificationStatus::Missing | crate::domain::VerificationStatus::Unsupported
    ) && !verification.observations.is_empty()
    {
        return Err(CortexError::Analysis(
            "missing or unsupported verification cannot contain observations".into(),
        ));
    }
    Ok(())
}

fn validate_assessment(assessment: &ExperienceAssessment) -> Result<()> {
    if assessment.id.trim().is_empty()
        || assessment.workspace_id.trim().is_empty()
        || assessment.experience_id.trim().is_empty()
        || assessment.actor.trim().is_empty()
        || assessment.reason.trim().is_empty()
    {
        return Err(CortexError::Analysis(
            "experience assessment requires identity, actor, and reason".into(),
        ));
    }
    if assessment.reason.len() > 4096
        || assessment.evidence_event_ids.len() > crate::domain::MAX_EXPERIENCE_ASSESSMENT_EVIDENCE
    {
        return Err(CortexError::Analysis(
            "experience assessment exceeds a hard bound".into(),
        ));
    }
    if (assessment.kind == crate::domain::ExperienceAssessmentKind::Superseded)
        != assessment.replacement_experience_id.is_some()
        || assessment
            .replacement_experience_id
            .as_deref()
            .is_some_and(|target| target == assessment.experience_id)
    {
        return Err(CortexError::Analysis(
            "only a superseded assessment may name a distinct replacement experience".into(),
        ));
    }
    let unique_evidence: HashSet<_> = assessment.evidence_event_ids.iter().collect();
    if unique_evidence.len() != assessment.evidence_event_ids.len() {
        return Err(CortexError::Analysis(
            "assessment evidence event IDs must be unique".into(),
        ));
    }
    Ok(())
}

fn validate_ordinals(values: impl Iterator<Item = u64>, label: &str) -> Result<()> {
    for (expected, actual) in values.enumerate() {
        if u64::try_from(expected).ok() != Some(actual) {
            return Err(CortexError::Analysis(format!(
                "{label} ordinals must begin at zero and be contiguous"
            )));
        }
    }
    Ok(())
}
fn ordinal_from_u64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| CortexError::Analysis("ordinal exceeds SQLite range".into()))
}
fn ordinal_to_i64(value: u64) -> Result<i64> {
    ordinal_from_u64(value)
}
fn ordinal_from_i64(value: i64) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| CortexError::Storage(sqlx::Error::Decode("negative ordinal".into())))
}
fn verification_subject_kind_name(kind: crate::domain::VerificationSubjectKind) -> &'static str {
    match kind {
        crate::domain::VerificationSubjectKind::Workspace => "workspace",
        crate::domain::VerificationSubjectKind::Package => "package",
        crate::domain::VerificationSubjectKind::Target => "target",
        crate::domain::VerificationSubjectKind::Test => "test",
        crate::domain::VerificationSubjectKind::Path => "path",
    }
}
fn verification_subject_kind(value: &str) -> Result<crate::domain::VerificationSubjectKind> {
    match value {
        "workspace" => Ok(crate::domain::VerificationSubjectKind::Workspace),
        "package" => Ok(crate::domain::VerificationSubjectKind::Package),
        "target" => Ok(crate::domain::VerificationSubjectKind::Target),
        "test" => Ok(crate::domain::VerificationSubjectKind::Test),
        "path" => Ok(crate::domain::VerificationSubjectKind::Path),
        _ => Err(CortexError::Storage(sqlx::Error::Decode(
            "invalid verification subject kind".into(),
        ))),
    }
}
fn stable_experience_component(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.trim() == value
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'_' | b'-' | b'.' | b':' | b'<' | b'>' | b',' | b' ' | b'&' | b'/'
                )
        })
}
fn valid_blake3_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn bounded_experience_key(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn workspace_relative_experience_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.starts_with('/')
        && !value.starts_with('\\')
        && !value.contains(':')
        && !value.split('/').any(|part| part.is_empty() || part == "..")
}

#[derive(FromRow)]
struct EpisodeRow {
    id: String,
    workspace_id: String,
    session_id: String,
    task_id: Option<String>,
    episode_type: String,
    status: String,
    title: Option<String>,
    created_by: String,
    version: i64,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
}

impl TryFrom<EpisodeRow> for Episode {
    type Error = CortexError;

    fn try_from(row: EpisodeRow) -> Result<Self> {
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            session_id: row.session_id,
            task_id: row.task_id,
            episode_type: crate::domain::EpisodeType::from_storage(&row.episode_type),
            status: EpisodeStatus::from_storage(&row.status),
            title: row.title,
            created_by: crate::domain::EpisodeCreator::from_storage(&row.created_by),
            version: u64::try_from(row.version).map_err(|_| {
                CortexError::Storage(sqlx::Error::Decode("episode version is negative".into()))
            })?,
            started_at: row.started_at,
            ended_at: row.ended_at,
            created_at: row.created_at,
        })
    }
}

#[derive(FromRow)]
struct EpisodeEventRow {
    workspace_id: String,
    episode_id: String,
    event_id: String,
    ordinal: i64,
    associated_at: DateTime<Utc>,
}

impl TryFrom<EpisodeEventRow> for EpisodeEvent {
    type Error = CortexError;

    fn try_from(row: EpisodeEventRow) -> Result<Self> {
        Ok(Self {
            workspace_id: row.workspace_id,
            episode_id: row.episode_id,
            event_id: row.event_id,
            ordinal: u64::try_from(row.ordinal).map_err(|_| {
                CortexError::Storage(sqlx::Error::Decode(
                    "episode event ordinal is negative".into(),
                ))
            })?,
            associated_at: row.associated_at,
        })
    }
}

#[derive(FromRow)]
struct EpisodeMutationRequestRow {
    operation: String,
    request_hash: String,
    resulting_version: i64,
}

async fn episode_mutation_request(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    episode_id: &str,
    request_key: &str,
) -> Result<Option<EpisodeMutationRequestRow>> {
    sqlx::query_as::<_, EpisodeMutationRequestRow>(
        "SELECT operation, request_hash, resulting_version FROM episode_mutation_requests WHERE workspace_id = ? AND episode_id = ? AND request_key = ?",
    )
    .bind(workspace_id)
    .bind(episode_id)
    .bind(request_key)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(Into::into)
}

async fn validate_consolidation_input_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    experience: &Experience,
    expected: &ConsolidationInputIdentity,
) -> Result<()> {
    if expected.workspace_id != experience.workspace_id
        || expected.episode_id != experience.episode_id
    {
        return Err(CortexError::Conflict(
            "consolidation input identity does not match the experience scope".into(),
        ));
    }
    let rows = sqlx::query(
        "SELECT member.ordinal, event.id, event.event_type, event.payload_json \
         FROM episode_events member \
         JOIN events event ON event.workspace_id = member.workspace_id AND event.id = member.event_id \
         WHERE member.workspace_id = ? AND member.episode_id = ? \
         ORDER BY member.ordinal ASC",
    )
    .bind(&experience.workspace_id)
    .bind(&experience.episode_id)
    .fetch_all(&mut **transaction)
    .await?;
    let actual = rows
        .into_iter()
        .map(|row| {
            let ordinal = ordinal_from_i64(row.try_get("ordinal")?)?;
            let payload: Value = serde_json::from_str(&row.try_get::<String, _>("payload_json")?)?;
            Ok(ConsolidationInputMember {
                ordinal,
                event_id: row.try_get("id")?,
                event_type: row.try_get("event_type")?,
                payload_hash: crate::domain::canonical_event_payload_hash(&payload)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if actual != expected.members {
        return Err(CortexError::Conflict(
            "episode membership or event identity changed after consolidation preview".into(),
        ));
    }
    Ok(())
}

async fn episode_in_transaction(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    episode_id: &str,
) -> Result<Option<Episode>> {
    let row = sqlx::query_as::<_, EpisodeRow>(
        "SELECT id, workspace_id, session_id, task_id, episode_type, status, title, created_by, version, started_at, ended_at, created_at FROM episodes WHERE workspace_id = ? AND id = ?",
    )
    .bind(workspace_id)
    .bind(episode_id)
    .fetch_optional(&mut **transaction)
    .await?;
    row.map(TryInto::try_into).transpose()
}

async fn episode_mutation_conflict(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    episode_id: &str,
    expected_version: i64,
) -> Result<Episode> {
    let episode = episode_in_transaction(transaction, workspace_id, episode_id)
        .await?
        .ok_or_else(|| CortexError::NotFound(format!("episode {episode_id}")))?;
    let detail = if episode.status.is_terminal() {
        format!("episode {episode_id} is terminal")
    } else {
        format!(
            "episode {episode_id} version {} does not match expected version {expected_version}",
            episode.version
        )
    };
    Err(CortexError::Conflict(detail))
}

fn episode_version(version: u64) -> Result<i64> {
    i64::try_from(version)
        .map_err(|_| CortexError::Conflict("episode version exceeds SQLite integer range".into()))
}

fn episode_result_version(version: i64) -> Result<u64> {
    u64::try_from(version).map_err(|_| {
        CortexError::Storage(sqlx::Error::Decode(
            "episode mutation result version is negative".into(),
        ))
    })
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

    use crate::domain::{
        AttemptResult, Document, Episode, EpisodeCreator, EpisodeType, EvidenceBasis,
        EvidenceStrength, EvidenceStrengthAssessment, Experience, ExperienceAssessment,
        ExperienceAssessmentKind, ExperienceAttempt, ExperienceCodeSnapshot, ExperienceDraft,
        ExperienceEligibilityInclusion, ExperienceEvidenceLink, ExperienceEvidenceRelation,
        ExperienceGraphSnapshot, ExperienceLifecycle, ExperienceOutcome, ExperienceRecord,
        ExperienceSearchRequest, ExperienceVerification, ExperienceVerificationObservation,
        FailureNormalizationResult, GraphEdgeType, GraphNode, GraphNodeType, GraphRepairGeneration,
        GraphRepairMode, GraphRepairState, HistoricalGraphReferenceStatus,
        HistoricalReferenceStatus, StoredChunk, UnresolvedRelationship, VerificationKind,
        VerificationStatus, VerificationSubject, VerificationSubjectKind,
    };
    use crate::service::{
        EventEvidenceDecoderRegistry, ExperienceSearchService, FailureNormalizationService,
    };

    use super::*;

    #[tokio::test]
    async fn graph_storage_enforces_workspace_isolation_and_preserves_node_identity() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let first_workspace = Workspace::new("C:/first", "first");
        let second_workspace = Workspace::new("C:/second", "second");
        storage.insert_workspace(&first_workspace).await.unwrap();
        storage.insert_workspace(&second_workspace).await.unwrap();

        let first_document = Document::new(&first_workspace.id, "src/lib.rs");
        let second_document = Document::new(&second_workspace.id, "src/lib.rs");
        storage.insert_document(&first_document).await.unwrap();
        storage.insert_document(&second_document).await.unwrap();

        let mut first_node = GraphNode::new(
            &first_workspace.id,
            GraphNodeType::Function,
            "symbol:tree-sitter-rust:src/lib.rs::function:run",
            "run",
        );
        first_node.document_id = Some(first_document.id.clone());
        let first_node = storage.upsert_graph_node(&first_node).await.unwrap();

        let mut replacement = first_node.clone();
        replacement.id = uuid::Uuid::new_v4().to_string();
        replacement.qualified_name = Some("crate::run".into());
        replacement.updated_at = Utc::now();
        let replacement = storage.upsert_graph_node(&replacement).await.unwrap();
        assert_eq!(replacement.id, first_node.id);
        assert_eq!(replacement.qualified_name.as_deref(), Some("crate::run"));

        let mut second_node = GraphNode::new(
            &second_workspace.id,
            GraphNodeType::Function,
            "symbol:tree-sitter-rust:src/lib.rs::function:run",
            "run",
        );
        second_node.document_id = Some(second_document.id.clone());
        let second_node = storage.upsert_graph_node(&second_node).await.unwrap();

        let cross_workspace_edge = GraphEdge::new(
            &first_workspace.id,
            "first-to-second",
            &first_node.id,
            &second_node.id,
            GraphEdgeType::Calls,
        );
        assert!(
            storage
                .upsert_graph_edge(&cross_workspace_edge)
                .await
                .is_err()
        );

        let mut unresolved = UnresolvedRelationship::new(
            &first_workspace.id,
            &first_document.id,
            "ambiguous-run",
            &first_node.stable_key,
            GraphEdgeType::Calls,
            RelationshipTargetKind::QualifiedSymbol,
            "run",
        );
        unresolved.from_node = Some(first_node.id.clone());
        let unresolved = storage
            .upsert_unresolved_relationship(&unresolved)
            .await
            .unwrap();
        assert!(
            storage
                .insert_unresolved_relationship_candidate(
                    &unresolved.id,
                    &first_workspace.id,
                    &second_node.id,
                    Utc::now(),
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn source_revisions_advance_only_for_committed_source_index_changes() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/project", "project");
        storage.insert_workspace(&workspace).await.unwrap();
        let initial = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(initial.content_revision, 0);
        assert!(initial.is_current());

        let document = Document::new(&workspace.id, "src/lib.rs");
        storage
            .apply_document_reconciliation(&document, &[], &[], &[], false, true)
            .await
            .unwrap();
        let changed = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(changed.content_revision, 1);
        assert_eq!(changed.graph_content_revision, 0);
        assert_eq!(changed.graph_state, GraphState::Stale);

        assert!(
            storage
                .acknowledge_graph_revision(&workspace.id, 1, Utc::now())
                .await
                .unwrap()
        );
        let acknowledged = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert!(acknowledged.is_current());

        let mut unchanged = document.clone();
        unchanged.content_revision = 1;
        storage
            .apply_document_reconciliation(&unchanged, &[], &[], &[], false, false)
            .await
            .unwrap();
        let no_op = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(no_op.content_revision, 1);
        assert!(no_op.is_current());
    }

    #[tokio::test]
    async fn stale_graph_compare_and_swap_does_not_mutate_the_graph() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/project", "project");
        storage.insert_workspace(&workspace).await.unwrap();
        let document = Document::new(&workspace.id, "src/lib.rs");
        storage
            .apply_document_reconciliation(&document, &[], &[], &[], false, true)
            .await
            .unwrap();
        let mut node = GraphNode::new(
            &workspace.id,
            GraphNodeType::Function,
            "symbol:test:run",
            "run",
        );
        node.document_id = Some(document.id.clone());
        let batch = GraphReconciliationBatch {
            workspace_id: workspace.id.clone(),
            target_content_revision: 0,
            expected_graph_updated_at: None,
            update_started_at: Some(Utc::now()),
            repair_generation_id: None,
            delete_relative_path: None,
            source_document_id: Some(document.id.clone()),
            nodes: vec![node],
            facts: Vec::new(),
            analysis_state: None,
            expected_analysis: Vec::new(),
            affected_relationships: Vec::new(),
            edges: Vec::new(),
            unresolved: Vec::new(),
        };

        assert_eq!(
            storage
                .apply_graph_reconciliation(&batch, Utc::now())
                .await
                .unwrap(),
            GraphReconciliationStatus::Superseded
        );
        assert!(storage.graph_nodes(&workspace.id).await.unwrap().is_empty());
        let revision = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(revision.content_revision, 1);
        assert_eq!(revision.graph_content_revision, 0);
        assert_eq!(revision.graph_state, GraphState::Stale);
    }

    #[tokio::test]
    async fn same_revision_graph_batches_require_the_current_update_token() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/project", "project");
        storage.insert_workspace(&workspace).await.unwrap();
        let document = Document::new(&workspace.id, "src/lib.rs");
        storage
            .apply_document_reconciliation(&document, &[], &[], &[], false, true)
            .await
            .unwrap();
        let snapshot = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        let first_token = snapshot.updated_at + chrono::Duration::milliseconds(1);
        assert!(
            storage
                .mark_graph_updating_if_current(
                    &workspace.id,
                    snapshot.content_revision,
                    snapshot.updated_at,
                    first_token,
                    None,
                )
                .await
                .unwrap()
        );
        let second_token = first_token + chrono::Duration::milliseconds(1);
        assert!(
            storage
                .mark_graph_updating_if_current(
                    &workspace.id,
                    snapshot.content_revision,
                    first_token,
                    second_token,
                    None,
                )
                .await
                .unwrap()
        );
        let batch = GraphReconciliationBatch {
            workspace_id: workspace.id.clone(),
            target_content_revision: snapshot.content_revision,
            expected_graph_updated_at: None,
            update_started_at: Some(first_token),
            repair_generation_id: None,
            delete_relative_path: None,
            source_document_id: None,
            nodes: Vec::new(),
            facts: Vec::new(),
            analysis_state: None,
            expected_analysis: Vec::new(),
            affected_relationships: Vec::new(),
            edges: Vec::new(),
            unresolved: Vec::new(),
        };

        assert_eq!(
            storage
                .apply_graph_reconciliation(&batch, Utc::now())
                .await
                .unwrap(),
            GraphReconciliationStatus::Superseded
        );
        let revision = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(revision.graph_state, GraphState::Updating);
        assert_eq!(revision.graph_update_started_at, Some(second_token));
        assert!(storage.graph_nodes(&workspace.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn active_repair_excludes_normal_graph_writes_and_owns_publication() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/repair-owner", "repair-owner");
        storage.insert_workspace(&workspace).await.unwrap();
        let snapshot = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        let now = snapshot.updated_at + chrono::Duration::milliseconds(1);
        let generation = test_repair_generation(&workspace.id, "owner", 0, now, 60);
        assert!(matches!(
            storage
                .acquire_graph_repair(&generation, now)
                .await
                .unwrap(),
            GraphRepairAcquire::Acquired(_)
        ));

        let normal_started = now + chrono::Duration::milliseconds(1);
        assert!(
            !storage
                .mark_graph_updating_if_current(
                    &workspace.id,
                    0,
                    snapshot.updated_at,
                    normal_started,
                    None,
                )
                .await
                .unwrap()
        );
        let owner_started = normal_started + chrono::Duration::milliseconds(1);
        assert!(
            storage
                .mark_graph_updating_if_current(
                    &workspace.id,
                    0,
                    snapshot.updated_at,
                    owner_started,
                    Some(&generation.generation_id),
                )
                .await
                .unwrap()
        );
        let batch = GraphReconciliationBatch {
            workspace_id: workspace.id.clone(),
            target_content_revision: 0,
            expected_graph_updated_at: None,
            update_started_at: Some(owner_started),
            repair_generation_id: Some(generation.generation_id.clone()),
            delete_relative_path: None,
            source_document_id: None,
            nodes: Vec::new(),
            facts: Vec::new(),
            analysis_state: None,
            expected_analysis: Vec::new(),
            affected_relationships: Vec::new(),
            edges: Vec::new(),
            unresolved: Vec::new(),
        };
        let applied_at = owner_started + chrono::Duration::milliseconds(1);
        assert_eq!(
            storage
                .apply_graph_reconciliation(&batch, applied_at)
                .await
                .unwrap(),
            GraphReconciliationStatus::Stale
        );
        let completed_at = applied_at + chrono::Duration::milliseconds(1);
        assert!(
            storage
                .complete_graph_repair(
                    &workspace.id,
                    &generation.generation_id,
                    0,
                    &[],
                    completed_at,
                )
                .await
                .unwrap()
        );
        assert!(
            storage
                .workspace_graph_revision(&workspace.id)
                .await
                .unwrap()
                .unwrap()
                .is_current()
        );
    }

    #[tokio::test]
    async fn expired_repair_cannot_renew_write_fail_or_publish_and_can_be_replaced() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/expired-repair", "expired-repair");
        storage.insert_workspace(&workspace).await.unwrap();
        let snapshot = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        let now = snapshot.updated_at + chrono::Duration::milliseconds(1);
        let expired = test_repair_generation(&workspace.id, "expired", 0, now, 1);
        assert!(matches!(
            storage.acquire_graph_repair(&expired, now).await.unwrap(),
            GraphRepairAcquire::Acquired(_)
        ));
        let after_expiry = now + chrono::Duration::seconds(2);
        assert!(
            !storage
                .record_graph_repair_progress(
                    &workspace.id,
                    &expired.generation_id,
                    1,
                    after_expiry + chrono::Duration::seconds(60),
                    after_expiry,
                )
                .await
                .unwrap()
        );
        assert!(
            !storage
                .mark_graph_updating_if_current(
                    &workspace.id,
                    0,
                    snapshot.updated_at,
                    after_expiry,
                    Some(&expired.generation_id),
                )
                .await
                .unwrap()
        );
        assert!(
            !storage
                .fail_graph_repair(
                    &workspace.id,
                    &expired.generation_id,
                    0,
                    "late failure",
                    after_expiry,
                )
                .await
                .unwrap()
        );
        assert!(
            !storage
                .complete_graph_repair(&workspace.id, &expired.generation_id, 0, &[], after_expiry,)
                .await
                .unwrap()
        );

        let replacement = test_repair_generation(&workspace.id, "replacement", 0, after_expiry, 60);
        assert!(matches!(
            storage
                .acquire_graph_repair(&replacement, after_expiry)
                .await
                .unwrap(),
            GraphRepairAcquire::Acquired(_)
        ));
        let persisted = storage
            .workspace_graph_repair(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted.generation_id, replacement.generation_id);
        assert_eq!(persisted.state, GraphRepairState::Active);
    }

    #[tokio::test]
    async fn failed_repair_blocks_unowned_graph_writes_until_replaced() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/failed-repair", "failed-repair");
        storage.insert_workspace(&workspace).await.unwrap();
        let snapshot = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        let now = snapshot.updated_at + chrono::Duration::milliseconds(1);
        let failed = test_repair_generation(&workspace.id, "failed", 0, now, 60);
        assert!(matches!(
            storage.acquire_graph_repair(&failed, now).await.unwrap(),
            GraphRepairAcquire::Acquired(_)
        ));
        let failed_at = now + chrono::Duration::milliseconds(1);
        assert!(
            storage
                .fail_graph_repair(
                    &workspace.id,
                    &failed.generation_id,
                    0,
                    "projection failed",
                    failed_at,
                )
                .await
                .unwrap()
        );
        assert!(
            !storage
                .mark_graph_updating_if_current(
                    &workspace.id,
                    0,
                    snapshot.updated_at,
                    failed_at + chrono::Duration::milliseconds(1),
                    None,
                )
                .await
                .unwrap()
        );

        let replacement = test_repair_generation(
            &workspace.id,
            "retry",
            0,
            failed_at + chrono::Duration::milliseconds(2),
            60,
        );
        assert!(matches!(
            storage
                .acquire_graph_repair(&replacement, replacement.started_at)
                .await
                .unwrap(),
            GraphRepairAcquire::Acquired(_)
        ));
    }

    #[tokio::test]
    async fn source_revision_change_supersedes_repair_before_graph_mutation() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/repair-source-race", "repair-source-race");
        storage.insert_workspace(&workspace).await.unwrap();
        let snapshot = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        let now = snapshot.updated_at + chrono::Duration::milliseconds(1);
        let generation = test_repair_generation(&workspace.id, "source-race", 0, now, 60);
        assert!(matches!(
            storage
                .acquire_graph_repair(&generation, now)
                .await
                .unwrap(),
            GraphRepairAcquire::Acquired(_)
        ));

        let mut document = Document::new(&workspace.id, "src/new.rs");
        document.indexed_at = now + chrono::Duration::milliseconds(1);
        let content_revision = storage
            .apply_document_reconciliation(&document, &[], &[], &[], false, true)
            .await
            .unwrap();
        assert_eq!(content_revision, 1);
        let attempted_at = document.indexed_at + chrono::Duration::milliseconds(1);
        assert!(
            !storage
                .mark_graph_updating_if_current(
                    &workspace.id,
                    generation.target_content_revision,
                    snapshot.updated_at,
                    attempted_at,
                    Some(&generation.generation_id),
                )
                .await
                .unwrap()
        );
        assert!(
            !storage
                .complete_graph_repair(
                    &workspace.id,
                    &generation.generation_id,
                    generation.target_content_revision,
                    &[],
                    attempted_at,
                )
                .await
                .unwrap()
        );
        let after = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.content_revision, 1);
        assert_eq!(after.graph_content_revision, 0);
        assert_eq!(after.graph_state, GraphState::Stale);
        assert!(storage.graph_nodes(&workspace.id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_repair_acquisition_has_exactly_one_owner() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("repair-race.sqlite");
        let storage = SqliteStorage::open(&database_path).await.unwrap();
        let competitor = SqliteStorage::open(&database_path).await.unwrap();
        let workspace = Workspace::new("C:/repair-race", "repair-race");
        storage.insert_workspace(&workspace).await.unwrap();
        let now = Utc::now();
        let first = test_repair_generation(&workspace.id, "first", 0, now, 60);
        let second = test_repair_generation(&workspace.id, "second", 0, now, 60);
        let (left, right) = tokio::join!(
            storage.acquire_graph_repair(&first, now),
            competitor.acquire_graph_repair(&second, now),
        );
        let outcomes = [left.unwrap(), right.unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, GraphRepairAcquire::Acquired(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, GraphRepairAcquire::InProgress(_)))
                .count(),
            1
        );
    }

    fn test_repair_generation(
        workspace_id: &str,
        generation_id: &str,
        target_content_revision: i64,
        started_at: DateTime<Utc>,
        lease_seconds: i64,
    ) -> GraphRepairGeneration {
        GraphRepairGeneration {
            workspace_id: workspace_id.into(),
            generation_id: generation_id.into(),
            mode: GraphRepairMode::IfNeeded,
            target_content_revision,
            state: GraphRepairState::Active,
            started_at,
            lease_expires_at: started_at + chrono::Duration::seconds(lease_seconds),
            updated_at: started_at,
            completed_at: None,
            documents_considered: 0,
            documents_repaired: 0,
            documents_failed: 0,
            last_error: None,
        }
    }

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
                    true,
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
    async fn experience_repository_is_atomic_immutable_scoped_and_fts_projected() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/experience", "experience");
        let foreign_workspace = Workspace::new("C:/experience-foreign", "experience-foreign");
        storage.insert_workspace(&workspace).await.unwrap();
        storage.insert_workspace(&foreign_workspace).await.unwrap();
        let session = Session::new(&workspace.id, serde_json::json!({}));
        storage.insert_session(&session).await.unwrap();
        let task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "experience persistence",
            serde_json::json!({}),
        );
        storage.insert_task(&task).await.unwrap();
        let episode = Episode::new(
            &workspace.id,
            &session.id,
            Some(task.id.clone()),
            EpisodeType::Debugging,
            None,
            EpisodeCreator::User,
        );
        storage.insert_episode(&episode).await.unwrap();
        let snapshot_content = "fn run() {}";
        let snapshot_hash = blake3::hash(snapshot_content.as_bytes())
            .to_hex()
            .to_string();
        let mut events = Vec::new();
        for (event_type, payload) in [
            (
                EventType::CompilerResult,
                serde_json::json!({
                    "contract": "cortexweave.rust_compiler_result",
                    "version": 1,
                    "subject": { "kind": "target", "value": "core-tests" },
                    "exit_code": 1,
                    "diagnostics": [{
                        "level": "error",
                        "code": "E0308",
                        "message": "mismatched types",
                        "expected_type": "String",
                        "actual_type": "u32",
                        "path": "src/lib.rs",
                        "start_line": 1,
                        "start_column": 1
                    }]
                }),
            ),
            (
                EventType::FileModified,
                serde_json::json!({
                    "contract": "cortexweave.source_change_observation",
                    "version": 1,
                    "change": "modified",
                    "path": "src/lib.rs",
                    "previous_path": null,
                    "workspace_content_revision": 12,
                    "document_content_revision": 3,
                    "content_hash": snapshot_hash
                }),
            ),
            (
                EventType::TestResult,
                serde_json::json!({
                    "contract": "cortexweave.cargo_test_result",
                    "version": 1,
                    "subject": { "kind": "target", "value": "core-tests" },
                    "exit_code": 0,
                    "executed_test_count": 1,
                    "failures": []
                }),
            ),
            (
                EventType::TestResult,
                serde_json::json!({
                    "contract": "cortexweave.cargo_test_result",
                    "version": 1,
                    "subject": { "kind": "test", "value": "other::still_fails" },
                    "exit_code": 101,
                    "executed_test_count": 1,
                    "failures": [{
                        "test_name": "other::still_fails",
                        "assertion_class": "assert_eq",
                        "message": "left != right"
                    }]
                }),
            ),
            (
                EventType::CompilerResult,
                serde_json::json!({
                    "contract": "cortexweave.rust_compiler_result",
                    "version": 1,
                    "subject": { "kind": "target", "value": "core-tests" },
                    "exit_code": 1,
                    "diagnostics": [{
                        "level": "error",
                        "code": "E0308",
                        "message": "mismatched types",
                        "expected_type": "String",
                        "actual_type": "u32",
                        "path": "src/lib.rs",
                        "start_line": 1,
                        "start_column": 1
                    }]
                }),
            ),
            (
                EventType::CompilerResult,
                serde_json::json!({
                    "contract": "cortexweave.rust_compiler_result",
                    "version": 1,
                    "subject": { "kind": "target", "value": "core-tests" },
                    "exit_code": 1,
                    "diagnostics": [{
                        "level": "error",
                        "code": "E0425",
                        "message": "cannot find value",
                        "path": "src/lib.rs",
                        "start_line": 1,
                        "start_column": 1
                    }]
                }),
            ),
        ]
        .into_iter()
        {
            let mut event = CortexEvent::new(&workspace.id, event_type, payload);
            event.session_id = Some(session.id.clone());
            event.task_id = Some(task.id.clone());
            storage.insert_event(&event).await.unwrap();
            events.push(event);
        }
        storage
            .associate_episode_events(
                &EpisodeEventAssociationRequest {
                    workspace_id: workspace.id.clone(),
                    episode_id: episode.id.clone(),
                    expected_version: 0,
                    request_key: "experience-test-members".into(),
                    event_ids: events.iter().map(|event| event.id.clone()).collect(),
                },
                &"f".repeat(64),
                Utc::now(),
            )
            .await
            .unwrap();
        let foreign = CortexEvent::new(
            &foreign_workspace.id,
            EventType::CompilerResult,
            serde_json::json!({}),
        );
        storage.insert_event(&foreign).await.unwrap();

        let decoder = EventEvidenceDecoderRegistry::standard().unwrap();
        for event in &events {
            assert!(
                decoder.decode(event).decoded().is_some(),
                "the hand-constructed proof uses only valid typed evidence"
            );
        }
        let decoded_failure = decoder.decode(&events[0]);
        let failure_signature = match FailureNormalizationService::standard()
            .unwrap()
            .normalize(decoded_failure.decoded().unwrap())
        {
            FailureNormalizationResult::Normalized { normalization } => normalization.signature,
            FailureNormalizationResult::Unsupported { reason } => {
                panic!("hand-constructed failure must normalize: {}", reason.code)
            }
        };
        let repeated_failure = decoder.decode(&events[4]);
        let changed_failure = decoder.decode(&events[5]);
        let normalizer = FailureNormalizationService::standard().unwrap();
        let signature_key = |result| match result {
            FailureNormalizationResult::Normalized { normalization } => {
                normalization.signature.normalized_key
            }
            FailureNormalizationResult::Unsupported { reason } => {
                panic!("comparison failure must normalize: {}", reason.code)
            }
        };
        assert_eq!(
            signature_key(normalizer.normalize(repeated_failure.decoded().unwrap())),
            failure_signature.normalized_key,
            "still_failing is backed by the exact normalized failure"
        );
        assert_ne!(
            signature_key(normalizer.normalize(changed_failure.decoded().unwrap())),
            failure_signature.normalized_key,
            "verification_changed_failure is backed by a distinct normalized failure"
        );

        let make_record = |id: &str, fingerprint: &str| {
            let experience = Experience::from_draft(ExperienceDraft {
                workspace_id: workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(task.id.clone()),
                episode_id: episode.id.clone(),
                failure_signature: Some(failure_signature.clone()),
                outcome: ExperienceOutcome::Inconclusive,
                verification: ExperienceVerification {
                    status: VerificationStatus::Missing,
                    observations: Vec::new(),
                    reasons: vec!["no scoped verifier".into()],
                },
                evidence_strength: EvidenceStrengthAssessment {
                    strength: EvidenceStrength::Weak,
                    bases: vec![EvidenceBasis::TemporalAssociation],
                },
                extractor_id: "test.extractor".into(),
                extractor_version: "1".into(),
                canonicalization_version: crate::domain::EXPERIENCE_CANONICALIZATION_VERSION.into(),
                consolidation_fingerprint: fingerprint.into(),
                proposal_hash: "b".repeat(64),
                attempts: 1,
            });
            let mut experience = experience;
            experience.id = id.into();
            ExperienceRecord {
                attempts: vec![ExperienceAttempt {
                    id: format!("{id}-attempt"),
                    workspace_id: workspace.id.clone(),
                    experience_id: id.into(),
                    ordinal: 0,
                    result: AttemptResult::Inconclusive,
                    change_evidence_ordinals: vec![1],
                    following_verification_ordinal: None,
                }],
                evidence: vec![
                    ExperienceEvidenceLink {
                        ordinal: 0,
                        relation: ExperienceEvidenceRelation::InitialFailure,
                        event_id: events[0].id.clone(),
                    },
                    ExperienceEvidenceLink {
                        ordinal: 1,
                        relation: ExperienceEvidenceRelation::AttemptChange,
                        event_id: events[1].id.clone(),
                    },
                ],
                code_snapshots: vec![ExperienceCodeSnapshot {
                    ordinal: 0,
                    source_event_id: events[1].id.clone(),
                    relative_path: "src/lib.rs".into(),
                    workspace_content_revision: 12,
                    document_content_revision: 3,
                    document_content_hash: snapshot_hash.clone(),
                    content: snapshot_content.into(),
                    chunk_stable_key: Some("rust:function:crate::run".into()),
                    chunk_content_hash: Some(snapshot_hash.clone()),
                    symbol_logical_key: Some("crate::run".into()),
                    symbol_label: Some("run".into()),
                    source_start_byte: Some(0),
                    source_end_byte: Some(snapshot_content.len() as i64),
                }],
                graph_snapshots: vec![ExperienceGraphSnapshot {
                    ordinal: 0,
                    code_snapshot_ordinal: 0,
                    graph_content_revision: 12,
                    graph_schema_version: 1,
                    graph_state: crate::domain::GraphState::Current,
                    analyzer_id: "rust".into(),
                    analyzer_version: "1".into(),
                    structure_version: "1".into(),
                    node_stable_key: "rust:function:crate::run".into(),
                    node_type: crate::domain::GraphNodeType::Function,
                    resolution_provenance: serde_json::json!({"edges": 2}),
                }],
                experience,
            }
        };
        let record = make_record("experience-1", &"a".repeat(64));
        assert!(matches!(
            storage.insert_experience(&record).await,
            Err(CortexError::Analysis(message)) if message.contains("closed")
        ));
        storage
            .transition_episode(
                &EpisodeTerminalRequest {
                    workspace_id: workspace.id.clone(),
                    episode_id: episode.id.clone(),
                    expected_version: 1,
                    request_key: "experience-test-close".into(),
                },
                EpisodeStatus::Closed,
                &"e".repeat(64),
                Utc::now(),
            )
            .await
            .unwrap();
        let mut mismatched_scope = make_record("experience-mismatched-scope", &"9".repeat(64));
        mismatched_scope.experience.task_id = None;
        assert!(matches!(
            storage.insert_experience(&mismatched_scope).await,
            Err(CortexError::Analysis(message)) if message.contains("scope")
        ));
        assert_eq!(storage.insert_experience(&record).await.unwrap(), record);
        assert_eq!(
            storage
                .experience(&workspace.id, "experience-1")
                .await
                .unwrap(),
            Some(record.clone())
        );
        let document = Document::new(&workspace.id, "src/lib.rs");
        storage.insert_document(&document).await.unwrap();
        let chunk = StoredChunk::new(&document.id, "crate::run", "fn run() {}");
        storage.insert_chunk(&chunk).await.unwrap();
        sqlx::query("DELETE FROM chunks WHERE id = ?")
            .bind(&chunk.id)
            .execute(storage.pool())
            .await
            .unwrap();
        assert_eq!(
            storage
                .experience(&workspace.id, "experience-1")
                .await
                .unwrap()
                .unwrap()
                .code_snapshots[0]
                .content,
            "fn run() {}",
            "historical code snapshots do not depend on current chunks"
        );
        let fts_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM experience_fts WHERE experience_id = ?")
                .bind("experience-1")
                .fetch_one(storage.pool())
                .await
                .unwrap();
        assert_eq!(fts_count, 1);
        let candidates = storage
            .experience_search_candidates(ExperienceCandidateQuery {
                workspace_id: &workspace.id,
                exact_failure_key: Some(&failure_signature.normalized_key),
                components: &failure_signature.components,
                lexical_query: Some("\"Observed\""),
                path: Some("src/lib.rs"),
                graph_stable_key: Some("rust:function:crate::run"),
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(candidates.exact_signature, vec!["experience-1"]);
        assert!(
            candidates
                .compatible_components
                .contains(&"experience-1".into())
        );
        assert_eq!(candidates.lexical, vec!["experience-1"]);
        assert_eq!(candidates.path, vec!["experience-1"]);
        assert_eq!(candidates.graph_stable_key, vec!["experience-1"]);

        let assessment = ExperienceAssessment {
            id: "assessment-1".into(),
            workspace_id: workspace.id.clone(),
            experience_id: "experience-1".into(),
            kind: ExperienceAssessmentKind::Confirmed,
            actor: "reviewer".into(),
            reason: "historical inspection".into(),
            replacement_experience_id: None,
            evidence_event_ids: vec![events[2].id.clone()],
            created_at: Utc::now(),
        };
        storage
            .append_experience_assessment(&assessment)
            .await
            .unwrap();
        assert_eq!(
            storage
                .experience_assessments(&workspace.id, "experience-1")
                .await
                .unwrap(),
            vec![assessment]
        );
        assert!(
            sqlx::query("UPDATE experiences SET summary = 'rewritten' WHERE id = ?")
                .bind("experience-1")
                .execute(storage.pool())
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE experience_assessments SET reason = 'rewritten' WHERE id = ?")
                .bind("assessment-1")
                .execute(storage.pool())
                .await
                .is_err()
        );

        let mut foreign_record = make_record("experience-foreign", &"c".repeat(64));
        foreign_record.evidence[1].event_id = foreign.id;
        assert!(storage.insert_experience(&foreign_record).await.is_err());
        let mut invalid_relation = make_record("experience-invalid-relation", &"f".repeat(64));
        invalid_relation.evidence[1].relation = ExperienceEvidenceRelation::InitialFailure;
        assert!(
            storage.insert_experience(&invalid_relation).await.is_err(),
            "SQLite rejects a file-change event labeled as initial failure evidence"
        );
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM experiences WHERE workspace_id = ?")
                .bind(&workspace.id)
                .fetch_one(storage.pool())
                .await
                .unwrap();
        assert_eq!(
            count, 1,
            "failed evidence link rolls back its parent and children"
        );

        assert_eq!(
            storage.insert_experience(&record).await.unwrap(),
            record,
            "same fingerprint and proposal is idempotent"
        );
        let mut conflicting = make_record("experience-conflict", &"a".repeat(64));
        conflicting.experience.proposal_hash = "c".repeat(64);
        assert!(matches!(
            storage.insert_experience(&conflicting).await,
            Err(CortexError::Conflict(_))
        ));

        let mut unsupported_success = make_record("unsupported-success", &"d".repeat(64));
        unsupported_success.experience.outcome = ExperienceOutcome::Success;
        unsupported_success.experience.summary = crate::domain::render_summary(
            unsupported_success.experience.failure_signature.as_ref(),
            ExperienceOutcome::Success,
            &unsupported_success.experience.verification,
            1,
        );
        assert!(matches!(
            validate_experience_record(&unsupported_success),
            Err(CortexError::Analysis(message)) if message.contains("outcome")
        ));

        let make_failed_attempt = |id: &str, event_index: usize, result| {
            let mut failed = make_record(id, &"7".repeat(64));
            failed.experience.outcome = ExperienceOutcome::Failure;
            failed.evidence.push(ExperienceEvidenceLink {
                ordinal: 2,
                relation: ExperienceEvidenceRelation::AttemptVerification,
                event_id: events[event_index].id.clone(),
            });
            failed.attempts[0].result = result;
            failed.attempts[0].following_verification_ordinal = Some(2);
            failed.experience.verification = ExperienceVerification {
                status: VerificationStatus::VerifiedFailed,
                observations: vec![ExperienceVerificationObservation {
                    ordinal: 0,
                    status: VerificationStatus::VerifiedFailed,
                    kind: VerificationKind::RustCompiler,
                    subject: VerificationSubject {
                        kind: VerificationSubjectKind::Target,
                        value: "core-tests".into(),
                    },
                    evidence_event_id: events[event_index].id.clone(),
                    rule_id: "rust.compiler".into(),
                    rule_version: "1".into(),
                }],
                reasons: vec!["scoped compiler verification failed".into()],
            };
            failed.experience.evidence_strength = EvidenceStrengthAssessment {
                strength: EvidenceStrength::Moderate,
                bases: vec![EvidenceBasis::DeterministicVerifier],
            };
            failed.experience.summary = crate::domain::render_summary(
                failed.experience.failure_signature.as_ref(),
                ExperienceOutcome::Failure,
                &failed.experience.verification,
                1,
            );
            failed
        };
        validate_experience_record(&make_failed_attempt(
            "still-failing",
            4,
            AttemptResult::StillFailing,
        ))
        .unwrap();
        validate_experience_record(&make_failed_attempt(
            "changed-failure",
            5,
            AttemptResult::VerificationChangedFailure,
        ))
        .unwrap();

        let mut verified_success = make_record("verified-success", &"e".repeat(64));
        verified_success.experience.outcome = ExperienceOutcome::Success;
        verified_success.evidence.push(ExperienceEvidenceLink {
            ordinal: 2,
            relation: ExperienceEvidenceRelation::AttemptVerification,
            event_id: events[2].id.clone(),
        });
        verified_success.experience.verification = ExperienceVerification {
            status: VerificationStatus::VerifiedPassed,
            observations: vec![ExperienceVerificationObservation {
                ordinal: 0,
                status: VerificationStatus::VerifiedPassed,
                kind: VerificationKind::CargoTest,
                subject: VerificationSubject {
                    kind: VerificationSubjectKind::Target,
                    value: "core-tests".into(),
                },
                evidence_event_id: events[2].id.clone(),
                rule_id: "cargo.test".into(),
                rule_version: "1".into(),
            }],
            reasons: vec!["scoped verifier passed".into()],
        };
        verified_success.experience.evidence_strength = EvidenceStrengthAssessment {
            strength: EvidenceStrength::Moderate,
            bases: vec![EvidenceBasis::DeterministicVerifier],
        };
        verified_success.attempts[0].result = AttemptResult::VerificationPassed;
        verified_success.attempts[0].following_verification_ordinal = Some(2);
        verified_success.experience.summary = crate::domain::render_summary(
            verified_success.experience.failure_signature.as_ref(),
            ExperienceOutcome::Success,
            &verified_success.experience.verification,
            1,
        );
        validate_experience_record(&verified_success).unwrap();
        assert_eq!(
            storage.insert_experience(&verified_success).await.unwrap(),
            verified_success,
            "ordered verification observations round-trip through their own table"
        );

        let distinct_record =
            |id: &str, fingerprint: &str, signature: crate::domain::FailureSignature| {
                let mut value = make_record(id, fingerprint);
                value.experience.failure_signature = Some(signature);
                value.experience.summary = crate::domain::render_summary(
                    value.experience.failure_signature.as_ref(),
                    value.experience.outcome,
                    &value.experience.verification,
                    value.attempts.len(),
                );
                value
            };
        let mut other_symbol_scope = failure_signature.scope.clone();
        other_symbol_scope.symbol_key = Some("crate::other_symbol".into());
        let other_symbol_signature = crate::domain::FailureSignature::new(
            failure_signature.domain,
            failure_signature.identity_capability,
            failure_signature.components.clone(),
            failure_signature.normalizer_id.clone(),
            failure_signature.normalizer_version.clone(),
            other_symbol_scope,
        )
        .unwrap();
        let other_symbol = distinct_record(
            "experience-other-symbol",
            &"1".repeat(64),
            other_symbol_signature,
        );
        storage.insert_experience(&other_symbol).await.unwrap();

        let mut other_components = failure_signature.components.clone();
        other_components.insert("diagnostic_code".into(), "E0425".into());
        let other_failure_signature = crate::domain::FailureSignature::new(
            failure_signature.domain,
            failure_signature.identity_capability,
            other_components,
            failure_signature.normalizer_id.clone(),
            failure_signature.normalizer_version.clone(),
            failure_signature.scope.clone(),
        )
        .unwrap();
        let other_failure = distinct_record(
            "experience-other-failure",
            &"2".repeat(64),
            other_failure_signature,
        );
        assert_eq!(
            other_failure.experience.summary, record.experience.summary,
            "the lexical torture case deliberately has a similar summary"
        );
        storage.insert_experience(&other_failure).await.unwrap();

        let disputed = make_record("experience-disputed", &"3".repeat(64));
        storage.insert_experience(&disputed).await.unwrap();
        storage
            .append_experience_assessment(&ExperienceAssessment {
                id: "assessment-disputed".into(),
                workspace_id: workspace.id.clone(),
                experience_id: disputed.experience.id.clone(),
                kind: ExperienceAssessmentKind::Disputed,
                actor: "reviewer".into(),
                reason: "retrieval exclusion proof".into(),
                replacement_experience_id: None,
                evidence_event_ids: Vec::new(),
                created_at: Utc::now(),
            })
            .await
            .unwrap();

        let foreign_session = Session::new(&foreign_workspace.id, serde_json::json!({}));
        storage.insert_session(&foreign_session).await.unwrap();
        let foreign_task = Task::new(
            &foreign_workspace.id,
            Some(foreign_session.id.clone()),
            "identical failure in another workspace",
            serde_json::json!({}),
        );
        storage.insert_task(&foreign_task).await.unwrap();
        let foreign_episode = Episode::new(
            &foreign_workspace.id,
            &foreign_session.id,
            Some(foreign_task.id.clone()),
            EpisodeType::Debugging,
            None,
            EpisodeCreator::User,
        );
        storage.insert_episode(&foreign_episode).await.unwrap();
        let mut foreign_events = Vec::new();
        for event_type in [EventType::CompilerResult, EventType::FileModified] {
            let mut event = CortexEvent::new(
                &foreign_workspace.id,
                event_type,
                serde_json::json!({"workspace_isolation": true}),
            );
            event.session_id = Some(foreign_session.id.clone());
            event.task_id = Some(foreign_task.id.clone());
            storage.insert_event(&event).await.unwrap();
            foreign_events.push(event);
        }
        storage
            .associate_episode_events(
                &EpisodeEventAssociationRequest {
                    workspace_id: foreign_workspace.id.clone(),
                    episode_id: foreign_episode.id.clone(),
                    expected_version: 0,
                    request_key: "foreign-experience-members".into(),
                    event_ids: foreign_events
                        .iter()
                        .map(|event| event.id.clone())
                        .collect(),
                },
                &"4".repeat(64),
                Utc::now(),
            )
            .await
            .unwrap();
        storage
            .transition_episode(
                &EpisodeTerminalRequest {
                    workspace_id: foreign_workspace.id.clone(),
                    episode_id: foreign_episode.id.clone(),
                    expected_version: 1,
                    request_key: "foreign-experience-close".into(),
                },
                EpisodeStatus::Closed,
                &"5".repeat(64),
                Utc::now(),
            )
            .await
            .unwrap();
        let mut foreign_scope = failure_signature.scope.clone();
        foreign_scope.workspace_id = foreign_workspace.id.clone();
        let foreign_signature = crate::domain::FailureSignature::new(
            failure_signature.domain,
            failure_signature.identity_capability,
            failure_signature.components.clone(),
            failure_signature.normalizer_id.clone(),
            failure_signature.normalizer_version.clone(),
            foreign_scope,
        )
        .unwrap();
        let mut identical_foreign = distinct_record(
            "experience-identical-foreign",
            &"6".repeat(64),
            foreign_signature.clone(),
        );
        identical_foreign.experience.workspace_id = foreign_workspace.id.clone();
        identical_foreign.experience.session_id = foreign_session.id.clone();
        identical_foreign.experience.task_id = Some(foreign_task.id.clone());
        identical_foreign.experience.episode_id = foreign_episode.id.clone();
        for attempt in &mut identical_foreign.attempts {
            attempt.workspace_id = foreign_workspace.id.clone();
        }
        identical_foreign.evidence[0].event_id = foreign_events[0].id.clone();
        identical_foreign.evidence[1].event_id = foreign_events[1].id.clone();
        identical_foreign.code_snapshots[0].source_event_id = foreign_events[1].id.clone();
        storage.insert_experience(&identical_foreign).await.unwrap();

        let search = ExperienceSearchService::new(std::sync::Arc::new(storage.clone()));
        let request = |query: Option<&str>| ExperienceSearchRequest {
            workspace_id: workspace.id.clone(),
            query: query.map(str::to_owned),
            exact_failure_signature: Some(failure_signature.clone()),
            compatible_components: BTreeMap::new(),
            path: None,
            graph_stable_key: None,
            outcomes: Vec::new(),
            strengths: Vec::new(),
            lifecycles: Vec::new(),
            include_historical: false,
            created_after: None,
            created_before: None,
            limit: 20,
        };
        let exact_and_lexical = search.search(&request(Some("Observed"))).await.unwrap();
        let by_id = exact_and_lexical
            .iter()
            .map(|hit| (hit.experience.id.as_str(), hit))
            .collect::<BTreeMap<_, _>>();
        assert!(by_id.contains_key("experience-1"));
        assert!(by_id.contains_key("verified-success"));
        assert!(by_id.contains_key("experience-other-symbol"));
        assert!(by_id.contains_key("experience-other-failure"));
        assert!(!by_id.contains_key("experience-disputed"));
        assert!(!by_id.contains_key("experience-identical-foreign"));
        assert!(
            by_id["experience-1"].explanation.scores.exact_signature
                > by_id["experience-other-failure"].explanation.scores.total
        );
        assert_eq!(
            by_id["experience-other-symbol"]
                .explanation
                .scores
                .exact_signature,
            0
        );
        assert!(
            by_id["experience-other-symbol"]
                .explanation
                .scores
                .compatible_components
                > 0
        );
        assert!(
            by_id["experience-other-failure"]
                .explanation
                .scores
                .compatible_components
                < by_id["experience-other-symbol"]
                    .explanation
                    .scores
                    .compatible_components
        );

        let mut query_only = request(Some("term-that-does-not-exist"));
        query_only.exact_failure_signature = None;
        assert!(search.search(&query_only).await.unwrap().is_empty());
        let mut path_only = request(None);
        path_only.exact_failure_signature = None;
        path_only.path = Some("src/lib.rs".into());
        assert!(!search.search(&path_only).await.unwrap().is_empty());
        let mut graph_only = path_only.clone();
        graph_only.path = None;
        graph_only.graph_stable_key = Some("rust:function:crate::run".into());
        assert!(!search.search(&graph_only).await.unwrap().is_empty());

        let mut successful = request(None);
        successful.outcomes = vec![ExperienceOutcome::Success];
        assert_eq!(
            search
                .search(&successful)
                .await
                .unwrap()
                .into_iter()
                .map(|hit| hit.experience.id)
                .collect::<Vec<_>>(),
            vec!["verified-success"]
        );
        let mut disputed_only = request(None);
        disputed_only.lifecycles = vec![ExperienceLifecycle::Disputed];
        assert_eq!(
            search.search(&disputed_only).await.unwrap()[0]
                .experience
                .id,
            "experience-disputed"
        );
        let mut historical = request(None);
        historical.include_historical = true;
        let historical_hits = search.search(&historical).await.unwrap();
        let disputed = historical_hits
            .iter()
            .find(|hit| hit.experience.id == "experience-disputed")
            .expect("explicit historical inspection returns disputed history");
        assert_eq!(
            disputed.explanation.eligibility.inclusion,
            ExperienceEligibilityInclusion::HistoricalInspection
        );
        let foreign_request = ExperienceSearchRequest {
            workspace_id: foreign_workspace.id.clone(),
            query: Some("Observed".into()),
            exact_failure_signature: Some(foreign_signature),
            compatible_components: BTreeMap::new(),
            path: None,
            graph_stable_key: None,
            outcomes: Vec::new(),
            strengths: Vec::new(),
            lifecycles: Vec::new(),
            include_historical: false,
            created_after: None,
            created_before: None,
            limit: 20,
        };
        assert_eq!(
            search.search(&foreign_request).await.unwrap()[0]
                .experience
                .id,
            "experience-identical-foreign"
        );

        let mut current_node = GraphNode::new(
            &workspace.id,
            GraphNodeType::Function,
            "rust:function:crate::run",
            "run",
        );
        current_node.document_id = Some(document.id.clone());
        let graph_revision = storage
            .workspace_graph_revision(&workspace.id)
            .await
            .unwrap()
            .unwrap();
        current_node.content_revision = graph_revision.content_revision;
        let current_node = storage.upsert_graph_node(&current_node).await.unwrap();
        assert!(
            storage
                .acknowledge_graph_revision(
                    &workspace.id,
                    graph_revision.content_revision,
                    Utc::now(),
                )
                .await
                .unwrap()
        );
        let explanation = search
            .get(&workspace.id, "experience-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            explanation.references[0].status,
            HistoricalReferenceStatus::ContentChanged
        );
        assert_eq!(
            explanation.references[0].graph_status,
            HistoricalGraphReferenceStatus::Current
        );
        assert_eq!(
            explanation.references[0].current_graph_node_id,
            Some(current_node.id.clone())
        );
        sqlx::query(
            "UPDATE workspace_graph_revisions SET graph_state = 'stale' WHERE workspace_id = ?",
        )
        .bind(&workspace.id)
        .execute(storage.pool())
        .await
        .unwrap();
        let stale_graph = search
            .get(&workspace.id, "experience-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stale_graph.references[0].status,
            HistoricalReferenceStatus::ContentChanged,
            "current document evidence stays independently visible"
        );
        assert_eq!(
            stale_graph.references[0].graph_status,
            HistoricalGraphReferenceStatus::Unavailable,
            "a stale graph cannot resolve historical symbols as current"
        );
        sqlx::query(
            "UPDATE workspace_graph_revisions SET graph_state = 'current' WHERE workspace_id = ?",
        )
        .bind(&workspace.id)
        .execute(storage.pool())
        .await
        .unwrap();
        storage
            .delete_graph_node(&workspace.id, &current_node.id)
            .await
            .unwrap();
        assert_eq!(
            search
                .get(&workspace.id, "experience-1")
                .await
                .unwrap()
                .unwrap()
                .references[0]
                .graph_status,
            HistoricalGraphReferenceStatus::Missing
        );
        storage
            .delete_document(&workspace.id, "src/lib.rs")
            .await
            .unwrap();
        assert_eq!(
            search
                .get(&workspace.id, "experience-1")
                .await
                .unwrap()
                .unwrap()
                .references[0]
                .status,
            HistoricalReferenceStatus::Deleted
        );

        storage
            .append_experience_assessment(&ExperienceAssessment {
                id: "assessment-superseded".into(),
                workspace_id: workspace.id.clone(),
                experience_id: "experience-1".into(),
                kind: ExperienceAssessmentKind::Superseded,
                actor: "reviewer".into(),
                reason: "new immutable interpretation".into(),
                replacement_experience_id: Some("verified-success".into()),
                evidence_event_ids: Vec::new(),
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        assert!(
            storage
                .append_experience_assessment(&ExperienceAssessment {
                    id: "assessment-cycle".into(),
                    workspace_id: workspace.id.clone(),
                    experience_id: "verified-success".into(),
                    kind: ExperienceAssessmentKind::Superseded,
                    actor: "reviewer".into(),
                    reason: "would form a cycle".into(),
                    replacement_experience_id: Some("experience-1".into()),
                    evidence_event_ids: Vec::new(),
                    created_at: Utc::now(),
                })
                .await
                .is_err(),
            "SQLite rejects experience supersession cycles"
        );

        let mut partial = verified_success.clone();
        partial.experience.id = "partial-success".into();
        partial.experience.outcome = ExperienceOutcome::PartialSuccess;
        partial.attempts[0].experience_id = partial.experience.id.clone();
        partial.evidence.push(ExperienceEvidenceLink {
            ordinal: 3,
            relation: ExperienceEvidenceRelation::TerminalVerification,
            event_id: events[3].id.clone(),
        });
        partial
            .experience
            .verification
            .observations
            .push(ExperienceVerificationObservation {
                ordinal: 1,
                status: VerificationStatus::VerifiedFailed,
                kind: VerificationKind::CargoTest,
                subject: VerificationSubject {
                    kind: VerificationSubjectKind::Test,
                    value: "other::still_fails".into(),
                },
                evidence_event_id: events[3].id.clone(),
                rule_id: "cargo.test".into(),
                rule_version: "1".into(),
            });
        partial.experience.verification.status = VerificationStatus::Conflicting;
        partial.experience.summary = crate::domain::render_summary(
            partial.experience.failure_signature.as_ref(),
            ExperienceOutcome::PartialSuccess,
            &partial.experience.verification,
            1,
        );
        validate_experience_record(&partial).unwrap();
        let mut same_scope_conflict = partial;
        same_scope_conflict.experience.verification.observations[1].subject = VerificationSubject {
            kind: VerificationSubjectKind::Target,
            value: "core-tests".into(),
        };
        same_scope_conflict.experience.summary = crate::domain::render_summary(
            same_scope_conflict.experience.failure_signature.as_ref(),
            ExperienceOutcome::PartialSuccess,
            &same_scope_conflict.experience.verification,
            1,
        );
        assert!(matches!(
            validate_experience_record(&same_scope_conflict),
            Err(CortexError::Analysis(message)) if message.contains("outcome")
        ));
    }

    #[tokio::test]
    async fn episode_repository_enforces_primary_scope_order_and_terminal_membership() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/episodes", "episodes");
        storage.insert_workspace(&workspace).await.unwrap();
        let session = Session::new(&workspace.id, serde_json::json!({}));
        storage.insert_session(&session).await.unwrap();
        let task = Task::new(
            &workspace.id,
            Some(session.id.clone()),
            "persist episode",
            serde_json::json!({}),
        );
        storage.insert_task(&task).await.unwrap();
        let episode = Episode::new(
            &workspace.id,
            &session.id,
            Some(task.id.clone()),
            crate::domain::EpisodeType::Debugging,
            None,
            crate::domain::EpisodeCreator::User,
        );
        storage.insert_episode(&episode).await.unwrap();

        let mut mismatched = CortexEvent::new(
            &workspace.id,
            EventType::FileModified,
            serde_json::json!({}),
        );
        mismatched.session_id = Some(session.id.clone());
        storage.insert_event(&mismatched).await.unwrap();
        let mismatch = sqlx::query(
            "INSERT INTO episode_events(workspace_id, episode_id, event_id, ordinal, associated_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&workspace.id)
        .bind(&episode.id)
        .bind(&mismatched.id)
        .bind(0_i64)
        .bind(Utc::now())
        .execute(storage.pool())
        .await
        .unwrap_err();
        assert!(
            mismatch
                .to_string()
                .contains("episode event provenance mismatch")
        );

        let mut matching =
            CortexEvent::new(&workspace.id, EventType::TaskUpdated, serde_json::json!({}));
        matching.session_id = Some(session.id.clone());
        matching.task_id = Some(task.id.clone());
        storage.insert_event(&matching).await.unwrap();
        let associated = storage
            .associate_episode_events(
                &EpisodeEventAssociationRequest {
                    workspace_id: workspace.id.clone(),
                    episode_id: episode.id.clone(),
                    expected_version: 0,
                    request_key: "repository-add".into(),
                    event_ids: vec![matching.id.clone()],
                },
                &"a".repeat(64),
                Utc::now(),
            )
            .await
            .unwrap();
        assert_eq!(associated.version, 1);
        assert_eq!(
            storage
                .episode_events(&workspace.id, &episode.id, 10)
                .await
                .unwrap()
                .into_iter()
                .map(|member| member.ordinal)
                .collect::<Vec<_>>(),
            vec![0]
        );

        let mut fill_ids = Vec::new();
        for index in 1..MAX_EPISODE_EVENTS {
            let mut event = CortexEvent::new(
                &workspace.id,
                EventType::TaskUpdated,
                serde_json::json!({"index": index}),
            );
            event.session_id = Some(session.id.clone());
            event.task_id = Some(task.id.clone());
            storage.insert_event(&event).await.unwrap();
            fill_ids.push(event.id);
        }
        let filled = storage
            .associate_episode_events(
                &EpisodeEventAssociationRequest {
                    workspace_id: workspace.id.clone(),
                    episode_id: episode.id.clone(),
                    expected_version: 1,
                    request_key: "repository-fill".into(),
                    event_ids: fill_ids,
                },
                &"c".repeat(64),
                Utc::now(),
            )
            .await
            .unwrap();
        assert_eq!(filled.version, 2);
        let mut overflow = CortexEvent::new(
            &workspace.id,
            EventType::TaskUpdated,
            serde_json::json!({"overflow": true}),
        );
        overflow.session_id = Some(session.id.clone());
        overflow.task_id = Some(task.id.clone());
        storage.insert_event(&overflow).await.unwrap();
        assert!(matches!(
            storage
                .associate_episode_events(
                    &EpisodeEventAssociationRequest {
                        workspace_id: workspace.id.clone(),
                        episode_id: episode.id.clone(),
                        expected_version: 2,
                        request_key: "repository-overflow".into(),
                        event_ids: vec![overflow.id],
                    },
                    &"d".repeat(64),
                    Utc::now(),
                )
                .await,
            Err(CortexError::Analysis(_))
        ));
        assert_eq!(
            storage
                .episode(&workspace.id, &episode.id)
                .await
                .unwrap()
                .unwrap()
                .version,
            2,
            "a rejected over-cap batch rolls back its optimistic version"
        );

        let closed = storage
            .transition_episode(
                &EpisodeTerminalRequest {
                    workspace_id: workspace.id.clone(),
                    episode_id: episode.id.clone(),
                    expected_version: 2,
                    request_key: "repository-close".into(),
                },
                EpisodeStatus::Closed,
                &"b".repeat(64),
                Utc::now(),
            )
            .await
            .unwrap();
        assert_eq!(closed.status, EpisodeStatus::Closed);
        let after_close = sqlx::query(
            "INSERT INTO episode_events(workspace_id, episode_id, event_id, ordinal, associated_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&workspace.id)
        .bind(&episode.id)
        .bind(&matching.id)
        .bind(1_i64)
        .bind(Utc::now())
        .execute(storage.pool())
        .await
        .unwrap_err();
        assert!(
            after_close
                .to_string()
                .contains("episode membership requires an open episode")
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
