# Architecture inventory

Status is **implemented** unless stated otherwise. “Partial” means code exists
but has a deliberately constrained capability; “planned” is documented but not
present in the current runtime.

## Repository structure

| Element | Path | Responsibility and primary callers | Dependencies/status |
|---|---|---|---|
| Binary adapter | `src/main.rs` | Clap CLI; opens facade; maps commands to service calls; `serve` hosts MCP | Tokio, Clap, tracing; implemented |
| Public crate | `src/lib.rs` | Re-exports configuration, errors, service and subsystem modules | Rust library API; implemented |
| Application facade | `src/service/cortex.rs` | Composes services; workspace/session/task/memory/event/episode/Experience APIs | SQLite, analyzers, embedding provider; implemented |
| MCP adapter | `src/adapters/mcp.rs` | JSON-RPC framing, initialize/tools dispatch, workspace resolution, watcher ownership | stdio, Tokio, facade; implemented |
| Configuration | `src/config.rs` | TOML parsing/validation and limits for all services | no persistence beyond TOML; implemented |
| Workspace | `src/workspace/` | canonical path identity, selector resolution, ignore-aware scan | analyzer registry; implemented |
| Parsing | `src/parsing/` | language-neutral analyzer trait/registry, generic splitter, Tree-sitter helpers and six analyzers | Tree-sitter; implemented |
| Indexing | `src/indexing/` | chunk segmentation/batching, reconciliation, graph repair, watcher | storage, provider, parsing; implemented |
| Graph | `src/graph/` | symbol registry, deterministic resolver, projection/reconciliation | SQLite and normalized analysis; implemented |
| Retrieval | `src/retrieval/service.rs` | semantic, FTS lexical, hybrid and structural expansion | provider, SQLite, structural service; implemented |
| Context | `src/service/context.rs` | context packet, budgets, ranking, working-set/session history | retrieval, SQLite; implemented |
| Memory/evidence/Experience | `src/service/{memory,evidence,failure,consolidation,experience_search,experience_assessment}.rs` | explicit memory integrity; typed evidence; failure normalization; historical learning | SQLite; implemented, deliberately narrow |
| Persistence | `src/storage/{sqlite,repositories}.rs`, `migrations/` | SQLx SQLite pool, migrations and all repository operations | SQLite/FTS5; implemented |
| Domain contracts | `src/domain/` | storage-neutral records, validation and vocabulary | used by all core layers; implemented |
| Observability | `src/instrumentation.rs` | process-local counters/latencies plus persisted count snapshot | std `Mutex`, SQLite; implemented |
| Tests | `tests/` plus module tests | CLI, stdio MCP, indexing, graph, retrieval, harness, Experience contracts | implemented acceptance coverage |
| Design records | `docs/`, `docs/checkpoints/` | current decisions plus historical checkpoints | some historical; evidence must be dated |

## System entry points

| Entry | Invocation and resulting path |
|---|---|
| CLI | `main` → `AppConfig::load` → logging → `CortexWeaveService::open` → `run`. Commands cover `workspace`, `reindex`, `search`, `context`, `resume`, `memory`, `episode`, `experience`, `graph`, `doctor`, readiness/status/metrics. |
| MCP stdio | CLI `serve` → `McpServer::serve_stdio` → newline frame → JSON-RPC `initialize`, `tools/list`, or `tools/call` → typed argument extraction → facade. Supports 40+ named tools including semantic/context, graph, state, Episode, Experience and workspace operations. |
| Native library | Consumers can construct `CortexWeaveService` (including `from_parts_with_embeddings`) and call transport-neutral operations directly. |
| Watcher | MCP server after initialize → registered/default-hint workspace selection → `WorkspaceWatcher::start` → `notify` callback → bounded channel → debounced worker → reconciler. It is not started by ordinary CLI commands. |
| Tests | Rust unit/integration tests use real services, temporary files and/or `SqliteStorage::in_memory`; no test-only runtime architecture is exposed. |

## Major subsystems

### Workspace and configuration

`AppConfig` validates embedding, indexing, retrieval, working-set, temporal and
context limits. `PathIdentity` resolves/canonicalizes existing roots and builds
cross-platform comparison keys. Registration is idempotent for equivalent
paths; selectors accept ID, name, root/subpath, `file://`, default hint, or an
unambiguous singleton. `WorkspaceScanner` uses `ignore::WalkBuilder`, ignores
binary/oversized/unreadable files, records failed paths, and only accepts files
for which an analyzer or generic fallback applies.

### Parsing and language extension

`LanguageAnalyzer` is the key language-neutral contract. `AnalyzerRegistry`
selects an enabled registered analyzer or generic fallback. Bundled analyzers
are Rust, Python, JavaScript, TypeScript, C#, and Go; they use Tree-sitter
structure helpers and per-language relationship extractors. Generic chunking
is deterministic by characters and preserves indexability for other text.
Normalized outputs contain chunks, symbols, relationships, stable keys,
metadata, locations, analyzer identity/version and structure version.

### Reconciler and embeddings

`IndexingService::reconcile_file` serializes a `(workspace,path)` operation
with a Tokio mutex and limits expensive jobs with a semaphore. It reads source,
rejects missing/too-large/binary/non-UTF-8 input by removing its document,
hashes content, analyzes, segments to provider capacity, preserves unchanged
stable chunks, embeds changed chunks in capacity-aware batches, re-reads source
to detect races, then applies a single document reconciliation transaction.
Compatible analyzer/segmentation/embedding-space state prevents needless
embedding. `OpenAiCompatibleEmbeddingProvider` uses HTTP JSON with timeout;
the trait also supports test providers/token counters. It validates response
shape/dimension and assigns a provider/model/dimension space identity.

### Graph

`GraphIndexer` projects a document’s normalized symbols/relationships. The
registry preserves stable node identities where supported. `SymbolResolver`
resolves target terms deterministically against workspace symbols/aliases;
unresolved facts and candidates are stored instead of guessed. Repair uses a
leased generation with durable state and publishes a current revision only when
appropriate. `StructuralService` supplies exact find, neighbor/relation reads,
bounded traversal and impact paths; current reads reject stale graph state
unless callers explicitly allow it.

### Retrieval and context

Semantic search embeds a query and compares only compatible vectors in memory.
Lexical search queries `chunk_fts`. Hybrid runs semantic and lexical searches
concurrently with `tokio::join!`, deduplicates chunks, combines normalized
scores, and can add graph-derived candidates for structural intent. Context
creates a bounded candidate pool; ranks current code/documents, trusted memory,
events, task/session/checkpoint/working-set state and structural evidence;
then uses a separate historical Experience budget. It emits reasons, scores,
token estimates and truncation/explanation data.

### State, evidence and Experience

Sessions/tasks, explicit memories, events, working-set entries/pins and
checkpoints are separate persistence domains. Memory integrity includes trust
reviews and explicit supersession. Events remain append-only raw facts.
`EvidenceService` decodes registered bounded payload contracts and
`FailureNormalizationService` produces only supported canonical signatures.
Episodes are explicitly started, mutated with expected version/request key,
and terminally closed/abandoned. `ConsolidationService` deterministically
previews and, only with matching fingerprint/proposal hash, accepts a durable
immutable Experience. Search uses signature/lexical/path/graph filters and
lifecycle eligibility; assessment is append-only/idempotent and dispute
proposal is read-only.
