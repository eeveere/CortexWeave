container cortexweave "CortexWeave-Containers" "C4 Level 2: CortexWeave is one Rust application and one SQLite database; CLI/MCP/indexing/retrieval are internal components, not separately deployed containers." {
    include operator
    include agentHarness
    include registeredWorkspace
    include embeddingService
    include cortexweave.application
    include cortexweave.database
    autoLayout lr
}
