use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::Row;

use super::SqliteStorage;
use crate::{
    CortexError, Result,
    domain::{CortexEvent, EventType, TestEvidenceRecordRequest},
};

pub(crate) struct StoredTestEvidenceImport {
    pub event: CortexEvent,
    pub replayed: bool,
}

impl SqliteStorage {
    pub(crate) async fn import_test_evidence(
        &self,
        request: &TestEvidenceRecordRequest,
        producer_id: &str,
        producer_version: &str,
        run_id: &str,
        event: &CortexEvent,
    ) -> Result<StoredTestEvidenceImport> {
        let request_value = json!({
            "workspace_id": request.workspace_id,
            "session_id": request.session_id,
            "task_id": request.task_id,
            "request_key": request.request_key,
            "bundle": request.bundle,
        });
        let canonical_request = crate::domain::canonical_json(&request_value)?;
        let mut transaction = self.pool().begin_with("BEGIN IMMEDIATE").await?;

        if let Some(receipt) = sqlx::query(
            "SELECT request_key, request_json, event_id FROM test_evidence_import_receipts WHERE workspace_id = ? AND producer_id = ? AND producer_version = ? AND run_id = ?",
        )
        .bind(&request.workspace_id)
        .bind(producer_id)
        .bind(producer_version)
        .bind(run_id)
        .fetch_optional(&mut *transaction)
        .await?
        {
            let original_key: String = receipt.try_get("request_key")?;
            let original_request: String = receipt.try_get("request_json")?;
            if original_key != request.request_key || original_request != canonical_request {
                return Err(CortexError::Conflict(format!(
                    "captured test run {producer_id} v{producer_version} {run_id} was already imported with different content or provenance"
                )));
            }
            let event_id: String = receipt.try_get("event_id")?;
            let original = event_in_transaction(
                &mut transaction,
                &request.workspace_id,
                &event_id,
            )
            .await?
            .ok_or_else(|| {
                CortexError::Storage(sqlx::Error::Decode(
                    "test evidence receipt refers to a missing event".into(),
                ))
            })?;
            transaction.commit().await?;
            return Ok(StoredTestEvidenceImport {
                event: original,
                replayed: true,
            });
        }

        if sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM test_evidence_import_receipts WHERE workspace_id = ? AND request_key = ?)",
        )
        .bind(&request.workspace_id)
        .bind(&request.request_key)
        .fetch_one(&mut *transaction)
        .await?
        {
            return Err(CortexError::Conflict(format!(
                "test evidence request key {} already identifies another captured run",
                request.request_key
            )));
        }

        let workspace_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workspaces WHERE id = ?)")
                .bind(&request.workspace_id)
                .fetch_one(&mut *transaction)
                .await?;
        if !workspace_exists {
            return Err(CortexError::NotFound(format!(
                "workspace {}",
                request.workspace_id
            )));
        }
        super::native_delivery::validate_owner(
            &mut transaction,
            &request.workspace_id,
            Some(&request.session_id),
            request.task_id.as_deref(),
            false,
        )
        .await?;
        super::repositories::insert_event_in_transaction(&mut transaction, event).await?;
        sqlx::query(
            "INSERT INTO test_evidence_import_receipts(workspace_id, producer_id, producer_version, run_id, request_key, request_json, event_id, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&request.workspace_id)
        .bind(producer_id)
        .bind(producer_version)
        .bind(run_id)
        .bind(&request.request_key)
        .bind(canonical_request)
        .bind(&event.id)
        .bind(event.created_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(StoredTestEvidenceImport {
            event: event.clone(),
            replayed: false,
        })
    }
}

pub(super) fn is_normalized_test_run_payload(payload: &Value) -> bool {
    payload.get("contract").and_then(Value::as_str) == Some("cortexweave.test_run_result")
}

async fn event_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    workspace_id: &str,
    event_id: &str,
) -> Result<Option<CortexEvent>> {
    let Some(row) = sqlx::query(
        "SELECT id, workspace_id, session_id, task_id, event_type, payload_json, created_at FROM events WHERE workspace_id = ? AND id = ?",
    )
    .bind(workspace_id)
    .bind(event_id)
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(None);
    };
    let event_type: String = row.try_get("event_type")?;
    let payload_json: String = row.try_get("payload_json")?;
    Ok(Some(CortexEvent {
        id: row.try_get("id")?,
        workspace_id: row.try_get("workspace_id")?,
        session_id: row.try_get("session_id")?,
        task_id: row.try_get("task_id")?,
        event_type: EventType::from_storage(&event_type),
        payload: serde_json::from_str(&payload_json)?,
        created_at: row.try_get::<DateTime<Utc>, _>("created_at")?,
    }))
}
