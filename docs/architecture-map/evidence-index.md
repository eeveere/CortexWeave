# Architecture evidence index

## Application facade and adapters

**Implementation:** `src/main.rs`; `src/service/cortex.rs`; `src/adapters/mcp.rs`.

**Key symbols:** `main`, `run`, `CortexWeaveService::open`,
`CortexWeaveService::from_parts_with_embeddings`, `McpServer::serve_stdio`,
`McpServer::handle_json`.

**Conclusion:** the facade owns application policy; CLI and MCP resolve/parse/
serialize only. MCP owns watcher lifecycle and uses newline JSON-RPC stdio.

## Workspace/source discovery

**Implementation:** `src/config.rs`; `src/workspace/{path_identity,selector,discovery}.rs`.

**Key symbols:** `AppConfig::load`, `PathIdentity::existing_directory`,
`WorkspaceScanner::scan`, `WorkspaceSelector`.

**Conclusion:** workspace identity is canonicalized/idempotent; scanning is
ignore-aware and records failures rather than pretending a partial scan is
complete.

## Analyzer registry and language support

**Implementation:** `src/parsing/{analyzer,registry,generic,tree_sitter_support}.rs`; `src/parsing/languages/`.

**Key symbols:** `LanguageAnalyzer`, `AnalyzerRegistry::configured`,
`GenericAnalyzer`, `RustAnalyzer`, `PythonAnalyzer`, `JavaScriptAnalyzer`,
`TypeScriptAnalyzer`, `CSharpAnalyzer`, `GoAnalyzer`.

**Conclusion:** parsing is pluggable and normalized; indexing is not coupled to
AST syntax. Generic fallback is deterministic.

## Indexing and watching

**Implementation:** `src/indexing/{reconciler,batcher,segmenter,watcher}.rs`.

**Key symbols:** `IndexingService::reconcile_file`, `reconcile_file_inner`,
`reindex_workspace`, `WorkspaceWatcher::start`, `run_worker`.

**Conclusion:** changed chunks alone are embedded when identifiers/spaces
match; storage update occurs after a race check. Watchers use bounded mpsc,
debouncing/coalescing and eventual rescan.

## Embedding provider

**Implementation:** `src/embedding/{provider,contracts,openai}.rs`.

**Key symbols:** `EmbeddingProvider`, `EmbeddingLimits`,
`OpenAiCompatibleEmbeddingProvider`, `validate_response`.

**Conclusion:** model-space/dimension and token-limit contracts are explicit;
providers are external and replaceable.

## Graph and structural reads

**Implementation:** `src/graph/{indexer,registry,resolver}.rs`; `src/service/structural.rs`.

**Key symbols:** `GraphIndexer::reconcile_document`,
`GraphIndexer::delete_document`, `SymbolRegistry::reconcile_document`,
`SymbolResolver::resolve`, `StructuralService`.

**Conclusion:** the graph is a durable projection of normalized analysis with
revision/repair freshness gates—not an independent parser or source authority.

## Retrieval and context

**Implementation:** `src/retrieval/service.rs`; `src/service/context.rs`.

**Key symbols:** `semantic_search`, `lexical_search`, `hybrid_search`,
`ContextService::assemble_context_packet`.

**Conclusion:** hybrid merges concurrent code candidate sources and optional
structural expansion; context has explicit categories/budgets/explanations.

## Evidence, Episodes and Experience

**Implementation:** `src/service/{evidence,failure,consolidation,experience_search,experience_assessment}.rs`; `src/domain/{evidence,failure,episode,experience,consolidation}.rs`.

**Key symbols:** `EvidenceService::diagnose`, `FailureNormalizationService`,
`ConsolidationService::preview`, `accept`, `ExperienceSearchService::search`,
`ExperienceAssessmentService::review`.

**Conclusion:** Events are factual input, Episodes explicitly organize them,
and Experience is an immutable derived historical record; review is append-only.

## SQLite and integrity

**Implementation:** `src/storage/{sqlite,repositories}.rs`; `migrations/0001_initial.sql` through `0012_assessment_request_identity.sql`.

**Key symbols:** `SqliteStorage::open`, `apply_document_reconciliation`,
graph repair and Experience repository methods.

**Conclusion:** SQLite owns atomic persistence and enforces key history/scope
rules with foreign keys, constraints, indexes and triggers.

## Cross-check tests and current boundary

**Implementation:** `tests/{mcp_stdio,incremental_graph_reconciliation,relationship_extraction,retrieval_evaluation,native_experience_full_cycle,experience_architecture_boundary}.rs`; `docs/v0.5-release-boundary.md`; `docs/verified-experience.md`.

**Conclusion:** tests exercise adapter framing, incremental behavior, graph,
retrieval and Experience integrity; release docs identify deliberately deferred
architecture.
