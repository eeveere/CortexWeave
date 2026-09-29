-- memory_fts.memory_id and experience_fts.experience_id are FTS5 UNINDEXED
-- columns, so the projection triggers that located rows through them scanned the
-- whole FTS table once per deleted or updated row. This applies the D112
-- chunk_fts_rows pattern to both projections: each map is written only by its
-- triggers, has no foreign key (a cascade would remove the map row before the
-- AFTER DELETE trigger reads it), and keys FTS rows by a VACUUM-stable rowid.
CREATE TABLE memory_fts_rows (
    fts_rowid INTEGER PRIMARY KEY,
    memory_id TEXT NOT NULL UNIQUE
);

CREATE TABLE experience_fts_rows (
    fts_rowid INTEGER PRIMARY KEY,
    experience_id TEXT NOT NULL UNIQUE
);

-- The previous triggers kept exactly one projection per row. Enforce that
-- before mapping: drop projections of missing rows and older duplicates, then
-- backfill any row that lacks a projection.
DELETE FROM memory_fts
WHERE rowid NOT IN (
    SELECT MAX(memory_fts.rowid)
    FROM memory_fts
    JOIN memories ON memories.id = memory_fts.memory_id
    GROUP BY memory_fts.memory_id
);

INSERT INTO memory_fts_rows(fts_rowid, memory_id)
SELECT rowid, memory_id FROM memory_fts;

INSERT INTO memory_fts_rows(memory_id)
SELECT memories.id
FROM memories
WHERE NOT EXISTS (SELECT 1 FROM memory_fts_rows WHERE memory_fts_rows.memory_id = memories.id);

INSERT INTO memory_fts(rowid, memory_id, content, kind)
SELECT memory_fts_rows.fts_rowid, memories.id, memories.content, memories.kind
FROM memories
JOIN memory_fts_rows ON memory_fts_rows.memory_id = memories.id
WHERE NOT EXISTS (SELECT 1 FROM memory_fts WHERE memory_fts.rowid = memory_fts_rows.fts_rowid);

DELETE FROM experience_fts
WHERE rowid NOT IN (
    SELECT MAX(experience_fts.rowid)
    FROM experience_fts
    JOIN experiences ON experiences.id = experience_fts.experience_id
    GROUP BY experience_fts.experience_id
);

INSERT INTO experience_fts_rows(fts_rowid, experience_id)
SELECT rowid, experience_id FROM experience_fts;

INSERT INTO experience_fts_rows(experience_id)
SELECT experiences.id
FROM experiences
WHERE NOT EXISTS (
    SELECT 1 FROM experience_fts_rows WHERE experience_fts_rows.experience_id = experiences.id
);

INSERT INTO experience_fts(rowid, experience_id, summary, failure_key, failure_components, failure_path, failure_symbol_key, outcome, verification_status)
SELECT experience_fts_rows.fts_rowid, experiences.id, experiences.summary, COALESCE(experiences.failure_key, ''), experiences.failure_components, COALESCE(experiences.failure_path, ''), COALESCE(experiences.failure_symbol_key, ''), experiences.outcome, experiences.verification_status
FROM experiences
JOIN experience_fts_rows ON experience_fts_rows.experience_id = experiences.id
WHERE NOT EXISTS (
    SELECT 1 FROM experience_fts WHERE experience_fts.rowid = experience_fts_rows.fts_rowid
);

DROP TRIGGER memories_fts_insert;
DROP TRIGGER memories_fts_update;
DROP TRIGGER memories_fts_delete;

CREATE TRIGGER memories_fts_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memory_fts_rows(memory_id) VALUES (NEW.id);
    INSERT INTO memory_fts(rowid, memory_id, content, kind)
    SELECT fts_rowid, NEW.id, NEW.content, NEW.kind
    FROM memory_fts_rows WHERE memory_id = NEW.id;
END;

CREATE TRIGGER memories_fts_update AFTER UPDATE ON memories BEGIN
    DELETE FROM memory_fts
    WHERE rowid = (SELECT fts_rowid FROM memory_fts_rows WHERE memory_id = OLD.id);
    DELETE FROM memory_fts_rows WHERE memory_id = OLD.id;
    INSERT INTO memory_fts_rows(memory_id) VALUES (NEW.id);
    INSERT INTO memory_fts(rowid, memory_id, content, kind)
    SELECT fts_rowid, NEW.id, NEW.content, NEW.kind
    FROM memory_fts_rows WHERE memory_id = NEW.id;
END;

CREATE TRIGGER memories_fts_delete AFTER DELETE ON memories BEGIN
    DELETE FROM memory_fts
    WHERE rowid = (SELECT fts_rowid FROM memory_fts_rows WHERE memory_id = OLD.id);
    DELETE FROM memory_fts_rows WHERE memory_id = OLD.id;
END;

-- Experiences reject UPDATE (experiences_immutable), so they have no update
-- projection trigger. Deletes happen only through the workspace cascade.
DROP TRIGGER experiences_fts_insert;
DROP TRIGGER experiences_fts_delete;

CREATE TRIGGER experiences_fts_insert AFTER INSERT ON experiences BEGIN
    INSERT INTO experience_fts_rows(experience_id) VALUES (NEW.id);
    INSERT INTO experience_fts(rowid, experience_id, summary, failure_key, failure_components, failure_path, failure_symbol_key, outcome, verification_status)
    SELECT fts_rowid, NEW.id, NEW.summary, COALESCE(NEW.failure_key, ''), NEW.failure_components, COALESCE(NEW.failure_path, ''), COALESCE(NEW.failure_symbol_key, ''), NEW.outcome, NEW.verification_status
    FROM experience_fts_rows WHERE experience_id = NEW.id;
END;

CREATE TRIGGER experiences_fts_delete AFTER DELETE ON experiences BEGIN
    DELETE FROM experience_fts
    WHERE rowid = (SELECT fts_rowid FROM experience_fts_rows WHERE experience_id = OLD.id);
    DELETE FROM experience_fts_rows WHERE experience_id = OLD.id;
END;
