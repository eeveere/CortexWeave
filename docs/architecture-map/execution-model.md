# Execution and concurrency model

## Runtime shape

The binary is `#[tokio::main]` on the multi-thread Tokio runtime. Most public
service operations are `async` because they issue SQLx, filesystem or HTTP I/O;
syntax analysis, hashing, segmentation and cosine scoring run synchronously in
the async request path. No dedicated CPU pool or job service is implemented.

| Area | Mechanism | Ownership / behavior |
|---|---|---|
| CLI | one async command dispatch | exits once command resolves; no background service except `serve` |
| MCP | stdio read loop; each parsed frame awaits dispatch | one server instance; line frame limited by `MAX_MCP_FRAME_BYTES` |
| Watcher | `notify` callback → bounded Tokio `mpsc` → spawned worker | MCP owns handles; sender overflow/failure asks worker to rescan |
| Watch batching | debounce/coalesce paths | worker uses configured debounce; rescan reconciles whole workspace |
| Indexing | semaphore + keyed Tokio mutex | limits concurrent embedding/reconciliation jobs; serializes same workspace/path, not all paths |
| Reindex | sequential file reconciliation followed by graph repair | file errors are collected/logged; other files continue |
| Hybrid retrieval | `tokio::join!` | semantic and lexical candidate searches proceed concurrently |
| Persistence | SQLx SQLite pool/transactions | SQLite serializes writes and owns atomic boundaries |
| Metrics | `std::sync::Mutex` | process-local; poisoned lock panics by design (`expect`) |

## Cancellation and shutdown

The watcher handle contains a Tokio oneshot shutdown sender and a join handle.
Explicit `shutdown` sends then awaits; `Drop` sends only. The MCP server holds
handles for its serve lifetime and awaits them on exit. There is no process-wide
cancellation tree, persisted work queue, resumable index job, retry scheduler,
or automatic background consolidator.

## Backpressure, batching, and failures

The watcher queue capacity is configured from the indexing concurrency limit
and is never zero. Reconciliation acquires one semaphore permit before file
read/analyze/embed work. Embedding batch planning honors declared input/batch
limits; an over-capacity response can lower segmentation input size up to eight
times, after which it fails. HTTP errors are classified by the provider and
surface to callers; retries are absent.

File races are handled by a final read/hash equality check before the storage
transaction; a changed file returns an error to be retried by a future event or
reindex. Missing, binary, too-large or non-UTF-8 input removes its indexed
document. Graph repair uses a 120-second durable lease and generation state,
so graph reads can reject stale/repairing projections unless a caller requests
stale data. Experience preview is read-only; acceptance recomputes under one
transaction and converges repeated fingerprint requests.
