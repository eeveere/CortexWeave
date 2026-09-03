pub mod adapters;
pub mod config;
pub mod domain;
pub mod embedding;
pub mod error;
pub mod evaluation;
pub mod graph;
pub mod indexing;
pub mod instrumentation;
pub mod parsing;
pub mod retrieval;
pub mod service;
pub mod storage;
pub mod workspace;

pub use config::AppConfig;
pub use error::{CortexError, Result};
pub use service::{
    AnalyzerRecommendation, ConsolidationService, ContextService, ContextSufficiencyAssessment,
    CortexWeaveService, EventEvidenceDecoder, EventEvidenceDecoderRegistry, EvidenceService,
    ExperienceSearchService, FailureNormalizationService, FailureNormalizer,
    FailureNormalizerRegistry, GraphLanguageStatus, HarnessContext, HarnessContextPolicy,
    HarnessContextRequest, HarnessHydrationRequest, HarnessSelectedSource, HydratedContextSource,
    HydrationAuthorization, HydrationScoreProvenance, LanguageReadiness,
    MemoryConsolidationProposal, MemoryConsolidationReport, MemoryConsolidationRequest,
    MemoryContradiction, MemorySupersessionReviewRequest, MemoryTrustReviewRequest,
    ProposedMemorySupersession, RebuildCost, StructuralService, VerifierRuleRegistry,
    WorkspaceGraphStatus, WorkspaceReadiness,
};
