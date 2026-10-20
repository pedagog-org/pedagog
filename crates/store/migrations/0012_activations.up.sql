-- Staff can make an earlier submission the one that counts. It counts until
-- the student submits something newer (by arrival); then the latest counts.
CREATE TABLE submission_activations (
    assignment_id BIGINT NOT NULL REFERENCES assignments (id),
    user_id       BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    submission_id BIGINT NOT NULL REFERENCES submissions (id),
    activated_by  BIGINT REFERENCES users (id) ON DELETE SET NULL,
    activated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (assignment_id, user_id)
);
