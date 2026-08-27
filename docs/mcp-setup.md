# MCP Setup

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

Required tools are `semantic_search`, `semantic_get`, `memory_record`,
`memory_search`, `memory_recent`, `workspace_list`, `workspace_status`, and
`workspace_reindex`. Additional v0.1 tools are `session_start`, `session_end`,
and `event_record`.

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
  --env CORTEXWEAVE_WORKSPACE_ROOT "$PWD"
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
  --env CORTEXWEAVE_WORKSPACE_ROOT "$PWD"
```

In a project-local config, `$PWD` is the directory where Crush was launched. A
repository root or a subdirectory both resolve to the registered root, including
intentionally nested registrations. Crush uses the closest project config to its
working directory, so this also works when a session starts below the repository
root. Windows path identity accepts native, slash-normalized, verbatim, UNC, and
Bash-style drive paths.

Do not put `--env CORTEXWEAVE_WORKSPACE_ROOT "$PWD"` in a global `crushrc`.
There, `$PWD` can be Crush's global configuration directory rather than the
repository. For a global configuration with one registered workspace, omit the
environment line and rely on singleton resolution. With several workspaces, use
a project-local override or have the agent supply an explicit `workspace`
selector.

After MCP initialization, `serve` starts a watcher for every registered workspace
and performs initial reconciliation in the background. A large first index does
not delay the handshake. Tool failures are model-visible MCP errors; JSON-RPC
protocol failures remain protocol errors.

CortexWeave uses one JSON-RPC message per line and negotiates MCP protocol version
`2025-06-18`. Stdout is reserved for MCP. Check the client server logs or run
`doctor` directly when initialization fails.
