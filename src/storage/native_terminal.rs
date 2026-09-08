//! Terminal mutations and their historical receipts commit under the same write lock.
use chrono::Utc;
use serde_json::Value;
use sqlx::{Row, Sqlite, Transaction};

use super::{
    SqliteStorage,
    native_delivery::validate_owner,
    repositories::{SessionRow, TaskRow},
};
use crate::{
    CortexError, Result,
    domain::{
        NativeDeliveryReceipt, NativeDeliveryRequest, NativeOperation, NativeRecord, Session, Task,
        TaskStatus,
    },
};

fn conflict(message: &str) -> CortexError {
    CortexError::Conflict(message.into())
}

async fn task_in(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<Task> {
    let row = sqlx::query_as::<_, TaskRow>("SELECT id, workspace_id, session_id, title, status, details_json, created_at, updated_at, completed_at FROM tasks WHERE id = ?")
        .bind(id).fetch_optional(&mut **tx).await?.ok_or_else(|| CortexError::NotFound(format!("task {id}")))?;
    row.try_into()
}
async fn session_in(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<Session> {
    let row = sqlx::query_as::<_, SessionRow>(
        "SELECT id, workspace_id, started_at, ended_at, metadata_json FROM sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| CortexError::NotFound(format!("session {id}")))?;
    row.try_into()
}

impl SqliteStorage {
    pub(super) async fn deliver_native_terminal(
        &self,
        request: &NativeDeliveryRequest,
    ) -> Result<NativeDeliveryReceipt> {
        let workspace = request.operation.workspace_id();
        let operation = request.operation.name();
        let canonical = serde_json::to_value(&request.operation)?;
        let request_json = serde_json::to_string(&canonical)?;
        if request_json.len() > 65_536 {
            return Err(CortexError::Analysis(
                "native terminal request exceeds 64 KiB".into(),
            ));
        }
        let mut tx = self.pool().begin_with("BEGIN IMMEDIATE").await?;
        if let Some(row) = sqlx::query("SELECT request_json, receipt_json FROM native_terminal_receipts WHERE workspace_id = ? AND operation = ? AND request_key = ?")
            .bind(workspace).bind(operation).bind(&request.request_key).fetch_optional(&mut *tx).await? {
            let original: Value = serde_json::from_str(row.try_get("request_json")?)?;
            if original != canonical { return Err(conflict("native terminal request key has different content")); }
            let receipt = serde_json::from_str(row.try_get("receipt_json")?)?;
            tx.commit().await?;
            return Ok(receipt);
        }
        let record = match &request.operation {
            NativeOperation::CompleteTask {
                session_id,
                task_id,
                expected_details,
                details,
                ..
            } => {
                validate_owner(&mut tx, workspace, Some(session_id), Some(task_id), true).await?;
                let task = task_in(&mut tx, task_id).await?;
                if task.status != TaskStatus::Active || task.details != *expected_details {
                    return Err(conflict(
                        "task is no longer active with the expected details; no matching completion receipt",
                    ));
                }
                let now = Utc::now();
                let changed = sqlx::query("UPDATE tasks SET status = 'completed', details_json = ?, updated_at = ?, completed_at = ? WHERE id = ? AND status = 'active'")
                    .bind(serde_json::to_string(details)?).bind(now).bind(now).bind(task_id).execute(&mut *tx).await?;
                if changed.rows_affected() != 1 {
                    return Err(conflict("task completion precondition changed"));
                }
                NativeRecord::Task(task_in(&mut tx, task_id).await?)
            }
            NativeOperation::EndSession {
                session_id,
                task_id,
                task_completion_key,
                completed_task_details,
                owner_metadata,
                ..
            } => {
                validate_owner(&mut tx, workspace, Some(session_id), Some(task_id), true).await?;
                let recorded: Option<String> = sqlx::query_scalar("SELECT receipt_json FROM native_terminal_receipts WHERE workspace_id = ? AND operation = 'complete_task' AND request_key = ?")
                    .bind(workspace).bind(task_completion_key).fetch_optional(&mut *tx).await?;
                let receipt: NativeDeliveryReceipt =
                    serde_json::from_str(&recorded.ok_or_else(|| {
                        conflict(
                            "session closure requires the exact native task-completion receipt",
                        )
                    })?)?;
                let NativeRecord::Task(completed) = receipt.record else {
                    return Err(conflict("invalid task-completion receipt"));
                };
                let current = task_in(&mut tx, task_id).await?;
                if current != completed
                    || current.id != *task_id
                    || current.workspace_id != workspace
                    || current.session_id.as_deref() != Some(session_id)
                    || current.status != TaskStatus::Completed
                    || current.details != *completed_task_details
                {
                    return Err(conflict(
                        "session closure task does not match the original completion receipt",
                    ));
                }
                let session = session_in(&mut tx, session_id).await?;
                let Some(owner) = owner_metadata.as_object().filter(|o| !o.is_empty()) else {
                    return Err(conflict(
                        "session ownership requires explicit nonempty metadata",
                    ));
                };
                let Some(metadata) = session.metadata.as_object() else {
                    return Err(conflict("session ownership metadata is missing"));
                };
                if owner
                    .iter()
                    .any(|(k, v)| v.is_null() || metadata.get(k) != Some(v))
                {
                    return Err(conflict("session ownership metadata changed"));
                }
                let changed = sqlx::query(
                    "UPDATE sessions SET ended_at = ? WHERE id = ? AND ended_at IS NULL",
                )
                .bind(Utc::now())
                .bind(session_id)
                .execute(&mut *tx)
                .await?;
                if changed.rows_affected() != 1 {
                    return Err(conflict("session closure precondition changed"));
                }
                NativeRecord::Session(session_in(&mut tx, session_id).await?)
            }
            _ => {
                return Err(CortexError::Analysis(
                    "not a native terminal operation".into(),
                ));
            }
        };
        let receipt = NativeDeliveryReceipt {
            request_key: request.request_key.clone(),
            record,
        };
        let receipt_json = serde_json::to_string(&receipt)?;
        if receipt_json.len() > 65_536 {
            return Err(CortexError::Analysis(
                "native terminal receipt exceeds 64 KiB".into(),
            ));
        }
        sqlx::query("INSERT INTO native_terminal_receipts(workspace_id, operation, request_key, request_json, receipt_json) VALUES (?, ?, ?, ?, ?)")
            .bind(workspace).bind(operation).bind(&request.request_key).bind(request_json).bind(receipt_json).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(receipt)
    }
}
