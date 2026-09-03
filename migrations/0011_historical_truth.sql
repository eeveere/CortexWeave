PRAGMA foreign_keys = ON;

-- This sequence is the durable SQLite-owned order for historical writes. The
-- producer timestamp on an Event remains occurrence metadata and never proves
-- persistence order.
CREATE TABLE IF NOT EXISTS historical_write_order (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    entity_kind TEXT NOT NULL CHECK(entity_kind IN ('event', 'experience_acceptance')),
    entity_id TEXT NOT NULL CHECK(length(trim(entity_id)) > 0 AND length(entity_id) <= 256),
    UNIQUE(workspace_id, entity_kind, entity_id)
);

CREATE INDEX IF NOT EXISTS idx_historical_write_order_workspace_sequence
ON historical_write_order(workspace_id, sequence);

CREATE TRIGGER IF NOT EXISTS historical_write_order_requires_entity
BEFORE INSERT ON historical_write_order BEGIN
    SELECT CASE WHEN NEW.entity_kind = 'event' AND NOT EXISTS (
        SELECT 1 FROM events event
        WHERE event.workspace_id = NEW.workspace_id
          AND event.id = NEW.entity_id
    ) THEN RAISE(ABORT, 'event ingress order requires its event') END;

    SELECT CASE WHEN NEW.entity_kind = 'experience_acceptance' AND NOT EXISTS (
        SELECT 1 FROM experiences experience
        WHERE experience.workspace_id = NEW.workspace_id
          AND experience.id = NEW.entity_id
    ) THEN RAISE(ABORT, 'experience acceptance order requires its experience') END;
END;

CREATE TABLE IF NOT EXISTS experience_seals (
    workspace_id TEXT NOT NULL,
    experience_id TEXT NOT NULL,
    acceptance_order INTEGER,
    PRIMARY KEY(workspace_id, experience_id),
    UNIQUE(acceptance_order),
    FOREIGN KEY(experience_id, workspace_id)
        REFERENCES experiences(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(acceptance_order)
        REFERENCES historical_write_order(sequence) ON DELETE CASCADE
);

CREATE TRIGGER IF NOT EXISTS experience_seals_require_matching_order
BEFORE INSERT ON experience_seals
WHEN NEW.acceptance_order IS NOT NULL
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM historical_write_order ordering
        WHERE ordering.sequence = NEW.acceptance_order
          AND ordering.workspace_id = NEW.workspace_id
          AND ordering.entity_kind = 'experience_acceptance'
          AND ordering.entity_id = NEW.experience_id
    ) THEN RAISE(ABORT, 'experience seal requires its acceptance order') END;
END;

-- Existing Experiences are sealed, but their relative order to legacy Events
-- is unknowable. A NULL frontier deliberately makes recurrence conservative.
INSERT OR IGNORE INTO experience_seals(workspace_id, experience_id, acceptance_order)
SELECT workspace_id, id, NULL FROM experiences;

-- Existing historical rows may disappear only as part of deleting their
-- owning workspace. The workspace guard permits that deliberate cascade.
CREATE TRIGGER IF NOT EXISTS events_immutable_delete
BEFORE DELETE ON events
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'events are append-only');
END;

CREATE TRIGGER IF NOT EXISTS experiences_immutable_delete
BEFORE DELETE ON experiences
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experiences are immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_attempts_immutable_delete
BEFORE DELETE ON experience_attempts
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experience attempts are immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_verifications_immutable_delete
BEFORE DELETE ON experience_verifications
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experience verifications are immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_evidence_immutable_delete
BEFORE DELETE ON experience_evidence
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experience evidence is immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_code_snapshots_immutable_delete
BEFORE DELETE ON experience_code_snapshots
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experience code snapshots are immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_graph_snapshots_immutable_delete
BEFORE DELETE ON experience_graph_snapshots
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experience graph snapshots are immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_strength_bases_immutable_delete
BEFORE DELETE ON experience_strength_bases
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experience strength bases are immutable');
END;

CREATE TRIGGER IF NOT EXISTS historical_write_order_immutable_update
BEFORE UPDATE ON historical_write_order BEGIN
    SELECT RAISE(ABORT, 'historical write order is immutable');
END;

CREATE TRIGGER IF NOT EXISTS historical_write_order_immutable_delete
BEFORE DELETE ON historical_write_order
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'historical write order is immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_seals_immutable_update
BEFORE UPDATE ON experience_seals BEGIN
    SELECT RAISE(ABORT, 'experience seals are immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_seals_immutable_delete
BEFORE DELETE ON experience_seals
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experience seals are immutable');
END;

-- The accepted aggregate is assembled before its seal is inserted in the same
-- transaction. After that point no core component may be appended.
CREATE TRIGGER IF NOT EXISTS experience_attempts_reject_after_seal
BEFORE INSERT ON experience_attempts
WHEN EXISTS (
    SELECT 1 FROM experience_seals seal
    WHERE seal.workspace_id = NEW.workspace_id
      AND seal.experience_id = NEW.experience_id
)
BEGIN
    SELECT RAISE(ABORT, 'sealed experience rejects new attempts');
END;

CREATE TRIGGER IF NOT EXISTS experience_verifications_reject_after_seal
BEFORE INSERT ON experience_verifications
WHEN EXISTS (
    SELECT 1 FROM experience_seals seal
    WHERE seal.workspace_id = NEW.workspace_id
      AND seal.experience_id = NEW.experience_id
)
BEGIN
    SELECT RAISE(ABORT, 'sealed experience rejects new verifications');
END;

CREATE TRIGGER IF NOT EXISTS experience_evidence_reject_after_seal
BEFORE INSERT ON experience_evidence
WHEN EXISTS (
    SELECT 1 FROM experience_seals seal
    WHERE seal.workspace_id = NEW.workspace_id
      AND seal.experience_id = NEW.experience_id
)
BEGIN
    SELECT RAISE(ABORT, 'sealed experience rejects new evidence');
END;

CREATE TRIGGER IF NOT EXISTS experience_code_snapshots_reject_after_seal
BEFORE INSERT ON experience_code_snapshots
WHEN EXISTS (
    SELECT 1 FROM experience_seals seal
    WHERE seal.workspace_id = NEW.workspace_id
      AND seal.experience_id = NEW.experience_id
)
BEGIN
    SELECT RAISE(ABORT, 'sealed experience rejects new code snapshots');
END;

CREATE TRIGGER IF NOT EXISTS experience_graph_snapshots_reject_after_seal
BEFORE INSERT ON experience_graph_snapshots
WHEN EXISTS (
    SELECT 1 FROM experience_seals seal
    WHERE seal.workspace_id = NEW.workspace_id
      AND seal.experience_id = NEW.experience_id
)
BEGIN
    SELECT RAISE(ABORT, 'sealed experience rejects new graph snapshots');
END;

CREATE TRIGGER IF NOT EXISTS experience_strength_bases_reject_after_seal
BEFORE INSERT ON experience_strength_bases
WHEN EXISTS (
    SELECT 1 FROM experience_seals seal
    WHERE seal.workspace_id = NEW.workspace_id
      AND seal.experience_id = NEW.experience_id
)
BEGIN
    SELECT RAISE(ABORT, 'sealed experience rejects new strength bases');
END;
