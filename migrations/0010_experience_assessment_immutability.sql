-- Assessment history is append-only while its workspace exists. The workspace
-- guard preserves the deliberate full-workspace cascade deletion contract.
DROP TRIGGER IF EXISTS episode_events_immutable_delete;

CREATE TRIGGER IF NOT EXISTS episode_events_immutable_delete
BEFORE DELETE ON episode_events
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'episode event membership is immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_assessments_immutable_delete
BEFORE DELETE ON experience_assessments
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experience assessments are append-only');
END;

CREATE TRIGGER IF NOT EXISTS experience_assessment_evidence_immutable_delete
BEFORE DELETE ON experience_assessment_evidence
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experience assessment evidence is immutable');
END;
