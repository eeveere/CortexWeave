PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS experiences (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    session_id TEXT NOT NULL,
    task_id TEXT,
    episode_id TEXT NOT NULL,
    failure_signature_json TEXT,
    failure_key TEXT,
    failure_components TEXT NOT NULL DEFAULT '',
    failure_path TEXT,
    failure_symbol_key TEXT,
    outcome TEXT NOT NULL CHECK(outcome IN ('success', 'failure', 'partial_success', 'inconclusive', 'abandoned')),
    verification_status TEXT NOT NULL CHECK(verification_status IN ('verified_passed', 'verified_failed', 'explicitly_accepted', 'conflicting', 'missing', 'unsupported')),
    verification_reasons_json TEXT NOT NULL,
    evidence_strength TEXT NOT NULL CHECK(evidence_strength IN ('strong', 'moderate', 'weak', 'unsupported')),
    summary TEXT NOT NULL CHECK(length(trim(summary)) > 0 AND length(summary) <= 4096),
    extractor_id TEXT NOT NULL CHECK(length(trim(extractor_id)) > 0 AND length(extractor_id) <= 256),
    extractor_version TEXT NOT NULL CHECK(length(trim(extractor_version)) > 0 AND length(extractor_version) <= 256),
    summary_renderer_version TEXT NOT NULL CHECK(length(trim(summary_renderer_version)) > 0 AND length(summary_renderer_version) <= 256),
    canonicalization_version TEXT NOT NULL CHECK(length(trim(canonicalization_version)) > 0 AND length(canonicalization_version) <= 256),
    consolidation_fingerprint TEXT NOT NULL CHECK(length(consolidation_fingerprint) = 64 AND consolidation_fingerprint NOT GLOB '*[^0-9a-f]*'),
    proposal_hash TEXT NOT NULL CHECK(length(proposal_hash) = 64 AND proposal_hash NOT GLOB '*[^0-9a-f]*'),
    created_at TEXT NOT NULL,
    UNIQUE(id, workspace_id),
    UNIQUE(workspace_id, consolidation_fingerprint),
    FOREIGN KEY(episode_id, workspace_id) REFERENCES episodes(id, workspace_id) ON DELETE RESTRICT,
    FOREIGN KEY(session_id, workspace_id) REFERENCES sessions(id, workspace_id) ON DELETE RESTRICT,
    FOREIGN KEY(task_id, workspace_id, session_id) REFERENCES tasks(id, workspace_id, session_id) ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS idx_experiences_workspace_created
ON experiences(workspace_id, created_at DESC, id DESC);

CREATE TABLE IF NOT EXISTS experience_verifications (
    workspace_id TEXT NOT NULL,
    experience_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    status TEXT NOT NULL CHECK(status IN ('verified_passed', 'verified_failed', 'explicitly_accepted')),
    kind TEXT NOT NULL CHECK(kind IN ('rust_compiler', 'cargo_test', 'registered_tool', 'user_acceptance')),
    subject_kind TEXT NOT NULL CHECK(subject_kind IN ('workspace', 'package', 'target', 'test', 'path')),
    subject_value TEXT NOT NULL CHECK(length(trim(subject_value)) > 0 AND length(subject_value) <= 512),
    evidence_event_id TEXT NOT NULL,
    rule_id TEXT NOT NULL CHECK(length(trim(rule_id)) > 0 AND length(rule_id) <= 256),
    rule_version TEXT NOT NULL CHECK(length(trim(rule_version)) > 0 AND length(rule_version) <= 256),
    PRIMARY KEY(workspace_id, experience_id, ordinal),
    FOREIGN KEY(experience_id, workspace_id) REFERENCES experiences(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(evidence_event_id, workspace_id) REFERENCES events(id, workspace_id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS experience_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    experience_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    result TEXT NOT NULL CHECK(result IN ('still_failing', 'verification_passed', 'verification_changed_failure', 'inconclusive')),
    change_evidence_ordinals_json TEXT NOT NULL,
    following_verification_ordinal INTEGER,
    UNIQUE(id, workspace_id),
    UNIQUE(workspace_id, experience_id, ordinal),
    FOREIGN KEY(experience_id, workspace_id) REFERENCES experiences(id, workspace_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS experience_evidence (
    workspace_id TEXT NOT NULL,
    experience_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    relation TEXT NOT NULL CHECK(relation IN ('initial_failure', 'attempt_change', 'attempt_verification', 'terminal_verification', 'supporting')),
    event_id TEXT NOT NULL,
    PRIMARY KEY(workspace_id, experience_id, ordinal),
    UNIQUE(workspace_id, experience_id, event_id),
    FOREIGN KEY(experience_id, workspace_id) REFERENCES experiences(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(event_id, workspace_id) REFERENCES events(id, workspace_id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS experience_code_snapshots (
    workspace_id TEXT NOT NULL,
    experience_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    source_event_id TEXT NOT NULL,
    relative_path TEXT NOT NULL CHECK(length(trim(relative_path)) > 0 AND length(relative_path) <= 512),
    workspace_content_revision INTEGER NOT NULL CHECK(workspace_content_revision >= 0),
    document_content_revision INTEGER NOT NULL CHECK(document_content_revision >= 0),
    document_content_hash TEXT NOT NULL CHECK(length(document_content_hash) = 64 AND document_content_hash NOT GLOB '*[^0-9a-f]*'),
    content TEXT NOT NULL CHECK(length(content) <= 65536),
    chunk_stable_key TEXT CHECK(chunk_stable_key IS NULL OR (length(trim(chunk_stable_key)) > 0 AND length(chunk_stable_key) <= 512)),
    chunk_content_hash TEXT CHECK(chunk_content_hash IS NULL OR (length(chunk_content_hash) = 64 AND chunk_content_hash NOT GLOB '*[^0-9a-f]*')),
    symbol_logical_key TEXT CHECK(symbol_logical_key IS NULL OR (length(trim(symbol_logical_key)) > 0 AND length(symbol_logical_key) <= 512)),
    symbol_label TEXT CHECK(symbol_label IS NULL OR (length(trim(symbol_label)) > 0 AND length(symbol_label) <= 512)),
    source_start_byte INTEGER CHECK(source_start_byte IS NULL OR source_start_byte >= 0),
    source_end_byte INTEGER CHECK(source_end_byte IS NULL OR source_end_byte >= 0),
    CHECK((chunk_stable_key IS NULL) = (chunk_content_hash IS NULL)),
    CHECK((source_start_byte IS NULL) = (source_end_byte IS NULL)),
    CHECK(source_start_byte IS NULL OR source_end_byte >= source_start_byte),
    PRIMARY KEY(workspace_id, experience_id, ordinal),
    FOREIGN KEY(experience_id, workspace_id) REFERENCES experiences(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(source_event_id, workspace_id) REFERENCES events(id, workspace_id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS experience_graph_snapshots (
    workspace_id TEXT NOT NULL,
    experience_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    code_snapshot_ordinal INTEGER NOT NULL CHECK(code_snapshot_ordinal >= 0),
    graph_content_revision INTEGER NOT NULL CHECK(graph_content_revision >= 0),
    graph_schema_version INTEGER NOT NULL CHECK(graph_schema_version >= 0),
    graph_state TEXT NOT NULL CHECK(graph_state IN ('current', 'updating', 'stale', 'error')),
    analyzer_id TEXT NOT NULL CHECK(length(trim(analyzer_id)) > 0 AND length(analyzer_id) <= 256),
    analyzer_version TEXT NOT NULL CHECK(length(trim(analyzer_version)) > 0 AND length(analyzer_version) <= 256),
    structure_version TEXT NOT NULL CHECK(length(trim(structure_version)) > 0 AND length(structure_version) <= 256),
    node_stable_key TEXT NOT NULL CHECK(length(trim(node_stable_key)) > 0 AND length(node_stable_key) <= 512),
    node_type TEXT NOT NULL CHECK(length(trim(node_type)) > 0 AND length(node_type) <= 256),
    resolution_provenance_json TEXT NOT NULL CHECK(length(resolution_provenance_json) <= 65536),
    PRIMARY KEY(workspace_id, experience_id, ordinal),
    FOREIGN KEY(experience_id, workspace_id) REFERENCES experiences(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(workspace_id, experience_id, code_snapshot_ordinal) REFERENCES experience_code_snapshots(workspace_id, experience_id, ordinal) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS experience_strength_bases (
    workspace_id TEXT NOT NULL,
    experience_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    basis TEXT NOT NULL CHECK(basis IN ('deterministic_verifier', 'repeated_deterministic_evidence', 'explicit_user_acceptance', 'explicit_harness_assertion', 'temporal_association', 'structural_association')),
    PRIMARY KEY(workspace_id, experience_id, ordinal),
    UNIQUE(workspace_id, experience_id, basis),
    FOREIGN KEY(experience_id, workspace_id) REFERENCES experiences(id, workspace_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS experience_assessments (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    experience_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('disputed', 'refuted', 'superseded', 'confirmed')),
    actor TEXT NOT NULL CHECK(length(trim(actor)) > 0 AND length(actor) <= 256),
    reason TEXT NOT NULL CHECK(length(trim(reason)) > 0 AND length(reason) <= 4096),
    replacement_experience_id TEXT,
    created_at TEXT NOT NULL,
    CHECK((kind = 'superseded' AND replacement_experience_id IS NOT NULL) OR (kind <> 'superseded' AND replacement_experience_id IS NULL)),
    UNIQUE(id, workspace_id),
    FOREIGN KEY(experience_id, workspace_id) REFERENCES experiences(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(replacement_experience_id, workspace_id) REFERENCES experiences(id, workspace_id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS experience_assessment_evidence (
    workspace_id TEXT NOT NULL,
    assessment_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    event_id TEXT NOT NULL,
    PRIMARY KEY(workspace_id, assessment_id, ordinal),
    FOREIGN KEY(assessment_id, workspace_id) REFERENCES experience_assessments(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(event_id, workspace_id) REFERENCES events(id, workspace_id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_experience_single_replacement
ON experience_assessments(workspace_id, experience_id)
WHERE kind = 'superseded';

CREATE TRIGGER IF NOT EXISTS experiences_require_eligible_episode
BEFORE INSERT ON experiences BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM episodes episode
        WHERE episode.workspace_id = NEW.workspace_id
          AND episode.id = NEW.episode_id
          AND episode.session_id = NEW.session_id
          AND episode.task_id IS NEW.task_id
          AND (
            (episode.status = 'closed' AND NEW.outcome <> 'abandoned')
            OR (episode.status = 'abandoned' AND NEW.outcome IN ('failure', 'abandoned'))
          )
    ) THEN RAISE(ABORT, 'experience requires an eligible terminal episode with exact scope') END;
END;

CREATE TRIGGER IF NOT EXISTS experience_evidence_requires_episode_membership
BEFORE INSERT ON experience_evidence BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1
        FROM experiences experience
        JOIN episode_events member
          ON member.workspace_id = experience.workspace_id
         AND member.episode_id = experience.episode_id
         AND member.event_id = NEW.event_id
        WHERE experience.workspace_id = NEW.workspace_id
          AND experience.id = NEW.experience_id
    ) THEN RAISE(ABORT, 'experience evidence must belong to its source episode') END;

    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM events event
        WHERE event.workspace_id = NEW.workspace_id
          AND event.id = NEW.event_id
          AND (
            NEW.relation = 'supporting'
            OR (NEW.relation = 'initial_failure' AND event.event_type IN ('compiler_result', 'test_result', 'external_tool_finished'))
            OR (NEW.relation = 'attempt_change' AND event.event_type IN ('file_created', 'file_modified', 'file_removed', 'file_renamed', 'external_tool_finished'))
            OR (NEW.relation IN ('attempt_verification', 'terminal_verification') AND event.event_type IN ('compiler_result', 'test_result', 'external_tool_finished', 'user_acceptance'))
          )
    ) THEN RAISE(ABORT, 'experience evidence relation does not match event type') END;
END;

CREATE TRIGGER IF NOT EXISTS experience_verifications_require_linked_evidence
BEFORE INSERT ON experience_verifications BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM experience_evidence evidence
        WHERE evidence.workspace_id = NEW.workspace_id
          AND evidence.experience_id = NEW.experience_id
          AND evidence.event_id = NEW.evidence_event_id
          AND evidence.relation IN ('attempt_verification', 'terminal_verification')
    ) THEN RAISE(ABORT, 'experience verification requires linked verification evidence') END;
END;

CREATE TRIGGER IF NOT EXISTS experience_code_snapshots_require_source_evidence
BEFORE INSERT ON experience_code_snapshots BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM experience_evidence evidence
        WHERE evidence.workspace_id = NEW.workspace_id
          AND evidence.experience_id = NEW.experience_id
          AND evidence.event_id = NEW.source_event_id
          AND evidence.relation = 'attempt_change'
    ) THEN RAISE(ABORT, 'experience code snapshot requires linked source-change evidence') END;
END;

CREATE TRIGGER IF NOT EXISTS experience_assessments_reject_supersession_cycle
BEFORE INSERT ON experience_assessments
WHEN NEW.kind = 'superseded'
BEGIN
    SELECT CASE WHEN EXISTS (
        WITH RECURSIVE replacements(experience_id) AS (
            SELECT NEW.replacement_experience_id
            UNION
            SELECT assessment.replacement_experience_id
            FROM experience_assessments assessment
            JOIN replacements prior ON assessment.experience_id = prior.experience_id
            WHERE assessment.workspace_id = NEW.workspace_id
              AND assessment.kind = 'superseded'
        )
        SELECT 1 FROM replacements WHERE experience_id = NEW.experience_id
    ) THEN RAISE(ABORT, 'experience supersession cycle') END;
END;

CREATE VIRTUAL TABLE IF NOT EXISTS experience_fts USING fts5(
    experience_id UNINDEXED,
    summary,
    failure_key,
    failure_components,
    failure_path,
    failure_symbol_key,
    outcome,
    verification_status,
    tokenize = "unicode61 tokenchars '_:'"
);

CREATE TRIGGER IF NOT EXISTS experiences_fts_insert AFTER INSERT ON experiences BEGIN
    INSERT INTO experience_fts(experience_id, summary, failure_key, failure_components, failure_path, failure_symbol_key, outcome, verification_status)
    VALUES (NEW.id, NEW.summary, COALESCE(NEW.failure_key, ''), NEW.failure_components, COALESCE(NEW.failure_path, ''), COALESCE(NEW.failure_symbol_key, ''), NEW.outcome, NEW.verification_status);
END;

CREATE TRIGGER IF NOT EXISTS experiences_fts_delete AFTER DELETE ON experiences BEGIN
    DELETE FROM experience_fts WHERE experience_id = OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS experiences_immutable
BEFORE UPDATE ON experiences BEGIN
    SELECT RAISE(ABORT, 'experiences are immutable');
END;

-- Consolidation identities hash event payloads.  Events are append-only, so a
-- later writer cannot reinterpret a previously previewed membership frontier.
CREATE TRIGGER IF NOT EXISTS events_immutable
BEFORE UPDATE ON events BEGIN
    SELECT RAISE(ABORT, 'events are immutable');
END;

CREATE TRIGGER IF NOT EXISTS episode_events_immutable_update
BEFORE UPDATE ON episode_events BEGIN
    SELECT RAISE(ABORT, 'episode event membership is immutable');
END;

CREATE TRIGGER IF NOT EXISTS episode_events_immutable_delete
BEFORE DELETE ON episode_events BEGIN
    SELECT RAISE(ABORT, 'episode event membership is immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_attempts_immutable
BEFORE UPDATE ON experience_attempts BEGIN
    SELECT RAISE(ABORT, 'experience attempts are immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_verifications_immutable
BEFORE UPDATE ON experience_verifications BEGIN
    SELECT RAISE(ABORT, 'experience verifications are immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_evidence_immutable
BEFORE UPDATE ON experience_evidence BEGIN
    SELECT RAISE(ABORT, 'experience evidence is immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_code_snapshots_immutable
BEFORE UPDATE ON experience_code_snapshots BEGIN
    SELECT RAISE(ABORT, 'experience code snapshots are immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_graph_snapshots_immutable
BEFORE UPDATE ON experience_graph_snapshots BEGIN
    SELECT RAISE(ABORT, 'experience graph snapshots are immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_strength_bases_immutable
BEFORE UPDATE ON experience_strength_bases BEGIN
    SELECT RAISE(ABORT, 'experience strength bases are immutable');
END;

CREATE TRIGGER IF NOT EXISTS experience_assessments_immutable
BEFORE UPDATE ON experience_assessments BEGIN
    SELECT RAISE(ABORT, 'experience assessments are append-only');
END;

CREATE TRIGGER IF NOT EXISTS experience_assessment_evidence_immutable
BEFORE UPDATE ON experience_assessment_evidence BEGIN
    SELECT RAISE(ABORT, 'experience assessment evidence is immutable');
END;
