-- Whose a submission is (a user; operator submissions keep an opaque
-- student_id instead), who sent it when that's someone else, and the time it
-- counts at (lateness, which is latest). created_at stays when it arrived.
ALTER TABLE submissions ADD COLUMN user_id BIGINT REFERENCES users (id);
ALTER TABLE submissions ADD COLUMN submitted_by BIGINT REFERENCES users (id);
ALTER TABLE submissions ADD COLUMN counted_at TIMESTAMPTZ;
UPDATE submissions SET counted_at = created_at;
ALTER TABLE submissions ALTER COLUMN counted_at SET NOT NULL;
ALTER TABLE submissions ALTER COLUMN counted_at SET DEFAULT now();
ALTER TABLE submissions ALTER COLUMN student_id DROP NOT NULL;
ALTER TABLE submissions ADD CONSTRAINT submissions_one_owner
    CHECK (num_nonnulls(user_id, student_id) = 1);

CREATE INDEX submissions_by_user ON submissions (assignment_id, user_id, counted_at DESC);
