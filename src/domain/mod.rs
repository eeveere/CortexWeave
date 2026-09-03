mod chunk;
mod consolidation;
mod context;
mod episode;
mod evidence;
mod experience;
mod failure;
mod graph;
mod models;
mod repair;
mod structural;

pub use chunk::{
    AnalysisResult, AnalyzedChunk, AnalyzedRelationship, AnalyzedSymbol, AnalyzerCapabilities,
    RelationshipTarget, SymbolKind,
};
pub use consolidation::{
    ConsolidationAcceptance, ConsolidationAcceptanceRequest, ConsolidationDiagnostic,
    ConsolidationNoResultReason, ConsolidationPreview, ConsolidationRequest, ExperienceProposal,
    FAILURE_TO_VERIFICATION_EXTRACTOR_ID, FAILURE_TO_VERIFICATION_EXTRACTOR_VERSION,
    ProposalDisposition,
};
pub(crate) use consolidation::{
    ConsolidationInputIdentity, ConsolidationInputMember, canonical_event_payload_hash,
    canonical_json,
};
pub(crate) use context::TemporalBounds;
pub use context::{
    Checkpoint, ContextCandidate, ContextCandidatePool, ContextExplanation, ContextFreshness,
    ContextItem, ContextPacket, ContextPin, ContextRequest, ContextScores,
    ContextSelectionExplanation, ContextSelectionReason, ContextSourceType,
    DEFAULT_CONTEXT_TOKEN_BUDGET, ExperienceContextAuthority, ExperienceContextDegradation,
    ExperienceContextExplanation, ExperienceContextSelectionExplanation, MAX_CONTEXT_ITEM_BYTES,
    MAX_CONTEXT_SCOPE_ITEMS, MAX_CONTEXT_SCOPE_VALUE_BYTES, MAX_CONTEXT_TOKEN_BUDGET,
    MemorySupersession, RecentChange, RecentWindow, ResumeContext, ResumeContextRequest,
    ResumeSessionSelection, ResumeTaskSelection, TemporalContextItem, TemporalFilter,
    TemporalQuery, TemporalSessionScope, WorkingSetEntry, WorkingSetSnapshot,
};
pub use episode::{
    Episode, EpisodeCreator, EpisodeEvent, EpisodeEventAssociationRequest, EpisodeListRequest,
    EpisodeStartRequest, EpisodeStatus, EpisodeTerminalRequest, EpisodeType, MAX_EPISODE_EVENTS,
};
pub use evidence::{
    CargoTestResultEvidence, DecodedEvidence, DiagnosticLevel, EvidenceContract,
    EvidenceDecodeIssue, EvidenceDecodeResult, EvidenceEnvelope, EvidenceObservation,
    EvidenceSubject, EvidenceSubjectKind, GenericVerifierResultEvidence,
    MAX_EVIDENCE_IDENTIFIER_BYTES, MAX_EVIDENCE_PAYLOAD_BYTES, MAX_EVIDENCE_TEXT_BYTES,
    MAX_RUST_DIAGNOSTICS, MAX_TEST_FAILURES, ProcessOutcome, RustCompilerResultEvidence,
    RustDiagnosticEvidence, SourceChangeEvidence, SourceChangeKind, TestFailureEvidence,
    ToolCompletionEvidence, UnsupportedEvidenceReason, UserAcceptanceEvidence, VerificationResult,
};
pub use experience::{
    AttemptResult, DEFAULT_EXPERIENCE_ASSESSMENT_PAGE_LIMIT, EXPERIENCE_CANONICALIZATION_VERSION,
    EXPERIENCE_SUMMARY_RENDERER_VERSION, EvidenceBasis, EvidenceStrength,
    EvidenceStrengthAssessment, Experience, ExperienceAssessment, ExperienceAssessmentCursor,
    ExperienceAssessmentKind, ExperienceAssessmentPage, ExperienceAssessmentReviewRequest,
    ExperienceAttempt, ExperienceCodeSnapshot, ExperienceDisputeProposal,
    ExperienceDisputeProposalRequest, ExperienceDraft, ExperienceEligibilityExplanation,
    ExperienceEligibilityInclusion, ExperienceEvidenceLink, ExperienceEvidenceRelation,
    ExperienceExplanation, ExperienceGraphSnapshot, ExperienceLifecycle,
    ExperienceNormalContextEligibility, ExperienceNormalContextReason, ExperienceOutcome,
    ExperienceRecord, ExperienceReferenceResolution, ExperienceSearchExplanation,
    ExperienceSearchHit, ExperienceSearchRequest, ExperienceSearchScores, ExperienceVerification,
    ExperienceVerificationObservation, HistoricalGraphReferenceStatus, HistoricalReferenceStatus,
    MAX_EXPERIENCE_ASSESSMENT_EVIDENCE, MAX_EXPERIENCE_ASSESSMENT_PAGE_LIMIT,
    MAX_EXPERIENCE_ATTEMPTS, MAX_EXPERIENCE_EVIDENCE, MAX_EXPERIENCE_SEARCH_CANDIDATES,
    MAX_EXPERIENCE_SEARCH_LIMIT, MAX_EXPERIENCE_SNAPSHOTS, VerificationKind, VerificationStatus,
    experience_lifecycle, render_summary,
};
pub use failure::{
    FAILURE_SIGNATURE_ENCODING_VERSION, FailureDomain, FailureIdentityCapability,
    FailureNormalization, FailureNormalizationDiagnostic, FailureNormalizationResult, FailureScope,
    FailureSignature, MAX_FAILURE_COMPONENT_BYTES, MAX_FAILURE_COMPONENTS,
    UnsupportedFailureNormalization, VerificationSubject, VerificationSubjectKind, VerifierRule,
};
pub use graph::{
    GraphAnalysisExpectation, GraphAnalysisState, GraphEdge, GraphEdgeType, GraphNode,
    GraphNodeType, GraphRelationshipFact, GraphState, RelationshipTargetKind, ResolutionAlias,
    ResolutionBasis, ResolutionOutcome, ResolutionRequest, UnresolvedRelationship,
    WorkspaceGraphRevision,
};
pub use models::{
    CortexEvent, Document, EmbeddingRecord, EventType, MemoryClaim, MemoryKind, MemoryOrigin,
    MemoryRecord, MemoryTrust, MemoryTrustReview, Session, SourceSegment, StoredChunk, Task,
    TaskStatus, Workspace,
};
pub use repair::{
    GraphProjectionManifest, GraphRepairDisposition, GraphRepairDocumentPlan,
    GraphRepairGeneration, GraphRepairMode, GraphRepairOutcome, GraphRepairPlan, GraphRepairReason,
    GraphRepairState,
};
pub use structural::{
    ImpactItem, ImpactReport, MAX_STRUCTURAL_DEPTH, MAX_STRUCTURAL_EDGES, MAX_STRUCTURAL_NODES,
    StructuralDirection, StructuralEvidence, StructuralPath, StructuralReadOptions,
    StructuralResult,
};
