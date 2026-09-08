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

## Durable terminal operations

`deliver_native` now also accepts `NativeOperation::CompleteTask` and
`NativeOperation::EndSession`. Persist the exact request and key before calling;
retry the same request after an uncertain acknowledgement. Terminal operations
return the original immutable receipt after a successful prior commit, even if
later lifecycle state has changed. Same-key changed content and competing terminal
writers conflict. Mutation and receipt commit together under one SQLite writer
transaction; failed receipt writes leave no terminal effect.

Completion names workspace, open session, active task, exact `expected_details`
and replacement `details`. Closure names that task's exact `task_completion_key`,
its completed details and a nonempty object of session `owner_metadata` markers.
Every marker must have a non-null value matching the session. Closure validates
the current task against the full original completion record, including times.
These are data preconditions; the caller retains harness policy and authorization.
Other task admission is not protected by a session-wide lease.

Operation JSON and receipt JSON each have a 64 KiB serialized-byte limit. Terminal
receipts share workspace lifecycle and count toward native receipt footprint on
explicit workspace deregistration. Legacy task/session terminal storage writes
cannot overwrite already-terminal records. An unkeyed historical completion has
no native receipt to replay; callers must retain the unresolved intent for
inspection instead of attributing another writer's state to their own request.
See D110 in [architecture decisions](decisions.md).

Qualification completed 2026-09-08 with Rust 1.98.0: 293 Windows and 294 Docker
Linux tests passed, including nine terminal-receipt contract tests. The existing
user-selected repository evaluation remains ignored. Formatting, locked tests and
warnings-denied Clippy passed on both platforms. Shuttle's dependent suite passed
77 Windows and 80 Linux tests against this same local patch. Docker used read-only
source mounts, an init process, two CPUs and 6 GiB memory. Human terminal usability
review and publication of an immutable dependency revision remain separate gates.
