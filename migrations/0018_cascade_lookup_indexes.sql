-- Workspace deregistration and reindexing delete parents one row at a time, and
-- SQLite resolves each foreign-key action and trigger per deleted row. Any child
-- lookup without a usable index therefore scans the child table once per parent
-- row, which made deregistering a 168k-chunk workspace run for hours.

-- chunk_fts.chunk_id is an FTS5 UNINDEXED column, and FTS5 cannot be indexed
-- with CREATE INDEX. The projection triggers now address FTS rows by rowid
-- through this map. It has no foreign key to chunks on purpose: a cascade would
-- remove the map row before the AFTER DELETE trigger reads it. The triggers are
-- its only writers. fts_rowid is an INTEGER PRIMARY KEY, so VACUUM keeps it
-- stable, and FTS5 keeps its own rowids stable too.
CREATE TABLE chunk_fts_rows (
    fts_rowid INTEGER PRIMARY KEY,
    chunk_id TEXT NOT NULL UNIQUE
);

-- The previous triggers kept exactly one projection per chunk. Enforce that
-- before mapping: drop projections of missing chunks and older duplicates, then
-- backfill any chunk that lacks a projection.
DELETE FROM chunk_fts
WHERE rowid NOT IN (
    SELECT MAX(chunk_fts.rowid)
    FROM chunk_fts
    JOIN chunks ON chunks.id = chunk_fts.chunk_id
    GROUP BY chunk_fts.chunk_id
);

INSERT INTO chunk_fts_rows(fts_rowid, chunk_id)
SELECT rowid, chunk_id FROM chunk_fts;

INSERT INTO chunk_fts_rows(chunk_id)
SELECT chunks.id
FROM chunks
WHERE NOT EXISTS (SELECT 1 FROM chunk_fts_rows WHERE chunk_fts_rows.chunk_id = chunks.id);

INSERT INTO chunk_fts(rowid, chunk_id, content, symbol, qualified_symbol, relative_path)
SELECT chunk_fts_rows.fts_rowid, chunks.id, chunks.content, COALESCE(chunks.symbol, ''), COALESCE(chunks.qualified_symbol, ''), documents.relative_path
FROM chunks
JOIN documents ON documents.id = chunks.document_id
JOIN chunk_fts_rows ON chunk_fts_rows.chunk_id = chunks.id
WHERE NOT EXISTS (SELECT 1 FROM chunk_fts WHERE chunk_fts.rowid = chunk_fts_rows.fts_rowid);

DROP TRIGGER chunks_fts_insert;
DROP TRIGGER chunks_fts_update;
DROP TRIGGER chunks_fts_delete;

CREATE TRIGGER chunks_fts_insert AFTER INSERT ON chunks BEGIN
    INSERT INTO chunk_fts_rows(chunk_id) VALUES (NEW.id);
    INSERT INTO chunk_fts(rowid, chunk_id, content, symbol, qualified_symbol, relative_path)
    SELECT chunk_fts_rows.fts_rowid, NEW.id, NEW.content, COALESCE(NEW.symbol, ''), COALESCE(NEW.qualified_symbol, ''), documents.relative_path
    FROM documents
    JOIN chunk_fts_rows ON chunk_fts_rows.chunk_id = NEW.id
    WHERE documents.id = NEW.document_id;
END;

CREATE TRIGGER chunks_fts_update AFTER UPDATE ON chunks BEGIN
    DELETE FROM chunk_fts
    WHERE rowid = (SELECT fts_rowid FROM chunk_fts_rows WHERE chunk_id = OLD.id);
    DELETE FROM chunk_fts_rows WHERE chunk_id = OLD.id;
    INSERT INTO chunk_fts_rows(chunk_id) VALUES (NEW.id);
    INSERT INTO chunk_fts(rowid, chunk_id, content, symbol, qualified_symbol, relative_path)
    SELECT chunk_fts_rows.fts_rowid, NEW.id, NEW.content, COALESCE(NEW.symbol, ''), COALESCE(NEW.qualified_symbol, ''), documents.relative_path
    FROM documents
    JOIN chunk_fts_rows ON chunk_fts_rows.chunk_id = NEW.id
    WHERE documents.id = NEW.document_id;
END;

CREATE TRIGGER chunks_fts_delete AFTER DELETE ON chunks BEGIN
    DELETE FROM chunk_fts
    WHERE rowid = (SELECT fts_rowid FROM chunk_fts_rows WHERE chunk_id = OLD.id);
    DELETE FROM chunk_fts_rows WHERE chunk_id = OLD.id;
END;

-- Child keys of foreign keys whose lookup was a table scan, or narrowed only by
-- workspace_id. Each index leads with the non-workspace key column. Foreign keys
-- to workspaces(id) alone get no new index: one workspace is deleted at a time,
-- so their lookup is a single linear pass, and a workspace-only index would
-- outrank the existing session- and memory-leading indexes for composite keys.

-- Code and graph projections.
CREATE INDEX idx_graph_nodes_chunk ON graph_nodes(chunk_id);
CREATE INDEX idx_graph_relationship_facts_from_node ON graph_relationship_facts(from_node);
CREATE INDEX idx_unresolved_relationships_from_node ON unresolved_relationships(from_node);
CREATE INDEX idx_graph_edges_relationship_fact ON graph_edges(relationship_fact_id, workspace_id);
CREATE INDEX idx_unresolved_relationship_candidates_node
ON unresolved_relationship_candidates(candidate_node_id, workspace_id);

-- Sessions, tasks and context state.
CREATE INDEX idx_tasks_session ON tasks(session_id, workspace_id);
CREATE INDEX idx_events_session ON events(session_id, workspace_id);
CREATE INDEX idx_events_task ON events(task_id, workspace_id);
CREATE INDEX idx_memories_session ON memories(session_id, workspace_id);
CREATE INDEX idx_memories_task ON memories(task_id, workspace_id);
CREATE INDEX idx_working_set_task ON working_set_entries(task_id, workspace_id);
CREATE INDEX idx_context_pins_task ON context_pins(task_id, workspace_id);

-- Episodes and Experiences.
CREATE INDEX idx_experiences_episode ON experiences(episode_id, workspace_id);
CREATE INDEX idx_experiences_session ON experiences(session_id, workspace_id);
CREATE INDEX idx_experiences_task ON experiences(task_id, workspace_id, session_id);
CREATE INDEX idx_experience_verifications_event
ON experience_verifications(evidence_event_id, workspace_id);
CREATE INDEX idx_experience_evidence_event ON experience_evidence(event_id, workspace_id);
CREATE INDEX idx_experience_code_snapshots_event
ON experience_code_snapshots(source_event_id, workspace_id);
CREATE INDEX idx_experience_assessments_replacement
ON experience_assessments(replacement_experience_id, workspace_id);
CREATE INDEX idx_experience_assessment_evidence_event
ON experience_assessment_evidence(event_id, workspace_id);
