# Structural Graph Architecture

CortexWeave maintains a deterministic, workspace-scoped structural graph beside
its code chunks, memories, and events. The graph is an indexed projection of
source analysis, not an agent-runtime model and not an MCP data model.

## Layers and ownership

Language analyzers return normalized symbols and relationship assertions. The
graph indexer persists source-owned relationship facts, resolves them against a
workspace-local symbol registry, and writes node/edge projections through the
SQLite storage boundary. `StructuralService` performs bounded reads and impact
traversal. The CortexWeave application service exposes those results to CLI,
MCP, and future adapters; adapters do not parse source, run resolver SQL, or
make graph-policy decisions.

SQLite owns a graph update transaction. A graph revision records source content
revision, projected graph content revision, schema version, update state, and
any latest graph error. Structural reads are current-only by default. A caller
may explicitly request stale evidence; every returned result retains the graph
snapshot that produced it. A revision change during a read is retryable rather
than a mixed-snapshot answer.

## Nodes, edges, and confidence

Nodes identify workspace, module/package/namespace, file, declaration, test,
and unknown-symbol entities. Stable keys, logical names, qualified names,
source segment, analyzer identity/version, structural version, and content
revision give every node provenance. Node IDs are opaque UUIDs returned by
`graph find` and used for relation calls.

Edges are typed: containment, imports/exports, calls/references, implements or
extends, type use/construction, overrides, tests, declarations, and dependency
relations. A relationship starts as a durable source-owned fact. Resolution
then projects it as resolved, ambiguous, external, or unresolved without losing
the assertion. An edge confidence comes from analyzer evidence and traversal
confidence is the product of edges on the returned path. Confidence means
structural support, not runtime certainty or a prediction of a source-control
diff.

## Symbol resolution and incremental updates

Resolution is exact and workspace-local. It considers stable keys, qualified
names, module identities, explicit aliases, and typed enclosing containers;
ambiguous candidates remain explicit. Analyzers own source-relative module
paths, extension/index candidates, namespace separators, and explicit external
roots; the graph resolver only consumes their normalized candidates. There is no
fuzzy model-based matching.
Changing a source document replaces its owned facts and re-resolves only the
typed resolution surface that may have changed. Unchanged chunks and compatible
embeddings remain intact.

## Adding graph support to a language

Implement the `LanguageAnalyzer` contract in the parsing layer. Declare its
extensions, analyzer identity/version, structural version, and precise
`AnalyzerCapabilities`; return normalized chunks, symbols, and typed
relationships. Keep language syntax and AST work inside the analyzer. Do not
add language branches to the indexing core, resolver, structural service, or
adapters. Register the analyzer through `AnalyzerRegistry`, add relationship
fixtures and an incremental reconciliation case, then expose only capabilities
that the analyzer actually produces.

If no analyzer is enabled for a text language, CortexWeave uses deterministic
generic chunking. Its graph capabilities are all false; it still indexes code
chunks but does not manufacture structural claims. The Breakpoint 13 extension
test covers this boundary with an unsupported `.lua` file.

## Querying and impact

Use `graph find <workspace-id> <symbol-or-path>` (or MCP `graph_find`) to
obtain exact node IDs, then issue one of the bounded relation reads. Each read
returns effective limits, typed edges, paths, graph snapshot, confidence, and a
truncation flag. CLI accepts core bounds; MCP additionally caps calls at 100
nodes, 500 edges, and depth 4.

`graph impact-symbol` and `graph impact-path` walk bounded incoming typed
dependencies. The output is reverse-dependency reachability with explainable
paths, not a claim that a change will alter runtime behavior. Graph-aware hybrid
retrieval and context assembly retain the same evidence path and snapshot.

`graph neighbors` is a direct one-hop relation read. Explicit exact seed IDs are
workspace-scoped and bounded before traversal. `Tests` edges are direct-call
associations from explicit test symbols and are labeled likely; they do not
establish behavioral coverage.

## Diagnostics and limitations

`status`, `graph status`, MCP `graph_status`, and `doctor` show revision,
staleness, node/edge/unresolved totals, and active per-language capabilities.
`doctor` treats a non-current graph for a registered workspace as unhealthy.

The v0.4 graph does not infer runtime dispatch, perform whole-program type
checking, predict diffs, map observations to nodes, or use model inference for
resolution. Unsupported languages remain usable through generic chunks, but do
not claim structural relationships until a focused analyzer implementation is
added.

## Optional integrations

| Integration | v0.4 status |
|---|---|
| Git-diff-seeded impact | DEFERRED |
| Diagnostic-to-symbol mapping | DEFERRED |
| Graph-aware working-set activation | DEFERRED |
| Cross-language inferred edges beyond explicit module/file facts | DEFERRED |
| Graph visualization export | DEFERRED |
