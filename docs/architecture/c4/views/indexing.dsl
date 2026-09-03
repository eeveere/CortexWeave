component cortexweave.application "CortexWeave-Indexing" "Source discovery, normalized analysis, incremental reconciliation, embeddings, and derived graph projection." {
    include registeredWorkspace
    include embeddingService
    include cortexweave.database
    include cortexweave.application.mcpAdapter
    include cortexweave.application.workspaceWatcher
    include cortexweave.application.workspaceScanner
    include cortexweave.application.analyzerRegistry
    include cortexweave.application.treeSitterAnalyzers
    include cortexweave.application.genericAnalyzer
    include cortexweave.application.indexingService
    include cortexweave.application.segmenterBatcher
    include cortexweave.application.embeddingAdapter
    include cortexweave.application.graphIndexer
    include cortexweave.application.symbolRegistry
    include cortexweave.application.symbolResolver
    autoLayout lr
}
