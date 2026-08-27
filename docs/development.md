# Development Workflow

## Prerequisites

- A current stable Rust toolchain compatible with edition 2024
- SQLite support supplied through `sqlx`
- An OpenAI-compatible embedding server for live CLI/MCP checks

## Build and Verify

```text
cargo fmt --all -- --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo test --test performance_baseline -- --nocapture
```

Most tests use deterministic in-process embeddings and temporary SQLite
databases, so they do not need a network service. The black-box CLI and MCP tests
launch the compiled binary.

## Change Discipline

Keep transport types in adapters, SQL in storage, language syntax in analyzers,
and orchestration in the service facade. Run focused tests while iterating, then
the full commands above. Add a regression test whenever changing reconciliation,
stable keys, watcher convergence, embedding compatibility, or transaction scope.

Use migrations for schema changes. Never mutate a released migration; add a new
numbered migration. Preserve analyzer IDs and versions deliberately because they
control chunk compatibility.

The performance baseline is not a hard wall-clock gate. Its assertions cover
the stable requirements: no restart embeddings and a one-to-one ratio between
changed and re-embedded chunks. Compare timing and memory output on the same
machine and build profile.
