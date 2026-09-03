use std::{collections::BTreeSet, sync::Arc};

use chrono::Utc;
use uuid::Uuid;

use crate::{
    CortexError, Result,
    domain::{
        ExperienceAssessment, ExperienceAssessmentKind, ExperienceAssessmentReviewRequest,
        ExperienceDisputeProposal, ExperienceDisputeProposalRequest, ExperienceLifecycle,
        ExperienceOutcome, ExperienceSearchRequest, FailureNormalizationResult, VerificationStatus,
    },
    storage::SqliteStorage,
};

use super::{EvidenceService, ExperienceSearchService, FailureNormalizationService};

/// Owns the reviewed eligibility transition for immutable Experiences.
///
/// It intentionally separates a deterministic recurrence observation from the
/// reviewed write that can change lifecycle. SQLite remains the transaction
/// owner and enforces append-only history and supersession-cycle safety.
pub struct ExperienceAssessmentService {
    storage: Arc<SqliteStorage>,
    evidence: Arc<EvidenceService>,
    failure_normalization: Arc<FailureNormalizationService>,
    search: Arc<ExperienceSearchService>,
}

impl ExperienceAssessmentService {
    pub fn new(
        storage: Arc<SqliteStorage>,
        evidence: Arc<EvidenceService>,
        failure_normalization: Arc<FailureNormalizationService>,
        search: Arc<ExperienceSearchService>,
    ) -> Self {
        Self {
            storage,
            evidence,
            failure_normalization,
            search,
        }
    }

    /// Applies one explicit reviewed operation. Automated analysis must use
    /// `propose_disputes` and cannot call this without a reviewer, reason, and
    /// immutable evidence references.
    pub async fn review(
        &self,
        request: ExperienceAssessmentReviewRequest,
    ) -> Result<ExperienceAssessment> {
        validate_review_request(&request)?;
        if self
            .storage
            .experience(&request.workspace_id, &request.experience_id)
            .await?
            .is_none()
        {
            return Err(CortexError::NotFound(format!(
                "experience {}",
                request.experience_id
            )));
        }
        if let Some(replacement) = &request.replacement_experience_id
            && self
                .storage
                .experience(&request.workspace_id, replacement)
                .await?
                .is_none()
        {
            return Err(CortexError::NotFound(format!("experience {replacement}")));
        }
        for event_id in &request.evidence_event_ids {
            if self
                .storage
                .event(&request.workspace_id, event_id)
                .await?
                .is_none()
            {
                return Err(CortexError::NotFound(format!("event {event_id}")));
            }
        }
        let request_key = request.request_key.clone();
        let assessment = ExperienceAssessment {
            id: Uuid::new_v4().to_string(),
            workspace_id: request.workspace_id,
            experience_id: request.experience_id,
            kind: request.kind,
            actor: request.reviewed_by,
            reason: request.reason,
            replacement_experience_id: request.replacement_experience_id,
            evidence_event_ids: request.evidence_event_ids,
            created_at: Utc::now(),
        };
        self.storage
            .append_reviewed_experience_assessment(
                &assessment,
                &request_key,
                &assessment_request_hash(&assessment),
            )
            .await
    }

    /// Produces deterministic, read-only dispute proposals for a later exact
    /// recurrence after a prior verified success. No lifecycle is changed.
    pub async fn propose_disputes(
        &self,
        request: &ExperienceDisputeProposalRequest,
    ) -> Result<Vec<ExperienceDisputeProposal>> {
        validate_proposal_request(request)?;
        for event_id in &request.recurring_failure_event_ids {
            let event = self
                .storage
                .event(&request.workspace_id, event_id)
                .await?
                .ok_or_else(|| CortexError::NotFound(format!("event {event_id}")))?;
            let decoded = self.evidence.diagnose(&event);
            let decoded = decoded.decoded().ok_or_else(|| {
                CortexError::Analysis(
                    "recurring failure evidence must decode through a registered contract".into(),
                )
            })?;
            let FailureNormalizationResult::Normalized { normalization } =
                self.failure_normalization.normalize(decoded)
            else {
                return Err(CortexError::Analysis(
                    "recurring failure evidence has no deterministic normalized signature".into(),
                ));
            };
            if !normalization.signature.is_exact_capable()
                || normalization.signature != request.failure_signature
            {
                return Err(CortexError::Analysis(
                    "recurring failure evidence does not exactly match the requested signature"
                        .into(),
                ));
            }
        }
        let hits = self
            .search
            .search(&ExperienceSearchRequest {
                workspace_id: request.workspace_id.clone(),
                query: None,
                exact_failure_signature: Some(request.failure_signature.clone()),
                compatible_components: Default::default(),
                path: None,
                graph_stable_key: None,
                outcomes: vec![ExperienceOutcome::Success],
                strengths: Vec::new(),
                lifecycles: vec![ExperienceLifecycle::Active],
                include_historical: false,
                created_after: None,
                created_before: None,
                limit: crate::domain::MAX_EXPERIENCE_SEARCH_LIMIT,
            })
            .await?;
        let mut proposals = Vec::new();
        for hit in hits {
            if hit.explanation.scores.exact_signature == 0
                || hit.experience.verification.status != VerificationStatus::VerifiedPassed
                || !self
                    .storage
                    .events_ingressed_after_experience(
                        &request.workspace_id,
                        &hit.experience.id,
                        &request.recurring_failure_event_ids,
                    )
                    .await?
            {
                continue;
            }
            proposals.push(ExperienceDisputeProposal {
                experience_id: hit.experience.id,
                lifecycle: hit.lifecycle,
                prior_verified_at: hit.experience.created_at,
                failure_signature: request.failure_signature.clone(),
                recurring_failure_event_ids: request.recurring_failure_event_ids.clone(),
                reason: "an exact normalized failure recurred after this verified success".into(),
            });
        }
        Ok(proposals)
    }
}

fn validate_review_request(request: &ExperienceAssessmentReviewRequest) -> Result<()> {
    if !bounded_audit_text(&request.workspace_id, 256)
        || !bounded_audit_text(&request.experience_id, 256)
        || !bounded_audit_text(&request.reviewed_by, 256)
        || !bounded_audit_text(&request.request_key, 256)
        || !bounded_audit_text(&request.reason, 4096)
    {
        return Err(CortexError::Analysis(
            "reviewed experience assessment requires bounded identity, reviewer, and reason".into(),
        ));
    }
    validate_evidence_ids(&request.evidence_event_ids)?;
    if (request.kind == ExperienceAssessmentKind::Superseded)
        != request.replacement_experience_id.is_some()
        || request
            .replacement_experience_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty() || value == request.experience_id)
    {
        return Err(CortexError::Analysis(
            "only supersession review may name a distinct replacement experience".into(),
        ));
    }
    Ok(())
}

fn assessment_request_hash(assessment: &ExperienceAssessment) -> String {
    let mut material = String::new();
    for field in [
        assessment.workspace_id.as_str(),
        assessment.experience_id.as_str(),
        assessment.kind.as_str(),
        assessment.actor.as_str(),
        assessment.reason.as_str(),
        assessment
            .replacement_experience_id
            .as_deref()
            .unwrap_or(""),
    ] {
        material.push_str(&field.len().to_string());
        material.push(':');
        material.push_str(field);
        material.push('|');
    }
    for event_id in &assessment.evidence_event_ids {
        material.push_str(&event_id.len().to_string());
        material.push(':');
        material.push_str(event_id);
        material.push('|');
    }
    format!(
        "assessment-request-v1:{}",
        blake3::hash(material.as_bytes()).to_hex()
    )
}

fn validate_proposal_request(request: &ExperienceDisputeProposalRequest) -> Result<()> {
    if request.workspace_id.trim().is_empty()
        || request.failure_signature.scope.workspace_id != request.workspace_id
    {
        return Err(CortexError::Analysis(
            "dispute proposal requires a failure signature scoped to its workspace".into(),
        ));
    }
    let rebuilt = crate::domain::FailureSignature::new(
        request.failure_signature.domain,
        request.failure_signature.identity_capability,
        request.failure_signature.components.clone(),
        request.failure_signature.normalizer_id.clone(),
        request.failure_signature.normalizer_version.clone(),
        request.failure_signature.scope.clone(),
    )?;
    if rebuilt != request.failure_signature {
        return Err(CortexError::Analysis(
            "dispute proposal failure signature is not canonically self-consistent".into(),
        ));
    }
    if !request.failure_signature.is_exact_capable() {
        return Err(CortexError::Analysis(
            "dispute proposals require an exact-capable failure identity".into(),
        ));
    }
    validate_evidence_ids(&request.recurring_failure_event_ids)
}

fn validate_evidence_ids(event_ids: &[String]) -> Result<()> {
    if event_ids.is_empty()
        || event_ids.len() > crate::domain::MAX_EXPERIENCE_ASSESSMENT_EVIDENCE
        || event_ids.iter().any(|id| !bounded_audit_text(id, 256))
        || event_ids.iter().collect::<BTreeSet<_>>().len() != event_ids.len()
    {
        return Err(CortexError::Analysis(
            "experience assessment evidence must be present, unique, and bounded".into(),
        ));
    }
    Ok(())
}

fn bounded_audit_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && value.trim() == value && !value.contains('\0')
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{validate_evidence_ids, validate_review_request};
    use crate::{
        AppConfig,
        domain::{
            ConsolidationAcceptance, ConsolidationPreview, ConsolidationRequest, CortexEvent,
            EpisodeCreator, EpisodeEventAssociationRequest, EpisodeStartRequest,
            EpisodeTerminalRequest, EpisodeType, EventType, ExperienceAssessmentKind,
            ExperienceAssessmentReviewRequest, ExperienceDisputeProposalRequest,
            ExperienceEligibilityInclusion, ExperienceLifecycle, ExperienceNormalContextReason,
            ExperienceSearchRequest, TaskStatus, Workspace,
        },
        service::CortexWeaveService,
        storage::SqliteStorage,
    };

    fn rust_failure(workspace_id: &str) -> CortexEvent {
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

    fn rust_pass(workspace_id: &str) -> CortexEvent {
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

    fn tool_change(workspace_id: &str) -> CortexEvent {
        CortexEvent::new(
            workspace_id,
            EventType::ExternalToolFinished,
            json!({
                "contract": "cortexweave.external_tool_completion",
                "version": 1,
                "tool": "editor",
                "operation": "edit",
                "exit_code": 0,
                "error_class": null,
                "message": null
            }),
        )
    }

    #[test]
    fn reviewed_operation_requires_auditable_actor_reason_and_evidence() {
        let request = ExperienceAssessmentReviewRequest {
            workspace_id: "workspace".into(),
            experience_id: "experience".into(),
            kind: ExperienceAssessmentKind::Disputed,
            reviewed_by: "reviewer".into(),
            request_key: "review-validation".into(),
            reason: "exact failure returned".into(),
            replacement_experience_id: None,
            evidence_event_ids: vec!["event".into()],
        };
        assert!(validate_review_request(&request).is_ok());
        assert!(validate_evidence_ids(&[]).is_err());
        assert!(validate_evidence_ids(&["event".into(), "event".into()]).is_err());
        let mut invalid_actor = request.clone();
        invalid_actor.reviewed_by = " reviewer".into();
        assert!(validate_review_request(&invalid_actor).is_err());
        let mut invalid_reason = request;
        invalid_reason.reason = "audit\0suffix".into();
        assert!(validate_review_request(&invalid_reason).is_err());
    }

    #[tokio::test]
    async fn exact_recurrence_proposes_only_then_review_changes_eligibility_and_history_explains_it()
     {
        let storage = SqliteStorage::in_memory().await.unwrap();
        let workspace = Workspace::new("C:/assessment", "assessment");
        storage.insert_workspace(&workspace).await.unwrap();
        let service = CortexWeaveService::from_parts(AppConfig::default(), storage).unwrap();
        let session = service
            .start_session(&workspace.id, json!({}))
            .await
            .unwrap();
        let task = service
            .start_task(
                &workspace.id,
                Some(session.id.clone()),
                "repair compiler failure",
                json!({}),
            )
            .await
            .unwrap();
        assert_eq!(task.status, TaskStatus::Active);
        let episode = service
            .start_episode(EpisodeStartRequest {
                workspace_id: workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(task.id.clone()),
                episode_type: EpisodeType::Debugging,
                title: Some("verified repair".into()),
                created_by: EpisodeCreator::NativeHarness,
            })
            .await
            .unwrap();
        let mut events = vec![
            rust_failure(&workspace.id),
            tool_change(&workspace.id),
            rust_pass(&workspace.id),
        ];
        for event in &mut events {
            event.session_id = Some(session.id.clone());
            event.task_id = Some(task.id.clone());
            *event = service.record_event(event.clone()).await.unwrap();
        }
        let associated = service
            .add_episode_events(EpisodeEventAssociationRequest {
                workspace_id: workspace.id.clone(),
                episode_id: episode.id.clone(),
                expected_version: episode.version,
                request_key: "assessment-associate".into(),
                event_ids: events.iter().map(|event| event.id.clone()).collect(),
            })
            .await
            .unwrap();
        let closed = service
            .close_episode(EpisodeTerminalRequest {
                workspace_id: workspace.id.clone(),
                episode_id: episode.id,
                expected_version: associated.version,
                request_key: "assessment-close".into(),
            })
            .await
            .unwrap();
        let request = ConsolidationRequest {
            workspace_id: workspace.id.clone(),
            episode_id: closed.id,
            expected_episode_version: closed.version,
        };
        let ConsolidationPreview::Proposal { proposal, .. } =
            service.preview_experience(&request).await.unwrap()
        else {
            panic!("fixture must propose a verified experience");
        };
        let ConsolidationAcceptance::Accepted { record } = service
            .accept_experience(&crate::domain::ConsolidationAcceptanceRequest {
                request,
                expected_fingerprint: proposal.fingerprint,
                expected_proposal_hash: proposal.proposal_hash,
            })
            .await
            .unwrap()
        else {
            panic!("fixture must accept a verified experience");
        };
        let signature = record.experience.failure_signature.clone().unwrap();
        let experience_id = record.experience.id.clone();
        let original_summary = record.experience.summary.clone();
        let original_evidence = record.evidence.clone();

        let replayed_original = ExperienceDisputeProposalRequest {
            workspace_id: workspace.id.clone(),
            failure_signature: signature.clone(),
            recurring_failure_event_ids: vec![events[0].id.clone()],
        };
        assert!(
            service
                .propose_experience_disputes(&replayed_original)
                .await
                .unwrap()
                .is_empty(),
            "the original failure cannot be replayed as later recurrence"
        );

        let mut recurring = rust_failure(&workspace.id);
        recurring.session_id = Some(session.id);
        recurring.task_id = Some(task.id);
        let recurring = service.record_event(recurring).await.unwrap();
        let proposal_request = ExperienceDisputeProposalRequest {
            workspace_id: workspace.id.clone(),
            failure_signature: signature.clone(),
            recurring_failure_event_ids: vec![recurring.id.clone()],
        };
        let proposals = service
            .propose_experience_disputes(&proposal_request)
            .await
            .unwrap();
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].experience_id, experience_id);
        assert_eq!(
            service
                .experience_get(&workspace.id, &experience_id)
                .await
                .unwrap()
                .unwrap()
                .lifecycle,
            ExperienceLifecycle::Active,
            "proposal is read-only"
        );

        service
            .review_experience_assessment(ExperienceAssessmentReviewRequest {
                workspace_id: workspace.id.clone(),
                experience_id: experience_id.clone(),
                kind: ExperienceAssessmentKind::Disputed,
                reviewed_by: "reviewer".into(),
                request_key: "review-dispute".into(),
                reason: "the exact verified failure recurred".into(),
                replacement_experience_id: None,
                evidence_event_ids: vec![recurring.id],
            })
            .await
            .unwrap();
        let default_hits = service
            .search_experiences(&ExperienceSearchRequest {
                workspace_id: workspace.id.clone(),
                query: None,
                exact_failure_signature: Some(signature),
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
        assert!(default_hits.is_empty());
        let mut historical_request = ExperienceSearchRequest {
            workspace_id: workspace.id.clone(),
            query: None,
            exact_failure_signature: None,
            compatible_components: BTreeMap::new(),
            path: None,
            graph_stable_key: None,
            outcomes: Vec::new(),
            strengths: Vec::new(),
            lifecycles: Vec::new(),
            include_historical: true,
            created_after: None,
            created_before: None,
            limit: 10,
        };
        historical_request.exact_failure_signature =
            Some(proposal_request.failure_signature.clone());
        let historical = service
            .search_experiences(&historical_request)
            .await
            .unwrap();
        assert_eq!(historical.len(), 1);
        assert_eq!(historical[0].experience.id, experience_id);
        assert_eq!(
            historical[0].explanation.eligibility.inclusion,
            ExperienceEligibilityInclusion::HistoricalInspection
        );
        let detail = service
            .experience_get(&workspace.id, &historical[0].experience.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(detail.lifecycle, ExperienceLifecycle::Disputed);
        assert_eq!(detail.assessments.len(), 1);
        assert!(!detail.normal_context.eligible);
        assert_eq!(
            detail.normal_context.reason,
            ExperienceNormalContextReason::DisputedLifecycle
        );
        assert_eq!(detail.record.experience.summary, original_summary);
        assert_eq!(detail.record.evidence, original_evidence);
        assert_eq!(
            service
                .storage()
                .event(&workspace.id, &events[0].id)
                .await
                .unwrap(),
            Some(events[0].clone()),
            "review does not rewrite original evidence"
        );
        let assessment_id = detail.assessments[0].id.clone();
        assert!(
            sqlx::query("DELETE FROM experience_assessments WHERE id = ?")
                .bind(&assessment_id)
                .execute(service.storage().pool())
                .await
                .is_err(),
            "assessment history cannot be deleted while its workspace exists"
        );
        assert!(
            sqlx::query("DELETE FROM experience_assessment_evidence WHERE assessment_id = ?")
                .bind(&assessment_id)
                .execute(service.storage().pool())
                .await
                .is_err(),
            "assessment evidence cannot be deleted while its workspace exists"
        );
        assert!(
            service
                .propose_experience_disputes(&proposal_request)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            service
                .storage()
                .delete_workspace(&workspace.id)
                .await
                .unwrap()
        );
    }
}
