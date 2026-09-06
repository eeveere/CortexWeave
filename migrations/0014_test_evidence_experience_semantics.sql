-- v0.5.1 adds one verification kind and one factual attempt result while
-- preserving every v0.5 row. SQLite CHECK constraints require bounded table
-- replacement; the surrounding sqlx migration transaction owns atomicity.

DROP TRIGGER IF EXISTS experience_attempts_immutable_delete;
DROP TRIGGER IF EXISTS experience_attempts_reject_after_seal;

ALTER TABLE experience_attempts RENAME TO experience_attempts_v050;

CREATE TABLE experience_attempts (
    id TEXT PRIMARY KEY NOT NULL,
    workspace_id TEXT NOT NULL,
    experience_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    result TEXT NOT NULL CHECK(result IN ('still_failing', 'verification_passed', 'verification_failed', 'verification_changed_failure', 'inconclusive')),
    change_evidence_ordinals_json TEXT NOT NULL,
    following_verification_ordinal INTEGER,
    UNIQUE(id, workspace_id),
    UNIQUE(workspace_id, experience_id, ordinal),
    FOREIGN KEY(experience_id, workspace_id) REFERENCES experiences(id, workspace_id) ON DELETE CASCADE
);

INSERT INTO experience_attempts(
    id,
    workspace_id,
    experience_id,
    ordinal,
    result,
    change_evidence_ordinals_json,
    following_verification_ordinal
)
SELECT
    id,
    workspace_id,
    experience_id,
    ordinal,
    result,
    change_evidence_ordinals_json,
    following_verification_ordinal
FROM experience_attempts_v050;

DROP TABLE experience_attempts_v050;

CREATE TRIGGER experience_attempts_immutable_delete
BEFORE DELETE ON experience_attempts
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experience attempts are immutable');
END;

CREATE TRIGGER experience_attempts_reject_after_seal
BEFORE INSERT ON experience_attempts
WHEN EXISTS (
    SELECT 1 FROM experience_seals seal
    WHERE seal.workspace_id = NEW.workspace_id
      AND seal.experience_id = NEW.experience_id
)
BEGIN
    SELECT RAISE(ABORT, 'sealed experience rejects new attempts');
END;

DROP TRIGGER IF EXISTS experience_verifications_require_linked_evidence;
DROP TRIGGER IF EXISTS experience_verifications_immutable_delete;
DROP TRIGGER IF EXISTS experience_verifications_reject_after_seal;

ALTER TABLE experience_verifications RENAME TO experience_verifications_v050;

CREATE TABLE experience_verifications (
    workspace_id TEXT NOT NULL,
    experience_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    status TEXT NOT NULL CHECK(status IN ('verified_passed', 'verified_failed', 'explicitly_accepted')),
    kind TEXT NOT NULL CHECK(kind IN ('rust_compiler', 'cargo_test', 'test_run', 'registered_tool', 'user_acceptance')),
    subject_kind TEXT NOT NULL CHECK(subject_kind IN ('workspace', 'package', 'target', 'test', 'path')),
    subject_value TEXT NOT NULL CHECK(length(trim(subject_value)) > 0 AND length(subject_value) <= 512),
    evidence_event_id TEXT NOT NULL,
    rule_id TEXT NOT NULL CHECK(length(trim(rule_id)) > 0 AND length(rule_id) <= 256),
    rule_version TEXT NOT NULL CHECK(length(trim(rule_version)) > 0 AND length(rule_version) <= 256),
    PRIMARY KEY(workspace_id, experience_id, ordinal),
    FOREIGN KEY(experience_id, workspace_id) REFERENCES experiences(id, workspace_id) ON DELETE CASCADE,
    FOREIGN KEY(evidence_event_id, workspace_id) REFERENCES events(id, workspace_id) ON DELETE RESTRICT
);

INSERT INTO experience_verifications(
    workspace_id,
    experience_id,
    ordinal,
    status,
    kind,
    subject_kind,
    subject_value,
    evidence_event_id,
    rule_id,
    rule_version
)
SELECT
    workspace_id,
    experience_id,
    ordinal,
    status,
    kind,
    subject_kind,
    subject_value,
    evidence_event_id,
    rule_id,
    rule_version
FROM experience_verifications_v050;

DROP TABLE experience_verifications_v050;

CREATE TRIGGER experience_verifications_require_linked_evidence
BEFORE INSERT ON experience_verifications BEGIN
    SELECT CASE WHEN NOT EXISTS (
        SELECT 1 FROM experience_evidence evidence
        WHERE evidence.workspace_id = NEW.workspace_id
          AND evidence.experience_id = NEW.experience_id
          AND evidence.event_id = NEW.evidence_event_id
          AND evidence.relation IN ('attempt_verification', 'terminal_verification')
    ) THEN RAISE(ABORT, 'experience verification requires linked verification evidence') END;
END;

CREATE TRIGGER experience_verifications_immutable_delete
BEFORE DELETE ON experience_verifications
WHEN EXISTS (
    SELECT 1 FROM workspaces workspace WHERE workspace.id = OLD.workspace_id
)
BEGIN
    SELECT RAISE(ABORT, 'experience verifications are immutable');
END;

CREATE TRIGGER experience_verifications_reject_after_seal
BEFORE INSERT ON experience_verifications
WHEN EXISTS (
    SELECT 1 FROM experience_seals seal
    WHERE seal.workspace_id = NEW.workspace_id
      AND seal.experience_id = NEW.experience_id
)
BEGIN
    SELECT RAISE(ABORT, 'sealed experience rejects new verifications');
END;
