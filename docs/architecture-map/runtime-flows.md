# Runtime flow traces

## A. Workspace registration and bootstrap

1. A CLI `workspace add` or MCP workspace-selection operation reaches the
   `CortexWeaveService` after adapter parsing.
2. Registration validates non-empty root/name; `PathIdentity::existing_directory`
   resolves a real directory and creates a comparison key.
3. The service lists existing `workspaces`; an equal comparison key returns the
   existing row, otherwise `insert_workspace_if_absent` persists a UUID-backed
   workspace row.
4. Registration does **not** index, start a watcher, or infer a default. The
   caller explicitly invokes reindex; MCP `serve` starts watchers only later.

## B. Initial source indexing

1. `workspace_reindex` requires the workspace and creates a `WorkspaceScanner`
   with configured patterns and file limit.
2. Scanner walks under ignore rules, filters eligible files and records failed
   relative paths.
3. Reindex reconciles files source-only, allowing many file failures to be
   collected while later files continue. The reconciler locks `(workspace,path)`
   and acquires its embedding-job semaphore permit.
4. It reads metadata/bytes, removes stale documents for missing/binary/oversize/
   non-UTF-8 input, hashes valid source, and selects an analyzer.
5. The analyzer returns normalized chunks/symbols/relationships. Segmenting
   respects provider limits; stable-key/hash comparison identifies only chunks
   requiring fresh vectors.
6. The batcher calls the embedding provider. Capacity failures may reduce the
   segment limit and retry segmentation; other provider failures stop that file.
7. The reconciler re-reads and hashes the source, then atomically applies the
   document, chunks, embedding records and deletions. A race rejects the pass.
8. After a second scan/deletion reconciliation, `reindex_workspace` runs graph
   repair so a coherent graph generation represents the source projection.

## C. Incremental file change

1. During MCP `serve`, `notify` emits a path event to the watcher callback.
2. A bounded Tokio channel receives normalized path signals; its worker
   debounces/coalesces them. Channel/callback problems request a full rescan.
3. The worker calls normal `reconcile_file`, which follows B but invokes
   `GraphIndexer::reconcile_document` after the source transaction.
4. A created/modified path updates chunks and graph; a vanished path removes
   its document and projection. Rename semantics are conveyed by events but
   ultimately reconcile the observed old/new paths; the durable identity is the
   current relative document path. A filesystem race returns a logged failure
   and later event/rescan can retry it.

## D. Search and context

1. CLI/MCP resolves one workspace, validates bounded request arguments, then
   calls facade retrieval/context methods.
2. Semantic search creates a provider query vector and reads only embeddings
   matching its declared space/dimension; cosine scores are calculated in
   process. Lexical search queries `chunk_fts`.
3. Hybrid executes semantic and lexical source searches concurrently,
   normalizes/deduplicates their candidates, and optionally requests structural
   candidates if graph intent/configuration permits.
4. Context reads those candidates plus workspace state. It ranks/budgets code,
   documents, trusted memories, events, tasks/checkpoints/working-set/pins and
   structural evidence, producing packet and optional explanation.
5. If a canonical active failure signature requests it, Experience search runs
   separately. Eligible historical Experience receives a distinct budget and
   cannot outrank or replace present task/source/active observations.

## E. MCP request lifecycle

1. `serve_stdio` reads one newline-delimited frame with a size ceiling and
   parses JSON.
2. `handle_json` accepts initialize first, responds to `tools/list`, or
   dispatches `tools/call` by name. Other methods/ordering are protocol errors.
3. Per-tool parsing validates JSON types/bounds and resolves workspace ID/name/
   path/default hint using the facade.
4. The adapter calls a transport-neutral facade operation, serializes a service
   value on success, or renders a bounded tool/protocol error. No MCP type
   reaches storage/indexing/domain code.

## F. Startup and shutdown

1. `main` parses Clap, loads/validates TOML, initializes tracing and opens the
   SQLite-backed facade.
2. SQLite creates directories as needed, enables foreign keys and runs SQLx
   migrations. The facade instantiates provider, analyzer registry and services.
3. For non-serve commands, work completes and the process exits. For `serve`,
   watcher startup is deferred until MCP initialization and only targets
   registered/default-hinted workspaces.
4. EOF/disconnect ends the stdio loop; watcher handles send shutdown and join.
   Durable changes rely on completed SQLite transactions; no separate shutdown
   flush/journal protocol exists.

## G. Failure and recovery behavior

* Malformed config/provider response/request bounds return typed errors before
  durable writes. `doctor` checks database/migrations/FTS/provider capacity,
  analyzer/watcher/graph readiness.
* Unsupported language uses generic deterministic chunking; analyzer failures
  fail the affected file rather than inventing structure.
* Embedding incompatibility forces re-embedding; vector spaces are never mixed.
  HTTP provider failures are surfaced, not retried automatically.
* Reindex logs/returns failed paths while retaining successful files; watcher
  rescans when signal loss is suspected. Interrupted/stale graph repair leaves
  state explicit and current graph reads fail by default.
* Scope/foreign-key/triggers reject cross-workspace or invalid historical
  writes. Episode version/request-key and consolidation fingerprint/proposal
  hash make retries idempotent or explicit conflicts. Events/Experience history
  are not silently repaired by mutation.
