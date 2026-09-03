mod consolidation;
mod context;
mod cortex;
mod evidence;
mod experience_assessment;
mod experience_search;
mod failure;
mod harness;
mod memory;
mod structural;

pub use consolidation::ConsolidationService;
pub use context::ContextService;
pub use cortex::{
    AnalyzerRecommendation, CortexWeaveService, GraphLanguageStatus, LanguageReadiness,
    RebuildCost, WorkspaceCatalog, WorkspaceGraphStatus, WorkspaceReadiness, WorkspaceStatus,
};
pub use evidence::{EventEvidenceDecoder, EventEvidenceDecoderRegistry, EvidenceService};
pub use experience_assessment::ExperienceAssessmentService;
pub use experience_search::ExperienceSearchService;
pub use failure::{
    FailureNormalizationService, FailureNormalizer, FailureNormalizerRegistry, VerifierRuleRegistry,
};
pub use harness::{
    ContextSufficiencyAssessment, HarnessContext, HarnessContextPolicy, HarnessContextRequest,
    HarnessHydrationRequest, HarnessSelectedSource, HydratedContextSource, HydrationAuthorization,
    HydrationScoreProvenance,
};
pub use memory::{
    MemoryConsolidationProposal, MemoryConsolidationReport, MemoryConsolidationRequest,
    MemoryContradiction, MemorySupersessionReviewRequest, MemoryTrustReviewRequest,
    ProposedMemorySupersession,
};
pub use structural::StructuralService;
