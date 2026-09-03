# Data model and persistence map

## Durable store

`DatabaseConfig.path` defaults to `.cortexweave/cortexweave.db`.
`SqliteStorage::open` creates the parent path, enables `PRAGMA foreign_keys`,
runs the embedded `migrations/` sequence through SQLx, and exposes a SQLite
pool. SQLite owns transaction boundaries. FTS5 virtual tables are maintained by
database triggers; vectors are BLOB records in the same database and are
scanned in process.

| Domain | Tables / projections | Producer → consumer |
|---|---|---|
| Workspace identity | `workspaces` | registration/resolution → all scoped services |
| Agent state | `sessions`, `tasks` | explicit callers → context/resume/Episode scope |
| Source index | `documents`, `chunks`, `embeddings`, `chunk_fts` | reconciler → retrieval/context/graph |
| Explicit memory | `memories`, `memory_fts`, `memory_trust_reviews`, `memory_supersession` | memory facade → trusted context/memory search |
| Raw observation | `events` | caller/watcher → context/Episode/evidence/consolidation |
| Context state | `working_set_entries`, `context_pins`, `checkpoints` | context/checkpoint APIs → resume/context |
| Graph | `workspace_graph_revisions`, `graph_document_states`, `graph_nodes`, `graph_relationship_facts`, `graph_edges`, `unresolved_relationships`, `unresolved_relationship_candidates`, `workspace_graph_repairs`, `graph_document_projections` | graph indexer → structural service/retrieval/context |
| Episodes | `episodes`, `episode_events`, `episode_mutation_requests` | episode facade → consolidation |
| Experience aggregate | `experiences`, `experience_attempts`, `experience_verifications`, `experience_evidence`, `experience_code_snapshots`, `experience_graph_snapshots`, `experience_strength_bases`, `experience_fts` | consolidator → Experience search/context |
| Assessment/history | `experience_assessments`, `experience_assessment_evidence`, `historical_write_order`, `experience_seals` | event/consolidation/assessment → lifecycle, audit and recurrence checks |

## Key relationships and integrity

`documents` belongs to a workspace; `chunks` belong to a document; each
`embedding` belongs to a chunk and records model-space/dimension. A document
also records analyzer and segmentation identities, hash and content revision.
FTS rows contain denormalized searchable values and are maintained from source
rows, not written directly by application services.

Graph nodes and facts are workspace-scoped, preserve source-document/chunk
provenance where available, and are tied to graph revision/projection state.
Unresolved relationships are durable uncertainty, not negative proof.

Events, Episodes, and Experience have strict workspace and session/task scope.
Episode membership is ordinal, versioned and idempotent by request key. The
Experience aggregate retains evidence links and immutable code/graph snapshots
instead of treating current mutable chunks/graph IDs as historical truth.

Migrations add foreign keys, uniqueness constraints and triggers that reject:
cross-workspace links, invalid Experience eligibility/evidence shapes,
supersession cycles, mutation of events/accepted Experience components and
assessment history, and additions after an Experience seal. Deleting a whole
workspace may cascade its contained history; ordinary in-place deletion is
blocked by guards.

## Ephemeral state

`RuntimeMetrics` uses standard mutex-protected process counters and latency
accumulators. `IndexingService` holds a semaphore and a map of per-path Tokio
mutexes. The MCP server retains initialization state and a map of watcher
startup outcomes. Watcher channel queues, debounce batches, file bytes,
analysis values, request objects and query vectors are ephemeral.
