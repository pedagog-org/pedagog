-- Emails to students, rendered when queued and sent by the api at a bounded
-- rate: regrade notices (one per run), results releases, and notices staff
-- send by hand. Failures are retried with backoff, then given up on.
CREATE TABLE notices (
    id             BIGSERIAL PRIMARY KEY,
    user_id        BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    assignment_id  BIGINT REFERENCES assignments (id),
    kind           TEXT NOT NULL CHECK (kind IN ('regrade', 'release', 'manual')),
    grading_run_id BIGINT UNIQUE REFERENCES grading_runs (id),
    subject        TEXT NOT NULL,
    body           TEXT NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    send_after     TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts       INTEGER NOT NULL DEFAULT 0,
    last_error     TEXT,
    sent_at        TIMESTAMPTZ,
    gave_up_at     TIMESTAMPTZ
);

CREATE INDEX notices_due ON notices (send_after) WHERE sent_at IS NULL AND gave_up_at IS NULL;
