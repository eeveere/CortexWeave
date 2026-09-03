# CortexWeave v0.5 C4 / Structurizr model

## Purpose and baseline

This directory is the canonical, maintainable C4 model for CortexWeave
**v0.5.0 as implemented**. It is a model of the current repository, not a
roadmap or a proposal. Its root is [workspace.dsl](workspace.dsl), which
includes the model, views, and visual vocabulary.

CortexWeave is one local Rust/Tokio application plus one SQLite/SQLx/FTS5
database container. CLI, stdio MCP, indexing, graph, retrieval, context, state,
and verified Experience are application components, not deployed services or
containers. The Coding Agent/Harness, registered workspace filesystem, and
OpenAI-compatible embedding provider are external systems.

## Source-of-truth policy

The implementation is authoritative. This model was cross-checked against the
repository extraction at `docs/architecture-map/` (overview, inventory,
relationships, data model, execution model, runtime flows, implementation
status, and evidence index) and the source/migrations it cites. The most useful
primary implementation anchors are `src/service/cortex.rs`, `src/main.rs`,
`src/adapters/mcp.rs`, `src/indexing/`, `src/retrieval/service.rs`, `src/graph/`,
`src/storage/`, and `migrations/`.

If a model element conflicts with current source or migration evidence, update
the model to match the implementation and record any material architecture
choice in `docs/decisions.md` through the normal engineering workflow.

## Layout

```text
c4/
├── workspace.dsl                 # Canonical DSL entry point
├── model/                        # people/systems, containers, components, relations
├── views/                        # C4, dynamic, and local deployment views
├── styles.dsl                    # shared tags/styles
└── README.md
```

## View catalog

| Key | Purpose |
|---|---|
| `CortexWeave-SystemContext` | User, external agent/harness, workspace, embedding provider, CortexWeave |
| `CortexWeave-Containers` | One Rust application plus SQLite database and external systems |
| `CortexWeave-Application` | Broad Level 3 application services |
| `CortexWeave-Indexing` | Scan → analysis → segments/vectors → source index → graph projection, plus MCP-lifetime watcher |
| `CortexWeave-Context` | Hybrid retrieval and current-first bounded context assembly |
| `CortexWeave-Structural` | Derived graph projection, resolution, repair and freshness-gated reads |
| `CortexWeave-Experience` | Events/Episodes/evidence/consolidation/assessment and historical supplemental use |
| `Indexing-Initial` | Ordered explicit reindex flow |
| `Indexing-Incremental` | Ordered watcher-driven change flow |
| `Context-Semantic` | Ordered `semantic_context` flow |
| `Graph-Repair` | Ordered repair/reprojection flow |
| `Experience-Consolidation` | Ordered preview and explicit acceptance flow |
| `Experience-Reuse` | Ordered current-first historical Experience reuse flow |
| `CortexWeave-LocalDeployment` | Local workstation topology, not Docker/distributed deployment |

## Render and validate locally

Use a Structurizr DSL CLI release locally (or the Structurizr Lite/container
workflow) with `workspace.dsl` as the entry file. A Docker-backed validation
command is:

```text
docker run --rm -v "<absolute-path-to>/docs/architecture/c4:/usr/local/structurizr" structurizr/structurizr validate -workspace /usr/local/structurizr/workspace.dsl
```

Typical local CLI export command is:

```text
docker run --rm -v "<absolute-path-to>/docs/architecture/c4:/usr/local/structurizr" structurizr/structurizr export -workspace /usr/local/structurizr/workspace.dsl -format plantuml
```

Exact command names/options vary by installed Structurizr release; validate the
root DSL after any edit, then export/render through the team’s chosen
Structurizr workflow. The current Docker-backed
`structurizr/structurizr validate` command completed successfully on 2026-09-02
after static/dynamic relationship and style-block audits.
The model is also structurally reviewed for include paths, identifiers,
component relationships, view scope, and deferred-feature exclusion.

## Deferred architecture policy

The active views intentionally exclude automatic Episode grouping, background
or model-backed consolidation, semantic Experience embeddings, automatic
memory merging/promotion, retention/deletion policy, reuse-driven confidence,
global Experience promotion, a dedicated CortexWeave agent harness,
high-frequency external event streaming, durable action-to-outcome modeling,
Git diff/base inspection, distributed storage, vector databases, and an HTTP
server. They may be documented as future work, but must not appear as current
C4 elements without implementation evidence.

## Update procedure

1. Change or verify the source and migrations first; do not use this model to
   invent architecture.
2. Update the relevant extraction/evidence documentation and relationships.
3. Update the smallest appropriate model file, keeping stable identifiers and
   view keys where possible.
4. Review affected static and dynamic views, especially authority boundaries:
   MCP remains an adapter; SQLite remains the durable store; graph remains
   derived; Experience remains historical supplemental state.
5. Run Structurizr validation/export and record material decisions when needed.
