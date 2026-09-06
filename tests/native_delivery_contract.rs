use cortexweave::{
    AppConfig, CortexError, CortexWeaveService,
    domain::{
        CortexEvent, EpisodeCreator, EpisodeStartRequest, EpisodeType, EventType,
        NativeDeliveryRequest, NativeOperation, NativeRecord,
    },
    storage::SqliteStorage,
};
use serde_json::json;
use tempfile::tempdir;

fn request(key: &str, operation: NativeOperation) -> NativeDeliveryRequest {
    NativeDeliveryRequest {
        request_key: key.into(),
        operation,
    }
}

#[tokio::test]
async fn lost_acknowledgements_recover_all_four_records_after_reopen() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("native.sqlite");
    let service = CortexWeaveService::from_parts(
        AppConfig::default(),
        SqliteStorage::open(&path).await.unwrap(),
    )
    .unwrap();
    let workspace = service
        .register_workspace(directory.path().to_string_lossy(), "receipts")
        .await
        .unwrap();
    let session_request = request(
        "session",
        NativeOperation::StartSession {
            workspace_id: workspace.id.clone(),
            metadata: json!({"b": 2, "a": 1}),
        },
    );
    let session_receipt = service
        .deliver_native(session_request.clone())
        .await
        .unwrap();
    let NativeRecord::Session(session) = &session_receipt.record else {
        panic!()
    };
    let task_request = request(
        "task",
        NativeOperation::StartTask {
            workspace_id: workspace.id.clone(),
            session_id: Some(session.id.clone()),
            title: "fixture".into(),
            details: json!({}),
        },
    );
    let task_receipt = service.deliver_native(task_request.clone()).await.unwrap();
    let NativeRecord::Task(task) = &task_receipt.record else {
        panic!()
    };
    let episode_request = request(
        "episode",
        NativeOperation::StartEpisode {
            request: EpisodeStartRequest {
                workspace_id: workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(task.id.clone()),
                episode_type: EpisodeType::Investigation,
                title: Some("fixture".into()),
                created_by: EpisodeCreator::NativeHarness,
            },
        },
    );
    let episode_receipt = service
        .deliver_native(episode_request.clone())
        .await
        .unwrap();
    let mut event = CortexEvent::new(
        &workspace.id,
        EventType::ExternalToolFinished,
        json!({"output": "observed"}),
    );
    event.session_id = Some(session.id.clone());
    event.task_id = Some(task.id.clone());
    let event_request = request("event", NativeOperation::RecordEvent { event });
    let event_receipt = service.deliver_native(event_request.clone()).await.unwrap();
    service.end_session(&session.id).await.unwrap();
    service.storage().pool().close().await;
    drop(service);

    let service = CortexWeaveService::from_parts(
        AppConfig::default(),
        SqliteStorage::open(&path).await.unwrap(),
    )
    .unwrap();
    for (req, receipt) in [
        (session_request.clone(), session_receipt),
        (task_request, task_receipt),
        (episode_request, episode_receipt),
        (event_request.clone(), event_receipt),
    ] {
        assert_eq!(service.deliver_native(req).await.unwrap(), receipt);
    }
    assert_eq!(
        service
            .recent_events(&workspace.id, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    let frontier: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM historical_write_order")
        .fetch_one(service.storage().pool())
        .await
        .unwrap();
    assert_eq!(frontier, 1);
    let mut changed = event_request;
    if let NativeOperation::RecordEvent { event } = &mut changed.operation {
        event.payload = json!({"output": "changed"});
    }
    assert!(matches!(
        service.deliver_native(changed).await,
        Err(CortexError::Conflict(_))
    ));
    let mut reordered = session_request;
    if let NativeOperation::StartSession { metadata, .. } = &mut reordered.operation {
        *metadata = serde_json::from_str("{\"a\":1,\"b\":2}").unwrap();
    }
    service.deliver_native(reordered).await.unwrap();
    service.storage().pool().close().await;
}

#[tokio::test]
async fn receipt_failure_rolls_back_domain_record_and_frontier() {
    let directory = tempdir().unwrap();
    let service = CortexWeaveService::from_parts(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
    )
    .unwrap();
    let workspace = service
        .register_workspace(directory.path().to_string_lossy(), "atomic")
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER inject_receipt_failure BEFORE INSERT ON native_delivery_receipts BEGIN SELECT RAISE(ABORT, 'injected failure'); END;")
        .execute(service.storage().pool()).await.unwrap();
    let session_request = request(
        "rollback",
        NativeOperation::StartSession {
            workspace_id: workspace.id.clone(),
            metadata: json!({}),
        },
    );
    assert!(
        service
            .deliver_native(session_request.clone())
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(service.storage().pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    let event_request = request(
        "event",
        NativeOperation::RecordEvent {
            event: CortexEvent::new(&workspace.id, EventType::ExternalToolFinished, json!({})),
        },
    );
    assert!(service.deliver_native(event_request.clone()).await.is_err());
    for table in [
        "events",
        "historical_write_order",
        "native_delivery_receipts",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(service.storage().pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} must roll back");
    }
    sqlx::query("DROP TRIGGER inject_receipt_failure")
        .execute(service.storage().pool())
        .await
        .unwrap();
    service.deliver_native(session_request).await.unwrap();
    service.deliver_native(event_request).await.unwrap();
}

#[tokio::test]
async fn concurrent_services_return_the_same_creation_receipt() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("native.sqlite");
    let a = CortexWeaveService::from_parts(
        AppConfig::default(),
        SqliteStorage::open(&path).await.unwrap(),
    )
    .unwrap();
    let b = CortexWeaveService::from_parts(
        AppConfig::default(),
        SqliteStorage::open(&path).await.unwrap(),
    )
    .unwrap();
    let workspace = a
        .register_workspace(directory.path().to_string_lossy(), "concurrent")
        .await
        .unwrap();
    let req = request(
        "same",
        NativeOperation::StartSession {
            workspace_id: workspace.id.clone(),
            metadata: json!({}),
        },
    );
    let (left, right) = tokio::join!(a.deliver_native(req.clone()), b.deliver_native(req));
    assert_eq!(left.unwrap(), right.unwrap());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
        .fetch_one(a.storage().pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    a.storage().pool().close().await;
    b.storage().pool().close().await;
}

#[tokio::test]
async fn invalid_owners_closed_sessions_and_key_changes_do_not_create_history() {
    let directory = tempdir().unwrap();
    let other = tempdir().unwrap();
    let service = CortexWeaveService::from_parts(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
    )
    .unwrap();
    let workspace = service
        .register_workspace(directory.path().to_string_lossy(), "a")
        .await
        .unwrap();
    let foreign = service
        .register_workspace(other.path().to_string_lossy(), "b")
        .await
        .unwrap();
    let session = service
        .start_session(&workspace.id, json!({}))
        .await
        .unwrap();
    let own_task = service
        .start_task(&workspace.id, Some(session.id.clone()), "own", json!({}))
        .await
        .unwrap();
    let sibling = service
        .start_session(&workspace.id, json!({}))
        .await
        .unwrap();
    for owner in [&foreign.id, &workspace.id] {
        let req = request(
            "bad-owner",
            NativeOperation::StartEpisode {
                request: EpisodeStartRequest {
                    workspace_id: owner.clone(),
                    session_id: sibling.id.clone(),
                    task_id: Some(own_task.id.clone()),
                    episode_type: EpisodeType::Debugging,
                    title: None,
                    created_by: EpisodeCreator::NativeHarness,
                },
            },
        );
        assert!(service.deliver_native(req).await.is_err());
    }
    let mut event = CortexEvent::new(&workspace.id, EventType::ExternalToolFinished, json!({}));
    event.session_id = Some(sibling.id.clone());
    event.task_id = Some(own_task.id);
    assert!(
        service
            .deliver_native(request("bad-event", NativeOperation::RecordEvent { event }))
            .await
            .is_err()
    );
    service.end_session(&session.id).await.unwrap();
    assert!(
        service
            .deliver_native(request(
                "closed",
                NativeOperation::StartTask {
                    workspace_id: workspace.id.clone(),
                    session_id: Some(session.id),
                    title: "closed".into(),
                    details: json!({})
                }
            ))
            .await
            .is_err()
    );
    let operation = NativeOperation::StartSession {
        workspace_id: workspace.id.clone(),
        metadata: json!({}),
    };
    for key in ["", " ", "nul\0", &"x".repeat(257)] {
        assert!(
            service
                .deliver_native(request(key, operation.clone()))
                .await
                .is_err()
        );
    }
    service
        .deliver_native(request("changed", operation.clone()))
        .await
        .unwrap();
    assert!(matches!(
        service
            .deliver_native(request(
                "changed",
                NativeOperation::StartSession {
                    workspace_id: workspace.id.clone(),
                    metadata: json!("different")
                }
            ))
            .await,
        Err(CortexError::Conflict(_))
    ));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM native_delivery_receipts")
        .fetch_one(service.storage().pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert!(
        sqlx::query("UPDATE native_delivery_receipts SET receipt_json = '{}' ")
            .execute(service.storage().pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM native_delivery_receipts")
            .execute(service.storage().pool())
            .await
            .is_err()
    );
}
