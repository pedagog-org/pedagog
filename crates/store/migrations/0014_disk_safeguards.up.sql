-- The nightly registry cleanup deletes images nothing uses; a build whose image
-- is gone is never reused (the same content builds again).
ALTER TABLE assignment_builds ADD COLUMN image_deleted_at TIMESTAMPTZ;

-- Scheduled maintenance claims each day's run once, across jobs replicas.
CREATE TABLE maintenance_runs (
    task       TEXT NOT NULL,
    day        DATE NOT NULL,
    claimed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (task, day)
);

-- A test run evicted for using too much local disk.
ALTER TABLE grading_runs DROP CONSTRAINT grading_runs_error_kind_check;
ALTER TABLE grading_runs ADD CONSTRAINT grading_runs_error_kind_check
    CHECK (error_kind IN ('load', 'timeout', 'out_of_memory', 'out_of_disk', 'crash', 'no_results', 'invalid', 'internal'));
