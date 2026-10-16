-- Revert 0004.
DROP TABLE run_queue;
DROP TABLE grading_runs;
DROP TABLE regrade_batches;
DROP TABLE submissions;
ALTER TABLE assignment_builds DROP COLUMN regrade_notify;
UPDATE assignment_builds SET validation_status = 'pending' WHERE validation_status = 'queued';
ALTER TABLE assignment_builds DROP CONSTRAINT assignment_builds_validation_status_check;
ALTER TABLE assignment_builds ADD CONSTRAINT assignment_builds_validation_status_check
    CHECK (validation_status IN ('pending', 'running', 'passed', 'failed', 'skipped', 'invalid'));
