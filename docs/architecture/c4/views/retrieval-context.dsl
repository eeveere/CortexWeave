component cortexweave.application "CortexWeave-Context" "Retrieval and context assembly. Current code/state/evidence remain authoritative; historical Experience is separately bounded supplemental input." {
    include agentHarness
    include cortexweave.database
    include embeddingService
    include cortexweave.application.mcpAdapter
    include cortexweave.application.facade
    include cortexweave.application.contextService
    include cortexweave.application.retrievalService
    include cortexweave.application.embeddingAdapter
    include cortexweave.application.structuralService
    include cortexweave.application.stateMemoryService
    include cortexweave.application.experienceSearchService
    autoLayout lr
}
