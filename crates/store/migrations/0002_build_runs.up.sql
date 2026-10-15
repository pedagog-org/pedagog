-- One row per build attempt: durable progress + failure history.
CREATE TABLE build_runs (
    id          BIGSERIAL PRIMARY KEY,
    os_id       TEXT NOT NULL,
    hash        TEXT NOT NULL,
    image_ref   TEXT NOT NULL,
    -- maps to BuildStatus via strum; CHECK is the DB-side backstop
    status      TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    started_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    error       TEXT
);

-- At most one in-flight attempt per OS — makes dispatch dedup a DB invariant.
CREATE UNIQUE INDEX build_runs_one_running_per_os
    ON build_runs (os_id)
    WHERE status = 'running';
