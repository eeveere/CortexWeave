# Final Hardening

This pass resolves the accepted Breakpoint 15 findings without adding
post-v0.1 orchestration features.

## Correctness

- Generic UTF-8 chunking always advances and prefers Markdown heading breaks.
- Generic chunk settings participate in the analyzer compatibility version.
- Per-path reconciliation is serialized in process and source content is
  reread before commit, preventing a slow job from publishing stale bytes.
- Workspace deletion uses a fresh discovery result and preserves documents for
  files that failed discovery.
- Declared embedding dimensions are enforced for both indexing and query
  vectors.

## Language and Discovery

- Indexing accepts explicit include and exclude glob patterns.
- Go keeps a simple display symbol while receiver-qualified identity remains
  stable and normalized.
- Identity signatures include type parameters and value parameters.
- Decorators, export wrappers, and Rust attributes are included in declaration
  chunk content.

## Operations and Adapters

- MCP returns only its supported protocol version, requires initialization for
  tool operations, bounds frames and result limits, and reports watcher state.
- Watcher event persistence cannot abort filesystem convergence for later
  paths.
- Doctor initializes each registered grammar with a minimal valid source.
- Runtime metrics identify themselves as process-scoped when combined with
  workspace-filtered persisted counts.
- Service lifecycle and session/task provenance are validated explicitly.

## Verification

Focused regression coverage includes UTF-8 progress, include/exclude handling,
stale same-path reconciliation, embedding-dimension contract enforcement,
wrapper retention, and provenance lifecycle rules. The final quality gate and
acceptance scenario are recorded after execution.

## Final Verification

Passed on the final implementation:

- `cargo fmt --all -- --check`
- `cargo check --all-targets --all-features`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features`

The final test run passed 55 unit tests and five integration demonstrations:
CLI service use, MCP stdio transport, fresh-context recovery, retrieval
evaluation, and the incremental performance baseline.
