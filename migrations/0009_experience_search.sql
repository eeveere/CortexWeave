CREATE INDEX IF NOT EXISTS idx_experiences_workspace_failure_key
ON experiences(workspace_id, failure_key, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_experiences_workspace_filters
ON experiences(workspace_id, outcome, evidence_strength, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_experience_code_snapshots_workspace_path
ON experience_code_snapshots(workspace_id, relative_path, experience_id);

CREATE INDEX IF NOT EXISTS idx_experience_graph_snapshots_workspace_stable_key
ON experience_graph_snapshots(workspace_id, node_stable_key, experience_id);
