use std::{collections::BTreeMap, fs, sync::Arc};

use async_trait::async_trait;
use cortexweave::{
    AppConfig, CortexWeaveService, Result,
    domain::{
        ConsolidationAcceptance, ConsolidationAcceptanceRequest, ConsolidationNoResultReason,
        ConsolidationPreview, ConsolidationRequest, ContextRequest, ContextSourceType, CortexEvent,
        EpisodeCreator, EpisodeEventAssociationRequest, EpisodeStartRequest,
        EpisodeTerminalRequest, EpisodeType, EventType, EvidenceDecodeResult,
        ExperienceSearchRequest, FailureNormalizationResult,
    },
    embedding::EmbeddingProvider,
    evaluation::{ContextEvaluationCase, evaluate_context_packets},
    storage::SqliteStorage,
};
use serde_json::json;
use tempfile::tempdir;

struct OfflineEvaluationEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for OfflineEvaluationEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| {
                if text.contains("E0308") {
                    vec![1.0, 0.0]
                } else {
                    vec![0.0, 1.0]
                }
            })
            .collect())
    }

    fn model_name(&self) -> &str {
        "offline-experience-evaluation"
    }

    fn dimension(&self) -> Option<usize> {
        Some(2)
    }
}

fn compiler_failure(workspace_id: &str, code: &str) -> CortexEvent {
    CortexEvent::new(
        workspace_id,
        EventType::CompilerResult,
        json!({
            "contract": "cortexweave.rust_compiler_result",
            "version": 1,
            "subject": {"kind": "target", "value": "core"},
            "exit_code": 1,
            "diagnostics": [{
                "level": "error",
                "code": code,
                "message": "typed evaluation failure",
                "expected_type": "String",
                "actual_type": "u32",
                "path": "src/lib.rs",
                "start_line": 4,
                "start_column": 2
            }]
        }),
    )
}

fn compiler_pass(workspace_id: &str) -> CortexEvent {
    CortexEvent::new(
        workspace_id,
        EventType::CompilerResult,
        json!({
            "contract": "cortexweave.rust_compiler_result",
            "version": 1,
            "subject": {"kind": "target", "value": "core"},
            "exit_code": 0,
            "diagnostics": []
        }),
    )
}

fn change(workspace_id: &str, operation: &str) -> CortexEvent {
    CortexEvent::new(
        workspace_id,
        EventType::ExternalToolFinished,
        json!({
            "contract": "cortexweave.external_tool_completion",
            "version": 1,
            "tool": "editor",
            "operation": operation,
            "exit_code": 0,
            "error_class": null,
            "message": null
        }),
    )
}

struct EpisodeFixture {
    session_id: String,
    episode_id: String,
    terminal_version: u64,
    events: Vec<CortexEvent>,
}

async fn closed_episode(
    service: &CortexWeaveService,
    workspace_id: &str,
    label: &str,
    mut events: Vec<CortexEvent>,
) -> EpisodeFixture {
    let session = service
        .start_session(workspace_id, json!({"evaluation": label}))
        .await
        .unwrap();
    let task = service
        .start_task(
            workspace_id,
            Some(session.id.clone()),
            format!("evaluate {label}"),
            json!({}),
        )
        .await
        .unwrap();
    let episode = service
        .start_episode(EpisodeStartRequest {
            workspace_id: workspace_id.into(),
            session_id: session.id.clone(),
            task_id: Some(task.id.clone()),
            episode_type: EpisodeType::Debugging,
            title: Some(label.into()),
            created_by: EpisodeCreator::User,
        })
        .await
        .unwrap();
    for event in &mut events {
        event.session_id = Some(session.id.clone());
        event.task_id = Some(task.id.clone());
        *event = service.record_event(event.clone()).await.unwrap();
    }
    let associated = service
        .add_episode_events(EpisodeEventAssociationRequest {
            workspace_id: workspace_id.into(),
            episode_id: episode.id.clone(),
            expected_version: episode.version,
            request_key: format!("{label}-events"),
            event_ids: events.iter().map(|event| event.id.clone()).collect(),
        })
        .await
        .unwrap();
    let closed = service
        .close_episode(EpisodeTerminalRequest {
            workspace_id: workspace_id.into(),
            episode_id: episode.id.clone(),
            expected_version: associated.version,
            request_key: format!("{label}-close"),
        })
        .await
        .unwrap();
    EpisodeFixture {
        session_id: session.id,
        episode_id: episode.id,
        terminal_version: closed.version,
        events,
    }
}

async fn accepted_fixture(
    service: &CortexWeaveService,
    workspace_id: &str,
) -> (EpisodeFixture, cortexweave::domain::ExperienceRecord) {
    let fixture = closed_episode(
        service,
        workspace_id,
        "same-failure-two-attempts",
        vec![
            compiler_failure(workspace_id, "E0308"),
            change(workspace_id, "first-attempt"),
            compiler_failure(workspace_id, "E0308"),
            change(workspace_id, "second-attempt"),
            compiler_pass(workspace_id),
        ],
    )
    .await;
    let request = ConsolidationRequest {
        workspace_id: workspace_id.into(),
        episode_id: fixture.episode_id.clone(),
        expected_episode_version: fixture.terminal_version,
    };
    let ConsolidationPreview::Proposal { proposal, .. } =
        service.preview_experience(&request).await.unwrap()
    else {
        panic!("the deterministic successful fixture must produce a proposal");
    };
    let ConsolidationAcceptance::Accepted { record } = service
        .accept_experience(&ConsolidationAcceptanceRequest {
            request,
            expected_fingerprint: proposal.fingerprint,
            expected_proposal_hash: proposal.proposal_hash,
        })
        .await
        .unwrap()
    else {
        panic!("the deterministic successful fixture must accept");
    };
    (fixture, *record)
}

fn normalized_key(service: &CortexWeaveService, event: &CortexEvent) -> String {
    let decoded = service.diagnose_event_evidence(event);
    let EvidenceDecodeResult::Decoded { evidence } = decoded else {
        panic!("typed compiler fixture must decode: {decoded:?}");
    };
    let FailureNormalizationResult::Normalized { normalization } =
        service.normalize_decoded_failure(&evidence)
    else {
        panic!("typed compiler fixture must normalize");
    };
    normalization.signature.normalized_key
}

#[tokio::test]
async fn offline_harness_evaluates_typed_experience_scenarios() {
    let directory = tempdir().unwrap();
    let root_a = directory.path().join("workspace-a");
    let root_b = directory.path().join("workspace-b");
    fs::create_dir_all(&root_a).unwrap();
    fs::create_dir_all(&root_b).unwrap();
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
        Arc::new(OfflineEvaluationEmbeddingProvider),
    )
    .unwrap();
    let workspace_a = service
        .register_workspace(root_a.to_string_lossy(), "evaluation-a")
        .await
        .unwrap();
    let workspace_b = service
        .register_workspace(root_b.to_string_lossy(), "evaluation-b")
        .await
        .unwrap();
    let signature_session = service
        .start_session(&workspace_a.id, json!({"evaluation": "signature-pairs"}))
        .await
        .unwrap();

    let pairs = [
        (
            "same",
            compiler_failure(&workspace_a.id, "E0308"),
            compiler_failure(&workspace_a.id, "E0308"),
            true,
        ),
        (
            "different",
            compiler_failure(&workspace_a.id, "E0308"),
            compiler_failure(&workspace_a.id, "E0425"),
            false,
        ),
    ];
    for (label, mut left, mut right, expected_same) in pairs {
        left.session_id = Some(signature_session.id.clone());
        right.session_id = Some(signature_session.id.clone());
        assert_eq!(
            normalized_key(&service, &left) == normalized_key(&service, &right),
            expected_same,
            "{label} pair label"
        );
    }

    let (fixture, record) = accepted_fixture(&service, &workspace_a.id).await;
    assert_eq!(record.attempts.len(), 2);
    let signature = record.experience.failure_signature.clone().unwrap();
    let experience_id = record.experience.id.clone();

    let relevant = service
        .search_experiences(&ExperienceSearchRequest {
            workspace_id: workspace_a.id.clone(),
            query: None,
            exact_failure_signature: Some(signature.clone()),
            compatible_components: BTreeMap::new(),
            path: None,
            graph_stable_key: None,
            outcomes: Vec::new(),
            strengths: Vec::new(),
            lifecycles: Vec::new(),
            include_historical: false,
            created_after: None,
            created_before: None,
            limit: 3,
        })
        .await
        .unwrap();
    assert_eq!(
        relevant
            .iter()
            .map(|hit| &hit.experience.id)
            .collect::<Vec<_>>(),
        vec![&experience_id]
    );
    let irrelevant_workspace = service
        .search_experiences(&ExperienceSearchRequest {
            workspace_id: workspace_b.id.clone(),
            query: Some("typed evaluation failure".into()),
            exact_failure_signature: None,
            compatible_components: BTreeMap::new(),
            path: None,
            graph_stable_key: None,
            outcomes: Vec::new(),
            strengths: Vec::new(),
            lifecycles: Vec::new(),
            include_historical: false,
            created_after: None,
            created_before: None,
            limit: 3,
        })
        .await
        .unwrap();
    assert!(
        irrelevant_workspace.is_empty(),
        "workspace isolation is a relevance control"
    );

    let ambiguous = closed_episode(
        &service,
        &workspace_a.id,
        "conflicting-terminal-results",
        vec![
            compiler_failure(&workspace_a.id, "E0308"),
            change(&workspace_a.id, "ambiguous-attempt"),
            compiler_pass(&workspace_a.id),
            compiler_failure(&workspace_a.id, "E0308"),
        ],
    )
    .await;
    let ambiguous_preview = service
        .preview_experience(&ConsolidationRequest {
            workspace_id: workspace_a.id.clone(),
            episode_id: ambiguous.episode_id,
            expected_episode_version: ambiguous.terminal_version,
        })
        .await
        .unwrap();
    assert!(matches!(
        ambiguous_preview,
        ConsolidationPreview::NoResult {
            reason: ConsolidationNoResultReason::ConflictingResults,
            ..
        }
    ));

    service.end_session(&fixture.session_id).await.unwrap();
    let session_b = service
        .start_session(&workspace_a.id, json!({"evaluation": "fresh-task"}))
        .await
        .unwrap();
    let task_b = service
        .start_task(
            &workspace_a.id,
            Some(session_b.id.clone()),
            "evaluate prior repair",
            json!({}),
        )
        .await
        .unwrap();
    let mut current = compiler_failure(&workspace_a.id, "E0308");
    current.session_id = Some(session_b.id.clone());
    current.task_id = Some(task_b.id.clone());
    let current = service.record_event(current).await.unwrap();
    service
        .activate_context_source(
            &workspace_a.id,
            &session_b.id,
            Some(&task_b.id),
            &current.id,
            ContextSourceType::Event,
        )
        .await
        .unwrap();
    let mut request = ContextRequest::new(&workspace_a.id);
    request.session_id = Some(session_b.id.clone());
    request.task_id = Some(task_b.id.clone());
    request.active_failure_signature = Some(signature);
    request.include_code = false;
    request.include_documents = false;
    request.include_memories = false;
    request.include_events = false;
    request.token_budget = 2_048;
    let packet = service.semantic_context(request).await.unwrap();
    let expected = vec![task_b.id.clone(), current.id.clone(), experience_id.clone()];
    assert!(
        expected
            .iter()
            .all(|id| packet.items.iter().any(|item| &item.source_id == id))
    );
    assert!(packet.items.iter().all(|item| {
        !fixture
            .events
            .iter()
            .any(|event| event.id == item.source_id)
    }));
    let metrics = evaluate_context_packets(&[ContextEvaluationCase {
        id: "paired-current-task-and-history".into(),
        packet: packet.clone(),
        expected_source_ids: expected,
        expected_current_paths: Vec::new(),
        expected_resume_task_id: None,
        selected_resume_task_id: None,
        selection_latency: std::time::Duration::ZERO,
    }]);
    assert_eq!(metrics.context_recall, 1.0);
    assert!(packet.estimated_tokens <= packet.token_budget);

    let repeated = service
        .accept_experience(&ConsolidationAcceptanceRequest {
            request: ConsolidationRequest {
                workspace_id: workspace_a.id.clone(),
                episode_id: fixture.episode_id,
                expected_episode_version: fixture.terminal_version,
            },
            expected_fingerprint: record.experience.consolidation_fingerprint.clone(),
            expected_proposal_hash: record.experience.proposal_hash.clone(),
        })
        .await
        .unwrap();
    let ConsolidationAcceptance::Accepted { record: repeated } = repeated else {
        panic!("repeated consolidation must return the stored immutable record");
    };
    assert_eq!(repeated.experience.id, experience_id);
}

#[tokio::test]
async fn offline_harness_reconciles_rename_delete_and_stale_graph_controls() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("workspace");
    let source = root.join("src/lib.rs");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "pub fn original() {}\n").unwrap();
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
        Arc::new(OfflineEvaluationEmbeddingProvider),
    )
    .unwrap();
    let workspace = service
        .register_workspace(root.to_string_lossy(), "evaluation-filesystem")
        .await
        .unwrap();
    service.workspace_reindex(&workspace.id).await.unwrap();
    assert!(
        service
            .workspace_graph_status(&workspace.id)
            .await
            .unwrap()
            .is_current
    );

    let renamed = root.join("src/renamed.rs");
    fs::rename(&source, &renamed).unwrap();
    sqlx::query(
        "UPDATE workspace_graph_revisions SET graph_state = 'stale' WHERE workspace_id = ?",
    )
    .bind(&workspace.id)
    .execute(service.storage().pool())
    .await
    .unwrap();
    assert!(
        !service
            .workspace_graph_status(&workspace.id)
            .await
            .unwrap()
            .is_current
    );
    service.workspace_reindex(&workspace.id).await.unwrap();
    assert!(
        service
            .workspace_graph_status(&workspace.id)
            .await
            .unwrap()
            .is_current
    );
    assert_eq!(
        service
            .storage()
            .list_documents(&workspace.id)
            .await
            .unwrap()[0]
            .relative_path,
        "src/renamed.rs"
    );

    fs::remove_file(renamed).unwrap();
    service.workspace_reindex(&workspace.id).await.unwrap();
    assert!(
        service
            .storage()
            .list_documents(&workspace.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        service
            .workspace_graph_status(&workspace.id)
            .await
            .unwrap()
            .is_current
    );
}
