use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{
    FailureSignature, GraphNodeType, GraphState, VerificationSubject, VerificationSubjectKind,
};

pub const MAX_EXPERIENCE_ATTEMPTS: usize = 32;
pub const MAX_EXPERIENCE_EVIDENCE: usize = 256;
pub const MAX_EXPERIENCE_SNAPSHOTS: usize = 64;
pub const MAX_EXPERIENCE_ASSESSMENT_EVIDENCE: usize = 64;
pub const DEFAULT_EXPERIENCE_ASSESSMENT_PAGE_LIMIT: usize = 20;
pub const MAX_EXPERIENCE_ASSESSMENT_PAGE_LIMIT: usize = 50;
pub const MAX_EXPERIENCE_SEARCH_LIMIT: usize = 50;
pub const MAX_EXPERIENCE_SEARCH_CANDIDATES: usize = 200;
pub const EXPERIENCE_SUMMARY_RENDERER_VERSION: &str = "2";
pub const EXPERIENCE_CANONICALIZATION_VERSION: &str = "1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceOutcome {
    Success,
    Failure,
    PartialSuccess,
    Inconclusive,
    Abandoned,
}

impl ExperienceOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::PartialSuccess => "partial_success",
            Self::Inconclusive => "inconclusive",
            Self::Abandoned => "abandoned",
        }
    }
    pub fn from_storage(value: &str) -> Self {
        match value {
            "success" => Self::Success,
            "failure" => Self::Failure,
            "partial_success" => Self::PartialSuccess,
            "abandoned" => Self::Abandoned,
            _ => Self::Inconclusive,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptResult {
    StillFailing,
    VerificationPassed,
    VerificationChangedFailure,
    Inconclusive,
}

impl AttemptResult {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StillFailing => "still_failing",
            Self::VerificationPassed => "verification_passed",
            Self::VerificationChangedFailure => "verification_changed_failure",
            Self::Inconclusive => "inconclusive",
        }
    }
    pub fn from_storage(value: &str) -> Self {
        match value {
            "still_failing" => Self::StillFailing,
            "verification_passed" => Self::VerificationPassed,
            "verification_changed_failure" => Self::VerificationChangedFailure,
            _ => Self::Inconclusive,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    VerifiedPassed,
    VerifiedFailed,
    ExplicitlyAccepted,
    Conflicting,
    Missing,
    Unsupported,
}

impl VerificationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VerifiedPassed => "verified_passed",
            Self::VerifiedFailed => "verified_failed",
            Self::ExplicitlyAccepted => "explicitly_accepted",
            Self::Conflicting => "conflicting",
            Self::Missing => "missing",
            Self::Unsupported => "unsupported",
        }
    }
    pub fn from_storage(value: &str) -> Self {
        match value {
            "verified_passed" => Self::VerifiedPassed,
            "verified_failed" => Self::VerifiedFailed,
            "explicitly_accepted" => Self::ExplicitlyAccepted,
            "conflicting" => Self::Conflicting,
            "missing" => Self::Missing,
            _ => Self::Unsupported,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationKind {
    RustCompiler,
    CargoTest,
    RegisteredTool,
    UserAcceptance,
    None,
}

impl VerificationKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RustCompiler => "rust_compiler",
            Self::CargoTest => "cargo_test",
            Self::RegisteredTool => "registered_tool",
            Self::UserAcceptance => "user_acceptance",
            Self::None => "none",
        }
    }
    pub fn from_storage(value: &str) -> Self {
        match value {
            "rust_compiler" => Self::RustCompiler,
            "cargo_test" => Self::CargoTest,
            "registered_tool" => Self::RegisteredTool,
            "user_acceptance" => Self::UserAcceptance,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStrength {
    Strong,
    Moderate,
    Weak,
    Unsupported,
}

impl EvidenceStrength {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Moderate => "moderate",
            Self::Weak => "weak",
            Self::Unsupported => "unsupported",
        }
    }
    pub fn from_storage(value: &str) -> Self {
        match value {
            "strong" => Self::Strong,
            "moderate" => Self::Moderate,
            "weak" => Self::Weak,
            _ => Self::Unsupported,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceBasis {
    DeterministicVerifier,
    RepeatedDeterministicEvidence,
    ExplicitUserAcceptance,
    ExplicitHarnessAssertion,
    TemporalAssociation,
    StructuralAssociation,
}

impl EvidenceBasis {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeterministicVerifier => "deterministic_verifier",
            Self::RepeatedDeterministicEvidence => "repeated_deterministic_evidence",
            Self::ExplicitUserAcceptance => "explicit_user_acceptance",
            Self::ExplicitHarnessAssertion => "explicit_harness_assertion",
            Self::TemporalAssociation => "temporal_association",
            Self::StructuralAssociation => "structural_association",
        }
    }
    pub fn from_storage(value: &str) -> Self {
        match value {
            "deterministic_verifier" => Self::DeterministicVerifier,
            "repeated_deterministic_evidence" => Self::RepeatedDeterministicEvidence,
            "explicit_user_acceptance" => Self::ExplicitUserAcceptance,
            "explicit_harness_assertion" => Self::ExplicitHarnessAssertion,
            "temporal_association" => Self::TemporalAssociation,
            _ => Self::StructuralAssociation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceEvidenceRelation {
    InitialFailure,
    AttemptChange,
    AttemptVerification,
    TerminalVerification,
    Supporting,
}

impl ExperienceEvidenceRelation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InitialFailure => "initial_failure",
            Self::AttemptChange => "attempt_change",
            Self::AttemptVerification => "attempt_verification",
            Self::TerminalVerification => "terminal_verification",
            Self::Supporting => "supporting",
        }
    }
    pub fn from_storage(value: &str) -> Self {
        match value {
            "initial_failure" => Self::InitialFailure,
            "attempt_change" => Self::AttemptChange,
            "attempt_verification" => Self::AttemptVerification,
            "terminal_verification" => Self::TerminalVerification,
            _ => Self::Supporting,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceAssessmentKind {
    Disputed,
    Refuted,
    Superseded,
    Confirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceLifecycle {
    Active,
    Disputed,
    Refuted,
    Superseded,
}

pub fn experience_lifecycle(assessments: &[ExperienceAssessment]) -> ExperienceLifecycle {
    if assessments
        .iter()
        .any(|assessment| assessment.kind == ExperienceAssessmentKind::Superseded)
    {
        ExperienceLifecycle::Superseded
    } else if assessments
        .iter()
        .any(|assessment| assessment.kind == ExperienceAssessmentKind::Refuted)
    {
        ExperienceLifecycle::Refuted
    } else if assessments
        .iter()
        .any(|assessment| assessment.kind == ExperienceAssessmentKind::Disputed)
    {
        ExperienceLifecycle::Disputed
    } else {
        ExperienceLifecycle::Active
    }
}

impl ExperienceAssessmentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disputed => "disputed",
            Self::Refuted => "refuted",
            Self::Superseded => "superseded",
            Self::Confirmed => "confirmed",
        }
    }
    pub fn from_storage(value: &str) -> Self {
        match value {
            "disputed" => Self::Disputed,
            "refuted" => Self::Refuted,
            "superseded" => Self::Superseded,
            _ => Self::Confirmed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceVerification {
    pub status: VerificationStatus,
    pub observations: Vec<ExperienceVerificationObservation>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceVerificationObservation {
    pub ordinal: u64,
    pub status: VerificationStatus,
    pub kind: VerificationKind,
    pub subject: VerificationSubject,
    pub evidence_event_id: String,
    pub rule_id: String,
    pub rule_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceStrengthAssessment {
    pub strength: EvidenceStrength,
    pub bases: Vec<EvidenceBasis>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Experience {
    pub id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub episode_id: String,
    pub failure_signature: Option<FailureSignature>,
    pub outcome: ExperienceOutcome,
    pub verification: ExperienceVerification,
    pub summary: String,
    pub evidence_strength: EvidenceStrengthAssessment,
    pub extractor_id: String,
    pub extractor_version: String,
    pub summary_renderer_version: String,
    pub canonicalization_version: String,
    pub consolidation_fingerprint: String,
    pub proposal_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceDraft {
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub episode_id: String,
    pub failure_signature: Option<FailureSignature>,
    pub outcome: ExperienceOutcome,
    pub verification: ExperienceVerification,
    pub evidence_strength: EvidenceStrengthAssessment,
    pub extractor_id: String,
    pub extractor_version: String,
    pub canonicalization_version: String,
    pub consolidation_fingerprint: String,
    pub proposal_hash: String,
    pub attempts: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceAttempt {
    pub id: String,
    pub workspace_id: String,
    pub experience_id: String,
    pub ordinal: u64,
    pub result: AttemptResult,
    pub change_evidence_ordinals: Vec<u64>,
    pub following_verification_ordinal: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceEvidenceLink {
    pub ordinal: u64,
    pub relation: ExperienceEvidenceRelation,
    pub event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceCodeSnapshot {
    pub ordinal: u64,
    pub source_event_id: String,
    pub relative_path: String,
    pub workspace_content_revision: i64,
    pub document_content_revision: i64,
    pub document_content_hash: String,
    pub content: String,
    pub chunk_stable_key: Option<String>,
    pub chunk_content_hash: Option<String>,
    pub symbol_logical_key: Option<String>,
    pub symbol_label: Option<String>,
    pub source_start_byte: Option<i64>,
    pub source_end_byte: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceGraphSnapshot {
    pub ordinal: u64,
    pub code_snapshot_ordinal: u64,
    pub graph_content_revision: i64,
    pub graph_schema_version: i64,
    pub graph_state: GraphState,
    pub analyzer_id: String,
    pub analyzer_version: String,
    pub structure_version: String,
    pub node_stable_key: String,
    pub node_type: GraphNodeType,
    pub resolution_provenance: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceAssessment {
    pub id: String,
    pub workspace_id: String,
    pub experience_id: String,
    pub kind: ExperienceAssessmentKind,
    pub actor: String,
    pub reason: String,
    pub replacement_experience_id: Option<String>,
    pub evidence_event_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceAssessmentCursor {
    pub created_at: DateTime<Utc>,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceAssessmentPage {
    pub assessments: Vec<ExperienceAssessment>,
    pub next_cursor: Option<ExperienceAssessmentCursor>,
}

/// An explicit human or policy review that may change an Experience's
/// eligibility. Proposal-producing analyzers cannot construct this record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceAssessmentReviewRequest {
    pub workspace_id: String,
    pub experience_id: String,
    pub kind: ExperienceAssessmentKind,
    pub reviewed_by: String,
    pub request_key: String,
    pub reason: String,
    pub replacement_experience_id: Option<String>,
    pub evidence_event_ids: Vec<String>,
}

/// Read-only input to the deterministic recurring-failure detector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceDisputeProposalRequest {
    pub workspace_id: String,
    pub failure_signature: FailureSignature,
    pub recurring_failure_event_ids: Vec<String>,
}

/// A recommendation for review. It is deliberately not an assessment and
/// does not alter lifecycle or context eligibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceDisputeProposal {
    pub experience_id: String,
    pub lifecycle: ExperienceLifecycle,
    pub prior_verified_at: DateTime<Utc>,
    pub failure_signature: FailureSignature,
    pub recurring_failure_event_ids: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceRecord {
    pub experience: Experience,
    pub attempts: Vec<ExperienceAttempt>,
    pub evidence: Vec<ExperienceEvidenceLink>,
    pub code_snapshots: Vec<ExperienceCodeSnapshot>,
    pub graph_snapshots: Vec<ExperienceGraphSnapshot>,
}

/// A workspace-scoped deterministic search request. Empty filter vectors mean
/// "any" except lifecycle, where empty means active records only unless
/// `include_historical` was explicitly requested.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceSearchRequest {
    pub workspace_id: String,
    pub query: Option<String>,
    pub exact_failure_signature: Option<FailureSignature>,
    pub compatible_components: BTreeMap<String, String>,
    pub path: Option<String>,
    pub graph_stable_key: Option<String>,
    pub outcomes: Vec<ExperienceOutcome>,
    pub strengths: Vec<EvidenceStrength>,
    pub lifecycles: Vec<ExperienceLifecycle>,
    #[serde(default)]
    pub include_historical: bool,
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceSearchScores {
    pub exact_signature: u32,
    pub compatible_components: u32,
    pub lexical: u32,
    pub exact_path: u32,
    pub graph_stable_key: u32,
    pub recency: u32,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceSearchExplanation {
    pub scores: ExperienceSearchScores,
    pub matched_components: BTreeMap<String, String>,
    pub lexical_match: bool,
    pub eligibility: ExperienceEligibilityExplanation,
    pub normal_context: ExperienceNormalContextEligibility,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceEligibilityInclusion {
    ActiveDefault,
    ExplicitLifecycle,
    HistoricalInspection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceEligibilityExplanation {
    pub lifecycle: ExperienceLifecycle,
    pub inclusion: ExperienceEligibilityInclusion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceNormalContextReason {
    ActiveLifecycle,
    DisputedLifecycle,
    RefutedLifecycle,
    SupersededLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperienceNormalContextEligibility {
    pub eligible: bool,
    pub lifecycle: ExperienceLifecycle,
    pub reason: ExperienceNormalContextReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceSearchHit {
    pub experience: Experience,
    pub lifecycle: ExperienceLifecycle,
    pub explanation: ExperienceSearchExplanation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoricalReferenceStatus {
    Current,
    ContentChanged,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoricalGraphReferenceStatus {
    NotCaptured,
    Current,
    Missing,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceReferenceResolution {
    pub snapshot_ordinal: u64,
    pub graph_snapshot_ordinal: Option<u64>,
    pub relative_path: String,
    pub historical_document_content_revision: i64,
    pub historical_document_content_hash: String,
    pub current_document_content_revision: Option<i64>,
    pub current_document_content_hash: Option<String>,
    pub status: HistoricalReferenceStatus,
    pub historical_graph_stable_key: Option<String>,
    pub graph_status: HistoricalGraphReferenceStatus,
    pub current_graph_node_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceExplanation {
    pub record: ExperienceRecord,
    pub assessments: Vec<ExperienceAssessment>,
    pub lifecycle: ExperienceLifecycle,
    pub normal_context: ExperienceNormalContextEligibility,
    pub references: Vec<ExperienceReferenceResolution>,
    pub search: Option<ExperienceSearchExplanation>,
}

impl Experience {
    pub fn from_draft(draft: ExperienceDraft) -> Self {
        let summary = render_summary(
            draft.failure_signature.as_ref(),
            draft.outcome,
            &draft.verification,
            draft.attempts,
        );
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: draft.workspace_id,
            session_id: draft.session_id,
            task_id: draft.task_id,
            episode_id: draft.episode_id,
            failure_signature: draft.failure_signature,
            outcome: draft.outcome,
            verification: draft.verification,
            summary,
            evidence_strength: draft.evidence_strength,
            extractor_id: draft.extractor_id,
            extractor_version: draft.extractor_version,
            summary_renderer_version: EXPERIENCE_SUMMARY_RENDERER_VERSION.into(),
            canonicalization_version: draft.canonicalization_version,
            consolidation_fingerprint: draft.consolidation_fingerprint,
            proposal_hash: draft.proposal_hash,
            created_at: Utc::now(),
        }
    }
}

pub fn render_summary(
    signature: Option<&FailureSignature>,
    outcome: ExperienceOutcome,
    verification: &ExperienceVerification,
    attempts: usize,
) -> String {
    let observed = match signature {
        Some(signature) => format!("Observed {} failure", signature.domain.as_str()),
        None => "Observed no supported failure signature".into(),
    };
    let scope = signature
        .and_then(|value| {
            value
                .scope
                .target
                .as_deref()
                .or(value.scope.path.as_deref())
        })
        .map(|value| format!(" in {value}"))
        .unwrap_or_default();
    let verified_scope = if verification.observations.is_empty() {
        String::new()
    } else {
        let subjects = verification
            .observations
            .iter()
            .map(|observation| {
                format!(
                    "{} {}",
                    verification_subject_kind_name(observation.subject.kind),
                    observation.subject.value
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(" for {subjects}")
    };
    format!(
        "{observed}{scope}. {attempts} associated attempt(s) were recorded. Terminal evidence observed {}{verified_scope}. The episode interpretation is {} within those stated scopes. This records a verified-after relationship; it establishes neither a dependency on any action nor the current state.",
        verification.status.as_str(),
        outcome.as_str()
    )
}

fn verification_subject_kind_name(kind: VerificationSubjectKind) -> &'static str {
    match kind {
        VerificationSubjectKind::Workspace => "workspace",
        VerificationSubjectKind::Package => "package",
        VerificationSubjectKind::Target => "target",
        VerificationSubjectKind::Test => "test",
        VerificationSubjectKind::Path => "path",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_is_deterministic_and_does_not_claim_causation() {
        let verification = ExperienceVerification {
            status: VerificationStatus::VerifiedPassed,
            observations: Vec::new(),
            reasons: vec!["registered verifier".into()],
        };
        let first = render_summary(None, ExperienceOutcome::Success, &verification, 2);
        assert_eq!(
            first,
            render_summary(None, ExperienceOutcome::Success, &verification, 2)
        );
        assert!(first.contains("associated attempt"));
        assert!(first.contains("verified-after relationship"));
        assert!(first.contains("neither a dependency"));
        assert!(first.contains("nor the current state"));
    }

    #[test]
    fn confirmation_never_erases_a_negative_lifecycle_assessment() {
        let assessment = |kind| ExperienceAssessment {
            id: Uuid::new_v4().to_string(),
            workspace_id: "workspace".into(),
            experience_id: "experience".into(),
            kind,
            actor: "reviewer".into(),
            reason: "review".into(),
            replacement_experience_id: None,
            evidence_event_ids: Vec::new(),
            created_at: Utc::now(),
        };
        assert_eq!(
            experience_lifecycle(&[
                assessment(ExperienceAssessmentKind::Disputed),
                assessment(ExperienceAssessmentKind::Confirmed),
            ]),
            ExperienceLifecycle::Disputed
        );
        assert_eq!(
            experience_lifecycle(&[
                assessment(ExperienceAssessmentKind::Refuted),
                assessment(ExperienceAssessmentKind::Disputed),
            ]),
            ExperienceLifecycle::Refuted
        );
    }
}
