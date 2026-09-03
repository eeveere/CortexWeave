component cortexweave.application "CortexWeave-Experience" "Verified Experience is a historical, immutable interpretation of explicit factual Events. It supplements but cannot displace current evidence in ContextService." {
    include agentHarness
    include cortexweave.database
    include cortexweave.application.mcpAdapter
    include cortexweave.application.facade
    include cortexweave.application.stateMemoryService
    include cortexweave.application.evidenceService
    include cortexweave.application.failureNormalizationService
    include cortexweave.application.episodeService
    include cortexweave.application.consolidationService
    include cortexweave.application.experienceSearchService
    include cortexweave.application.experienceAssessmentService
    include cortexweave.application.contextService
    autoLayout lr
}
