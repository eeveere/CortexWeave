use std::{fs, sync::Arc};

use async_trait::async_trait;
use cortexweave::{
    AppConfig, ContextSufficiencyAssessment, CortexWeaveService, HarnessContext,
    HarnessContextPolicy, HarnessContextRequest, HarnessHydrationRequest, HydrationAuthorization,
    HydrationScoreProvenance, Result,
    domain::{Checkpoint, ContextSourceType, CortexEvent, EventType, MemoryKind, MemoryRecord},
    embedding::EmbeddingProvider,
    storage::SqliteStorage,
};
use serde_json::json;
use tempfile::tempdir;

struct HarnessEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for HarnessEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| {
                if text.contains("ensure_ready") {
                    vec![1.0, 0.0]
                } else {
                    vec![0.0, 1.0]
                }
            })
            .collect())
    }

    fn model_name(&self) -> &str {
        "native-harness-contract"
    }

    fn dimension(&self) -> Option<usize> {
        Some(2)
    }
}

struct HydrateBestCode;

impl HarnessContextPolicy for HydrateBestCode {
    fn evaluate(&self, context: &HarnessContext) -> ContextSufficiencyAssessment {
        let source_id = context
            .selected_sources
            .iter()
            .find(|source| source.source_type == ContextSourceType::Code)
            .map(|source| source.source_id.clone())
            .into_iter()
            .collect();
        ContextSufficiencyAssessment {
            sufficient: false,
            selected_chunk_ids_to_hydrate: source_id,
            reason: "hydrate the selected implementation before editing".into(),
        }
    }
}

#[tokio::test]
async fn direct_harness_cycle_enforces_hydration_policy_and_keeps_provenance() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("process_manager.py"),
        "class ProcessManager:\n    def ensure_ready(self):\n        return self.running\n",
    )
    .unwrap();
    fs::write(
        root.join("unrelated.py"),
        format!(
            "def unrelated():\n    return {:?}\n",
            "background ".repeat(200)
        ),
    )
    .unwrap();
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
        Arc::new(HarnessEmbeddingProvider),
    )
    .unwrap();
    let workspace = service
        .register_workspace(root.to_string_lossy(), "native-harness")
        .await
        .unwrap();
    service.workspace_reindex(&workspace.id).await.unwrap();
    let session = service
        .start_session(&workspace.id, json!({ "client": "native-harness" }))
        .await
        .unwrap();
    let task = service
        .start_task(
            &workspace.id,
            Some(session.id.clone()),
            "verify process readiness",
            json!({}),
        )
        .await
        .unwrap();

    let context = service
        .prepare_harness_context(HarnessContextRequest::new(
            &workspace.id,
            &session.id,
            &task.id,
            "ProcessManager ensure_ready",
            160,
        ))
        .await
        .unwrap();
    assert_eq!(context.workspace, workspace);
    assert_eq!(context.session, session);
    assert_eq!(context.task, task);
    assert!(context.packet.explanation.is_some());
    assert!(
        context
            .selected_sources
            .iter()
            .all(|source| source.workspace_id == context.workspace.id)
    );
    let selected_code = context
        .selected_sources
        .iter()
        .find(|source| source.source_type == ContextSourceType::Code)
        .unwrap();
    assert_eq!(selected_code.path.as_deref(), Some("process_manager.py"));
    assert!(selected_code.symbol.is_some());
    assert_eq!(selected_code.source_segments.len(), 1);
    assert_eq!(
        selected_code.source_segments[0].source,
        "process_manager.py"
    );
    assert!(
        selected_code.source_segments[0].start_byte < selected_code.source_segments[0].end_byte
    );
    assert!(selected_code.scores.final_score > 0.0);

    let assessment = context.evaluate_with(&HydrateBestCode);
    assert!(!assessment.sufficient);
    assert_eq!(assessment.selected_chunk_ids_to_hydrate.len(), 1);
    let selected_hydration = service
        .hydrate_harness_context(HarnessHydrationRequest::from_context(
            &context,
            assessment.selected_chunk_ids_to_hydrate.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(selected_hydration.len(), 1);
    assert_eq!(
        selected_hydration[0].authorization,
        HydrationAuthorization::PacketSelection
    );
    assert!(matches!(
        selected_hydration[0].score_provenance,
        HydrationScoreProvenance::PacketSelection(_)
    ));
    assert_eq!(selected_hydration[0].source.workspace_id, workspace.id);
    assert_eq!(selected_hydration[0].source.path, "process_manager.py");
    assert!(selected_hydration[0].source.qualified_symbol.is_some());

    let unrelated_document = service
        .storage()
        .find_document(&workspace.id, "unrelated.py")
        .await
        .unwrap()
        .unwrap();
    let unrelated_chunk = service
        .storage()
        .list_chunks(&unrelated_document.id)
        .await
        .unwrap()
        .into_iter()
        .find(|chunk| {
            !context
                .packet
                .items
                .iter()
                .any(|item| item.source_id == chunk.id)
        })
        .unwrap();
    let denied = service
        .hydrate_harness_context(HarnessHydrationRequest::from_context(
            &context,
            vec![unrelated_chunk.id.clone()],
        ))
        .await
        .unwrap_err();
    assert!(denied.to_string().contains("not authorized"));

    let override_reason = "inspect a caller-approved neighboring implementation";
    let overridden = service
        .hydrate_harness_context(
            HarnessHydrationRequest::from_context(&context, vec![unrelated_chunk.id.clone()])
                .with_override_reason(override_reason),
        )
        .await
        .unwrap();
    assert_eq!(overridden[0].source.workspace_id, workspace.id);
    assert_eq!(overridden[0].source.path, "unrelated.py");
    assert!(matches!(
        overridden[0].score_provenance,
        HydrationScoreProvenance::OutOfPacketNotScored
    ));
    let audit_event_id = match &overridden[0].authorization {
        HydrationAuthorization::HarnessOverride {
            reason,
            audit_event_id,
        } => {
            assert_eq!(reason, override_reason);
            audit_event_id.clone()
        }
        HydrationAuthorization::PacketSelection => panic!("override authorization expected"),
    };

    for event_type in [
        EventType::ExternalToolFinished,
        EventType::CompilerResult,
        EventType::TestResult,
    ] {
        let mut event = CortexEvent::new(&workspace.id, event_type, json!({ "ok": true }));
        event.session_id = Some(session.id.clone());
        event.task_id = Some(task.id.clone());
        service.record_event(event).await.unwrap();
    }
    service
        .activate_context_source(
            &workspace.id,
            &session.id,
            Some(&task.id),
            &selected_hydration[0].source.chunk_id,
            ContextSourceType::Code,
        )
        .await
        .unwrap();
    let mut decision = MemoryRecord::new(
        &workspace.id,
        MemoryKind::Decision,
        "Keep readiness checks inside ProcessManager.",
    );
    decision.session_id = Some(session.id.clone());
    decision.task_id = Some(task.id.clone());
    decision.related_paths = vec!["process_manager.py".into()];
    let decision = service.record_memory(decision).await.unwrap();
    let mut checkpoint = Checkpoint::new(&workspace.id, &session.id, "verified readiness flow");
    checkpoint.task_id = Some(task.id.clone());
    checkpoint.decision_ids = vec![decision.id];
    checkpoint.related_paths = vec!["process_manager.py".into()];
    service.create_checkpoint(checkpoint).await.unwrap();

    let events = service.recent_events(&workspace.id, 10).await.unwrap();
    let override_event = events
        .iter()
        .find(|event| event.id == audit_event_id)
        .unwrap();
    assert_eq!(
        override_event.event_type,
        EventType::ContextHydrationOverride
    );
    assert_eq!(
        override_event.session_id.as_deref(),
        Some(session.id.as_str())
    );
    assert_eq!(override_event.task_id.as_deref(), Some(task.id.as_str()));
    assert_eq!(override_event.payload["reason"], override_reason);
    assert_eq!(override_event.payload["sources"][0]["path"], "unrelated.py");
    assert_eq!(
        override_event.payload["sources"][0]["score_provenance"],
        "out_of_packet_not_scored"
    );
}
