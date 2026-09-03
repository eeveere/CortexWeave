# CortexWeave architecture overview

## Scope and evidence

This map describes the repository state of CortexWeave `0.5.0`, observed on
2026-09-01. It is an evidence-based runtime map, not a target architecture.
Primary evidence is the executable crate in `src/`, its 12 SQLite migrations,
the integration tests in `tests/`, and the current v0.5 release material.
Paths and symbols are collected in [evidence-index.md](evidence-index.md).

## What the system is

CortexWeave is a local-first, persistent context substrate for coding agents
and native harnesses. It stores workspace-scoped code intelligence, explicit
agent state, factual events, and a deliberately constrained historical
Experience domain. It returns bounded, explainable context through a Rust
application facade, CLI, and stdio JSON-RPC MCP adapter. It does **not** invoke
reasoning models, execute developer tools, manage agent loops, or host an HTTP
service.

The main construction point is `CortexWeaveService::open` in
`src/service/cortex.rs`. It opens SQLite, configures a registry of analyzers,
constructs an OpenAI-compatible embedding provider, then composes indexing,
structural, retrieval, context, evidence, failure-normalization,
consolidation, Experience search, and assessment services. CLI and MCP call
this facade rather than being used by core code.

## Architectural regions

1. **External boundary.** A person/agent runs the Clap CLI; an MCP client sends
   newline-delimited JSON-RPC over stdio; a registered workspace supplies files;
   and an OpenAI-compatible service supplies embedding vectors.
2. **Adapters and application facade.** `main.rs` loads TOML configuration,
   initializes tracing, opens the facade, and dispatches commands. `mcp.rs`
   validates protocol and arguments, resolves workspaces, serializes responses,
   and starts watchers for its own process lifetime.
3. **Source intelligence and indexing.** Discovery observes ignore rules and
   configured patterns; the analyzer registry selects a Tree-sitter-backed
   language analyzer or deterministic generic fallback. The reconciler compares
   content/hash, analyzer/version, segmentation policy, and embedding space,
   preserving compatible chunk IDs and embeddings.
4. **Structural intelligence.** Per-document analysis projects symbols and
   relationship facts into a durable graph. A resolver turns references into
   graph edges when possible, retains unresolved facts/candidates otherwise,
   and graph revision/repair state gates current reads.
5. **Retrieval and context.** Semantic search uses one compatible embedding
   space and in-process cosine similarity. Lexical search uses SQLite FTS5.
   Hybrid search normalizes and combines semantic, lexical, and optional
   structural candidates. The context service adds task/session state, working
   set, pins, trusted memory, events, and separately budgeted historical
   Experience; current evidence remains authoritative.
6. **Persistence and integrity.** SQLite owns transactions, migrations, foreign
   keys, FTS synchronization triggers, revision state, and append-only guards
   for Events and Experience history. There is no separate vector database,
   queue broker, cache server, or object store.
7. **Verified Experience.** Explicit externally recorded Events can be placed
   in a caller-controlled Episode. Registered evidence contracts and
   normalizers feed deterministic preview/explicit acceptance of immutable
   Experience records. Search and assessment are transport-neutral services.

## Principal boundaries

Core domain/application code is independent of MCP/JSON-RPC/stdio. The parser
contract returns normalized chunks, symbols, and relationships; the indexing
core consumes this normalized output rather than language ASTs. Embedding
inference is a provider trait. Provider/model/dimension compatibility is
checked before preserving or comparing vectors. SQLite is the durable
authority; in-memory metrics and per-path locks are process-local only.

## Lifecycle summary

* **Startup:** configuration parses and validates; tracing initializes; SQLite
  creates its parent directory, enables foreign keys, runs embedded migrations,
  and opens its pool; the facade builds its services. `serve` then starts stdio
  handling and, after MCP initialization, starts workspace watchers.
* **Indexing:** explicit `reindex` scans a workspace and reconciles source data
  without graph per-file projection, then runs graph repair. A watcher batches
  path signals and calls normal reconciliation, including graph reconciliation.
* **Retrieval:** an adapter resolves a workspace then calls search/context. No
  retrieval listener, cache, or recurring job is involved.
* **Shutdown:** `McpServer::serve_stdio` drops watcher handles after the input
  loop; handle shutdown signals a watcher task and joins it. There is no global
  daemon shutdown coordinator or recovery journal beyond durable SQLite state.

## Known limits

The SQLite vector search is a bounded in-process scan, not ANN indexing.
Embedding retries are not implemented; capacity errors trigger bounded adaptive
resegmentation, while other provider failures surface. Watch processing logs
per-path failures and triggers a later rescan rather than retaining a durable
work queue. Only stdio MCP is configured; no HTTP/API server exists. Graph
relationships are analyzer-supported and resolution is deterministic but can
remain unresolved. Historical Experience is intentionally supplemental and
does not use semantic embeddings in v0.5.
