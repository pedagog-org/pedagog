-- Validation waits in the admission queue before it runs.
ALTER TABLE assignment_builds DROP CONSTRAINT assignment_builds_validation_status_check;
ALTER TABLE assignment_builds ADD CONSTRAINT assignment_builds_validation_status_check
    CHECK (validation_status IN ('pending', 'queued', 'running', 'passed', 'failed', 'skipped', 'invalid'));

-- Set when the upload asked for a regrade: once this build becomes current,
-- every student's latest submission is graded again against it.
ALTER TABLE assignment_builds ADD COLUMN regrade_notify TEXT
    CHECK (regrade_notify IN ('never', 'changed', 'always'));

-- One row per accepted submission; the normalized tree lives in Garage at
-- object_key. build_id is the build current at acceptance (pinned).
CREATE TABLE submissions (
    id            BIGSERIAL PRIMARY KEY,
    assignment_id TEXT NOT NULL REFERENCES assignments (id),
    -- Opaque until auth exists.
    student_id    TEXT NOT NULL,
    build_id      BIGINT NOT NULL REFERENCES assignment_builds (id),
    source        TEXT NOT NULL CHECK (source IN ('upload', 'workspace')),
    tree_hash     TEXT NOT NULL,
    object_key    TEXT NOT NULL,
    size_bytes    BIGINT NOT NULL,
    -- What intake did: files kept, ignored, the stripped folder.
    receipt       JSONB NOT NULL DEFAULT '{}',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX submissions_by_student ON submissions (assignment_id, student_id, id DESC);

-- The regrades one build triggered when it became current.
CREATE TABLE regrade_batches (
    id            BIGSERIAL PRIMARY KEY,
    assignment_id TEXT NOT NULL REFERENCES assignments (id),
    build_id      BIGINT NOT NULL UNIQUE REFERENCES assignment_builds (id),
    notify        TEXT NOT NULL CHECK (notify IN ('never', 'changed', 'always')),
    runs          INTEGER NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One row per grading of a submission: its first run, then one per regrade.
CREATE TABLE grading_runs (
    id               BIGSERIAL PRIMARY KEY,
    submission_id    BIGINT NOT NULL REFERENCES submissions (id),
    build_id         BIGINT NOT NULL REFERENCES assignment_builds (id),
    reason           TEXT NOT NULL CHECK (reason IN ('submission', 'regrade')),
    regrade_batch_id BIGINT REFERENCES regrade_batches (id),
    status           TEXT NOT NULL DEFAULT 'queued'
        CHECK (status IN ('queued', 'grading', 'graded', 'error')),
    -- Recorded when the run starts, from the build's assignment.
    isolation        TEXT CHECK (isolation IN ('process', 'in-process')),
    job_name         TEXT,
    queued_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at       TIMESTAMPTZ,
    finished_at      TIMESTAMPTZ,
    score            DOUBLE PRECISION,
    total            DOUBLE PRECISION,
    -- Parsed results.yaml; NULL until graded, and for errors (untrusted).
    results          JSONB,
    problems         TEXT[] NOT NULL DEFAULT '{}',
    error            TEXT,
    log_tail         TEXT,
    -- Regrades: the difference from the submission's previous graded run.
    changes          JSONB,
    -- Whether the student should be told about this run (delivery comes with the api).
    notify           BOOLEAN NOT NULL DEFAULT false,
    CHECK ((reason = 'regrade') = (regrade_batch_id IS NOT NULL))
);

CREATE INDEX grading_runs_by_submission ON grading_runs (submission_id, id DESC);
CREATE INDEX grading_runs_in_flight ON grading_runs (id) WHERE status = 'grading';

-- Test runs (validation and grading) waiting for admission, released in
-- (priority, queued_at) order while the cluster has room.
CREATE TABLE run_queue (
    id             BIGSERIAL PRIMARY KEY,
    build_id       BIGINT UNIQUE REFERENCES assignment_builds (id),
    grading_run_id BIGINT UNIQUE REFERENCES grading_runs (id),
    -- 0: submissions and validations; 1: regrades.
    priority       SMALLINT NOT NULL,
    queued_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    released_at    TIMESTAMPTZ,
    CHECK (num_nonnulls(build_id, grading_run_id) = 1)
);

CREATE INDEX run_queue_waiting ON run_queue (priority, queued_at, id) WHERE released_at IS NULL;
