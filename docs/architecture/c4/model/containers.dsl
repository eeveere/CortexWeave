!element cortexweave {
    application = container "CortexWeave Application" "One Rust application/runtime containing the CLI and stdio MCP adapters, application facade, workspace/source intelligence, indexing, graph, retrieval/context, state, and verified Experience services." "Rust / Tokio" "CortexWeave,Application" {
        !include components.dsl
    }

    database = container "CortexWeave Database" "Single durable store for workspace identity, source index and vectors, FTS projections, memory/state/events, structural graph/revisions, Episodes, Experiences, and assessment history. SQLite owns transaction boundaries and integrity constraints." "SQLite / SQLx / FTS5" "Persistence,Authoritative"
}
