# Checkpoint 09: Harness Compatibility Review

**Status:** Passed after completing the application facade

## Answer

Yes. A dedicated CortexWeave harness can use `CortexWeaveService` directly
without depending on MCP or reproducing MCP-specific behavior.

## Review Findings

The underlying services were already transport-neutral, but the facade did not
cover the complete application workflow. A harness would have needed to reach
into SQLite and the indexing/retrieval services for workspace, lifecycle, and
item-lookup operations.

The facade now provides:

- workspace registration, listing, status, and reindexing;
- semantic, lexical, and hybrid search plus score-free item lookup;
- explicit memory recording, search, and recency retrieval;
- session start/end and task start/update/complete;
- event recording and recency retrieval.

All inputs and outputs are domain models, Serde values, and application result
types. No MCP, JSON-RPC, CLI, or stdio type crosses the service boundary.

## Architecture Checks

- Analyzer selection remains behind `AnalyzerRegistry`; the indexing service
  consumes normalized chunks and has no language-specific branches.
- Retrieval results preserve source, workspace, path, language, symbol, line,
  content, and component-score provenance.
- Memory and events remain distinct durable records with optional session/task
  provenance.
- Adapter configuration does not affect service method contracts.

The executable proof is
`service::cortex::tests::dedicated_harness_can_use_the_facade_without_an_adapter`.
After service construction, it performs the entire workflow through the facade.
