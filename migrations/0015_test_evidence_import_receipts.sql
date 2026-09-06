CREATE TABLE test_evidence_import_receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    producer_id TEXT NOT NULL CHECK(length(trim(producer_id)) BETWEEN 1 AND 256),
    producer_version TEXT NOT NULL CHECK(length(trim(producer_version)) BETWEEN 1 AND 256),
    run_id TEXT NOT NULL CHECK(length(trim(run_id)) BETWEEN 1 AND 256),
    request_key TEXT NOT NULL CHECK(length(trim(request_key)) BETWEEN 1 AND 256),
    request_json TEXT NOT NULL,
    event_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id, producer_id, producer_version, run_id),
    UNIQUE(workspace_id, request_key),
    UNIQUE(workspace_id, event_id),
    FOREIGN KEY(event_id, workspace_id) REFERENCES events(id, workspace_id) ON DELETE RESTRICT
);

CREATE TRIGGER test_evidence_import_receipts_immutable_update
BEFORE UPDATE ON test_evidence_import_receipts BEGIN
    SELECT RAISE(ABORT, 'test evidence import receipts are immutable');
END;

CREATE TRIGGER test_evidence_import_receipts_immutable_delete
BEFORE DELETE ON test_evidence_import_receipts
WHEN EXISTS (SELECT 1 FROM workspaces WHERE id = OLD.workspace_id)
BEGIN
    SELECT RAISE(ABORT, 'test evidence import receipts are immutable');
END;
