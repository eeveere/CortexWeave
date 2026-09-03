-- Public reviewed-assessment writes are idempotent. Historical direct rows
-- from earlier releases remain readable with a NULL request identity.
ALTER TABLE experience_assessments ADD COLUMN request_key TEXT;
ALTER TABLE experience_assessments ADD COLUMN request_hash TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_experience_assessments_request_key
ON experience_assessments(workspace_id, experience_id, request_key)
WHERE request_key IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_experience_assessments_lifecycle
ON experience_assessments(workspace_id, experience_id, kind);

CREATE INDEX IF NOT EXISTS idx_experience_assessments_history_page
ON experience_assessments(workspace_id, experience_id, created_at DESC, id DESC);
