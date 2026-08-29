# MCP Setup

For a plain-language guide to registering multiple projects and making Crush
select the right one automatically, see [Using CortexWeave with More Than One
Crush Project](crush-workspaces.md).

Build the release binary and register every workspace before starting an MCP
client. Use absolute executable and configuration paths because clients commonly
launch servers from an unrelated working directory.

```json
{
  "mcpServers": {
    "cortexweave": {
      "command": "C:\\dev\\CortexWeave\\target\\release\\cortexweave.exe",
      "args": [
        "--config",
        "C:\\dev\\CortexWeave\\cortexweave.toml",
        "serve",
        "--workspace-root",
        "C:\\dev\\your-project"
      ]
    }
  }
}
```

The root hint may instead be supplied as
`CORTEXWEAVE_WORKSPACE_ROOT`. `serve --workspace-root` has precedence when both
are present. A hint may name a registered root or any subdirectory inside one;
CortexWeave chooses the longest containing registered root. Hints affect tool
resolution only: they never register a workspace, grant access to an
unregistered path, or narrow watcher coverage.

## Workspace Selection

Every workspace-scoped MCP tool accepts these optional fields:

- `workspace_id`: the legacy exact UUID field, retained unchanged;
- `workspace`: a UUID, unique exact name, absolute root/subdirectory path, or
  `file://` URI.

Resolution precedence is explicit `workspace_id`, explicit `workspace`, adapter
root hint, then a single registered workspace. If both explicit fields are
present, they must resolve to the same registration. Duplicate or
case-insensitively ambiguous names fail with candidate UUIDs, names, and roots;
there is no fuzzy matching.

`workspace_list` takes no selector. It reports every registered UUID, name, and
root, marks the root matched by the adapter hint, and includes any hint-resolution
error. It remains available when ordinary tool calls cannot choose a workspace.

Core tools are `semantic_search`, `semantic_context`, `resume_context`,
`semantic_get`, `memory_record`, `memory_search`, `memory_recent`,
`working_set`, `context_pin`, `context_unpin`, `checkpoint_create`,
`checkpoint_latest`, `workspace_list`, `workspace_status`,
`workspace_readiness`, and `workspace_reindex`. Structural tools are
`graph_status`, `graph_find`, `graph_neighbors`, `graph_callers`, `graph_callees`,
`graph_references`, `graph_implementations`, `graph_tests`,
`graph_dependencies`, `graph_dependents`, `graph_impact_symbol`, and
`session_start`, `session_end`, and `event_record`.

The [Structural Graph Architecture](graph-architecture.md) describes graph
semantics, current-state behavior, analyzer capabilities, and limitations.

`workspace_readiness` is read-only. It identifies supported languages currently
using generic fallback and estimates the persisted chunks and embeddings that
an explicit analyzer enablement and reindex would replace. It never edits the
configuration or starts reconciliation.

`semantic_context` requires `query` and returns a bounded `ContextPacket`. It
accepts optional `session_id`, `task_id`, `token_budget`, `include_code`,
`include_documents`, `include_memories`, `include_events`, `path_scope`, and
`language_scope`. Source filters default to enabled. MCP accepts budgets from
zero through 65,536 tokens to keep one stdio response bounded; the packet
returns the requested budget and the actual selected-item estimate.

Set `include_explanation` on `semantic_context` or `resume_context` to include
the selected items' reasons, component scores, token estimates, and truncation
status. This diagnostic is outside the prompt packet budget.

Use `graph_find` to resolve an exact graph symbol or source path, then pass a
returned node ID to a graph relation tool. All graph tools accept optional
`allow_stale`, `max_nodes`, `max_edges`, and `max_depth`. MCP caps those bounds
at 100 nodes, 500 edges, and depth 4 to keep stdio responses compact. They
return graph revision, applied limits, typed paths, confidence, and truncation.
Current graph state is required unless `allow_stale` is explicitly true.
`graph_impact_symbol` and `graph_impact_path` report reverse graph reachability,
not an inferred source-control diff.

`graph_neighbors` is always direct (one hop), regardless of a supplied depth.
`graph_tests` returns likely direct-call test associations with their evidence
and confidence; it does not prove behavioral coverage. Exact node IDs must
belong to the selected workspace, and explicit seed requests that exceed the
effective node bound fail rather than returning a partial answer.

Use `graph_status` before a structural read when a client needs an explicit
health decision. It returns the workspace graph revision, whether it is
current, node/edge/unresolved counts, and per-language active analyzer
capabilities. Unlike relation and impact tools, it is diagnostic and always
reports the persisted state; it does not require an `allow_stale` opt-in.

Use `semantic_context` as the one-call retrieval operation when an agent needs
bounded evidence to answer a question. `semantic_get` is different: it accepts
only an exact chunk ID returned by a previous CortexWeave result, not a path,
symbol name, or natural-language question.

`resume_context` accepts optional `session_id`, `task_id`, and `token_budget`.
It returns the selected interaction scope, evidence session, checkpoint,
recent-change aggregates, working-set snapshots, and a bounded `ContextPacket`.
The working-set and pin tools require `session_id`; pin tools also require a
source ID and source type. `checkpoint_create` requires `session_id` and
`content`, while `checkpoint_latest` optionally scopes the read by session or
task.

## Crush

Put this release-build entry in the project's `.crushrc`, not the global
`%USERPROFILE%\.config\crush\crushrc`:

```text
mcp add cortexweave \
  --type stdio \
  --command C:/dev/CortexWeave/target/release/cortexweave.exe \
  --args --config \
  --args C:/dev/CortexWeave/cortexweave.toml \
  --args serve \
  --args --workspace-root \
  --args "$PWD" \
  --timeout 120
```

For development through Cargo, use:

```text
mcp add cortexweave \
  --type stdio \
  --command cargo \
  --args run \
  --args --manifest-path \
  --args C:/dev/CortexWeave/Cargo.toml \
  --args -- \
  --args --config \
  --args C:/dev/CortexWeave/cortexweave.toml \
  --args serve \
  --args --workspace-root \
  --args "$PWD" \
  --timeout 120
```

In a project-local config, `$PWD` is the directory where Crush was launched and
is passed to `serve --workspace-root`. A repository root or a subdirectory both
resolve to the registered root, including intentionally nested registrations.
Crush uses the closest project config to its working directory, so this also
works when a session starts below the repository root. Windows path identity
accepts native, slash-normalized, verbatim, UNC, and Bash-style drive paths.

Do not put `--args "$PWD"` in a global `crushrc`. There, `$PWD` can be Crush's
global configuration directory rather than the repository. For a global
configuration with one registered workspace, omit the workspace-root arguments
and rely on singleton resolution. With several workspaces, use a project-local
override or have the agent supply an explicit `workspace` selector.

After MCP initialization, `serve` starts a watcher for every registered workspace
and performs initial reconciliation in the background. A large first index does
not delay the handshake. Tool failures are model-visible MCP errors; JSON-RPC
protocol failures remain protocol errors.

CortexWeave uses one JSON-RPC message per line and negotiates MCP protocol version
`2025-06-18`. Stdout is reserved for MCP. Check the client server logs or run
`doctor` directly when initialization fails.
