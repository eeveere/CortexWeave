use std::{collections::BTreeMap, sync::Arc};

use blake3::Hasher;
use serde_json::{Value, json};

use crate::{
    CortexError, Result,
    domain::{
        AttemptResult, ConsolidationAcceptance, ConsolidationAcceptanceRequest,
        ConsolidationDiagnostic, ConsolidationInputIdentity, ConsolidationInputMember,
        ConsolidationNoResultReason, ConsolidationPreview, ConsolidationRequest, DecodedEvidence,
        EXPERIENCE_CANONICALIZATION_VERSION, EpisodeStatus, EvidenceBasis, EvidenceDecodeResult,
        EvidenceObservation, EvidenceStrength, EvidenceStrengthAssessment, Experience,
        ExperienceAttempt, ExperienceEvidenceLink, ExperienceEvidenceRelation, ExperienceOutcome,
        ExperienceProposal, ExperienceRecord, ExperienceVerification,
        ExperienceVerificationObservation, FAILURE_TO_VERIFICATION_EXTRACTOR_ID,
        FAILURE_TO_VERIFICATION_EXTRACTOR_VERSION, MAX_EPISODE_EVENTS, ProcessOutcome,
        ProposalDisposition, VerificationKind, VerificationStatus, VerificationSubject,
        VerificationSubjectKind, canonical_event_payload_hash,
    },
    service::{EvidenceService, FailureNormalizationService},
    storage::SqliteStorage,
};

const FINGERPRINT_DOMAIN: &[u8] = b"cortexweave.consolidation-input.v1\0";
const PROPOSAL_DOMAIN: &[u8] = b"cortexweave.experience-proposal.v1\0";
const RUST_COMPILER_RULE_ID: &str = "cortexweave.rust_compiler";
const RUST_COMPILER_RULE_VERSION: &str = "1";
const CARGO_TEST_RULE_ID: &str = "cortexweave.cargo_test";
const CARGO_TEST_RULE_VERSION: &str = "1";

pub struct ConsolidationService {
    storage: Arc<SqliteStorage>,
    evidence: Arc<EvidenceService>,
    normalization: Arc<FailureNormalizationService>,
}

impl ConsolidationService {
    pub fn new(
        storage: Arc<SqliteStorage>,
        evidence: Arc<EvidenceService>,
        normalization: Arc<FailureNormalizationService>,
    ) -> Self {
        Self {
            storage,
            evidence,
            normalization,
        }
    }

    pub async fn preview(&self, request: &ConsolidationRequest) -> Result<ConsolidationPreview> {
        let Some(episode) = self
            .storage
            .episode(&request.workspace_id, &request.episode_id)
            .await?
        else {
            return Ok(no_result(
                ConsolidationNoResultReason::EpisodeIneligible,
                "episode_not_found",
                "the requested episode does not exist",
                None,
                None,
            ));
        };
        if episode.version != request.expected_episode_version {
            return Ok(no_result(
                ConsolidationNoResultReason::StaleEpisodeVersion,
                "stale_episode_version",
                "the episode changed after the supplied version",
                None,
                None,
            ));
        }
        if !matches!(
            episode.status,
            EpisodeStatus::Closed | EpisodeStatus::Abandoned
        ) {
            return Ok(no_result(
                ConsolidationNoResultReason::EpisodeNotClosed,
                "episode_not_terminal",
                "only terminal episodes can be consolidated",
                None,
                None,
            ));
        }
        let members = self
            .storage
            .episode_events(
                &request.workspace_id,
                &request.episode_id,
                MAX_EPISODE_EVENTS + 1,
            )
            .await?;
        if members.len() > MAX_EPISODE_EVENTS {
            return Ok(no_result(
                ConsolidationNoResultReason::EvidenceLimitExceeded,
                "episode_event_limit",
                "episode membership exceeds the deterministic evidence limit",
                None,
                None,
            ));
        }
        if members.is_empty() {
            return Ok(no_result(
                ConsolidationNoResultReason::EpisodeEmpty,
                "episode_empty",
                "the episode has no evidence events",
                None,
                None,
            ));
        }
        let mut events = Vec::with_capacity(members.len());
        for member in members {
            let Some(event) = self
                .storage
                .event(&request.workspace_id, &member.event_id)
                .await?
            else {
                return Err(CortexError::Storage(sqlx::Error::Decode(
                    "episode member event is missing".into(),
                )));
            };
            if event.session_id.as_deref() != Some(&episode.session_id)
                || event.task_id != episode.task_id
            {
                return Err(CortexError::Analysis(
                    "episode membership contains an event outside the episode scope".into(),
                ));
            }
            events.push((member.ordinal, event));
        }
        let fingerprint =
            input_fingerprint(&episode, &events, &self.evidence, &self.normalization)?;
        extract_proposal(
            &self.evidence,
            &self.normalization,
            &episode,
            &events,
            fingerprint,
        )
    }

    pub async fn accept(
        &self,
        request: &ConsolidationAcceptanceRequest,
    ) -> Result<ConsolidationAcceptance> {
        let preview = self.preview(&request.request).await?;
        let ConsolidationPreview::Proposal {
            proposal,
            disposition,
        } = preview
        else {
            return Ok(match preview {
                ConsolidationPreview::NoResult {
                    reason,
                    diagnostics,
                } => ConsolidationAcceptance::NoResult {
                    reason,
                    diagnostics,
                },
                _ => unreachable!(),
            });
        };
        if !matches!(disposition, ProposalDisposition::Automatic) {
            return Ok(ConsolidationAcceptance::NoResult {
                reason: ConsolidationNoResultReason::ReviewRequired,
                diagnostics: proposal.diagnostics,
            });
        }
        if proposal.fingerprint != request.expected_fingerprint {
            return Ok(no_accept(
                ConsolidationNoResultReason::FingerprintMismatch,
                "fingerprint_mismatch",
                "the episode input changed since preview",
            ));
        }
        if proposal.proposal_hash != request.expected_proposal_hash {
            return Ok(no_accept(
                ConsolidationNoResultReason::ProposalHashMismatch,
                "proposal_hash_mismatch",
                "the material proposal changed since preview",
            ));
        }
        let input_identity = self.acceptance_input_identity(&request.request).await?;
        let record = self
            .storage
            .insert_consolidated_experience(
                &proposal.record,
                request.request.expected_episode_version,
                &input_identity,
            )
            .await?;
        Ok(ConsolidationAcceptance::Accepted {
            record: Box::new(record),
        })
    }

    async fn acceptance_input_identity(
        &self,
        request: &ConsolidationRequest,
    ) -> Result<ConsolidationInputIdentity> {
        let members = self
            .storage
            .episode_events(
                &request.workspace_id,
                &request.episode_id,
                MAX_EPISODE_EVENTS + 1,
            )
            .await?;
        if members.len() > MAX_EPISODE_EVENTS {
            return Err(CortexError::Analysis(
                "episode membership exceeds the deterministic evidence limit".into(),
            ));
        }
        let mut identity_members = Vec::with_capacity(members.len());
        for member in members {
            let event = self
                .storage
                .event(&request.workspace_id, &member.event_id)
                .await?
                .ok_or_else(|| {
                    CortexError::Storage(sqlx::Error::Decode(
                        "episode member event is missing".into(),
                    ))
                })?;
            identity_members.push(ConsolidationInputMember {
                ordinal: member.ordinal,
                event_id: event.id,
                event_type: event.event_type.storage_name(),
                payload_hash: canonical_event_payload_hash(&event.payload)?,
            });
        }
        Ok(ConsolidationInputIdentity {
            workspace_id: request.workspace_id.clone(),
            episode_id: request.episode_id.clone(),
            members: identity_members,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct VerificationScopeKey {
    rule_id: String,
    rule_version: String,
    subject: VerificationSubject,
}

#[derive(Debug, Clone)]
struct VerificationFact {
    event_index: usize,
    status: VerificationStatus,
    kind: VerificationKind,
    scope: VerificationScopeKey,
    normalized_failure: Option<crate::domain::FailureSignature>,
}

impl VerificationFact {
    fn is_positive(&self) -> bool {
        matches!(
            self.status,
            VerificationStatus::VerifiedPassed | VerificationStatus::ExplicitlyAccepted
        )
    }

    fn is_user_acceptance(&self) -> bool {
        self.status == VerificationStatus::ExplicitlyAccepted
    }
}

#[derive(Debug)]
struct BuiltAttempt {
    changes: Vec<usize>,
    following: Option<VerificationFact>,
    additional_results: Vec<VerificationFact>,
    result: AttemptResult,
}

enum ClassifiedObservation {
    AttemptAction { source_change: bool },
    Verification(Box<VerificationFact>),
}

fn extract_proposal(
    evidence_service: &EvidenceService,
    normalization: &FailureNormalizationService,
    episode: &crate::domain::Episode,
    events: &[(u64, crate::domain::CortexEvent)],
    fingerprint: String,
) -> Result<ConsolidationPreview> {
    let mut initial: Option<VerificationFact> = None;
    let mut supporting_initial_repeats = Vec::new();
    let mut attempts: Vec<BuiltAttempt> = Vec::new();
    let mut pending_changes = Vec::new();
    let mut historical_snapshot_required = false;
    let mut diagnostics = Vec::new();

    for (event_index, (membership_ordinal, event)) in events.iter().enumerate() {
        let decoded = match evidence_service.diagnose(event) {
            EvidenceDecodeResult::Decoded { evidence } => evidence,
            EvidenceDecodeResult::Unsupported { .. } if material_event(event) => {
                return Ok(no_result(
                    ConsolidationNoResultReason::UnsupportedPayloadContract,
                    "unsupported_payload_contract",
                    "a material evidence event uses an unsupported contract",
                    Some(*membership_ordinal),
                    Some(event.id.clone()),
                ));
            }
            EvidenceDecodeResult::Invalid { issue } if material_event(event) => {
                return Ok(no_result(
                    ConsolidationNoResultReason::InvalidPayloadContract,
                    &issue.code,
                    &issue.message,
                    Some(*membership_ordinal),
                    Some(event.id.clone()),
                ));
            }
            _ => {
                diagnostics.push(diag(
                    "nonmaterial_event_excluded",
                    "event does not participate in the v1 observed sequence",
                    Some(*membership_ordinal),
                    Some(event.id.clone()),
                ));
                continue;
            }
        };

        let classified = match classify_observation(normalization, &decoded, event_index) {
            Ok(Some(classified)) => classified,
            Ok(None) => {
                diagnostics.push(diag(
                    "nonmaterial_observation_excluded",
                    "decoded evidence does not establish an attempt action or verifier result",
                    Some(*membership_ordinal),
                    Some(event.id.clone()),
                ));
                continue;
            }
            Err((reason, code, message)) => {
                return Ok(no_result(
                    reason,
                    code,
                    &message,
                    Some(*membership_ordinal),
                    Some(event.id.clone()),
                ));
            }
        };

        match classified {
            ClassifiedObservation::AttemptAction { source_change } => {
                if initial.is_some() {
                    historical_snapshot_required |= source_change;
                    pending_changes.push(event_index);
                } else {
                    diagnostics.push(diag(
                        "prelude_action_excluded",
                        "an action observed before the initial failure is outside the associated attempt sequence",
                        Some(*membership_ordinal),
                        Some(event.id.clone()),
                    ));
                }
            }
            ClassifiedObservation::Verification(fact) => {
                let fact = *fact;
                let Some(initial_fact) = initial.as_ref() else {
                    if fact.status == VerificationStatus::VerifiedFailed {
                        if fact.normalized_failure.is_none() {
                            return Ok(no_result(
                                ConsolidationNoResultReason::NoSupportedFailure,
                                "unsupported_initial_failure",
                                "the first mechanically failed verifier result has no supported normalized signature",
                                Some(*membership_ordinal),
                                Some(event.id.clone()),
                            ));
                        }
                        initial = Some(fact);
                    } else {
                        diagnostics.push(diag(
                            "prelude_positive_result_excluded",
                            "a positive result before the initial failure is not resolution evidence for this episode interpretation",
                            Some(*membership_ordinal),
                            Some(event.id.clone()),
                        ));
                    }
                    continue;
                };

                if pending_changes.is_empty() && attempts.is_empty() {
                    let is_exact_repeat = fact.status == VerificationStatus::VerifiedFailed
                        && fact.scope == initial_fact.scope
                        && fact
                            .normalized_failure
                            .as_ref()
                            .zip(initial_fact.normalized_failure.as_ref())
                            .is_some_and(|(candidate, initial)| {
                                exact_failure_equals(candidate, initial)
                            });
                    if is_exact_repeat {
                        supporting_initial_repeats.push(event_index);
                        continue;
                    }
                    return Ok(no_result(
                        ConsolidationNoResultReason::AmbiguousInitialFailure,
                        "ambiguous_pre_attempt_result",
                        "a distinct verifier result was observed before the first associated attempt",
                        Some(*membership_ordinal),
                        Some(event.id.clone()),
                    ));
                }

                if !pending_changes.is_empty() {
                    if !supports_initial_scope(&fact, initial_fact) {
                        return Ok(no_result(
                            ConsolidationNoResultReason::AmbiguousVerificationScope,
                            "following_verification_scope_mismatch",
                            "the first result after an associated attempt does not cover the initial failure's exact verifier scope",
                            Some(*membership_ordinal),
                            Some(event.id.clone()),
                        ));
                    }
                    let result = attempt_result(&fact, initial_fact).ok_or_else(|| {
                        CortexError::Analysis(
                            "classified failed verification lost normalized failure material"
                                .into(),
                        )
                    })?;
                    attempts.push(BuiltAttempt {
                        changes: std::mem::take(&mut pending_changes),
                        following: Some(fact),
                        additional_results: Vec::new(),
                        result,
                    });
                    continue;
                }

                if !eligible_terminal_fact(&fact, initial_fact) {
                    return Ok(no_result(
                        ConsolidationNoResultReason::AmbiguousVerificationScope,
                        "terminal_verification_scope_mismatch",
                        "a terminal result belongs to a different verifier family or unsupported subject",
                        Some(*membership_ordinal),
                        Some(event.id.clone()),
                    ));
                }
                attempts
                    .last_mut()
                    .expect("an attempt exists when extending a result batch")
                    .additional_results
                    .push(fact);
            }
        }
    }

    let Some(initial_fact) = initial else {
        return Ok(no_result(
            ConsolidationNoResultReason::NoSupportedFailure,
            "no_supported_failure",
            "no normalized mechanically failed verifier result was observed",
            None,
            None,
        ));
    };
    let signature = initial_fact
        .normalized_failure
        .clone()
        .expect("initial failure is normalized");

    if !pending_changes.is_empty() {
        if episode.status == EpisodeStatus::Closed {
            return Ok(no_result(
                ConsolidationNoResultReason::NoVerificationEvidence,
                "pending_attempt_has_no_verification",
                "the final associated attempt has no following verifier evidence",
                None,
                None,
            ));
        }
        attempts.push(BuiltAttempt {
            changes: pending_changes,
            following: None,
            additional_results: Vec::new(),
            result: AttemptResult::Inconclusive,
        });
    }
    if attempts.is_empty() {
        return Ok(no_result(
            ConsolidationNoResultReason::NoAttemptEvidence,
            "no_attempt_evidence",
            "the initial failure was not followed by an associated attempt",
            None,
            None,
        ));
    }

    let abandoned = episode.status == EpisodeStatus::Abandoned;
    let terminal_facts = if abandoned {
        Vec::new()
    } else {
        let final_attempt = attempts.last().expect("attempt exists");
        let mut facts = vec![
            final_attempt
                .following
                .clone()
                .expect("a closed final attempt has following verification"),
        ];
        facts.extend(final_attempt.additional_results.clone());
        facts
    };

    let (outcome, verification_status) = if abandoned {
        (ExperienceOutcome::Abandoned, VerificationStatus::Missing)
    } else {
        match terminal_outcome(&terminal_facts, &initial_fact) {
            Ok(outcome) => outcome,
            Err((reason, code, message)) => {
                return Ok(no_result(reason, code, message, None, None));
            }
        }
    };

    let observations = terminal_facts
        .iter()
        .enumerate()
        .map(|(ordinal, fact)| ExperienceVerificationObservation {
            ordinal: ordinal as u64,
            status: fact.status,
            kind: fact.kind,
            subject: fact.scope.subject.clone(),
            evidence_event_id: events[fact.event_index].1.id.clone(),
            rule_id: fact.scope.rule_id.clone(),
            rule_version: fact.scope.rule_version.clone(),
        })
        .collect::<Vec<_>>();

    let uses_user_acceptance = terminal_facts
        .iter()
        .any(VerificationFact::is_user_acceptance);
    let repeated = !terminal_facts.is_empty()
        && !uses_user_acceptance
        && terminal_facts.iter().all(|fact| {
            terminal_facts
                .iter()
                .filter(|candidate| {
                    candidate.status == fact.status && candidate.scope == fact.scope
                })
                .count()
                >= 2
        });
    let mut bases = vec![EvidenceBasis::TemporalAssociation];
    if terminal_facts.iter().any(|fact| !fact.is_user_acceptance()) {
        bases.push(EvidenceBasis::DeterministicVerifier);
    }
    if uses_user_acceptance {
        bases.push(EvidenceBasis::ExplicitUserAcceptance);
    }
    if repeated {
        bases.push(EvidenceBasis::RepeatedDeterministicEvidence);
    }
    let evidence_strength = EvidenceStrengthAssessment {
        strength: if abandoned {
            EvidenceStrength::Weak
        } else if repeated {
            EvidenceStrength::Strong
        } else {
            EvidenceStrength::Moderate
        },
        bases,
    };

    let mut relations = BTreeMap::new();
    relations.insert(
        initial_fact.event_index,
        ExperienceEvidenceRelation::InitialFailure,
    );
    for event_index in supporting_initial_repeats {
        relations.insert(event_index, ExperienceEvidenceRelation::Supporting);
    }
    for (attempt_ordinal, attempt) in attempts.iter().enumerate() {
        for event_index in &attempt.changes {
            relations.insert(*event_index, ExperienceEvidenceRelation::AttemptChange);
        }
        if let Some(following) = &attempt.following {
            relations.insert(
                following.event_index,
                ExperienceEvidenceRelation::AttemptVerification,
            );
        }
        for fact in &attempt.additional_results {
            relations.insert(
                fact.event_index,
                if !abandoned && attempt_ordinal + 1 == attempts.len() {
                    ExperienceEvidenceRelation::TerminalVerification
                } else {
                    ExperienceEvidenceRelation::Supporting
                },
            );
        }
    }
    let evidence_links = relations
        .iter()
        .enumerate()
        .map(
            |(ordinal, (event_index, relation))| ExperienceEvidenceLink {
                ordinal: ordinal as u64,
                relation: *relation,
                event_id: events[*event_index].1.id.clone(),
            },
        )
        .collect::<Vec<_>>();
    let relation_ordinals = relations
        .keys()
        .enumerate()
        .map(|(ordinal, event_index)| (*event_index, ordinal as u64))
        .collect::<BTreeMap<_, _>>();

    let verification = ExperienceVerification {
        status: verification_status,
        observations,
        reasons: Vec::new(),
    };
    let experience = Experience::from_draft(crate::domain::ExperienceDraft {
        workspace_id: episode.workspace_id.clone(),
        session_id: episode.session_id.clone(),
        task_id: episode.task_id.clone(),
        episode_id: episode.id.clone(),
        failure_signature: Some(signature),
        outcome,
        verification,
        evidence_strength,
        extractor_id: FAILURE_TO_VERIFICATION_EXTRACTOR_ID.into(),
        extractor_version: FAILURE_TO_VERIFICATION_EXTRACTOR_VERSION.into(),
        canonicalization_version: EXPERIENCE_CANONICALIZATION_VERSION.into(),
        consolidation_fingerprint: fingerprint.clone(),
        proposal_hash: String::new(),
        attempts: attempts.len(),
    });
    let experience_attempts = attempts
        .iter()
        .enumerate()
        .map(|(ordinal, attempt)| ExperienceAttempt {
            id: uuid::Uuid::new_v4().to_string(),
            workspace_id: episode.workspace_id.clone(),
            experience_id: experience.id.clone(),
            ordinal: ordinal as u64,
            result: attempt.result,
            change_evidence_ordinals: attempt
                .changes
                .iter()
                .map(|event_index| relation_ordinals[event_index])
                .collect(),
            following_verification_ordinal: attempt
                .following
                .as_ref()
                .map(|fact| relation_ordinals[&fact.event_index]),
        })
        .collect();
    let mut record = ExperienceRecord {
        experience,
        attempts: experience_attempts,
        evidence: evidence_links,
        code_snapshots: Vec::new(),
        graph_snapshots: Vec::new(),
    };
    let proposal_hash = proposal_hash(&record)?;
    record.experience.proposal_hash = proposal_hash.clone();

    let mut review_reasons = Vec::new();
    if historical_snapshot_required {
        review_reasons.push("historical_snapshot_unavailable".into());
    }
    if uses_user_acceptance {
        review_reasons.push("unauthenticated_user_acceptance".into());
    }
    if abandoned {
        review_reasons.push("abandoned_episode".into());
    }
    let disposition = if review_reasons.is_empty() {
        ProposalDisposition::Automatic
    } else {
        ProposalDisposition::ReviewRequired {
            reasons: review_reasons,
        }
    };
    Ok(ConsolidationPreview::Proposal {
        proposal: Box::new(ExperienceProposal {
            record,
            fingerprint,
            proposal_hash,
            diagnostics,
        }),
        disposition,
    })
}

fn classify_observation(
    normalization: &FailureNormalizationService,
    decoded: &DecodedEvidence,
    event_index: usize,
) -> std::result::Result<
    Option<ClassifiedObservation>,
    (ConsolidationNoResultReason, &'static str, String),
> {
    let verification = match &decoded.observation {
        EvidenceObservation::ExternalToolCompletion(_) => {
            return Ok(Some(ClassifiedObservation::AttemptAction {
                source_change: false,
            }));
        }
        EvidenceObservation::SourceChange(_) => {
            return Ok(Some(ClassifiedObservation::AttemptAction {
                source_change: true,
            }));
        }
        EvidenceObservation::RustCompilerResult(report) => VerificationFact {
            event_index,
            status: if report.process_outcome == ProcessOutcome::ExitedZero {
                VerificationStatus::VerifiedPassed
            } else {
                VerificationStatus::VerifiedFailed
            },
            kind: VerificationKind::RustCompiler,
            scope: VerificationScopeKey {
                rule_id: RUST_COMPILER_RULE_ID.into(),
                rule_version: RUST_COMPILER_RULE_VERSION.into(),
                subject: subject(report.subject.kind, &report.subject.value),
            },
            normalized_failure: normalized_failure(normalization, decoded),
        },
        EvidenceObservation::CargoTestResult(report) => {
            if report.process_outcome == ProcessOutcome::ExitedZero
                && report.executed_test_count == 0
            {
                return Err((
                    ConsolidationNoResultReason::EmptyVerificationRun,
                    "empty_verification_run",
                    "a successful Cargo test report executed zero tests".into(),
                ));
            }
            VerificationFact {
                event_index,
                status: if report.process_outcome == ProcessOutcome::ExitedZero {
                    VerificationStatus::VerifiedPassed
                } else {
                    VerificationStatus::VerifiedFailed
                },
                kind: VerificationKind::CargoTest,
                scope: VerificationScopeKey {
                    rule_id: CARGO_TEST_RULE_ID.into(),
                    rule_version: CARGO_TEST_RULE_VERSION.into(),
                    subject: subject(report.subject.kind, &report.subject.value),
                },
                normalized_failure: normalized_failure(normalization, decoded),
            }
        }
        EvidenceObservation::GenericVerifierResult(report) => {
            let Some(rule) = normalization
                .verifier_rules()
                .find(&report.verifier_id, &report.verifier_version)
            else {
                return Err((
                    ConsolidationNoResultReason::NoSupportedFailure,
                    "unregistered_verifier_rule",
                    "generic verifier evidence has no exact registered rule".into(),
                ));
            };
            let allowed_kind = match report.subject.kind {
                crate::domain::EvidenceSubjectKind::Workspace => VerificationSubjectKind::Workspace,
                crate::domain::EvidenceSubjectKind::Package => VerificationSubjectKind::Package,
                crate::domain::EvidenceSubjectKind::Target => VerificationSubjectKind::Target,
                crate::domain::EvidenceSubjectKind::Test => VerificationSubjectKind::Test,
                crate::domain::EvidenceSubjectKind::Path => VerificationSubjectKind::Path,
            };
            if report.tool != rule.required_tool
                || report.operation != rule.required_operation
                || !rule.allowed_subject_kinds.contains(&allowed_kind)
            {
                return Err((
                    ConsolidationNoResultReason::NoSupportedFailure,
                    "verifier_rule_mismatch",
                    "generic verifier evidence does not satisfy its registered tool, operation, and subject rule".into(),
                ));
            }
            VerificationFact {
                event_index,
                status: match report.reported_result {
                    crate::domain::VerificationResult::Passed => VerificationStatus::VerifiedPassed,
                    crate::domain::VerificationResult::Failed => VerificationStatus::VerifiedFailed,
                },
                kind: VerificationKind::RegisteredTool,
                scope: VerificationScopeKey {
                    rule_id: rule.id.clone(),
                    rule_version: rule.version.clone(),
                    subject: subject(report.subject.kind, &report.subject.value),
                },
                normalized_failure: normalized_failure(normalization, decoded),
            }
        }
        EvidenceObservation::UserAcceptance(report) => VerificationFact {
            event_index,
            status: VerificationStatus::ExplicitlyAccepted,
            kind: VerificationKind::UserAcceptance,
            scope: VerificationScopeKey {
                rule_id: "cortexweave.user_acceptance".into(),
                rule_version: "1".into(),
                subject: subject(report.subject.kind, &report.subject.value),
            },
            normalized_failure: None,
        },
    };
    if verification.status == VerificationStatus::VerifiedFailed
        && verification.normalized_failure.is_none()
    {
        let message = match normalization.normalize(decoded) {
            crate::domain::FailureNormalizationResult::Unsupported { reason } => reason.message,
            crate::domain::FailureNormalizationResult::Normalized { .. } => {
                "failed verifier result lost normalized failure material".into()
            }
        };
        return Err((
            ConsolidationNoResultReason::NoSupportedFailure,
            "unsupported_failed_verifier_result",
            message,
        ));
    }
    Ok(Some(ClassifiedObservation::Verification(Box::new(
        verification,
    ))))
}

fn supports_initial_scope(fact: &VerificationFact, initial: &VerificationFact) -> bool {
    if fact.is_user_acceptance() {
        fact.scope.subject == initial.scope.subject
    } else {
        fact.scope == initial.scope
    }
}

fn eligible_terminal_fact(fact: &VerificationFact, initial: &VerificationFact) -> bool {
    if fact.is_user_acceptance() {
        fact.scope.subject == initial.scope.subject
    } else {
        fact.scope.rule_id == initial.scope.rule_id
            && fact.scope.rule_version == initial.scope.rule_version
    }
}

fn attempt_result(fact: &VerificationFact, initial: &VerificationFact) -> Option<AttemptResult> {
    if fact.is_positive() {
        return Some(AttemptResult::VerificationPassed);
    }
    let failure = fact.normalized_failure.as_ref()?;
    let initial_failure = initial.normalized_failure.as_ref()?;
    Some(if exact_failure_equals(failure, initial_failure) {
        AttemptResult::StillFailing
    } else {
        AttemptResult::VerificationChangedFailure
    })
}

fn exact_failure_equals(
    candidate: &crate::domain::FailureSignature,
    initial: &crate::domain::FailureSignature,
) -> bool {
    candidate.is_exact_capable()
        && initial.is_exact_capable()
        && candidate.normalized_key == initial.normalized_key
}

fn terminal_outcome(
    facts: &[VerificationFact],
    initial: &VerificationFact,
) -> std::result::Result<
    (ExperienceOutcome, VerificationStatus),
    (ConsolidationNoResultReason, &'static str, &'static str),
> {
    if facts.is_empty() {
        return Err((
            ConsolidationNoResultReason::NoVerificationEvidence,
            "missing_terminal_frontier",
            "the closed episode has no terminal verifier frontier",
        ));
    }
    if !facts
        .iter()
        .any(|fact| supports_initial_scope(fact, initial))
    {
        return Err((
            ConsolidationNoResultReason::AmbiguousVerificationScope,
            "initial_scope_not_covered",
            "the terminal verifier frontier does not cover the initial failure scope",
        ));
    }
    let mut by_subject: BTreeMap<VerificationSubject, (bool, bool)> = BTreeMap::new();
    for fact in facts {
        let status = by_subject.entry(fact.scope.subject.clone()).or_default();
        if fact.is_positive() {
            status.0 = true;
        } else {
            status.1 = true;
        }
    }
    if by_subject
        .values()
        .any(|(positive, failed)| *positive && *failed)
    {
        return Err((
            ConsolidationNoResultReason::ConflictingResults,
            "conflicting_terminal_results",
            "opposing terminal results were observed for the same exact subject",
        ));
    }
    let positive = facts.iter().any(VerificationFact::is_positive);
    let failed = facts
        .iter()
        .any(|fact| fact.status == VerificationStatus::VerifiedFailed);
    Ok(match (positive, failed) {
        (true, false) => (
            ExperienceOutcome::Success,
            if facts.iter().all(VerificationFact::is_user_acceptance) {
                VerificationStatus::ExplicitlyAccepted
            } else {
                VerificationStatus::VerifiedPassed
            },
        ),
        (false, true) => (
            ExperienceOutcome::Failure,
            VerificationStatus::VerifiedFailed,
        ),
        (true, true) => (
            ExperienceOutcome::PartialSuccess,
            VerificationStatus::Conflicting,
        ),
        (false, false) => {
            return Err((
                ConsolidationNoResultReason::NoVerificationEvidence,
                "inconclusive_terminal_frontier",
                "the terminal verifier frontier has no conclusive result",
            ));
        }
    })
}

fn material_event(event: &crate::domain::CortexEvent) -> bool {
    matches!(
        event.event_type,
        crate::domain::EventType::CompilerResult
            | crate::domain::EventType::TestResult
            | crate::domain::EventType::ExternalToolFinished
            | crate::domain::EventType::FileCreated
            | crate::domain::EventType::FileModified
            | crate::domain::EventType::FileRemoved
            | crate::domain::EventType::FileRenamed
            | crate::domain::EventType::UserAcceptance
    )
}
fn normalized_failure(
    service: &FailureNormalizationService,
    decoded: &DecodedEvidence,
) -> Option<crate::domain::FailureSignature> {
    match service.normalize(decoded) {
        crate::domain::FailureNormalizationResult::Normalized { normalization } => {
            Some(normalization.signature)
        }
        _ => None,
    }
}
fn subject(kind: crate::domain::EvidenceSubjectKind, value: &str) -> VerificationSubject {
    VerificationSubject {
        kind: match kind {
            crate::domain::EvidenceSubjectKind::Workspace => VerificationSubjectKind::Workspace,
            crate::domain::EvidenceSubjectKind::Package => VerificationSubjectKind::Package,
            crate::domain::EvidenceSubjectKind::Target => VerificationSubjectKind::Target,
            crate::domain::EvidenceSubjectKind::Test => VerificationSubjectKind::Test,
            crate::domain::EvidenceSubjectKind::Path => VerificationSubjectKind::Path,
        },
        value: value.into(),
    }
}
fn diag(
    code: &str,
    message: &str,
    membership_ordinal: Option<u64>,
    event_id: Option<String>,
) -> ConsolidationDiagnostic {
    ConsolidationDiagnostic {
        code: code.into(),
        message: message.into(),
        membership_ordinal,
        event_id,
    }
}
fn no_result(
    reason: ConsolidationNoResultReason,
    code: &str,
    message: &str,
    ordinal: Option<u64>,
    event_id: Option<String>,
) -> ConsolidationPreview {
    ConsolidationPreview::NoResult {
        reason,
        diagnostics: vec![diag(code, message, ordinal, event_id)],
    }
}
fn no_accept(
    reason: ConsolidationNoResultReason,
    code: &str,
    message: &str,
) -> ConsolidationAcceptance {
    ConsolidationAcceptance::NoResult {
        reason,
        diagnostics: vec![diag(code, message, None, None)],
    }
}
fn digest(domain: &[u8], value: &Value) -> Result<String> {
    let mut hasher = Hasher::new();
    hasher.update(domain);
    hasher.update(canonical_json(value)?.as_bytes());
    Ok(hasher.finalize().to_hex().to_string())
}
fn input_fingerprint(
    episode: &crate::domain::Episode,
    events: &[(u64, crate::domain::CortexEvent)],
    evidence: &EvidenceService,
    normalization: &FailureNormalizationService,
) -> Result<String> {
    let members = events
        .iter()
        .map(|(ordinal, event)| {
            Ok(json!({
                "ordinal": ordinal,
                "event_id": event.id,
                "event_type": event.event_type.storage_name(),
                "payload_hash": canonical_event_payload_hash(&event.payload)?,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    digest(
        FINGERPRINT_DOMAIN,
        &json!({"workspace_id":episode.workspace_id,"episode_id":episode.id,"session_id":episode.session_id,"task_id":episode.task_id,"status":episode.status.as_str(),"creator":episode.created_by.as_str(),"version":episode.version,"members":members,"extractor":[FAILURE_TO_VERIFICATION_EXTRACTOR_ID,FAILURE_TO_VERIFICATION_EXTRACTOR_VERSION],"decoders":evidence.registry().identities(),"normalizers":normalization.normalizer_identities(),"verifier_rules":normalization.verifier_rules().catalog(),"built_in_verifiers":[[RUST_COMPILER_RULE_ID,RUST_COMPILER_RULE_VERSION],[CARGO_TEST_RULE_ID,CARGO_TEST_RULE_VERSION]],"summary_renderer":crate::domain::EXPERIENCE_SUMMARY_RENDERER_VERSION,"canonicalizer":EXPERIENCE_CANONICALIZATION_VERSION}),
    )
}
fn proposal_hash(record: &ExperienceRecord) -> Result<String> {
    let mut material = record.clone();
    material.experience.id.clear();
    material.experience.created_at = chrono::DateTime::UNIX_EPOCH;
    material.experience.consolidation_fingerprint.clear();
    material.experience.proposal_hash.clear();
    for attempt in &mut material.attempts {
        attempt.id.clear();
        attempt.experience_id.clear();
    }
    digest(
        PROPOSAL_DOMAIN,
        &serde_json::to_value(material).map_err(|e| CortexError::Analysis(e.to_string()))?,
    )
}

pub(crate) fn canonical_json(value: &Value) -> Result<String> {
    crate::domain::canonical_json(value)
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use chrono::Utc;
    use serde_json::json;
    use tempfile::TempDir;

    use super::{ConsolidationService, canonical_json, extract_proposal};
    use crate::{
        domain::{
            ConsolidationNoResultReason, ConsolidationPreview, CortexEvent, Episode,
            EpisodeCreator, EpisodeEventAssociationRequest, EpisodeStatus, EpisodeTerminalRequest,
            EpisodeType, EventType, ExperienceAssessment, ExperienceAssessmentKind,
            ExperienceEvidenceRelation, ExperienceOutcome, ProposalDisposition, Session,
            VerificationStatus, Workspace,
        },
        service::{EvidenceService, FailureNormalizationService},
        storage::SqliteStorage,
    };

    fn episode(status: EpisodeStatus) -> Episode {
        let mut episode = Episode::new(
            "workspace",
            "session",
            None,
            EpisodeType::Debugging,
            None,
            EpisodeCreator::User,
        );
        episode.id = "episode".into();
        episode.status = status;
        episode.version = 4;
        episode.ended_at = Some(Utc::now());
        episode
    }

    fn event(event_type: EventType, payload: serde_json::Value) -> CortexEvent {
        let mut event = CortexEvent::new("workspace", event_type, payload);
        event.session_id = Some("session".into());
        event
    }

    fn rust_failure(subject_kind: &str, subject: &str, code: &str) -> CortexEvent {
        event(
            EventType::CompilerResult,
            json!({
                "contract": "cortexweave.rust_compiler_result",
                "version": 1,
                "subject": {"kind": subject_kind, "value": subject},
                "exit_code": 1,
                "diagnostics": [{
                    "level": "error",
                    "code": code,
                    "message": "structured compiler failure",
                    "expected_type": if code == "E0308" { Some("String") } else { None },
                    "actual_type": if code == "E0308" { Some("u32") } else { None },
                    "path": "src/lib.rs",
                    "start_line": 4,
                    "start_column": 2
                }]
            }),
        )
    }

    fn rust_pass(subject_kind: &str, subject: &str) -> CortexEvent {
        event(
            EventType::CompilerResult,
            json!({
                "contract": "cortexweave.rust_compiler_result",
                "version": 1,
                "subject": {"kind": subject_kind, "value": subject},
                "exit_code": 0,
                "diagnostics": []
            }),
        )
    }

    fn cargo_result(subject_kind: &str, subject: &str, passed: bool, tests: u64) -> CortexEvent {
        event(
            EventType::TestResult,
            json!({
                "contract": "cortexweave.cargo_test_result",
                "version": 1,
                "subject": {"kind": subject_kind, "value": subject},
                "exit_code": if passed { 0 } else { 101 },
                "executed_test_count": tests,
                "failures": if passed { json!([]) } else { json!([{
                    "test_name": subject,
                    "assertion_class": "assert_eq",
                    "message": "observed assertion failure"
                }]) }
            }),
        )
    }

    fn tool_action(operation: &str) -> CortexEvent {
        event(
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

    fn acceptance(subject_kind: &str, subject: &str) -> CortexEvent {
        event(
            EventType::UserAcceptance,
            json!({
                "contract": "cortexweave.user_acceptance",
                "version": 1,
                "accepted": true,
                "accepted_by": "reported-user",
                "subject": {"kind": subject_kind, "value": subject}
            }),
        )
    }

    fn generic_result(tool: &str, operation: &str, passed: bool, subject: &str) -> CortexEvent {
        event(
            EventType::ExternalToolFinished,
            json!({
                "contract": "cortexweave.generic_verifier_result",
                "version": 1,
                "verifier_id": "cargo.check",
                "verifier_version": "1",
                "tool": tool,
                "operation": operation,
                "result": if passed { "passed" } else { "failed" },
                "exit_code": if passed { 0 } else { 1 },
                "subject": {"kind": "target", "value": subject}
            }),
        )
    }

    fn extract(events: Vec<CortexEvent>) -> ConsolidationPreview {
        let evidence = EvidenceService::standard().unwrap();
        let normalization = FailureNormalizationService::standard().unwrap();
        let events = events
            .into_iter()
            .enumerate()
            .map(|(ordinal, event)| (ordinal as u64, event))
            .collect::<Vec<_>>();
        extract_proposal(
            &evidence,
            &normalization,
            &episode(EpisodeStatus::Closed),
            &events,
            "a".repeat(64),
        )
        .unwrap()
    }

    fn proposal(preview: ConsolidationPreview) -> crate::domain::ExperienceProposal {
        match preview {
            ConsolidationPreview::Proposal { proposal, .. } => *proposal,
            ConsolidationPreview::NoResult {
                reason,
                diagnostics,
            } => panic!("unexpected no-result {reason:?}: {diagnostics:?}"),
        }
    }

    fn assert_no_result(preview: ConsolidationPreview, expected: ConsolidationNoResultReason) {
        match preview {
            ConsolidationPreview::NoResult { reason, .. } => assert_eq!(reason, expected),
            ConsolidationPreview::Proposal { proposal, .. } => {
                panic!(
                    "unexpected proposal: {}",
                    proposal.record.experience.summary
                )
            }
        }
    }

    struct DurableFixture {
        _directory: TempDir,
        database: PathBuf,
        storage: Arc<SqliteStorage>,
        request: crate::domain::ConsolidationRequest,
        raw_events: Vec<CortexEvent>,
    }

    fn service(storage: Arc<SqliteStorage>) -> ConsolidationService {
        ConsolidationService::new(
            storage,
            Arc::new(EvidenceService::standard().unwrap()),
            Arc::new(FailureNormalizationService::standard().unwrap()),
        )
    }

    async fn durable_fixture() -> DurableFixture {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("consolidation.sqlite");
        let storage = Arc::new(SqliteStorage::open(&database).await.unwrap());
        let workspace = Workspace::new(directory.path().to_string_lossy(), "consolidation-proof");
        storage.insert_workspace(&workspace).await.unwrap();
        let session = Session::new(&workspace.id, json!({"proof": "b6"}));
        storage.insert_session(&session).await.unwrap();
        let episode = Episode::new(
            &workspace.id,
            &session.id,
            None,
            EpisodeType::Debugging,
            Some("deterministic proof".into()),
            EpisodeCreator::NativeHarness,
        );
        storage.insert_episode(&episode).await.unwrap();
        let mut events = vec![
            rust_failure("target", "core", "E0308"),
            tool_action("edit"),
            rust_pass("target", "core"),
        ];
        for event in &mut events {
            event.workspace_id = workspace.id.clone();
            event.session_id = Some(session.id.clone());
            storage.insert_event(event).await.unwrap();
        }
        let associated = storage
            .associate_episode_events(
                &EpisodeEventAssociationRequest {
                    workspace_id: workspace.id.clone(),
                    episode_id: episode.id.clone(),
                    expected_version: episode.version,
                    request_key: "b6-associate".into(),
                    event_ids: events.iter().map(|event| event.id.clone()).collect(),
                },
                &"a".repeat(64),
                Utc::now(),
            )
            .await
            .unwrap();
        let closed = storage
            .transition_episode(
                &EpisodeTerminalRequest {
                    workspace_id: workspace.id.clone(),
                    episode_id: episode.id.clone(),
                    expected_version: associated.version,
                    request_key: "b6-close".into(),
                },
                EpisodeStatus::Closed,
                &"b".repeat(64),
                Utc::now(),
            )
            .await
            .unwrap();
        DurableFixture {
            _directory: directory,
            database,
            storage,
            request: crate::domain::ConsolidationRequest {
                workspace_id: workspace.id,
                episode_id: episode.id,
                expected_episode_version: closed.version,
            },
            raw_events: events,
        }
    }

    async fn acceptance_request(
        service: &ConsolidationService,
        request: &crate::domain::ConsolidationRequest,
    ) -> crate::domain::ConsolidationAcceptanceRequest {
        let preview = service.preview(request).await.unwrap();
        let ConsolidationPreview::Proposal {
            proposal,
            disposition,
        } = preview
        else {
            panic!("fixture must produce a proposal");
        };
        assert!(matches!(disposition, ProposalDisposition::Automatic));
        crate::domain::ConsolidationAcceptanceRequest {
            request: request.clone(),
            expected_fingerprint: proposal.fingerprint.clone(),
            expected_proposal_hash: proposal.proposal_hash.clone(),
        }
    }

    async fn accept_once(
        service: &ConsolidationService,
        request: &crate::domain::ConsolidationRequest,
    ) -> crate::domain::ExperienceRecord {
        let acceptance = acceptance_request(service, request).await;
        match service.accept(&acceptance).await.unwrap() {
            crate::domain::ConsolidationAcceptance::Accepted { record } => *record,
            result => panic!("expected accepted consolidation, got {result:?}"),
        }
    }

    async fn experience_count(storage: &SqliteStorage) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM experiences")
            .fetch_one(storage.pool())
            .await
            .unwrap()
    }

    async fn experience_fts_count(storage: &SqliteStorage) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM experience_fts")
            .fetch_one(storage.pool())
            .await
            .unwrap()
    }

    #[test]
    fn canonical_json_sorts_object_keys_without_reordering_arrays() {
        assert_eq!(
            canonical_json(&json!({"z": [2, 1], "a": {"b": true, "a": 1}})).unwrap(),
            r#"{"a":{"a":1,"b":true},"z":[2,1]}"#
        );
    }

    #[test]
    fn canonical_json_covers_number_classes_escapes_unicode_and_domain_separation() {
        let value = json!({
            "é": "line\nquote\"",
            "z": [-0.0, -2_i64, u64::MAX, 1.25]
        });
        assert_eq!(
            canonical_json(&value).unwrap(),
            "{\"z\":[-0.0,-2,18446744073709551615,1.25],\"é\":\"line\\nquote\\\"\"}"
        );
        assert_ne!(
            super::digest(super::FINGERPRINT_DOMAIN, &value).unwrap(),
            super::digest(super::PROPOSAL_DOMAIN, &value).unwrap()
        );
        assert_ne!(
            super::digest(super::FINGERPRINT_DOMAIN, &value).unwrap(),
            super::digest(
                super::FINGERPRINT_DOMAIN,
                &json!({"é": "line\nquote\"", "z": [-0.0, -2_i64, u64::MAX, 1.5]})
            )
            .unwrap()
        );
    }

    #[test]
    fn proposal_hash_ignores_generated_ids_but_not_material() {
        let events = vec![
            rust_failure("target", "core", "E0308"),
            tool_action("edit"),
            rust_pass("target", "core"),
        ];
        let first = proposal(extract(events.clone()));
        let second = proposal(extract(events));
        assert_ne!(first.record.experience.id, second.record.experience.id);
        assert_eq!(first.proposal_hash, second.proposal_hash);

        let changed = proposal(extract(vec![
            rust_failure("target", "core", "E0308"),
            tool_action("edit-config"),
            rust_pass("target", "core"),
        ]));
        assert_ne!(first.proposal_hash, changed.proposal_hash);
    }

    #[test]
    fn two_actions_are_one_associated_attempt_and_never_rendered_as_causal() {
        let proposal = proposal(extract(vec![
            rust_failure("target", "core", "E0308"),
            tool_action("edit-source"),
            tool_action("edit-config"),
            rust_pass("target", "core"),
        ]));
        assert_eq!(
            proposal.record.experience.outcome,
            ExperienceOutcome::Success
        );
        assert_eq!(proposal.record.attempts.len(), 1);
        assert_eq!(
            proposal.record.attempts[0].change_evidence_ordinals.len(),
            2
        );
        let summary = &proposal.record.experience.summary;
        assert!(summary.contains("associated attempt"));
        assert!(summary.contains("verified-after relationship"));
        assert!(summary.contains("neither a dependency"));
        for unsupported in ["fixed by", "resolved by", "proven current"] {
            assert!(!summary.contains(unsupported));
        }
    }

    #[test]
    fn exact_subject_kind_and_value_are_required_after_an_attempt() {
        assert_no_result(
            extract(vec![
                cargo_result("test", "core::works", false, 1),
                tool_action("edit"),
                cargo_result("workspace", "core::works", true, 1),
            ]),
            ConsolidationNoResultReason::AmbiguousVerificationScope,
        );
        assert_no_result(
            extract(vec![
                cargo_result("test", "core::works", false, 1),
                tool_action("edit"),
                cargo_result("test", "other::works", false, 1),
            ]),
            ConsolidationNoResultReason::AmbiguousVerificationScope,
        );
    }

    #[test]
    fn verifier_rule_identity_does_not_authorize_a_different_tool_or_operation() {
        assert_no_result(
            extract(vec![
                generic_result("cargo", "check", false, "core"),
                tool_action("edit"),
                generic_result("rustc", "check", true, "core"),
            ]),
            ConsolidationNoResultReason::NoSupportedFailure,
        );
        assert_no_result(
            extract(vec![
                generic_result("cargo", "check", false, "core"),
                tool_action("edit"),
                generic_result("cargo", "test", true, "core"),
            ]),
            ConsolidationNoResultReason::NoSupportedFailure,
        );
    }

    #[test]
    fn final_result_batch_can_prove_partial_success_without_same_subject_conflict() {
        let proposal = proposal(extract(vec![
            cargo_result("test", "core::works", false, 1),
            tool_action("edit"),
            cargo_result("test", "core::works", true, 1),
            cargo_result("test", "other::still_fails", false, 1),
        ]));
        assert_eq!(
            proposal.record.experience.outcome,
            ExperienceOutcome::PartialSuccess
        );
        assert_eq!(
            proposal.record.experience.verification.status,
            VerificationStatus::Conflicting
        );
        assert_eq!(
            proposal.record.experience.verification.observations.len(),
            2
        );
        assert_eq!(
            proposal.record.evidence.last().unwrap().relation,
            ExperienceEvidenceRelation::TerminalVerification
        );
    }

    #[test]
    fn opposing_results_for_one_subject_are_a_typed_conflict() {
        assert_no_result(
            extract(vec![
                cargo_result("test", "core::works", false, 1),
                tool_action("edit"),
                cargo_result("test", "core::works", true, 1),
                cargo_result("test", "core::works", false, 1),
            ]),
            ConsolidationNoResultReason::ConflictingResults,
        );
    }

    #[test]
    fn zero_executed_tests_cannot_prove_that_a_failure_disappeared() {
        assert_no_result(
            extract(vec![
                cargo_result("test", "core::works", false, 1),
                tool_action("edit"),
                cargo_result("test", "core::works", true, 0),
            ]),
            ConsolidationNoResultReason::EmptyVerificationRun,
        );
    }

    #[test]
    fn task_completion_and_successful_unknown_commands_are_not_verification() {
        let task_completed = event(EventType::TaskCompleted, json!({"status": "complete"}));
        assert_no_result(
            extract(vec![
                rust_failure("target", "core", "E0308"),
                tool_action("unknown-postcondition"),
                task_completed,
            ]),
            ConsolidationNoResultReason::NoVerificationEvidence,
        );
    }

    #[test]
    fn earlier_failed_attempt_is_historical_not_terminal() {
        let proposal = proposal(extract(vec![
            rust_failure("target", "core", "E0308"),
            tool_action("first-edit"),
            rust_failure("target", "core", "E0308"),
            tool_action("second-edit"),
            rust_pass("target", "core"),
        ]));
        assert_eq!(proposal.record.attempts.len(), 2);
        assert_eq!(
            proposal.record.attempts[0].result,
            crate::domain::AttemptResult::StillFailing
        );
        assert_eq!(
            proposal.record.experience.outcome,
            ExperienceOutcome::Success
        );
        assert_eq!(
            proposal.record.experience.verification.observations.len(),
            1
        );
        assert_eq!(
            proposal.record.attempts[1].following_verification_ordinal,
            Some(proposal.record.evidence.last().unwrap().ordinal)
        );
        assert_eq!(
            proposal.record.evidence.last().unwrap().relation,
            ExperienceEvidenceRelation::AttemptVerification
        );
    }

    #[test]
    fn repeated_initial_failure_is_supporting_and_changed_failure_is_historical() {
        let proposal = proposal(extract(vec![
            rust_failure("target", "core", "E0308"),
            rust_failure("target", "core", "E0308"),
            tool_action("edit"),
            rust_failure("target", "core", "E0502"),
        ]));
        assert_eq!(
            proposal.record.attempts[0].result,
            crate::domain::AttemptResult::VerificationChangedFailure
        );
        assert_eq!(
            proposal.record.experience.outcome,
            ExperienceOutcome::Failure
        );
        assert!(
            proposal
                .record
                .evidence
                .iter()
                .any(|link| link.relation == ExperienceEvidenceRelation::Supporting)
        );
    }

    #[test]
    fn exact_user_acceptance_is_review_only_and_partial_acceptance_is_rejected() {
        let preview = extract(vec![
            rust_failure("target", "core", "E0308"),
            tool_action("edit"),
            acceptance("target", "core"),
        ]);
        match preview {
            ConsolidationPreview::Proposal { disposition, .. } => assert!(matches!(
                disposition,
                ProposalDisposition::ReviewRequired { .. }
            )),
            other => panic!("unexpected preview: {other:?}"),
        }
        assert_no_result(
            extract(vec![
                rust_failure("target", "core", "E0308"),
                tool_action("edit"),
                acceptance("target", "other"),
            ]),
            ConsolidationNoResultReason::AmbiguousVerificationScope,
        );
    }

    #[tokio::test]
    async fn acceptance_is_idempotent_through_ten_retries_restart_and_assessment() {
        let fixture = durable_fixture().await;
        let consolidator = service(Arc::clone(&fixture.storage));
        let first = accept_once(&consolidator, &fixture.request).await;

        for _ in 1..10 {
            assert_eq!(accept_once(&consolidator, &fixture.request).await, first);
        }
        assert_eq!(experience_count(&fixture.storage).await, 1);
        assert_eq!(experience_fts_count(&fixture.storage).await, 1);

        let assessment = ExperienceAssessment {
            id: uuid::Uuid::new_v4().to_string(),
            workspace_id: fixture.request.workspace_id.clone(),
            experience_id: first.experience.id.clone(),
            kind: ExperienceAssessmentKind::Confirmed,
            actor: "reviewer".into(),
            reason: "idempotency proof".into(),
            replacement_experience_id: None,
            evidence_event_ids: vec![fixture.raw_events[2].id.clone()],
            created_at: Utc::now(),
        };
        fixture
            .storage
            .append_experience_assessment(&assessment)
            .await
            .unwrap();
        assert_eq!(
            fixture
                .storage
                .experience_assessments(&fixture.request.workspace_id, &first.experience.id)
                .await
                .unwrap(),
            vec![assessment]
        );
        assert_eq!(accept_once(&consolidator, &fixture.request).await, first);

        for event in &fixture.raw_events {
            assert_eq!(
                fixture
                    .storage
                    .event(&fixture.request.workspace_id, &event.id)
                    .await
                    .unwrap(),
                Some(event.clone())
            );
        }

        let restarted_storage = Arc::new(SqliteStorage::open(&fixture.database).await.unwrap());
        let restarted = service(Arc::clone(&restarted_storage));
        assert_eq!(accept_once(&restarted, &fixture.request).await, first);
        assert_eq!(experience_count(&restarted_storage).await, 1);
        assert_eq!(experience_fts_count(&restarted_storage).await, 1);
        let persisted = restarted_storage
            .experience(&fixture.request.workspace_id, &first.experience.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted, first);
        assert_eq!(
            persisted.experience.canonicalization_version,
            crate::domain::EXPERIENCE_CANONICALIZATION_VERSION
        );
    }

    #[tokio::test]
    async fn concurrent_service_instances_converge_on_one_canonical_experience() {
        let fixture = durable_fixture().await;
        let first = service(Arc::new(
            SqliteStorage::open(&fixture.database).await.unwrap(),
        ));
        let second = service(Arc::new(
            SqliteStorage::open(&fixture.database).await.unwrap(),
        ));

        let (left, right) = tokio::join!(
            accept_once(&first, &fixture.request),
            accept_once(&second, &fixture.request)
        );
        assert_eq!(left, right);
        assert_eq!(experience_count(&fixture.storage).await, 1);
        assert_eq!(experience_fts_count(&fixture.storage).await, 1);
    }

    #[tokio::test]
    async fn acceptance_revalidation_rejects_stale_mismatched_and_rolled_back_writes() {
        let fixture = durable_fixture().await;
        let consolidator = service(Arc::clone(&fixture.storage));
        let acceptance = acceptance_request(&consolidator, &fixture.request).await;
        let mut input_identity = consolidator
            .acceptance_input_identity(&fixture.request)
            .await
            .unwrap();
        input_identity.members[0].payload_hash = "0".repeat(64);
        let preview = consolidator.preview(&fixture.request).await.unwrap();
        let record = match preview {
            ConsolidationPreview::Proposal { proposal, .. } => proposal.record.clone(),
            other => panic!("expected proposal, got {other:?}"),
        };

        assert!(
            fixture
                .storage
                .insert_consolidated_experience(
                    &record,
                    fixture.request.expected_episode_version,
                    &input_identity,
                )
                .await
                .is_err()
        );
        assert_eq!(experience_count(&fixture.storage).await, 0);

        let stale = crate::domain::ConsolidationAcceptanceRequest {
            request: crate::domain::ConsolidationRequest {
                expected_episode_version: fixture.request.expected_episode_version - 1,
                ..fixture.request.clone()
            },
            ..acceptance.clone()
        };
        assert!(matches!(
            consolidator.accept(&stale).await.unwrap(),
            crate::domain::ConsolidationAcceptance::NoResult {
                reason: ConsolidationNoResultReason::StaleEpisodeVersion,
                ..
            }
        ));
        assert_eq!(experience_count(&fixture.storage).await, 0);

        let identity = consolidator
            .acceptance_input_identity(&fixture.request)
            .await
            .unwrap();
        let mut bad_evidence = record;
        bad_evidence.experience.id = uuid::Uuid::new_v4().to_string();
        bad_evidence.experience.consolidation_fingerprint = "c".repeat(64);
        bad_evidence.experience.proposal_hash = "d".repeat(64);
        for attempt in &mut bad_evidence.attempts {
            attempt.id = uuid::Uuid::new_v4().to_string();
            attempt.experience_id = bad_evidence.experience.id.clone();
        }
        bad_evidence.evidence[0].event_id = uuid::Uuid::new_v4().to_string();
        assert!(
            fixture
                .storage
                .insert_consolidated_experience(
                    &bad_evidence,
                    fixture.request.expected_episode_version,
                    &identity,
                )
                .await
                .is_err()
        );
        assert_eq!(experience_count(&fixture.storage).await, 0);
        assert_eq!(experience_fts_count(&fixture.storage).await, 0);

        let mut wrong_workspace = identity;
        wrong_workspace.workspace_id = "other-workspace".into();
        assert!(
            fixture
                .storage
                .insert_consolidated_experience(
                    &bad_evidence,
                    fixture.request.expected_episode_version,
                    &wrong_workspace,
                )
                .await
                .is_err()
        );
        assert_eq!(experience_count(&fixture.storage).await, 0);
        for event in &fixture.raw_events {
            assert_eq!(
                fixture
                    .storage
                    .event(&fixture.request.workspace_id, &event.id)
                    .await
                    .unwrap(),
                Some(event.clone())
            );
        }
    }

    #[tokio::test]
    async fn extractor_version_is_persisted_as_a_distinct_interpretation() {
        let fixture = durable_fixture().await;
        let consolidator = service(Arc::clone(&fixture.storage));
        let first = accept_once(&consolidator, &fixture.request).await;
        let preview = consolidator.preview(&fixture.request).await.unwrap();
        let mut evolved = match preview {
            ConsolidationPreview::Proposal { proposal, .. } => proposal.record.clone(),
            other => panic!("expected proposal, got {other:?}"),
        };
        evolved.experience.id = uuid::Uuid::new_v4().to_string();
        evolved.experience.extractor_version = "2".into();
        evolved.experience.consolidation_fingerprint = "e".repeat(64);
        evolved.experience.proposal_hash = "f".repeat(64);
        for attempt in &mut evolved.attempts {
            attempt.id = uuid::Uuid::new_v4().to_string();
            attempt.experience_id = evolved.experience.id.clone();
        }
        let identity = consolidator
            .acceptance_input_identity(&fixture.request)
            .await
            .unwrap();
        let stored = fixture
            .storage
            .insert_consolidated_experience(
                &evolved,
                fixture.request.expected_episode_version,
                &identity,
            )
            .await
            .unwrap();
        assert_ne!(stored.experience.id, first.experience.id);
        assert_eq!(stored.experience.extractor_version, "2");
        assert_eq!(
            stored.experience.canonicalization_version,
            crate::domain::EXPERIENCE_CANONICALIZATION_VERSION
        );
        assert_eq!(experience_count(&fixture.storage).await, 2);
        assert_eq!(experience_fts_count(&fixture.storage).await, 2);
    }
}
