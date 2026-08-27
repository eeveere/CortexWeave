# CortexWeave Engineering Rules

- CortexWeave is a cognitive substrate, not an agent harness.
- MCP is an adapter. Core code must not depend on MCP types.
- Adapters call application services; core subsystems never call adapters.
- Language analyzers are pluggable and return normalized chunks.
- The indexing core must contain no language-specific syntax or AST logic.
- Unsupported text languages must fall back to deterministic generic chunking.
- Embedding inference is external and accessed through a provider abstraction.
- Never silently mix vectors produced by different models or dimensions.
- SQLite is the v0.1 persistent store and owns transaction boundaries.
- Code, memories, and events are distinct domains with explicit provenance.
- Incremental indexing must preserve unchanged chunks and embeddings.
- Prefer deterministic mechanisms before model-based mechanisms.
- Keep `cargo fmt --check`, `cargo check`, Clippy, and tests green.
- Record material architecture choices in `docs/decisions.md`.
- Do not implement later-roadmap orchestration features prematurely.

