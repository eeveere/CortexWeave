# CortexWeave v0.1 Architecture

## Purpose

CortexWeave is a local-first cognitive substrate for coding agents. Version 0.1
persists workspace structure, explicit memories, and operational events; keeps
workspace content indexed as files change; and exposes retrieval through a
service API. It does not invoke reasoning models, plan tasks, execute tools, or
run verification loops.

## Dependency Direction

The dependency rule is:

```text
CLI / MCP / future native adapters
              |
              v
      CortexWeaveService
              |
              v
domain services and ports (storage, embeddings, analyzers)
              |
              v
SQLite, OpenAI-compatible HTTP, tree-sitter, filesystem watcher
```

Adapters translate transport-specific requests into application-level inputs.
No MCP, CLI, JSON-RPC, or future transport type may cross into the domain or
storage interfaces. A native harness can call `CortexWeaveService` directly or
place another adapter over it without rewriting MCP behavior.

## Application Boundary

`CortexWeaveService` is the public application facade. It owns references to
small, independently testable services rather than a single mutable application
state lock. Its operations cover workspace registration and status, indexing,
retrieval, memories, sessions, tasks, and events.

Workspace selection is also an application concern. The facade resolves UUIDs,
unique names, canonical roots, file URIs, nested paths, and adapter hints from a
single workspace snapshot. Adapters do not reproduce path comparison or
ambiguity rules.

Long-running work is coordinated outside database locks. SQLite transactions
cover only consistent persistence changes. Embedding HTTP calls and source
analysis occur before the reconciliation transaction begins.

Runtime instrumentation is shared by the facade's indexing and retrieval
services. Persisted totals come from SQLite, while job, embedding, analyzer,
watcher, chunk-change, and latency counters describe the current process. The
CLI exposes the combined snapshot through `metrics`.

## Domain Separation

- **Workspace content** consists of documents, normalized chunks, and vector
  representations. Its lifecycle follows the filesystem.
- **Memory** is explicit durable knowledge such as a decision, observation,
  failure, solution, todo, note, or checkpoint. It is never inferred merely
  from a conversation turn.
- **Events** are time-ordered facts about activity, including file changes,
  sessions, tasks, tools, compilers, and tests.

Every retrieval result identifies its source domain. Workspace, session, and
task identifiers are UUID-backed strings at service boundaries. Session and task
references remain optional for records that are workspace-wide.

## Storage Ownership

The storage port presents domain-oriented operations and explicit transaction
methods required by reconciliation. The SQLite implementation owns migrations,
foreign keys, cascades, FTS5 synchronization, vector serialization, and query
details. Other core modules do not issue SQL.

Each stored embedding includes its model identity and dimension. Search may only
compare vectors whose model and dimension match the query vector. Documents own
chunks, chunks own embeddings, and deletion cascades remove stale search data.

## Embedding Boundary

`EmbeddingProvider` accepts batches of text and returns vectors. The initial
provider calls an OpenAI-compatible `/v1/embeddings` endpoint. It validates the
response count, non-empty and consistent vector dimensions, configured model,
HTTP status, decoding, and timeout behavior.

The optional configured vector dimension participates in compatibility checks,
including when a provider keeps the same model name. The provider does not write
storage. The indexer decides what needs embedding;
the storage layer records the model metadata. An embedding outage leaves the
previous committed index intact.

## Language Analysis

`LanguageAnalyzer` maps a path and UTF-8 source string to `AnalyzedChunk` values.
Tree-sitter nodes never escape an analyzer. Each chunk contains normalized symbol
metadata, byte and line ranges, content, a stable key, analyzer identity, and
optional language-specific JSON metadata.

`AnalyzerRegistry` maps extensions and language IDs to analyzers and exposes
capabilities. The indexing core asks the registry for an analyzer and processes
only normalized chunks. A deterministic generic analyzer handles unsupported
text files. Adding a language requires registering one analyzer, not modifying
the indexer or schema.

Stable keys are analyzer-produced path-relative identities such as
`src/index.rs::impl:SemanticIndex::method:search`. A chunk content hash detects
changes independently of identity. Analyzer ID and version are stored so an
analyzer change triggers explicit replacement rather than accidental reuse.

## Indexing and Reconciliation

Workspace discovery applies ignore rules, nested `.gitignore` files, binary and
size limits, and language detection. For each candidate document, reconciliation:

1. Reads and hashes the file, returning immediately when the document hash and
   analyzer identity are unchanged.
2. Runs the selected analyzer and matches new chunks to stored chunks by stable
   key.
3. Embeds only added or content-modified chunks.
4. Atomically inserts or updates changed rows, preserves unchanged vectors,
   removes stale chunks, updates FTS data, and records document metadata.

The filesystem is authoritative. A missing file deletes its document record and
dependent index data.

## Watcher and Queue Ownership

The watcher adapter converts raw filesystem notifications into normalized path
signals. A bounded indexing queue owns debounce and coalescing. Watch callbacks
perform no parsing, embedding, or database work. Workers reconcile the final
filesystem state, so duplicate, reordered, or rename-like event sequences remain
safe. Queue saturation is observable and converges through a workspace rescan.

## Retrieval

Lexical retrieval uses SQLite FTS5. Semantic retrieval embeds the query and
computes cosine similarity against compatible vectors. Hybrid retrieval
normalizes component scores, merges and deduplicates records, applies configured
weights, and returns both component scores with provenance.

## Concurrency and Failure Model

Services share immutable configuration and thread-safe ports through `Arc`.
SQLite uses a connection pool rather than a global mutex. Per-document work may
run concurrently up to configured limits; persistence transactions are brief.
Malformed source falls back or produces a scoped indexing error without stopping
the service. Network failure cannot erase previously committed embeddings.
Workspace scans isolate per-file failures and report them while continuing to
reconcile healthy files. Lock contention and mid-transaction errors roll back
without exposing partially updated document trees.

## Adapter Isolation Review

The service operations have no MCP-specific inputs or outputs. The application
facade directly exposes workspace registration/status/reindexing, retrieval and
item lookup, explicit memory, session/task lifecycle, and event ingest using
domain and Serde types. MCP can be removed while CLI, tests, and a future
harness continue to use the same core. Likewise, the indexing service consumes
normalized analyzer output and contains no Rust, Python, TypeScript,
JavaScript, C#, or Go syntax.

## MCP Adapter

`cortexweave serve` starts a line-delimited JSON-RPC MCP server over stdio.
Its stdout carries protocol messages only; structured logs are sent to stderr.
The adapter implements initialization, tool discovery, tool calls, and ping.
It exposes semantic search/item lookup, memory operations, workspace status and
reindexing, workspace discovery, plus straightforward session and event
operations. Existing `workspace_id` UUID inputs remain compatible. An optional
`workspace` input accepts a UUID, unique name, absolute path, or file URI; calls
may omit both when an explicit launch hint or singleton registration resolves
unambiguously. Conflicting inputs fail before the operation executes. On startup it
arms a filesystem watcher for every registered workspace so an agent sees fresh
indexed source without a manual reindex.

The adapter translates JSON values into facade calls and translates their
domain results back into MCP tool results. Tool execution failures are returned
as model-visible MCP tool errors; protocol failures use JSON-RPC errors.
The adapter's root hint affects resolution only and cannot register a workspace
or expand access beyond registered roots.
