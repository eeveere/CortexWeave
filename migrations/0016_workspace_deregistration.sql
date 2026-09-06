-- Confirmation plans and receipts deliberately have no workspace foreign key:
-- they must survive the workspace cascade long enough to make a lost
-- acknowledgement retryable. They contain identity and counts, never content.
CREATE TABLE workspace_deregistration_plans (
    plan_id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    snapshot_hash TEXT NOT NULL CHECK(length(snapshot_hash) = 64 AND snapshot_hash NOT GLOB '*[^0-9a-f]*'),
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_at TEXT,
    CHECK(length(trim(plan_id)) BETWEEN 1 AND 256),
    CHECK(length(trim(workspace_id)) BETWEEN 1 AND 256)
);

CREATE INDEX idx_workspace_deregistration_plans_workspace
ON workspace_deregistration_plans(workspace_id, created_at DESC);

CREATE TABLE workspace_deregistration_receipts (
    workspace_id TEXT NOT NULL,
    plan_id TEXT NOT NULL,
    request_key TEXT NOT NULL,
    request_hash TEXT NOT NULL CHECK(length(request_hash) = 64 AND request_hash NOT GLOB '*[^0-9a-f]*'),
    counts_json TEXT NOT NULL,
    completed_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id, request_key),
    UNIQUE(plan_id),
    CHECK(length(trim(workspace_id)) BETWEEN 1 AND 256),
    CHECK(length(trim(plan_id)) BETWEEN 1 AND 256),
    CHECK(length(trim(request_key)) BETWEEN 1 AND 256)
);

CREATE TRIGGER workspace_deregistration_plans_consume_once
BEFORE UPDATE ON workspace_deregistration_plans
WHEN OLD.plan_id IS NOT NEW.plan_id
  OR OLD.workspace_id IS NOT NEW.workspace_id
  OR OLD.snapshot_json IS NOT NEW.snapshot_json
  OR OLD.snapshot_hash IS NOT NEW.snapshot_hash
  OR OLD.created_at IS NOT NEW.created_at
  OR OLD.expires_at IS NOT NEW.expires_at
  OR OLD.consumed_at IS NOT NULL
  OR NEW.consumed_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'workspace deregistration plan may only be consumed once');
END;

CREATE TRIGGER workspace_deregistration_plans_immutable_delete
BEFORE DELETE ON workspace_deregistration_plans BEGIN
    SELECT RAISE(ABORT, 'workspace deregistration plans are immutable');
END;

CREATE TRIGGER workspace_deregistration_receipts_immutable_update
BEFORE UPDATE ON workspace_deregistration_receipts BEGIN
    SELECT RAISE(ABORT, 'workspace deregistration receipts are immutable');
END;

CREATE TRIGGER workspace_deregistration_receipts_immutable_delete
BEFORE DELETE ON workspace_deregistration_receipts BEGIN
    SELECT RAISE(ABORT, 'workspace deregistration receipts are immutable');
END;
