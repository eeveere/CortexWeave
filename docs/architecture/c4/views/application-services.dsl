component cortexweave.application "CortexWeave-Application" "C4 Level 3: major component groups inside the single Rust application." {
    include cortexweave.application.cliAdapter
    include cortexweave.application.mcpAdapter
    include cortexweave.application.facade
    include cortexweave.application.workspaceService
    include cortexweave.application.workspaceScanner
    include cortexweave.application.analyzerRegistry
    include cortexweave.application.workspaceWatcher
    include cortexweave.application.indexingService
    include cortexweave.application.embeddingAdapter
    include cortexweave.application.graphIndexer
    include cortexweave.application.structuralService
    include cortexweave.application.retrievalService
    include cortexweave.application.contextService
    include cortexweave.application.stateMemoryService
    include cortexweave.application.evidenceService
    include cortexweave.application.episodeService
    include cortexweave.application.consolidationService
    include cortexweave.application.experienceSearchService
    include cortexweave.application.experienceAssessmentService
    include cortexweave.database
    include registeredWorkspace
    include embeddingService
    include agentHarness
    autoLayout lr
}
