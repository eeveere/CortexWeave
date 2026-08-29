PRAGMA foreign_keys = ON;

ALTER TABLE documents
ADD COLUMN content_revision INTEGER NOT NULL DEFAULT 0;

UPDATE documents
SET content_revision = 1
WHERE content_revision = 0;

CREATE UNIQUE INDEX IF NOT EXISTS idx_documents_id_workspace
ON documents(id, workspace_id);

CREATE TABLE IF NOT EXISTS workspace_graph_revisions (
    workspace_id TEXT PRIMARY KEY NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    content_revision INTEGER NOT NULL DEFAULT 0 CHECK(content_revision >= 0),
    graph_content_revision INTEGER NOT NULL DEFAULT 0
        CHECK(graph_content_revision >= 0 AND graph_content_revision <= content_revision),
    graph_schema_version INTEGER NOT NULL DEFAULT 1 CHECK(graph_schema_version > 0),
    graph_state TEXT NOT NULL DEFAULT 'current'
        CHECK(graph_state IN ('current', 'updating', 'stale', 'error')),
    graph_update_started_at TEXT,
    failed_graph_target_revision INTEGER
        CHECK(failed_graph_target_revision IS NULL OR failed_graph_target_revision >= 0),
    last_graph_error TEXT,
    updated_at TEXT NOT NULL
);

CREATE TRIGGER IF NOT EXISTS workspaces_graph_revisions_insert
AFTER INSERT ON workspaces
BEGIN
    INSERT INTO workspace_graph_revisions(
        workspace_id,
        content_revision,
        graph_content_revision,
        graph_schema_version,
        graph_state,
        updated_at
    ) VALUES (NEW.id, 0, 0, 1, 'current', NEW.created_at);
END;

INSERT OR IGNORE INTO workspace_graph_revisions(
    workspace_id,
    content_revision,
    graph_content_revision,
    graph_schema_version,
    graph_state,
    updated_at
)
SELECT
    workspaces.id,
    CASE WHEN EXISTS(
        SELECT 1 FROM documents WHERE documents.workspace_id = workspaces.id
    ) THEN 1 ELSE 0 END,
    0,
    1,
    CASE WHEN EXISTS(
        SELECT 1 FROM documents WHERE documents.workspace_id = workspaces.id
    ) THEN 'stale' ELSE 'current' END,
    workspaces.updated_at
FROM workspaces;

CREATE TABLE IF NOT EXISTS graph_document_states (
    document_id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    content_revision INTEGER NOT NULL CHECK(content_revision >= 0),
    analyzer_id TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    structure_version TEXT NOT NULL,
    last_error TEXT,
    analyzed_at TEXT NOT NULL,
    FOREIGN KEY(document_id, workspace_id)
        REFERENCES documents(id, workspace_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_graph_document_states_workspace_revision
ON graph_document_states(workspace_id, content_revision);

CREATE TABLE IF NOT EXISTS graph_nodes (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    node_type TEXT NOT NULL,
    stable_key TEXT NOT NULL,
    language TEXT,
    name TEXT NOT NULL,
    qualified_name TEXT,
    document_id TEXT,
    chunk_id TEXT REFERENCES chunks(id) ON DELETE SET NULL,
    source_path TEXT,
    source_start_byte INTEGER,
    source_end_byte INTEGER,
    analyzer_id TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    structure_version TEXT NOT NULL,
    content_revision INTEGER NOT NULL CHECK(content_revision >= 0),
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(id, workspace_id),
    UNIQUE(workspace_id, stable_key),
    FOREIGN KEY(document_id, workspace_id)
        REFERENCES documents(id, workspace_id) ON DELETE CASCADE,
    CHECK(
        (source_path IS NULL AND source_start_byte IS NULL AND source_end_byte IS NULL)
        OR (
            source_path IS NOT NULL
            AND source_start_byte IS NOT NULL
            AND source_end_byte IS NOT NULL
            AND source_start_byte >= 0
            AND source_end_byte >= source_start_byte
        )
    )
);

CREATE INDEX IF NOT EXISTS idx_graph_nodes_workspace_qualified_name
ON graph_nodes(workspace_id, qualified_name);

CREATE INDEX IF NOT EXISTS idx_graph_nodes_workspace_name
ON graph_nodes(workspace_id, name);

CREATE INDEX IF NOT EXISTS idx_graph_nodes_workspace_type
ON graph_nodes(workspace_id, node_type);

CREATE INDEX IF NOT EXISTS idx_graph_nodes_document
ON graph_nodes(document_id);

CREATE TRIGGER IF NOT EXISTS graph_nodes_chunk_workspace_insert
BEFORE INSERT ON graph_nodes
FOR EACH ROW
WHEN NEW.chunk_id IS NOT NULL AND NOT EXISTS(
    SELECT 1
    FROM chunks
    JOIN documents ON documents.id = chunks.document_id
    WHERE chunks.id = NEW.chunk_id
      AND chunks.document_id = NEW.document_id
      AND documents.workspace_id = NEW.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'graph node chunk must belong to node document and workspace');
END;

CREATE TRIGGER IF NOT EXISTS graph_nodes_chunk_workspace_update
BEFORE UPDATE OF workspace_id, document_id, chunk_id ON graph_nodes
FOR EACH ROW
WHEN NEW.chunk_id IS NOT NULL AND NOT EXISTS(
    SELECT 1
    FROM chunks
    JOIN documents ON documents.id = chunks.document_id
    WHERE chunks.id = NEW.chunk_id
      AND chunks.document_id = NEW.document_id
      AND documents.workspace_id = NEW.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'graph node chunk must belong to node document and workspace');
END;

CREATE TABLE IF NOT EXISTS graph_relationship_facts (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    source_document_id TEXT NOT NULL,
    relationship_key TEXT NOT NULL,
    from_node TEXT REFERENCES graph_nodes(id) ON DELETE SET NULL,
    from_stable_key TEXT NOT NULL,
    edge_type TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_value TEXT NOT NULL,
    confidence REAL NOT NULL CHECK(confidence >= 0.0 AND confidence <= 1.0),
    analyzer_id TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    structure_version TEXT NOT NULL,
    source_path TEXT,
    source_start_byte INTEGER,
    source_end_byte INTEGER,
    content_revision INTEGER NOT NULL CHECK(content_revision >= 0),
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(id, workspace_id),
    UNIQUE(workspace_id, source_document_id, relationship_key),
    FOREIGN KEY(source_document_id, workspace_id)
        REFERENCES documents(id, workspace_id) ON DELETE CASCADE,
    CHECK(
        (source_path IS NULL AND source_start_byte IS NULL AND source_end_byte IS NULL)
        OR (
            source_path IS NOT NULL
            AND source_start_byte IS NOT NULL
            AND source_end_byte IS NOT NULL
            AND source_start_byte >= 0
            AND source_end_byte >= source_start_byte
        )
    )
);

CREATE INDEX IF NOT EXISTS idx_graph_relationship_facts_workspace_target
ON graph_relationship_facts(workspace_id, target_kind, target_value);

CREATE TRIGGER IF NOT EXISTS graph_relationship_facts_from_node_workspace_insert
BEFORE INSERT ON graph_relationship_facts
FOR EACH ROW
WHEN NEW.from_node IS NOT NULL AND NOT EXISTS(
    SELECT 1 FROM graph_nodes
    WHERE id = NEW.from_node AND workspace_id = NEW.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'graph relationship fact source node must belong to workspace');
END;

CREATE TRIGGER IF NOT EXISTS graph_relationship_facts_from_node_workspace_update
BEFORE UPDATE OF workspace_id, from_node ON graph_relationship_facts
FOR EACH ROW
WHEN NEW.from_node IS NOT NULL AND NOT EXISTS(
    SELECT 1 FROM graph_nodes
    WHERE id = NEW.from_node AND workspace_id = NEW.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'graph relationship fact source node must belong to workspace');
END;

CREATE TABLE IF NOT EXISTS graph_edges (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    relationship_key TEXT NOT NULL,
    relationship_fact_id TEXT,
    from_node TEXT NOT NULL,
    to_node TEXT NOT NULL,
    edge_type TEXT NOT NULL,
    confidence REAL NOT NULL CHECK(confidence >= 0.0 AND confidence <= 1.0),
    analyzer_id TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    structure_version TEXT NOT NULL,
    source_document_id TEXT,
    source_path TEXT,
    source_start_byte INTEGER,
    source_end_byte INTEGER,
    content_revision INTEGER NOT NULL CHECK(content_revision >= 0),
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    UNIQUE(id, workspace_id),
    UNIQUE(workspace_id, relationship_key),
    FOREIGN KEY(relationship_fact_id, workspace_id)
        REFERENCES graph_relationship_facts(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(from_node, workspace_id)
        REFERENCES graph_nodes(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(to_node, workspace_id)
        REFERENCES graph_nodes(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(source_document_id, workspace_id)
        REFERENCES documents(id, workspace_id) ON DELETE CASCADE,
    CHECK(
        (source_path IS NULL AND source_start_byte IS NULL AND source_end_byte IS NULL)
        OR (
            source_path IS NOT NULL
            AND source_start_byte IS NOT NULL
            AND source_end_byte IS NOT NULL
            AND source_start_byte >= 0
            AND source_end_byte >= source_start_byte
        )
    )
);

CREATE INDEX IF NOT EXISTS idx_graph_edges_from_type
ON graph_edges(workspace_id, from_node, edge_type);

CREATE INDEX IF NOT EXISTS idx_graph_edges_to_type
ON graph_edges(workspace_id, to_node, edge_type);

CREATE INDEX IF NOT EXISTS idx_graph_edges_source_document
ON graph_edges(workspace_id, source_document_id);

CREATE TABLE IF NOT EXISTS unresolved_relationships (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    source_document_id TEXT NOT NULL,
    relationship_key TEXT NOT NULL,
    from_node TEXT REFERENCES graph_nodes(id) ON DELETE SET NULL,
    from_stable_key TEXT NOT NULL,
    edge_type TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_value TEXT NOT NULL,
    confidence REAL NOT NULL CHECK(confidence >= 0.0 AND confidence <= 1.0),
    analyzer_id TEXT NOT NULL,
    analyzer_version TEXT NOT NULL,
    structure_version TEXT NOT NULL,
    source_path TEXT,
    source_start_byte INTEGER,
    source_end_byte INTEGER,
    content_revision INTEGER NOT NULL CHECK(content_revision >= 0),
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(id, workspace_id),
    UNIQUE(workspace_id, source_document_id, relationship_key),
    FOREIGN KEY(source_document_id, workspace_id)
        REFERENCES documents(id, workspace_id) ON DELETE CASCADE,
    CHECK(
        (source_path IS NULL AND source_start_byte IS NULL AND source_end_byte IS NULL)
        OR (
            source_path IS NOT NULL
            AND source_start_byte IS NOT NULL
            AND source_end_byte IS NOT NULL
            AND source_start_byte >= 0
            AND source_end_byte >= source_start_byte
        )
    )
);

CREATE TRIGGER IF NOT EXISTS unresolved_relationships_from_node_workspace_insert
BEFORE INSERT ON unresolved_relationships
FOR EACH ROW
WHEN NEW.from_node IS NOT NULL AND NOT EXISTS(
    SELECT 1 FROM graph_nodes
    WHERE id = NEW.from_node AND workspace_id = NEW.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'unresolved relationship source node must belong to workspace');
END;

CREATE TRIGGER IF NOT EXISTS unresolved_relationships_from_node_workspace_update
BEFORE UPDATE OF workspace_id, from_node ON unresolved_relationships
FOR EACH ROW
WHEN NEW.from_node IS NOT NULL AND NOT EXISTS(
    SELECT 1 FROM graph_nodes
    WHERE id = NEW.from_node AND workspace_id = NEW.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'unresolved relationship source node must belong to workspace');
END;

CREATE TABLE IF NOT EXISTS unresolved_relationship_candidates (
    unresolved_relationship_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    candidate_node_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(unresolved_relationship_id, candidate_node_id),
    FOREIGN KEY(unresolved_relationship_id, workspace_id)
        REFERENCES unresolved_relationships(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(candidate_node_id, workspace_id)
        REFERENCES graph_nodes(id, workspace_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_unresolved_relationships_workspace_target
ON unresolved_relationships(workspace_id, target_kind, target_value);
