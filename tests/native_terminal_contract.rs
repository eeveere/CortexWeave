use cortexweave::workspace::WorkspaceSelector;
use cortexweave::{
    AppConfig, CortexWeaveService,
    domain::{NativeDeliveryRequest, NativeOperation, NativeRecord, TaskStatus},
    storage::SqliteStorage,
};
use serde_json::json;
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    service: CortexWeaveService,
    complete: NativeDeliveryRequest,
    end: NativeDeliveryRequest,
}
async fn setup() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let service = CortexWeaveService::from_parts(
        AppConfig::default(),
        SqliteStorage::open(dir.path().join("native.sqlite"))
            .await
            .unwrap(),
    )
    .unwrap();
    let workspace = service
        .register_workspace(dir.path().to_string_lossy(), "terminal receipts")
        .await
        .unwrap();
    let session = service
        .start_session(&workspace.id, json!({"owner": "run-1", "mode": "test"}))
        .await
        .unwrap();
    let task = service
        .start_task(
            &workspace.id,
            Some(session.id.clone()),
            "check",
            json!({"owner": "run-1"}),
        )
        .await
        .unwrap();
    let complete = NativeDeliveryRequest {
        request_key: "task/complete".into(),
        operation: NativeOperation::CompleteTask {
            workspace_id: workspace.id.clone(),
            session_id: session.id.clone(),
            task_id: task.id.clone(),
            expected_details: task.details,
            details: json!({"owner": "run-1", "verified": "snapshot-1"}),
        },
    };
    let end = NativeDeliveryRequest {
        request_key: "session/end".into(),
        operation: NativeOperation::EndSession {
            workspace_id: workspace.id,
            session_id: session.id,
            task_id: task.id,
            task_completion_key: complete.request_key.clone(),
            completed_task_details: json!({"owner": "run-1", "verified": "snapshot-1"}),
            owner_metadata: json!({"owner": "run-1"}),
        },
    };
    Fixture {
        dir,
        service,
        complete,
        end,
    }
}
fn ids(f: &Fixture) -> (&str, &str) {
    let NativeOperation::CompleteTask {
        session_id,
        task_id,
        ..
    } = &f.complete.operation
    else {
        panic!()
    };
    (session_id, task_id)
}
async fn reopen(f: &Fixture) -> CortexWeaveService {
    CortexWeaveService::from_parts(
        AppConfig::default(),
        SqliteStorage::open(f.dir.path().join("native.sqlite"))
            .await
            .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn terminal_receipts_survive_restart_and_preserve_timestamps_and_identity() {
    let f = setup().await;
    let complete = f.service.deliver_native(f.complete.clone()).await.unwrap();
    let ended = f.service.deliver_native(f.end.clone()).await.unwrap();
    f.service.storage().pool().close().await;
    let service = reopen(&f).await;
    assert_eq!(
        service.deliver_native(f.complete.clone()).await.unwrap(),
        complete
    );
    assert_eq!(service.deliver_native(f.end.clone()).await.unwrap(), ended);
    let mut changed = f.complete.clone();
    if let NativeOperation::CompleteTask { details, .. } = &mut changed.operation {
        *details = json!({"different": true});
    }
    assert!(service.deliver_native(changed).await.is_err());
    let mut changed = f.end.clone();
    if let NativeOperation::EndSession { owner_metadata, .. } = &mut changed.operation {
        *owner_metadata = json!({"owner": "other"});
    }
    assert!(service.deliver_native(changed).await.is_err());
    for sql in [
        "UPDATE native_terminal_receipts SET receipt_json = '{}'",
        "DELETE FROM native_terminal_receipts",
    ] {
        assert!(
            sqlx::query(sql)
                .execute(service.storage().pool())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn receipt_rollback_cannot_leave_a_terminal_mutation() {
    for operation in ["complete_task", "end_session"] {
        let f = setup().await;
        if operation == "end_session" {
            f.service.deliver_native(f.complete.clone()).await.unwrap();
        }
        sqlx::query(&format!("CREATE TRIGGER fail_terminal BEFORE INSERT ON native_terminal_receipts WHEN NEW.operation = '{operation}' BEGIN SELECT RAISE(ABORT, 'injected receipt rollback'); END"))
            .execute(f.service.storage().pool()).await.unwrap();
        let request = if operation == "end_session" {
            f.end.clone()
        } else {
            f.complete.clone()
        };
        assert!(f.service.deliver_native(request.clone()).await.is_err());
        let (session, task) = ids(&f);
        assert!(
            f.service
                .storage()
                .get_session(session)
                .await
                .unwrap()
                .unwrap()
                .ended_at
                .is_none()
        );
        if operation == "complete_task" {
            assert_eq!(
                f.service
                    .storage()
                    .get_task(task)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                TaskStatus::Active
            );
        }
        sqlx::query("DROP TRIGGER fail_terminal")
            .execute(f.service.storage().pool())
            .await
            .unwrap();
        f.service.deliver_native(request).await.unwrap();
    }
}

#[tokio::test]
async fn concurrent_same_key_delivery_commits_one_original_receipt() {
    let f = setup().await;
    let other = reopen(&f).await;
    for request in [f.complete.clone(), f.end.clone()] {
        let (a, b) = tokio::join!(
            f.service.deliver_native(request.clone()),
            other.deliver_native(request)
        );
        assert_eq!(a.unwrap(), b.unwrap());
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM native_terminal_receipts")
        .fetch_one(f.service.storage().pool())
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn concurrent_distinct_receipt_keys_cannot_claim_the_same_terminal_effect() {
    for session in [false, true] {
        let f = setup().await;
        let other = reopen(&f).await;
        if session {
            f.service.deliver_native(f.complete.clone()).await.unwrap();
        }
        let first = if session {
            f.end.clone()
        } else {
            f.complete.clone()
        };
        let mut second = first.clone();
        second.request_key.push_str("-other");
        let (a, b) = tokio::join!(
            f.service.deliver_native(first),
            other.deliver_native(second)
        );
        assert_ne!(a.is_ok(), b.is_ok());
    }
}

#[tokio::test]
async fn legacy_terminal_writers_cannot_overwrite_or_be_misattributed_to_receipts() {
    for session in [false, true] {
        for native_first in [false, true] {
            let f = setup().await;
            let (session_id, task_id) = ids(&f);
            if session {
                f.service.deliver_native(f.complete.clone()).await.unwrap();
            }
            let request = if session {
                f.end.clone()
            } else {
                f.complete.clone()
            };
            if native_first {
                let receipt = f.service.deliver_native(request.clone()).await.unwrap();
                // Direct public storage calls exercise the stale-read window of old services.
                let result = if session {
                    f.service
                        .storage()
                        .end_session(session_id, chrono::Utc::now())
                        .await
                } else {
                    f.service
                        .storage()
                        .update_task_status(
                            task_id,
                            TaskStatus::Failed,
                            &json!({"competitor": true}),
                        )
                        .await
                };
                assert!(result.is_err());
                assert_eq!(f.service.deliver_native(request).await.unwrap(), receipt);
            } else {
                if session {
                    f.service.end_session(session_id).await.unwrap();
                } else {
                    f.service
                        .complete_task(task_id, json!({"owner": "run-1", "verified": "snapshot-1"}))
                        .await
                        .unwrap();
                }
                assert!(f.service.deliver_native(request).await.is_err());
            }
        }
    }
}

#[tokio::test]
async fn session_end_requires_exact_completion_receipt_scope_details_and_owner() {
    let f = setup().await;
    assert!(f.service.deliver_native(f.end.clone()).await.is_err());
    f.service.deliver_native(f.complete.clone()).await.unwrap();
    for field in [
        "key",
        "task",
        "session",
        "workspace",
        "details",
        "owner",
        "empty",
        "null",
    ] {
        let mut wrong = f.end.clone();
        if let NativeOperation::EndSession {
            workspace_id,
            session_id,
            task_id,
            task_completion_key,
            completed_task_details,
            owner_metadata,
        } = &mut wrong.operation
        {
            match field {
                "key" => task_completion_key.push_str("wrong"),
                "task" => task_id.push_str("wrong"),
                "session" => session_id.push_str("wrong"),
                "workspace" => workspace_id.push_str("wrong"),
                "details" => *completed_task_details = json!({"changed": true}),
                "owner" => *owner_metadata = json!({"owner": "another"}),
                "empty" => *owner_metadata = json!({}),
                "null" => *owner_metadata = json!({"missing": null}),
                _ => unreachable!(),
            }
        }
        assert!(f.service.deliver_native(wrong).await.is_err(), "{field}");
    }
    assert!(
        f.service
            .storage()
            .get_session(ids(&f).0)
            .await
            .unwrap()
            .unwrap()
            .ended_at
            .is_none()
    );
    f.service.deliver_native(f.end).await.unwrap();
}

#[tokio::test]
async fn changed_prior_task_details_and_bounded_terminal_artifacts_fail_without_effects() {
    let f = setup().await;
    let (_, task_id) = ids(&f);
    f.service
        .update_task(task_id, TaskStatus::Active, json!({"changed": true}))
        .await
        .unwrap();
    assert!(f.service.deliver_native(f.complete.clone()).await.is_err());
    let f = setup().await;
    let mut huge = f.complete.clone();
    if let NativeOperation::CompleteTask { details, .. } = &mut huge.operation {
        *details = json!({"large": "x".repeat(65_536)});
    }
    assert!(f.service.deliver_native(huge).await.is_err());
    assert_eq!(
        f.service
            .storage()
            .get_task(ids(&f).1)
            .await
            .unwrap()
            .unwrap()
            .status,
        TaskStatus::Active
    );
    let receipt = f.service.deliver_native(f.complete).await.unwrap();
    assert!(matches!(receipt.record, NativeRecord::Task(_)));
}

#[tokio::test]
async fn oversized_existing_record_rolls_back_terminal_effect() {
    let f = setup().await;
    let (_, task) = ids(&f);
    sqlx::query("UPDATE tasks SET title = ? WHERE id = ?")
        .bind("x".repeat(65_536))
        .bind(task)
        .execute(f.service.storage().pool())
        .await
        .unwrap();
    let before = f.service.storage().get_task(task).await.unwrap().unwrap();
    assert!(f.service.deliver_native(f.complete.clone()).await.is_err());
    assert_eq!(
        f.service.storage().get_task(task).await.unwrap().unwrap(),
        before
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM native_terminal_receipts")
        .fetch_one(f.service.storage().pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn workspace_removal_counts_and_cascades_terminal_receipts() {
    use cortexweave::domain::{WorkspaceDeregistrationOutcome, WorkspaceDeregistrationRequest};
    let f = setup().await;
    f.service.deliver_native(f.complete.clone()).await.unwrap();
    f.service.deliver_native(f.end.clone()).await.unwrap();
    let workspace = f.complete.operation.workspace_id().to_owned();
    let plan = f
        .service
        .preview_workspace_deregistration(WorkspaceSelector::Id(workspace.clone()))
        .await
        .unwrap();
    assert_eq!(plan.counts.native_delivery_receipts, 2);
    let result = f
        .service
        .deregister_workspace(WorkspaceDeregistrationRequest {
            workspace_id: workspace,
            plan_id: plan.plan_id,
            request_key: "remove-fixture".into(),
        })
        .await
        .unwrap();
    assert!(matches!(
        result,
        WorkspaceDeregistrationOutcome::Deregistered { .. }
    ));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM native_terminal_receipts")
        .fetch_one(f.service.storage().pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}
