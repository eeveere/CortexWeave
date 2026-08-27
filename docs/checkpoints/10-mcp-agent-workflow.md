# Checkpoint 10: MCP Agent Workflow

**Status:** Protocol workflow passed

The stdio server was exercised as an MCP client would use it:

- initialize and tool discovery return newline-delimited JSON-RPC responses;
- stdout remains protocol-only while logs use stderr;
- required code, memory, and workspace tools are advertised;
- a mixed Rust, Python, and TypeScript workspace is searched through MCP;
- edits to each language are recovered through watchers without manual reindex;
- a BLAKE3 decision is recorded, a session is ended, and the decision is
  recovered from a new session through MCP memory search.

The executable proofs are `tests/mcp_stdio.rs` and
`adapters::mcp::tests::mcp_mixed_language_workflow_observes_watcher_freshness_and_memory`.

The current task environment does not expose safe registration of a new local
MCP server into a separate coding-client configuration. Connecting the finished
`cortexweave serve` command to a chosen external client remains the final host
integration step; it does not require changes to CortexWeave's core or adapter.
