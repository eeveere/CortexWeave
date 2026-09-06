use serde_json::Value;
use sqlx::{Row, Sqlite, Transaction};

use super::SqliteStorage;
use crate::{
    CortexError, Result,
    domain::{
        Episode, NativeDeliveryReceipt, NativeDeliveryRequest, NativeOperation, NativeRecord,
        Session, Task, TaskStatus,
    },
};

impl SqliteStorage {
    pub(crate) async fn deliver_native(
        &self,
        request: &NativeDeliveryRequest,
    ) -> Result<NativeDeliveryReceipt> {
        let workspace_id = request.operation.workspace_id();
        let operation = request.operation.name();
        // Compare canonical values, not object insertion order or caller-generated timestamps.
        // Event identity and its occurrence timestamp ARE part of the request and must be persisted by callers.
        let canonical = serde_json::to_value(&request.operation)?;
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        if let Some(row) = sqlx::query("SELECT request_json, receipt_json FROM native_delivery_receipts WHERE workspace_id = ? AND operation = ? AND request_key = ?")
            .bind(workspace_id).bind(operation).bind(&request.request_key)
            .fetch_optional(&mut *tx).await? {
            let original: Value = serde_json::from_str(row.try_get("request_json")?)?;
            if original != canonical {
                return Err(CortexError::Conflict(format!("native request key {} already has different content", request.request_key)));
            }
            let receipt = serde_json::from_str(row.try_get("receipt_json")?)?;
            tx.commit().await?;
            return Ok(receipt);
        }
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = ?)")
                .bind(workspace_id)
                .fetch_one(&mut *tx)
                .await?;
        if !exists {
            return Err(CortexError::NotFound(format!("workspace {workspace_id}")));
        }
        let record = match &request.operation {
            NativeOperation::StartSession { metadata, .. } => {
                let session = Session::new(workspace_id, metadata.clone());
                sqlx::query("INSERT INTO sessions(id, workspace_id, started_at, ended_at, metadata_json) VALUES (?, ?, ?, ?, ?)")
                    .bind(&session.id).bind(workspace_id).bind(session.started_at).bind(session.ended_at)
                    .bind(serde_json::to_string(&session.metadata)?).execute(&mut *tx).await?;
                NativeRecord::Session(session)
            }
            NativeOperation::StartTask {
                session_id,
                title,
                details,
                ..
            } => {
                validate_owner(&mut tx, workspace_id, session_id.as_deref(), None, true).await?;
                let mut task = Task::new(workspace_id, session_id.clone(), title, details.clone());
                task.status = TaskStatus::Active;
                sqlx::query("INSERT INTO tasks(id, workspace_id, session_id, title, status, details_json, created_at, updated_at, completed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
                    .bind(&task.id).bind(workspace_id).bind(&task.session_id).bind(&task.title).bind(task.status.as_str())
                    .bind(serde_json::to_string(&task.details)?).bind(task.created_at).bind(task.updated_at).bind(task.completed_at)
                    .execute(&mut *tx).await?;
                NativeRecord::Task(task)
            }
            NativeOperation::StartEpisode { request } => {
                validate_owner(
                    &mut tx,
                    workspace_id,
                    Some(&request.session_id),
                    request.task_id.as_deref(),
                    true,
                )
                .await?;
                let episode = Episode::new(
                    workspace_id,
                    &request.session_id,
                    request.task_id.clone(),
                    request.episode_type,
                    request.title.clone(),
                    request.created_by,
                );
                sqlx::query("INSERT INTO episodes(id, workspace_id, session_id, task_id, episode_type, status, title, created_by, version, started_at, ended_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                    .bind(&episode.id).bind(workspace_id).bind(&episode.session_id).bind(&episode.task_id)
                    .bind(episode.episode_type.as_str()).bind(episode.status.as_str()).bind(&episode.title)
                    .bind(episode.created_by.as_str()).bind(0_i64).bind(episode.started_at).bind(episode.ended_at).bind(episode.created_at)
                    .execute(&mut *tx).await?;
                NativeRecord::Episode(episode)
            }
            NativeOperation::RecordEvent { event } => {
                if super::test_evidence::is_normalized_test_run_payload(&event.payload) {
                    return Err(CortexError::Analysis(
                        "normalized test-run evidence requires the capture-specific importer"
                            .into(),
                    ));
                }
                // Delayed factual delivery may refer to ended sessions and terminal tasks.
                validate_owner(
                    &mut tx,
                    workspace_id,
                    event.session_id.as_deref(),
                    event.task_id.as_deref(),
                    false,
                )
                .await?;
                super::repositories::insert_event_in_transaction(&mut tx, event).await?;
                NativeRecord::Event(event.clone())
            }
        };
        let receipt = NativeDeliveryReceipt {
            request_key: request.request_key.clone(),
            record,
        };
        sqlx::query("INSERT INTO native_delivery_receipts(workspace_id, operation, request_key, request_json, receipt_json) VALUES (?, ?, ?, ?, ?)")
            .bind(workspace_id).bind(operation).bind(&request.request_key).bind(serde_json::to_string(&canonical)?)
            .bind(serde_json::to_string(&receipt)?).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(receipt)
    }
}

pub(super) async fn validate_owner(
    tx: &mut Transaction<'_, Sqlite>,
    workspace_id: &str,
    session_id: Option<&str>,
    task_id: Option<&str>,
    require_open_session: bool,
) -> Result<()> {
    if let Some(id) = session_id {
        let row = sqlx::query("SELECT workspace_id, ended_at FROM sessions WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("session {id}")))?;
        if row.try_get::<String, _>("workspace_id")? != workspace_id {
            return Err(CortexError::Analysis(
                "session belongs to a different workspace".into(),
            ));
        }
        if require_open_session && row.try_get::<Option<String>, _>("ended_at")?.is_some() {
            return Err(CortexError::Analysis(
                "cannot create on an ended session".into(),
            ));
        }
    }
    if let Some(id) = task_id {
        let row = sqlx::query("SELECT workspace_id, session_id FROM tasks WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| CortexError::NotFound(format!("task {id}")))?;
        if row.try_get::<String, _>("workspace_id")? != workspace_id
            || session_id.is_some_and(|session| {
                row.get::<Option<String>, _>("session_id").as_deref() != Some(session)
            })
        {
            return Err(CortexError::Analysis(
                "task/session provenance does not match".into(),
            ));
        }
    }
    Ok(())
}
