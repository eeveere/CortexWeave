# Checkpoint 01: Foundation

**Status:** Passed

## Boundary Review

- The core starts through `CortexWeaveService::from_parts` without MCP or any
  other adapter.
- The CLI depends on the public service facade; no core module imports CLI types.
- Tests instantiate the service with an isolated in-memory SQLite database.
- `SqliteStorage` owns connection and migration details behind a service-held
  component.
- A future harness can construct or call `CortexWeaveService` directly.
- `AnalyzerRegistry` is independent of indexing and selects analyzers by path or
  language.
- The generic analyzer emits normalized chunks; no language-specific syntax is
  present in the service or storage layer.
- Shared components use `Arc`; there is no application-wide mutex.

## Verification

Completed successfully on 2026-08-25:

```text
cargo fmt --check
cargo check
cargo clippy --all-targets --all-features -- -D warnings
cargo test (5 passed)
```

The initial migration creates all required v0.1 domain tables and FTS5 indexes,
with foreign keys and cascade ownership defined.

