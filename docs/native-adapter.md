# Future Native Adapter

MCP is one transport over the application boundary, not part of CortexWeave's
core. A future coding harness can hold `Arc<CortexWeaveService>` and call the
facade directly for workspace registration, reconciliation, retrieval, explicit
memory, sessions, tasks, events, and instrumentation.

A native adapter should:

- translate harness identities and requests into domain values
- preserve workspace/session/task association
- record memories only on explicit harness intent
- record factual tool/compiler/test events without interpreting them
- start and own watcher handles for active workspaces
- map typed failures into the harness error model

It should not duplicate SQL, call analyzers directly, implement its own hybrid
ranking, or route through MCP JSON. Transport-specific request and response
types stay in the adapter.

Lifecycle ownership is the principal design choice. A long-running harness
should open one service, start watchers after registration, reuse the service
across tasks and sessions, and shut watchers down before closing the process.
The existing facade compatibility test demonstrates this path without an
adapter and guards against MCP types leaking into core APIs.
