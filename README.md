![CortexWeave-banner](https://github.com/eeveere/CortexWeave/blob/9c9fd34b5557527124bb3f1039d45dfc4a564553/CortexWeave-banner.JPG)

# CortexWeave

CortexWeave v0.5 is a local-first context substrate for coding agents and
harnesses. It indexes registered workspaces, keeps explicit memory, durable
structural code provenance, and verified historical Experience, and returns
bounded, explainable context packets through a CLI and MCP over stdio.

It is not an agent harness. CortexWeave does not invoke reasoning models,
execute tools, or orchestrate agent loops. A harness such as [Crush](https://github.com/charmbracelet/crush "Glamourous agentic coding for all 💘") decides when
to call it and owns any subsequent tool use.

## What It Provides

- Semantic, lexical, and hybrid retrieval over code and documents.
- A persistent, incrementally reconciled structural graph with normalized
  symbols, typed code relationships, provenance, revisions, and freshness
  state.
- Exact symbol and path lookup; callers, callees, references, implementations,
  dependencies, dependents, likely direct-call tests, and bounded impact
  analysis.
- Graph-aware context that retains bounded structural evidence without
  displacing required task, checkpoint, or pinned context.
- Token-bounded context packets assembled from code, documents, trusted
  memories, events, and task or session state.
- Working sets, pins, checkpoints, and resume context for long-running work.
- Context explanations with selection reasons, scores, token estimates, and
  truncation status.
- Analyzer readiness reporting that identifies generic fallback use and estimates
  the cost of explicitly enabling an available analyzer and reindexing.
- Imported-memory trust review, source-range provenance, duplicate accounting,
  and explicit, audited supersession.
- Explicit episodes and deterministic, immutable Experience consolidation with
  bounded historical retrieval and reviewed assessment history.
- Pluggable language analyzers that produce normalized chunks; unsupported text
  formats use deterministic generic chunking.

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
cargo test --all-targets --all-features
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

5. Check analyzer readiness whenever a workspace contains supported languages
   that are currently using generic fallback:

```text
target/release/cortexweave --config cortexweave.toml readiness <workspace-id>
```

   `readiness` is read-only. It reports the exact `languages.<name>` settings
   to enable and the expected rebuild cost; configuration and reindexing remain
   explicit actions.

6. Add the `serve` command and an explicit project-root hint to an MCP client
   using `docs/mcp-setup.md`. The hint lets ordinary MCP calls omit the UUID; it
   never registers a workspace automatically.

7. Inspect the structural graph after indexing:

```text
target/release/cortexweave --config cortexweave.toml graph status <workspace-id>
target/release/cortexweave --config cortexweave.toml graph find <workspace-id> <symbol-or-path>
```

   Use `graph find` to obtain an exact node ID before requesting relationships
   or impact. Graph reads require a current projection by default; pass
   `--allow-stale` only when a caller can safely accept a stale result.

For multiple Crush projects, follow
[Using CortexWeave with More Than One Crush Project](docs/crush-workspaces.md).
Use [`.crushrc.example`](.crushrc.example) as the project-local Crush template.

## Documentation

- `docs/architecture.md`: boundaries, data flow, and failure model
- `docs/configuration.md`: every configuration setting
- `docs/embedding-server.md`: embedding endpoint contract and checks
- `docs/mcp-setup.md`: stdio MCP client setup
- `docs/cli.md`: command reference
- `docs/development.md`: local development and verification
- `docs/troubleshooting.md`: diagnosis and recovery
- `docs/analyzers.md`: analyzer API and complete language-extension path
- `docs/context.md`: context packets, ranking, evaluation, and native harness contract
- `docs/resume-context-design.md`: checkpoint and cross-session resume behavior
- `docs/crush-workspaces.md`: project-local Crush configuration
- `docs/memory-integrity.md`: imported-memory trust and consolidation policy
- `docs/v0.3-plan.md`: completed v0.3 delivery plan
- `docs/native-adapter.md`: direct-harness compatibility constraints
- `docs/graph-architecture.md`: structural graph semantics, provenance, freshness, and extension boundaries
- `docs/verified-experience.md`: v0.5 episodes, typed evidence, Experience, retrieval, authority, and limits
- `docs/v0.5-release-boundary.md`: accepted verified-experience release scope and deferred research
- [v0.5.1 implementation plan](<CortexWeave v0.5.1 Test Evidence and Vitest Integration Plan.md>): proposed Vitest and Python unittest evidence, with Crush qualification on emCP and PiHype

## Context and Memory Boundaries

`semantic_context` is the primary one-call MCP retrieval operation. It produces
a bounded evidence packet; an agent may still choose ordinary file tools after
receiving it. The packet and explanation are the CortexWeave result to inspect
when evaluating retrieval quality.

CortexWeave stores memories only when a caller records or imports them.
Human-authorized memories are trusted when recorded. Imported memories require
source segments and remain outside automatic context until an explicit trust
review accepts them.

Experience is different from Memory: it is an immutable interpretation of an
explicit episode with typed evidence and scoped verification. It may supplement
a later matching failure, but never replaces current source, current task, or
current deterministic observations. See [Verified Experience Core](docs/verified-experience.md).
