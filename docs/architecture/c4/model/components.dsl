group "Interfaces / Adapters" {
    cliAdapter = component "CLI Adapter" "Parses Clap commands, loads configuration at startup, invokes the facade, and renders results." "Rust / Clap" "Adapter"
    mcpAdapter = component "MCP Adapter" "Implements newline-delimited JSON-RPC over stdio: initialization, tools/list, tools/call, bounded argument validation, workspace selection, response/error serialization, and watcher lifetime." "Rust / Tokio stdio" "Adapter"
}

group "Application Boundary" {
    facade = component "CortexWeaveService" "Transport-neutral application facade that composes services and owns application policy. Core services do not depend on MCP/JSON-RPC/stdio." "Rust" "Application"
}

group "Workspace and Source Intelligence" {
    workspaceService = component "Workspace Service and Resolver" "Registers canonical workspace paths; resolves IDs, names, roots/subpaths, file URIs, default hints, and singleton selection." "Rust" "Application"
    workspaceScanner = component "Workspace Scanner" "Walks a workspace under ignore rules and configured patterns; filters binary, unreadable, oversized, and unsupported files while recording scan failures." "Rust / ignore" "SourceIntelligence"
    analyzerRegistry = component "Analyzer Registry" "Selects enabled language analyzers by path and supplies deterministic generic fallback." "Rust" "SourceIntelligence"
    treeSitterAnalyzers = component "Tree-sitter Language Analyzers" "Grouped Rust, Python, JavaScript, TypeScript, C#, and Go analyzers emitting normalized chunks, symbols, relationships, stable keys, and metadata." "Rust / Tree-sitter" "SourceIntelligence"
    genericAnalyzer = component "Generic Fallback Analyzer" "Deterministically chunks unsupported text without language-specific AST logic." "Rust" "SourceIntelligence"
}

group "Indexing" {
    workspaceWatcher = component "Workspace Watcher" "MCP-lifetime watcher: notify callback sends path signals to a bounded channel; a worker debounces/coalesces and requests reconciliation or rescan." "Rust / notify / Tokio mpsc" "Indexing"
    indexingService = component "IndexingService / Reconciler" "Serializes same-path work, bounds concurrent jobs, hashes and analyzes source, preserves compatible chunks/embeddings, detects source races, and applies reconciliation." "Rust / Tokio" "Indexing"
    segmenterBatcher = component "Capacity-Aware Segmenter and Batcher" "Segments normalized chunks to provider limits; batches changed text and performs bounded adaptive capacity reduction." "Rust" "Indexing"
    embeddingAdapter = component "Embedding Provider Adapter" "Provider abstraction and OpenAI-compatible HTTP client; validates token limits, vector shape, model space, and dimensions." "Rust / reqwest" "Indexing"
}

group "Structural Intelligence" {
    graphIndexer = component "GraphIndexer and Repair" "Projects normalized symbols and relationship facts, performs graph-only reprojection/repair with durable generation and revision state." "Rust" "Structural,Derived"
    symbolRegistry = component "Symbol Registry" "Preserves stable graph-node identities while reconciling analyzed documents." "Rust" "Structural,Derived"
    symbolResolver = component "Symbol Resolver" "Deterministically resolves relationship targets and retains unresolved facts/candidates instead of guessing." "Rust" "Structural,Derived"
    structuralService = component "Structural Service" "Provides current-state-gated symbol/path lookup, bounded relations/traversal, and impact analysis." "Rust" "Structural,Derived"
}

group "Retrieval and Context" {
    retrievalService = component "RetrievalService" "Performs compatible-vector semantic search, SQLite FTS lexical search, hybrid score combination, and optional structural candidate expansion." "Rust" "Retrieval"
    contextService = component "ContextService" "Acquires and deduplicates candidates; applies state/freshness/structural ranking and category/token budgets; returns ContextPacket with explanation. Current evidence remains authoritative." "Rust" "Context"
}

group "State and Memory" {
    stateMemoryService = component "State and Memory Services" "Owns explicit memories and trust/supersession, sessions/tasks, raw append-only Events, working sets/pins, and checkpoints as distinct durable domains." "Rust" "State"
}

group "Verified Experience" {
    evidenceService = component "EvidenceService" "Decodes registered, bounded factual evidence contracts; unsupported payloads remain raw Events." "Rust" "Experience"
    failureNormalizationService = component "FailureNormalizationService" "Produces supported deterministic, canonical, workspace-scoped failure signatures without inventing claims." "Rust" "Experience"
    episodeService = component "Episode Service" "Controls explicit versioned Episode lifecycle and ordered primary Event membership using caller request keys." "Rust" "Experience"
    consolidationService = component "ConsolidationService" "Creates deterministic read-only previews and explicitly accepted immutable Experience aggregates." "Rust" "Experience,Historical"
    experienceSearchService = component "ExperienceSearchService" "Searches eligible Experience with deterministic signature, lexical, path, and graph filters and explains matches." "Rust" "Experience,Historical"
    experienceAssessmentService = component "ExperienceAssessmentService" "Records reviewed append-only assessments and produces read-only recurrence dispute proposals." "Rust" "Experience,Historical"
}
