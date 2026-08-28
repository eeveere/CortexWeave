# Native Harness Contract

MCP is one transport over the application boundary, not part of CortexWeave's
core. A coding harness can hold `Arc<CortexWeaveService>` and call the facade
directly for workspace registration, reconciliation, retrieval, explicit
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
The direct contract begins with `HarnessContextRequest`. The service validates
an active workspace/session/task scope and returns `HarnessContext` containing a
bounded explained packet plus explicit selected-source audit records. Each
selected record carries workspace, source type, path, symbol, and component
scores. The harness evaluates that result through its own
`HarnessContextPolicy`; CortexWeave does not decide whether the evidence is
sufficient.

The harness also owns evidence policy. It may use a `ContextPacket` as the
default evidence boundary, inspect its explanation, and allow selected chunk
hydration or an explicitly audited out-of-packet read when its policy permits.
`HarnessHydrationRequest::from_context` authorizes selected code IDs. Any other
chunk ID is rejected unless the harness supplies a non-empty override reason.
An accepted override records a durable `context_hydration_override` event with
the reason, session/task scope, chunk, path, symbol, and the fact that the item
was not packet-scored. Returned hydration distinguishes packet selection scores
from out-of-packet, unscored evidence.

After evidence use, the harness calls the existing facade methods to record
factual tool/compiler/test events, activate useful sources, record explicit
memory, and create checkpoints. MCP clients cannot enforce this discipline by
prompt alone. See the [v0.3 harness-controlled context plan](v0.3-plan.md) for
the remaining staged work.
