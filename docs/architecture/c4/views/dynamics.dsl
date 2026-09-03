dynamic cortexweave.application "Indexing-Initial" "Initial explicit workspace reindex; source reconciliation precedes coherent graph repair." {
    operator -> cortexweave.application.cliAdapter "1. Requests reindex"
    cortexweave.application.cliAdapter -> cortexweave.application.facade "2. Dispatches reindex command"
    cortexweave.application.facade -> cortexweave.application.indexingService "3. Requests workspace reindex"
    cortexweave.application.indexingService -> cortexweave.application.workspaceScanner "4. Scans eligible files"
    cortexweave.application.workspaceScanner -> registeredWorkspace "5. Reads source files"
    cortexweave.application.indexingService -> cortexweave.application.analyzerRegistry "6. Selects analyzer"
    cortexweave.application.analyzerRegistry -> cortexweave.application.treeSitterAnalyzers "7. Produces normalized analysis (or generic fallback)"
    cortexweave.application.indexingService -> cortexweave.application.segmenterBatcher "8. Segments changed chunks"
    cortexweave.application.segmenterBatcher -> cortexweave.application.embeddingAdapter "9. Batches changed text"
    cortexweave.application.embeddingAdapter -> embeddingService "10. Generates document vectors"
    cortexweave.application.indexingService -> cortexweave.database "11. Atomically persists source index"
    cortexweave.application.indexingService -> cortexweave.application.graphIndexer "12. Runs graph repair/reprojection"
    cortexweave.application.graphIndexer -> cortexweave.database "13. Publishes graph projection/revision when complete"
    autoLayout lr
}

dynamic cortexweave.application "Indexing-Incremental" "Incremental source change while an MCP server is running; watcher ownership is limited to that MCP process." {
    registeredWorkspace -> cortexweave.application.workspaceWatcher "1. Emits filesystem notification"
    cortexweave.application.workspaceWatcher -> cortexweave.application.indexingService "2. Debounces/coalesces path and requests reconciliation"
    cortexweave.application.indexingService -> cortexweave.application.analyzerRegistry "3. Selects analyzer for changed path"
    cortexweave.application.analyzerRegistry -> cortexweave.application.treeSitterAnalyzers "4. Produces normalized analysis"
    cortexweave.application.indexingService -> cortexweave.application.segmenterBatcher "5. Embeds changed chunks only"
    cortexweave.application.segmenterBatcher -> cortexweave.application.embeddingAdapter "6. Requests vectors"
    cortexweave.application.indexingService -> cortexweave.database "7. Applies source reconciliation transaction"
    cortexweave.application.indexingService -> cortexweave.application.graphIndexer "8. Reconciles graph projection"
    cortexweave.application.graphIndexer -> cortexweave.database "9. Persists graph facts/revision state"
    autoLayout lr
}

dynamic cortexweave.application "Context-Semantic" "semantic_context: current candidate sources are ranked/budgeted first; eligible Experience is historical supplemental context." {
    agentHarness -> cortexweave.application.mcpAdapter "1. Requests semantic_context"
    cortexweave.application.mcpAdapter -> cortexweave.application.facade "2. Dispatches validated request"
    cortexweave.application.facade -> cortexweave.application.contextService "3. Requests bounded ContextPacket"
    cortexweave.application.contextService -> cortexweave.application.retrievalService "4. Acquires code/document candidates"
    cortexweave.application.retrievalService -> cortexweave.application.embeddingAdapter "5. Embeds query when semantic search applies"
    cortexweave.application.retrievalService -> cortexweave.database "6. Reads compatible vectors and lexical candidates"
    cortexweave.application.contextService -> cortexweave.application.stateMemoryService "7. Acquires current state, trusted memory, Events, pins/checkpoints"
    cortexweave.application.contextService -> cortexweave.application.structuralService "8. Acquires bounded structural evidence"
    cortexweave.application.contextService -> cortexweave.application.experienceSearchService "9. Searches eligible Experience only when requested/eligible"
    cortexweave.application.contextService -> cortexweave.application.mcpAdapter "10. Returns ContextPacket after current-first ranking and historical budgeting"
    cortexweave.application.mcpAdapter -> agentHarness "11. Returns bounded explainable context"
    autoLayout lr
}

dynamic cortexweave.application "Graph-Repair" "Graph-only repair/reprojection preserves compatible source index state and publishes graph state only through durable repair/revision rules." {
    operator -> cortexweave.application.cliAdapter "1. Requests graph rebuild"
    cortexweave.application.cliAdapter -> cortexweave.application.facade "2. Dispatches repair request"
    cortexweave.application.facade -> cortexweave.application.indexingService "3. Requests graph repair plan"
    cortexweave.application.indexingService -> cortexweave.database "4. Acquires durable repair lease/generation"
    cortexweave.application.indexingService -> cortexweave.application.analyzerRegistry "5. Verifies source/analyzer expectations"
    cortexweave.application.indexingService -> cortexweave.application.graphIndexer "6. Reprojects normalized analysis"
    cortexweave.application.graphIndexer -> cortexweave.application.symbolRegistry "7. Preserves stable node identities"
    cortexweave.application.graphIndexer -> cortexweave.application.symbolResolver "8. Resolves current relationship surface"
    cortexweave.application.graphIndexer -> cortexweave.database "9. Persists coherent projection and revision state"
    autoLayout lr
}

dynamic cortexweave.application "Experience-Consolidation" "Verified Experience is created only from a caller-controlled terminal Episode and explicit matching acceptance." {
    agentHarness -> cortexweave.application.mcpAdapter "1. Records factual Events and controls an Episode"
    cortexweave.application.mcpAdapter -> cortexweave.application.facade "2. Dispatches episode/consolidation requests"
    cortexweave.application.facade -> cortexweave.application.episodeService "3. Creates/associates/closes versioned Episode"
    cortexweave.application.episodeService -> cortexweave.database "4. Persists ordered Event membership"
    cortexweave.application.facade -> cortexweave.application.consolidationService "5. Requests deterministic preview"
    cortexweave.application.consolidationService -> cortexweave.application.evidenceService "6. Validates typed factual evidence"
    cortexweave.application.consolidationService -> cortexweave.application.failureNormalizationService "7. Builds supported signature"
    cortexweave.application.consolidationService -> cortexweave.database "8. Reads terminal frontier and returns preview"
    agentHarness -> cortexweave.application.mcpAdapter "9. Explicitly accepts matching fingerprint/proposal hash"
    cortexweave.application.mcpAdapter -> cortexweave.application.facade "10. Dispatches acceptance"
    cortexweave.application.facade -> cortexweave.application.consolidationService "11. Recomputes and accepts"
    cortexweave.application.consolidationService -> cortexweave.database "12. Persists sealed immutable Experience transactionally"
    autoLayout lr
}

dynamic cortexweave.application "Experience-Reuse" "A fresh session may receive eligible historical Experience only as bounded supplemental context after current evidence." {
    agentHarness -> cortexweave.application.mcpAdapter "1. Sends current task and active failure context"
    cortexweave.application.mcpAdapter -> cortexweave.application.facade "2. Dispatches semantic_context"
    cortexweave.application.facade -> cortexweave.application.contextService "3. Requests current-first ContextPacket"
    cortexweave.application.contextService -> cortexweave.application.stateMemoryService "4. Selects current task and active Event evidence"
    cortexweave.application.contextService -> cortexweave.application.experienceSearchService "5. Searches matching eligible history"
    cortexweave.application.experienceSearchService -> cortexweave.database "6. Filters lifecycle/signature/path/graph and assessment history"
    cortexweave.application.contextService -> cortexweave.application.mcpAdapter "7. Returns packet labeled historical supplemental only after authority checks and separate budget"
    cortexweave.application.mcpAdapter -> agentHarness "8. Returns context; harness retains reasoning/tool authority"
    autoLayout lr
}

deployment cortexweave "Local development" "CortexWeave-LocalDeployment" "Evidence-supported local developer-workstation topology; this is not a Docker or distributed deployment model." {
    include *
    autoLayout lr
}
