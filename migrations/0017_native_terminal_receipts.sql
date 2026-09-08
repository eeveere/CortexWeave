CREATE TABLE native_terminal_receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    operation TEXT NOT NULL CHECK(operation IN ('complete_task', 'end_session')),
    request_key TEXT NOT NULL CHECK(length(trim(request_key)) BETWEEN 1 AND 256),
    request_json TEXT NOT NULL CHECK(length(CAST(request_json AS BLOB)) <= 65536),
    receipt_json TEXT NOT NULL CHECK(length(CAST(receipt_json AS BLOB)) <= 65536),
    PRIMARY KEY(workspace_id, operation, request_key)
);
CREATE TRIGGER native_terminal_receipts_immutable_update
BEFORE UPDATE ON native_terminal_receipts BEGIN
    SELECT RAISE(ABORT, 'native terminal receipts are immutable');
END;
CREATE TRIGGER native_terminal_receipts_immutable_delete
BEFORE DELETE ON native_terminal_receipts
WHEN EXISTS (SELECT 1 FROM workspaces WHERE id = OLD.workspace_id)
BEGIN
    SELECT RAISE(ABORT, 'native terminal receipts are immutable');
END;
