use std::{collections::BTreeMap, fs, sync::Arc};

use async_trait::async_trait;
use cortexweave::{
    AppConfig, CortexWeaveService, Result,
    domain::{
        AttemptResult, ConsolidationAcceptance, ConsolidationAcceptanceRequest,
        ConsolidationPreview, ConsolidationRequest, ContextRequest, ContextSourceType, CortexEvent,
        EpisodeCreator, EpisodeEventAssociationRequest, EpisodeStartRequest,
        EpisodeTerminalRequest, EpisodeType, EventType, EvidenceBasis, EvidenceDecodeResult,
        ExperienceContextAuthority, ExperienceEvidenceRelation, ExperienceLifecycle,
        ExperienceOutcome, ExperienceSearchRequest, FailureNormalizationResult, TaskStatus,
        VerificationKind, VerificationStatus,
    },
    embedding::EmbeddingProvider,
    storage::SqliteStorage,
};
use serde_json::json;
use tempfile::tempdir;

struct CycleEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for CycleEmbeddingProvider {
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
        "native-experience-full-cycle"
    }

    fn dimension(&self) -> Option<usize> {
        Some(2)
    }
}

fn compiler_failure(workspace_id: &str) -> CortexEvent {
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
                "code": "E0308",
                "message": "structured compiler failure",
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

fn tool_change(workspace_id: &str, operation: &str) -> CortexEvent {
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

fn normalized_failure_key(service: &CortexWeaveService, event: &CortexEvent) -> String {
    let EvidenceDecodeResult::Decoded { evidence } = service.diagnose_event_evidence(event) else {
        panic!("typed current failure must decode");
    };
    let FailureNormalizationResult::Normalized { normalization } =
        service.normalize_decoded_failure(&evidence)
    else {
        panic!("typed current failure must normalize");
    };
    normalization.signature.normalized_key
}

#[tokio::test]
async fn native_service_full_cycle_uses_persisted_experience_without_transcript_or_adapter() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("workspace");
    let source = root.join("src/lib.rs");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(
        &source,
        "// Current E0308: expected String, observed u32.\npub fn current_source() {}\n",
    )
    .unwrap();
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
        Arc::new(CycleEmbeddingProvider),
    )
    .unwrap();
    let workspace = service
        .register_workspace(root.to_string_lossy(), "native-experience-cycle")
        .await
        .unwrap();
    service.workspace_reindex(&workspace.id).await.unwrap();
    let other_root = directory.path().join("other-workspace");
    fs::create_dir_all(&other_root).unwrap();
    let other_workspace = service
        .register_workspace(other_root.to_string_lossy(), "native-experience-control")
        .await
        .unwrap();
    let session_a = service
        .start_session(&workspace.id, json!({"phase": "initial repair"}))
        .await
        .unwrap();
    let task_a = service
        .start_task(
            &workspace.id,
            Some(session_a.id.clone()),
            "repair structured compiler mismatch",
            json!({}),
        )
        .await
        .unwrap();
    assert_eq!(task_a.status, TaskStatus::Active);
    let episode = service
        .start_episode(EpisodeStartRequest {
            workspace_id: workspace.id.clone(),
            session_id: session_a.id.clone(),
            task_id: Some(task_a.id.clone()),
            episode_type: EpisodeType::Debugging,
            title: Some("E0308 repair".into()),
            created_by: EpisodeCreator::User,
        })
        .await
        .unwrap();

    let mut events = vec![
        compiler_failure(&workspace.id),
        tool_change(&workspace.id, "edit-initial-attempt"),
        compiler_failure(&workspace.id),
        tool_change(&workspace.id, "edit-follow-up-attempt"),
        compiler_pass(&workspace.id),
    ];
    for event in &mut events {
        event.session_id = Some(session_a.id.clone());
        event.task_id = Some(task_a.id.clone());
        *event = service.record_event(event.clone()).await.unwrap();
    }
    let associated = service
        .add_episode_events(EpisodeEventAssociationRequest {
            workspace_id: workspace.id.clone(),
            episode_id: episode.id.clone(),
            expected_version: episode.version,
            request_key: "native-full-cycle-events".into(),
            event_ids: events.iter().map(|event| event.id.clone()).collect(),
        })
        .await
        .unwrap();
    let closed = service
        .close_episode(EpisodeTerminalRequest {
            workspace_id: workspace.id.clone(),
            episode_id: episode.id,
            expected_version: associated.version,
            request_key: "native-full-cycle-close".into(),
        })
        .await
        .unwrap();
    let request = ConsolidationRequest {
        workspace_id: workspace.id.clone(),
        episode_id: closed.id.clone(),
        expected_episode_version: closed.version,
    };
    let ConsolidationPreview::Proposal {
        proposal,
        disposition,
    } = service.preview_experience(&request).await.unwrap()
    else {
        panic!("typed failure, failed verification, follow-up change, and pass must consolidate");
    };
    assert!(matches!(
        disposition,
        cortexweave::domain::ProposalDisposition::Automatic
    ));
    let ConsolidationAcceptance::Accepted { record } = service
        .accept_experience(&ConsolidationAcceptanceRequest {
            request: request.clone(),
            expected_fingerprint: proposal.fingerprint,
            expected_proposal_hash: proposal.proposal_hash,
        })
        .await
        .unwrap()
    else {
        panic!("automatic proposal must accept");
    };
    let record = *record;
    let experience_id = record.experience.id.clone();
    let signature = record.experience.failure_signature.clone().unwrap();
    assert_eq!(record.experience.workspace_id, workspace.id);
    assert_eq!(record.experience.session_id, session_a.id);
    assert_eq!(
        record.experience.task_id.as_deref(),
        Some(task_a.id.as_str())
    );
    assert_eq!(record.experience.episode_id, closed.id);
    assert!(events.iter().all(|event| {
        event.workspace_id == workspace.id
            && event.session_id.as_deref() == Some(session_a.id.as_str())
            && event.task_id.as_deref() == Some(task_a.id.as_str())
    }));
    assert_eq!(record.experience.outcome, ExperienceOutcome::Success);
    assert_eq!(record.attempts.len(), 2);
    assert_eq!(record.attempts[0].result, AttemptResult::StillFailing);
    assert_eq!(record.attempts[1].result, AttemptResult::VerificationPassed);
    assert!(
        record
            .attempts
            .iter()
            .all(|attempt| attempt.following_verification_ordinal.is_some())
    );
    assert_eq!(record.evidence.len(), events.len());
    assert_eq!(
        record.experience.verification.status,
        VerificationStatus::VerifiedPassed
    );
    assert!(
        record
            .experience
            .verification
            .observations
            .iter()
            .any(
                |observation| observation.kind == VerificationKind::RustCompiler
                    && observation.status == VerificationStatus::VerifiedPassed
            )
    );
    assert!(
        record
            .experience
            .evidence_strength
            .bases
            .contains(&EvidenceBasis::DeterministicVerifier)
    );
    assert!(record.evidence.iter().any(|evidence| {
        matches!(
            evidence.relation,
            ExperienceEvidenceRelation::AttemptVerification
                | ExperienceEvidenceRelation::TerminalVerification
        ) && evidence.event_id == events[4].id
    }));
    assert!(
        record
            .experience
            .summary
            .contains("verified-after relationship")
    );
    assert!(record.experience.summary.contains("neither a dependency"));
    for unsupported in ["fixed by", "resolved by", "caused by", "proven current"] {
        assert!(!record.experience.summary.contains(unsupported));
    }

    let explanation = service
        .experience_get(&workspace.id, &experience_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(explanation.record, record);
    assert_eq!(explanation.lifecycle, ExperienceLifecycle::Active);
    assert!(explanation.normal_context.eligible);
    service.end_session(&session_a.id).await.unwrap();

    // This session starts with no Session A transcript. Its only active event
    // is a fresh typed failure recorded below; the Experience arrives through
    // persisted, bounded historical search rather than session carry-over.
    let session_b = service
        .start_session(&workspace.id, json!({"phase": "fresh investigation"}))
        .await
        .unwrap();
    let task_b = service
        .start_task(
            &workspace.id,
            Some(session_b.id.clone()),
            "investigate current compiler mismatch",
            json!({}),
        )
        .await
        .unwrap();
    let mut current_failure = compiler_failure(&workspace.id);
    current_failure.session_id = Some(session_b.id.clone());
    current_failure.task_id = Some(task_b.id.clone());
    let current_failure = service.record_event(current_failure).await.unwrap();
    assert_eq!(
        normalized_failure_key(&service, &current_failure),
        signature.normalized_key
    );
    service
        .activate_context_source(
            &workspace.id,
            &session_b.id,
            Some(&task_b.id),
            &current_failure.id,
            ContextSourceType::Event,
        )
        .await
        .unwrap();

    let hits = service
        .search_experiences(&ExperienceSearchRequest {
            workspace_id: workspace.id.clone(),
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
            limit: 10,
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].experience.id, experience_id);
    assert!(hits[0].explanation.scores.exact_signature > 0);
    assert!(hits[0].explanation.normal_context.eligible);

    let cross_workspace = service
        .search_experiences(&ExperienceSearchRequest {
            workspace_id: other_workspace.id,
            query: Some("E0308 expected String observed u32".into()),
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
            limit: 10,
        })
        .await
        .unwrap();
    assert!(cross_workspace.is_empty());

    let mut context_request = ContextRequest::new(&workspace.id);
    context_request.session_id = Some(session_b.id.clone());
    context_request.task_id = Some(task_b.id.clone());
    context_request.query = Some("E0308 expected String observed u32 in src/lib.rs".into());
    context_request.active_failure_signature = Some(signature);
    context_request.include_code = true;
    context_request.include_documents = false;
    context_request.include_memories = false;
    context_request.include_events = false;
    context_request.include_explanation = true;
    context_request.token_budget = 2_048;
    let packet = service.semantic_context(context_request).await.unwrap();
    assert_eq!(packet.workspace_id, workspace.id);
    assert_eq!(packet.session_id.as_deref(), Some(session_b.id.as_str()));
    assert_eq!(packet.task_id.as_deref(), Some(task_b.id.as_str()));
    assert!(packet.estimated_tokens <= packet.token_budget);
    assert!(
        packet
            .items
            .iter()
            .all(|item| !events.iter().any(|event| event.id == item.source_id)),
        "Session A events do not leak into Session B as a transcript"
    );
    for session_a_only_text in ["initial repair", "repair structured compiler mismatch"] {
        assert!(
            packet
                .items
                .iter()
                .all(|item| !item.content.contains(session_a_only_text)),
            "Session A transcript-only text must be absent"
        );
    }
    let task_index = packet
        .items
        .iter()
        .position(|item| {
            item.source_type == ContextSourceType::TaskState && item.source_id == task_b.id
        })
        .expect("current task state is authoritative context");
    let active_failure_index = packet
        .items
        .iter()
        .position(|item| {
            item.source_type == ContextSourceType::Event && item.source_id == current_failure.id
        })
        .expect("fresh current failure remains in context");
    let experience_index = packet
        .items
        .iter()
        .position(|item| {
            item.source_type == ContextSourceType::Experience && item.source_id == experience_id
        })
        .expect("bounded matching Experience supplements current evidence");
    let source_index = packet
        .items
        .iter()
        .position(|item| {
            item.source_type == ContextSourceType::Code
                && item.path.as_deref() == Some("src/lib.rs")
        })
        .expect("current indexed source remains selected");
    assert!(task_index < experience_index);
    assert!(active_failure_index < experience_index);
    assert!(source_index < experience_index);
    assert!(
        packet.items[experience_index].estimated_tokens
            <= service.config().context.experience.token_budget
    );
    let experience = &packet.explanation.unwrap().experience;
    assert_eq!(
        experience.selected_experience_ids,
        vec![experience_id.clone()]
    );
    assert_eq!(experience.selections.len(), 1);
    assert_eq!(
        experience.selections[0].authority,
        ExperienceContextAuthority::HistoricalSupplemental
    );
    assert_eq!(
        experience.selections[0].lifecycle,
        ExperienceLifecycle::Active
    );

    let repeated = service
        .accept_experience(&ConsolidationAcceptanceRequest {
            request,
            expected_fingerprint: record.experience.consolidation_fingerprint.clone(),
            expected_proposal_hash: record.experience.proposal_hash.clone(),
        })
        .await
        .unwrap();
    let ConsolidationAcceptance::Accepted { record: repeated } = repeated else {
        panic!("repeating an accepted consolidation must return the same experience");
    };
    assert_eq!(repeated.experience.id, experience_id);
}
