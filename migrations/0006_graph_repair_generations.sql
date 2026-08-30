PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS workspace_graph_repairs (
    workspace_id TEXT PRIMARY KEY NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    generation_id TEXT NOT NULL UNIQUE,
    mode TEXT NOT NULL CHECK(mode IN ('if_needed', 'force')),
    target_content_revision INTEGER NOT NULL CHECK(target_content_revision >= 0),
    state TEXT NOT NULL CHECK(state IN ('active', 'failed', 'interrupted', 'completed')),
    started_at TEXT NOT NULL,
    lease_expires_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    completed_at TEXT,
    documents_considered INTEGER NOT NULL DEFAULT 0 CHECK(documents_considered >= 0),
    documents_repaired INTEGER NOT NULL DEFAULT 0 CHECK(documents_repaired >= 0),
    documents_failed INTEGER NOT NULL DEFAULT 0 CHECK(documents_failed >= 0),
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS idx_workspace_graph_repairs_state_lease
ON workspace_graph_repairs(state, lease_expires_at);

CREATE TABLE IF NOT EXISTS graph_document_projections (
    document_id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    content_revision INTEGER NOT NULL CHECK(content_revision >= 0),
    analyzer_id TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    structure_version TEXT NOT NULL,
    node_count INTEGER NOT NULL CHECK(node_count >= 0),
    fact_count INTEGER NOT NULL CHECK(fact_count >= 0),
    edge_count INTEGER NOT NULL CHECK(edge_count >= 0),
    unresolved_count INTEGER NOT NULL CHECK(unresolved_count >= 0),
    projected_at TEXT NOT NULL,
    FOREIGN KEY(document_id, workspace_id)
        REFERENCES documents(id, workspace_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_graph_document_projections_workspace_revision
ON graph_document_projections(workspace_id, content_revision);

INSERT INTO graph_document_projections(
    document_id,
    workspace_id,
    content_revision,
    analyzer_id,
    analyzer_version,
    structure_version,
    node_count,
    fact_count,
    edge_count,
    unresolved_count,
    projected_at
)
SELECT
    states.document_id,
    states.workspace_id,
    states.content_revision,
    states.analyzer_id,
    states.analyzer_version,
    states.structure_version,
    (SELECT COUNT(*) FROM graph_nodes nodes WHERE nodes.document_id = states.document_id AND nodes.workspace_id = states.workspace_id),
    (SELECT COUNT(*) FROM graph_relationship_facts facts WHERE facts.source_document_id = states.document_id AND facts.workspace_id = states.workspace_id),
    (SELECT COUNT(*) FROM graph_edges edges WHERE edges.source_document_id = states.document_id AND edges.workspace_id = states.workspace_id),
    (SELECT COUNT(*) FROM unresolved_relationships unresolved WHERE unresolved.source_document_id = states.document_id AND unresolved.workspace_id = states.workspace_id),
    states.analyzed_at
FROM graph_document_states states;
