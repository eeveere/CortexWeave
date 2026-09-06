CREATE TABLE native_delivery_receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    operation TEXT NOT NULL CHECK(operation IN ('start_session', 'start_task', 'start_episode', 'record_event')),
    request_key TEXT NOT NULL CHECK(length(trim(request_key)) BETWEEN 1 AND 256),
    request_json TEXT NOT NULL,
    receipt_json TEXT NOT NULL,
    PRIMARY KEY(workspace_id, operation, request_key)
);

CREATE TRIGGER native_delivery_receipts_immutable_update
BEFORE UPDATE ON native_delivery_receipts BEGIN
    SELECT RAISE(ABORT, 'native delivery receipts are immutable');
END;

CREATE TRIGGER native_delivery_receipts_immutable_delete
BEFORE DELETE ON native_delivery_receipts
WHEN EXISTS (SELECT 1 FROM workspaces WHERE id = OLD.workspace_id)
BEGIN
    SELECT RAISE(ABORT, 'native delivery receipts are immutable');
END;
