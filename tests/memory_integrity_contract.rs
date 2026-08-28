use chrono::{Duration, Utc};
use cortexweave::{
    AppConfig, CortexWeaveService, MemoryConsolidationRequest, MemorySupersessionReviewRequest,
    MemoryTrustReviewRequest,
    domain::{
        ContextSourceType, MemoryClaim, MemoryKind, MemoryRecord, MemoryTrust, SourceSegment,
        TemporalQuery, Workspace,
    },
    storage::SqliteStorage,
};
use serde_json::json;

#[tokio::test]
async fn imported_memory_requires_review_before_context_and_explicit_supersession() {
    let storage = SqliteStorage::in_memory().await.unwrap();
    let workspace = Workspace::new("C:/memory-integrity", "memory-integrity");
    storage.insert_workspace(&workspace).await.unwrap();
    let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();

    let mut older = MemoryRecord::new(
        &workspace.id,
        MemoryKind::Decision,
        "Use L2 distance for nearest-neighbor ranking.",
    );
    older.id = "older-metric".into();
    older.claim = Some(MemoryClaim::new("embedding.metric", json!("l2")));
    older.source_segments = vec![SourceSegment::new("docs/retrieval.md", 0, 100)];
    older.created_at = Utc::now() - Duration::hours(1);
    let older = service.record_memory(older).await.unwrap();

    let mut imported = MemoryRecord::imported(
        &workspace.id,
        MemoryKind::Decision,
        "Use cosine distance for nearest-neighbor ranking.",
        vec![SourceSegment::new("docs/retrieval.md", 10, 100)],
    );
    imported.id = "imported-metric".into();
    imported.claim = Some(MemoryClaim::new("embedding.metric", json!("cosine")));
    let imported = service.record_memory(imported).await.unwrap();

    let visible_for_review = service
        .search_memories(&workspace.id, "cosine", 10)
        .await
        .unwrap();
    assert_eq!(visible_for_review, vec![imported.clone()]);
    assert_eq!(visible_for_review[0].trust, MemoryTrust::Unreviewed);

    let mut memory_query = TemporalQuery::new(&workspace.id);
    memory_query.source_types = vec![ContextSourceType::Memory];
    let before_review = service
        .temporal_retrieval(memory_query.clone())
        .await
        .unwrap();
    assert_eq!(
        before_review
            .iter()
            .map(|item| item.source_id.as_str())
            .collect::<Vec<_>>(),
        vec![older.id.as_str()]
    );

    let untrusted_report = service
        .consolidate_memories(MemoryConsolidationRequest::new(
            &workspace.id,
            [older.id.clone(), imported.id.clone()],
        ))
        .await
        .unwrap();
    assert!(untrusted_report.proposals[0].contradiction.is_some());
    assert!(untrusted_report.proposals[0].source_overlap_ratio > 0.8);
    assert!(
        untrusted_report.proposals[0]
            .proposed_supersession
            .is_none()
    );

    let review = service
        .review_memory_trust(MemoryTrustReviewRequest {
            workspace_id: workspace.id.clone(),
            memory_id: imported.id.clone(),
            new_trust: MemoryTrust::Trusted,
            reviewed_by: "maintainer".into(),
            reason: "Confirmed against the current retrieval design.".into(),
        })
        .await
        .unwrap();
    assert_eq!(review.previous_trust, MemoryTrust::Unreviewed);
    assert_eq!(review.new_trust, MemoryTrust::Trusted);
    assert_eq!(
        service
            .memory_trust_reviews(&workspace.id, &imported.id)
            .await
            .unwrap(),
        vec![review]
    );

    let trusted_report = service
        .consolidate_memories(MemoryConsolidationRequest::new(
            &workspace.id,
            [older.id.clone(), imported.id.clone()],
        ))
        .await
        .unwrap();
    let proposal = &trusted_report.proposals[0];
    assert_eq!(
        proposal.contradiction.as_ref().unwrap().claim_key,
        "embedding.metric"
    );
    let proposed_supersession = proposal.proposed_supersession.as_ref().unwrap();
    assert_eq!(proposed_supersession.superseded_memory_id, older.id);
    assert_eq!(proposed_supersession.superseding_memory_id, imported.id);

    let before_application = service
        .temporal_retrieval(memory_query.clone())
        .await
        .unwrap();
    assert_eq!(before_application.len(), 2);

    let applied = service
        .apply_memory_supersession(MemorySupersessionReviewRequest {
            workspace_id: workspace.id.clone(),
            superseded_memory_id: older.id.clone(),
            superseding_memory_id: imported.id.clone(),
            reviewed_by: "maintainer".into(),
            reason: "Accepted the structured metric contradiction.".into(),
        })
        .await
        .unwrap();
    assert_eq!(applied.reviewed_by.as_deref(), Some("maintainer"));

    let after_application = service.temporal_retrieval(memory_query).await.unwrap();
    assert_eq!(after_application.len(), 1);
    assert_eq!(after_application[0].source_id, imported.id);
}
