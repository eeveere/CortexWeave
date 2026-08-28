ALTER TABLE memories
ADD COLUMN origin TEXT NOT NULL DEFAULT 'human_authorized'
CHECK(origin IN ('human_authorized', 'imported'));

ALTER TABLE memories
ADD COLUMN trust TEXT NOT NULL DEFAULT 'trusted'
CHECK(trust IN ('trusted', 'unreviewed', 'rejected'));

ALTER TABLE memories
ADD COLUMN source_segments_json TEXT NOT NULL DEFAULT '[]';

ALTER TABLE memories
ADD COLUMN claim_key TEXT;

ALTER TABLE memories
ADD COLUMN claim_value_json TEXT;

CREATE TABLE IF NOT EXISTS memory_trust_reviews (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    memory_id TEXT NOT NULL,
    previous_trust TEXT NOT NULL CHECK(previous_trust IN ('trusted', 'unreviewed', 'rejected')),
    new_trust TEXT NOT NULL CHECK(new_trust IN ('trusted', 'unreviewed', 'rejected')),
    reviewed_by TEXT NOT NULL,
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(memory_id, workspace_id)
        REFERENCES memories(id, workspace_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_memory_trust_reviews_memory_created
ON memory_trust_reviews(memory_id, created_at DESC);

ALTER TABLE memory_supersession
ADD COLUMN reviewed_by TEXT;

ALTER TABLE memory_supersession
ADD COLUMN reason TEXT;
