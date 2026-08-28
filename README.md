![CortexWeave-banner](https://github.com/eeveere/CortexWeave/blob/9c9fd34b5557527124bb3f1039d45dfc4a564553/CortexWeave-banner.JPG)

# CortexWeave

CortexWeave v0.1 is a local-first semantic code index and explicit memory store
for coding agents. It watches registered workspaces, analyzes Rust, Python,
JavaScript, TypeScript, C#, and Go with tree-sitter, and exposes retrieval and memory
operations through a CLI, an application facade, and MCP over stdio.

## Quick Start

1. Run an OpenAI-compatible embedding server on `http://127.0.0.1:8081`.
2. Create your local configuration from the example, then adjust the embedding
   model and optional dimension:

```text
Copy-Item cortexweave.example.toml cortexweave.toml
```

   Use an absolute `database.path` in `cortexweave.toml` when an MCP client will
   launch CortexWeave from an unrelated working directory.
3. Build and verify CortexWeave:

```text
cargo build --release
cargo test --all-targets
target/release/cortexweave --config cortexweave.toml doctor
```

4. Register and index a workspace:

```text
target/release/cortexweave --config cortexweave.toml workspace add C:/dev/project --name project
target/release/cortexweave --config cortexweave.toml workspace list
target/release/cortexweave --config cortexweave.toml reindex <workspace-id>
```

   Registration prints the workspace UUID. Existing UUID-based calls remain
   supported, but MCP clients may instead select an unambiguous workspace by
   name, absolute root or subdirectory path, or `file://` URI.

5. Add the `serve` command and an explicit project-root hint to an MCP client
   using `docs/mcp-setup.md`. The hint lets normal MCP calls omit the UUID; it
   never registers a workspace automatically.

## Documentation

- `docs/architecture.md`: boundaries, data flow, and failure model
- `docs/configuration.md`: every configuration setting
- `docs/embedding-server.md`: embedding endpoint contract and checks
- `docs/mcp-setup.md`: stdio MCP client setup
- `docs/cli.md`: command reference
- `docs/development.md`: local development and verification
- `docs/troubleshooting.md`: diagnosis and recovery
- `docs/analyzers.md`: analyzer API and complete language-extension path
- `docs/native-adapter.md`: future direct harness integration

CortexWeave stores explicit memories only when a caller records them. It does
not infer memories, invoke reasoning models, execute tools, or orchestrate agent
loops.
