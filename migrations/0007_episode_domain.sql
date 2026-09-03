PRAGMA foreign_keys = ON;

CREATE UNIQUE INDEX IF NOT EXISTS idx_events_id_workspace
ON events(id, workspace_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_tasks_id_workspace_session
ON tasks(id, workspace_id, session_id);

CREATE TABLE IF NOT EXISTS episodes (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    task_id TEXT,
    episode_type TEXT NOT NULL CHECK(episode_type IN (
        'implementation', 'debugging', 'verification', 'investigation', 'refactor',
        'configuration', 'dependency_change', 'architecture_decision', 'documentation', 'other'
    )),
    status TEXT NOT NULL CHECK(status IN ('open', 'closed', 'abandoned', 'invalid')),
    title TEXT CHECK(title IS NULL OR (length(trim(title)) > 0 AND length(title) <= 512)),
    created_by TEXT NOT NULL CHECK(created_by IN ('user', 'native_harness')),
    version INTEGER NOT NULL DEFAULT 0 CHECK(version >= 0),
    started_at TEXT NOT NULL,
    ended_at TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(id, workspace_id),
    FOREIGN KEY(session_id, workspace_id)
        REFERENCES sessions(id, workspace_id) ON DELETE RESTRICT,
    FOREIGN KEY(task_id, workspace_id, session_id)
        REFERENCES tasks(id, workspace_id, session_id) ON DELETE RESTRICT,
    CHECK((status = 'open' AND ended_at IS NULL) OR (status != 'open' AND ended_at IS NOT NULL))
);

CREATE INDEX IF NOT EXISTS idx_episodes_workspace_session_created
ON episodes(workspace_id, session_id, created_at DESC, id DESC);

CREATE INDEX IF NOT EXISTS idx_episodes_workspace_task_created
ON episodes(workspace_id, task_id, created_at DESC, id DESC);

CREATE TABLE IF NOT EXISTS episode_events (
    workspace_id TEXT NOT NULL,
    episode_id TEXT NOT NULL,
    event_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    associated_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id, event_id),
    UNIQUE(workspace_id, episode_id, ordinal),
    FOREIGN KEY(episode_id, workspace_id)
        REFERENCES episodes(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(event_id, workspace_id)
        REFERENCES events(id, workspace_id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_episode_events_episode_ordinal
ON episode_events(workspace_id, episode_id, ordinal);

CREATE TABLE IF NOT EXISTS episode_mutation_requests (
    workspace_id TEXT NOT NULL,
    episode_id TEXT NOT NULL,
    operation TEXT NOT NULL CHECK(operation IN ('add_events', 'close', 'abandon')),
    request_key TEXT NOT NULL CHECK(length(trim(request_key)) > 0 AND length(request_key) <= 256),
    request_hash TEXT NOT NULL CHECK(length(request_hash) = 64),
    resulting_version INTEGER NOT NULL CHECK(resulting_version >= 0),
    created_at TEXT NOT NULL,
    PRIMARY KEY(workspace_id, episode_id, request_key),
    FOREIGN KEY(episode_id, workspace_id)
        REFERENCES episodes(id, workspace_id) ON DELETE CASCADE
);

CREATE TRIGGER IF NOT EXISTS episode_events_require_open_matching_scope
BEFORE INSERT ON episode_events
BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM episodes
        WHERE id = NEW.episode_id
          AND workspace_id = NEW.workspace_id
          AND status = 'open'
    ) THEN RAISE(ABORT, 'episode membership requires an open episode') END;

    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM episodes episode
        JOIN events event
          ON event.id = NEW.event_id
         AND event.workspace_id = NEW.workspace_id
        WHERE episode.id = NEW.episode_id
          AND episode.workspace_id = NEW.workspace_id
          AND event.session_id = episode.session_id
          AND event.task_id IS episode.task_id
    ) THEN RAISE(ABORT, 'episode event provenance mismatch') END;
END;

CREATE TRIGGER IF NOT EXISTS episodes_reject_scope_mutation
BEFORE UPDATE OF workspace_id, session_id, task_id, episode_type, title, created_by, started_at, created_at
ON episodes
BEGIN
    SELECT RAISE(ABORT, 'episode scope is immutable');
END;

CREATE TRIGGER IF NOT EXISTS episodes_reject_terminal_reopen
BEFORE UPDATE OF status ON episodes
WHEN OLD.status <> 'open' OR NEW.status = 'open'
BEGIN
    SELECT RAISE(ABORT, 'episode lifecycle is terminal');
END;

CREATE TRIGGER IF NOT EXISTS episodes_require_terminal_time
BEFORE UPDATE OF status, ended_at ON episodes
WHEN (NEW.status = 'open' AND NEW.ended_at IS NOT NULL)
  OR (NEW.status <> 'open' AND NEW.ended_at IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'episode terminal time does not match status');
END;
