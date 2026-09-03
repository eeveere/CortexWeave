component cortexweave.application "CortexWeave-Structural" "Derived structural intelligence: normalized analysis is projected, deterministically resolved, and read through freshness-gated navigation and impact operations." {
    include cortexweave.database
    include cortexweave.application.indexingService
    include cortexweave.application.treeSitterAnalyzers
    include cortexweave.application.genericAnalyzer
    include cortexweave.application.graphIndexer
    include cortexweave.application.symbolRegistry
    include cortexweave.application.symbolResolver
    include cortexweave.application.structuralService
    include cortexweave.application.retrievalService
    include cortexweave.application.contextService
    autoLayout lr
}
