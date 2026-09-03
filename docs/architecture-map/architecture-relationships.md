# Architecture relationships

The following is the diagram-level relationship inventory. Solid/synchronous
relations happen in one request unless noted. Every persisted relation goes
through `SqliteStorage` repository methods.

| Source | Relationship | Destination | Payload/state | Mode | State | Status/evidence |
|---|---|---|---|---|---|---|
| CLI / MCP | calls | `CortexWeaveService` | validated command/tool request | sync async call | ephemeral | implemented: `main.rs`, `adapters/mcp.rs` |
| MCP stdio loop | spawns/owns | workspace watchers | registered workspace roots | async task | ephemeral | implemented: `McpServer::serve_stdio` |
| filesystem notify callback | sends | watcher worker | normalized `PathSignal` | async bounded mpsc | ephemeral | implemented: `watcher.rs` |
| watcher worker | calls | IndexingService | debounced/coalesced paths; rescan event | async | durable outcome/events | implemented |
| explicit reindex | calls | WorkspaceScanner | workspace root/patterns | synchronous scan | ephemeral | implemented: `reconciler.rs` |
| scanner | reads | workspace filesystem | eligible UTF-8 text files | sync filesystem I/O | ephemeral | implemented |
| reconciler | selects | AnalyzerRegistry | relative path | sync | ephemeral | implemented |
| registry | parses | language analyzer / generic | source text → normalized analysis | sync CPU | ephemeral | implemented |
| reconciler | sends | embedding provider | changed segmented chunk texts | async HTTP | ephemeral | implemented |
| embedding provider | returns | reconciler | float vectors + model space | async HTTP | ephemeral | implemented |
| reconciler | writes | SQLite | document, chunks, compatible embeddings; removals | SQL transaction | persistent | implemented |
| reconciler | calls | GraphIndexer | analysis plus content/analyzer expectations | async | persistent graph projection | implemented |
| GraphIndexer | resolves | SymbolResolver | relationship targets/aliases | async SQL-backed | persistent facts/edges | implemented |
| retrieval semantic | queries | embedding provider + SQLite | query vector + compatible embeddings | async + SQL | ephemeral read | implemented |
| retrieval lexical | queries | SQLite FTS5 | escaped FTS query | async SQL | persistent read | implemented |
| hybrid retrieval | calls | semantic + lexical + structural | candidate results | concurrent `join!` | ephemeral | implemented |
| context service | reads/writes | SQLite | state, working set activation/pins/checkpoints and candidates | async SQL | persistent | implemented |
| context service | calls | Experience search | optional active failure signature | async | persistent read | implemented |
| event ingress | writes | SQLite | append-only `events`, write order | SQL transaction | persistent | implemented |
| episode facade | writes | SQLite | versioned episode/membership/request identity | SQL transaction | persistent | implemented |
| consolidator | reads/writes | SQLite | terminal episode → proposal/immutable Experience + seal | transaction | persistent | implemented |
| assessment service | reads/writes | SQLite | evidence-backed reviewed assessment | transaction | persistent | implemented |
| metrics snapshot | reads | SQLite + RuntimeMetrics | aggregate counts and process counters | async + mutex | persisted/process-local | implemented |
| TOML config | configures | facade/subsystems | limits, weights, paths, enabled analyzers | startup | filesystem source | implemented |
| tracing | observes | stderr/log consumer | startup/errors/warnings | synchronous | ephemeral | implemented |

## Important relationship constraints

* MCP is an adapter only: no core module imports MCP types.
* A file reconciliation atomically replaces the document/chunk/embedding view
  after a final source-hash race check; graph repair has its own durable
  generation/revision lifecycle.
* Semantic candidates are filtered by embedding-space identity and dimension;
  heterogeneous vectors are never compared.
* Workspace ownership is checked both in services and composite SQLite foreign
  keys/triggers for graph, episodes, Experience, and historical evidence.
* Events and accepted Experience components are immutable while their owning
  workspace exists; later interpretation is an assessment/supersession, not a
  rewrite.
