use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, Sqlite, Transaction};

use super::SqliteStorage;
use crate::{
    CortexError, Result,
    domain::{
        WORKSPACE_DEREGISTRATION_PLAN_TTL_SECONDS, Workspace, WorkspaceDeregistrationCounts,
        WorkspaceDeregistrationOutcome, WorkspaceDeregistrationPreview,
        WorkspaceDeregistrationReceipt, WorkspaceDeregistrationRequest,
        WorkspaceDeregistrationStaleReason, WorkspaceDeregistrationState, canonical_json,
    },
};

#[derive(Serialize, Deserialize)]
struct WorkspaceDeregistrationSnapshot {
    workspace: Workspace,
    counts: WorkspaceDeregistrationCounts,
    state: WorkspaceDeregistrationState,
}

impl SqliteStorage {
    pub(crate) async fn create_workspace_deregistration_plan(
        &self,
        workspace_id: &str,
        created_at: DateTime<Utc>,
    ) -> Result<WorkspaceDeregistrationPreview> {
        let mut transaction = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        let Some(snapshot) =
            workspace_deregistration_snapshot(&mut transaction, workspace_id).await?
        else {
            return Err(CortexError::NotFound(format!("workspace {workspace_id}")));
        };
        let snapshot_json = canonical_json(&serde_json::to_value(&snapshot)?)?;
        let snapshot_hash = snapshot_hash(&snapshot_json);
        let plan_id = uuid::Uuid::new_v4().to_string();
        let expires_at = created_at + Duration::seconds(WORKSPACE_DEREGISTRATION_PLAN_TTL_SECONDS);
        sqlx::query(
            "INSERT INTO workspace_deregistration_plans(plan_id, workspace_id, snapshot_json, snapshot_hash, created_at, expires_at, consumed_at) VALUES (?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(&plan_id)
        .bind(workspace_id)
        .bind(snapshot_json)
        .bind(snapshot_hash)
        .bind(created_at)
        .bind(expires_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(WorkspaceDeregistrationPreview {
            plan_id,
            workspace: snapshot.workspace,
            counts: snapshot.counts,
            state: snapshot.state,
            expires_at,
        })
    }

    pub(crate) async fn deregister_workspace(
        &self,
        request: &WorkspaceDeregistrationRequest,
        completed_at: DateTime<Utc>,
    ) -> Result<WorkspaceDeregistrationOutcome> {
        let request_hash = request_hash(request)?;
        let mut transaction = self.pool().begin_with("BEGIN IMMEDIATE").await?;

        if let Some(row) = sqlx::query(
            "SELECT plan_id, request_hash, counts_json, completed_at FROM workspace_deregistration_receipts WHERE workspace_id = ? AND request_key = ?",
        )
        .bind(&request.workspace_id)
        .bind(&request.request_key)
        .fetch_optional(&mut *transaction)
        .await?
        {
            let stored_plan_id: String = row.try_get("plan_id")?;
            let stored_request_hash: String = row.try_get("request_hash")?;
            if stored_plan_id != request.plan_id || stored_request_hash != request_hash {
                return Err(CortexError::Conflict(format!(
                    "workspace deregistration request key {} was already used with different content",
                    request.request_key
                )));
            }
            let receipt = WorkspaceDeregistrationReceipt {
                workspace_id: request.workspace_id.clone(),
                plan_id: stored_plan_id,
                request_key: request.request_key.clone(),
                counts: serde_json::from_str(row.try_get("counts_json")?)?,
                completed_at: row.try_get("completed_at")?,
            };
            transaction.commit().await?;
            return Ok(WorkspaceDeregistrationOutcome::Deregistered {
                receipt,
                replayed: true,
            });
        }

        let Some(plan) = sqlx::query(
            "SELECT workspace_id, snapshot_json, snapshot_hash, expires_at, consumed_at FROM workspace_deregistration_plans WHERE plan_id = ?",
        )
        .bind(&request.plan_id)
        .fetch_optional(&mut *transaction)
        .await?
        else {
            return Ok(WorkspaceDeregistrationOutcome::StalePreview {
                reason: WorkspaceDeregistrationStaleReason::PlanNotFound,
            });
        };
        let planned_workspace_id: String = plan.try_get("workspace_id")?;
        if planned_workspace_id != request.workspace_id {
            return Err(CortexError::Conflict(
                "workspace deregistration plan belongs to another workspace".into(),
            ));
        }
        let consumed_at: Option<DateTime<Utc>> = plan.try_get("consumed_at")?;
        if consumed_at.is_some() {
            return Err(CortexError::Conflict(
                "workspace deregistration plan was already consumed by another request".into(),
            ));
        }
        let expires_at: DateTime<Utc> = plan.try_get("expires_at")?;
        if completed_at > expires_at {
            return Ok(WorkspaceDeregistrationOutcome::StalePreview {
                reason: WorkspaceDeregistrationStaleReason::PlanExpired,
            });
        }

        let Some(current) =
            workspace_deregistration_snapshot(&mut transaction, &request.workspace_id).await?
        else {
            return Ok(WorkspaceDeregistrationOutcome::StalePreview {
                reason: WorkspaceDeregistrationStaleReason::WorkspaceNotFound,
            });
        };
        let current_json = canonical_json(&serde_json::to_value(&current)?)?;
        let planned_json: String = plan.try_get("snapshot_json")?;
        let planned_hash: String = plan.try_get("snapshot_hash")?;
        if current_json != planned_json || snapshot_hash(&current_json) != planned_hash {
            return Ok(WorkspaceDeregistrationOutcome::StalePreview {
                reason: WorkspaceDeregistrationStaleReason::WorkspaceChanged,
            });
        }

        let deleted = sqlx::query("DELETE FROM workspaces WHERE id = ?")
            .bind(&request.workspace_id)
            .execute(&mut *transaction)
            .await?;
        if deleted.rows_affected() != 1 {
            return Ok(WorkspaceDeregistrationOutcome::StalePreview {
                reason: WorkspaceDeregistrationStaleReason::WorkspaceNotFound,
            });
        }
        let counts_json = canonical_json(&serde_json::to_value(&current.counts)?)?;
        sqlx::query(
            "INSERT INTO workspace_deregistration_receipts(workspace_id, plan_id, request_key, request_hash, counts_json, completed_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&request.workspace_id)
        .bind(&request.plan_id)
        .bind(&request.request_key)
        .bind(request_hash)
        .bind(counts_json)
        .bind(completed_at)
        .execute(&mut *transaction)
        .await?;
        let consumed = sqlx::query(
            "UPDATE workspace_deregistration_plans SET consumed_at = ? WHERE plan_id = ? AND consumed_at IS NULL",
        )
        .bind(completed_at)
        .bind(&request.plan_id)
        .execute(&mut *transaction)
        .await?;
        if consumed.rows_affected() != 1 {
            return Err(CortexError::Conflict(
                "workspace deregistration plan was consumed concurrently".into(),
            ));
        }
        let receipt = WorkspaceDeregistrationReceipt {
            workspace_id: request.workspace_id.clone(),
            plan_id: request.plan_id.clone(),
            request_key: request.request_key.clone(),
            counts: current.counts,
            completed_at,
        };
        transaction.commit().await?;
        Ok(WorkspaceDeregistrationOutcome::Deregistered {
            receipt,
            replayed: false,
        })
    }
}

fn request_hash(request: &WorkspaceDeregistrationRequest) -> Result<String> {
    let canonical = canonical_json(&serde_json::to_value(request)?)?;
    Ok(snapshot_hash(&canonical))
}

fn snapshot_hash(canonical: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cortexweave.workspace-deregistration.v1\0");
    hasher.update(canonical.as_bytes());
    hasher.finalize().to_hex().to_string()
}

async fn workspace_deregistration_snapshot(
    transaction: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
) -> Result<Option<WorkspaceDeregistrationSnapshot>> {
    let Some(row) = sqlx::query(
        "SELECT id, root_path, name, created_at, updated_at FROM workspaces WHERE id = ?",
    )
    .bind(workspace_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(None);
    };
    let workspace = Workspace {
        id: row.try_get("id")?,
        root_path: row.try_get("root_path")?,
        name: row.try_get("name")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    };
    let counts = WorkspaceDeregistrationCounts {
        documents: count(transaction, "SELECT COUNT(*) FROM documents WHERE workspace_id = ?", workspace_id).await?,
        chunks: count(transaction, "SELECT COUNT(*) FROM chunks chunk JOIN documents document ON document.id = chunk.document_id WHERE document.workspace_id = ?", workspace_id).await?,
        embeddings: count(transaction, "SELECT COUNT(*) FROM embeddings embedding JOIN chunks chunk ON chunk.id = embedding.chunk_id JOIN documents document ON document.id = chunk.document_id WHERE document.workspace_id = ?", workspace_id).await?,
        graph_nodes: count(transaction, "SELECT COUNT(*) FROM graph_nodes WHERE workspace_id = ?", workspace_id).await?,
        graph_edges: count(transaction, "SELECT COUNT(*) FROM graph_edges WHERE workspace_id = ?", workspace_id).await?,
        memories: count(transaction, "SELECT COUNT(*) FROM memories WHERE workspace_id = ?", workspace_id).await?,
        sessions: count(transaction, "SELECT COUNT(*) FROM sessions WHERE workspace_id = ?", workspace_id).await?,
        tasks: count(transaction, "SELECT COUNT(*) FROM tasks WHERE workspace_id = ?", workspace_id).await?,
        events: count(transaction, "SELECT COUNT(*) FROM events WHERE workspace_id = ?", workspace_id).await?,
        episodes: count(transaction, "SELECT COUNT(*) FROM episodes WHERE workspace_id = ?", workspace_id).await?,
        experiences: count(transaction, "SELECT COUNT(*) FROM experiences WHERE workspace_id = ?", workspace_id).await?,
        assessments: count(transaction, "SELECT COUNT(*) FROM experience_assessments WHERE workspace_id = ?", workspace_id).await?,
        working_set_entries: count(transaction, "SELECT COUNT(*) FROM working_set_entries WHERE workspace_id = ?", workspace_id).await?,
        checkpoints: count(transaction, "SELECT COUNT(*) FROM checkpoints WHERE workspace_id = ?", workspace_id).await?,
        test_evidence_receipts: count(transaction, "SELECT COUNT(*) FROM test_evidence_import_receipts WHERE workspace_id = ?", workspace_id).await?,
        native_delivery_receipts: count(transaction, "SELECT COUNT(*) FROM native_delivery_receipts WHERE workspace_id = ?", workspace_id).await?,
    };
    let graph = sqlx::query(
        "SELECT content_revision, graph_content_revision, graph_state FROM workspace_graph_revisions WHERE workspace_id = ?",
    )
    .bind(workspace_id)
    .fetch_optional(&mut **transaction)
    .await?;
    let (content_revision, graph_content_revision, graph_state) = match graph {
        Some(graph) => (
            nonnegative(graph.try_get("content_revision")?)?,
            nonnegative(graph.try_get("graph_content_revision")?)?,
            graph.try_get("graph_state")?,
        ),
        None => (0, 0, "missing".into()),
    };
    let state = WorkspaceDeregistrationState {
        active_sessions: count(transaction, "SELECT COUNT(*) FROM sessions WHERE workspace_id = ? AND ended_at IS NULL", workspace_id).await?,
        open_tasks: count(transaction, "SELECT COUNT(*) FROM tasks WHERE workspace_id = ? AND status IN ('pending', 'active')", workspace_id).await?,
        open_episodes: count(transaction, "SELECT COUNT(*) FROM episodes WHERE workspace_id = ? AND status = 'open'", workspace_id).await?,
        graph_state,
        graph_content_revision,
        content_revision,
        graph_repair_active: count(transaction, "SELECT COUNT(*) FROM workspace_graph_repairs WHERE workspace_id = ? AND state = 'active'", workspace_id).await? > 0,
    };
    Ok(Some(WorkspaceDeregistrationSnapshot {
        workspace,
        counts,
        state,
    }))
}

async fn count(
    transaction: &mut Transaction<'_, Sqlite>,
    query: &str,
    workspace_id: &str,
) -> Result<u64> {
    let value: i64 = sqlx::query_scalar(query)
        .bind(workspace_id)
        .fetch_one(&mut **transaction)
        .await?;
    nonnegative(value)
}

fn nonnegative(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| {
        CortexError::Storage(sqlx::Error::Decode(
            "workspace deregistration summary contained a negative value".into(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};

    use crate::domain::{
        Workspace, WorkspaceDeregistrationOutcome, WorkspaceDeregistrationRequest,
        WorkspaceDeregistrationStaleReason,
    };

    use super::SqliteStorage;

    #[tokio::test]
    async fn empty_workspace_deletion_is_replayable_and_plan_is_single_use() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/empty-deregistration", "empty");
        storage.insert_workspace(&workspace).await.unwrap();
        let preview = storage
            .create_workspace_deregistration_plan(&workspace.id, Utc::now())
            .await
            .unwrap();
        assert_eq!(preview.counts, Default::default());
        let request = WorkspaceDeregistrationRequest {
            workspace_id: workspace.id.clone(),
            plan_id: preview.plan_id,
            request_key: "empty-delete".into(),
        };
        let first = storage
            .deregister_workspace(&request, Utc::now())
            .await
            .unwrap();
        let WorkspaceDeregistrationOutcome::Deregistered {
            receipt,
            replayed: false,
        } = first
        else {
            panic!("empty workspace must be deleted")
        };
        assert!(
            storage
                .get_workspace(&workspace.id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            storage
                .deregister_workspace(&request, Utc::now())
                .await
                .unwrap(),
            WorkspaceDeregistrationOutcome::Deregistered {
                receipt,
                replayed: true
            }
        );
        let changed_key = WorkspaceDeregistrationRequest {
            request_key: "different-request".into(),
            ..request
        };
        assert!(
            storage
                .deregister_workspace(&changed_key, Utc::now())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn expired_plan_is_typed_and_never_deletes() {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/expired-deregistration", "expired");
        storage.insert_workspace(&workspace).await.unwrap();
        let created_at = Utc::now() - Duration::minutes(10);
        let preview = storage
            .create_workspace_deregistration_plan(&workspace.id, created_at)
            .await
            .unwrap();
        let outcome = storage
            .deregister_workspace(
                &WorkspaceDeregistrationRequest {
                    workspace_id: workspace.id.clone(),
                    plan_id: preview.plan_id,
                    request_key: "expired-delete".into(),
                },
                Utc::now(),
            )
            .await
            .unwrap();
        assert_eq!(
            outcome,
            WorkspaceDeregistrationOutcome::StalePreview {
                reason: WorkspaceDeregistrationStaleReason::PlanExpired
            }
        );
        assert_eq!(
            storage.get_workspace(&workspace.id).await.unwrap(),
            Some(workspace)
        );
    }
}
