mod chunk;
mod context;
mod graph;
mod models;
mod repair;
mod structural;

pub use chunk::{
    AnalysisResult, AnalyzedChunk, AnalyzedRelationship, AnalyzedSymbol, AnalyzerCapabilities,
    RelationshipTarget, SymbolKind,
};
pub(crate) use context::TemporalBounds;
pub use context::{
    Checkpoint, ContextCandidate, ContextCandidatePool, ContextExplanation, ContextFreshness,
    ContextItem, ContextPacket, ContextPin, ContextRequest, ContextScores,
    ContextSelectionExplanation, ContextSelectionReason, ContextSourceType,
    DEFAULT_CONTEXT_TOKEN_BUDGET, MemorySupersession, RecentChange, RecentWindow, ResumeContext,
    ResumeContextRequest, ResumeSessionSelection, ResumeTaskSelection, TemporalContextItem,
    TemporalFilter, TemporalQuery, TemporalSessionScope, WorkingSetEntry, WorkingSetSnapshot,
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
