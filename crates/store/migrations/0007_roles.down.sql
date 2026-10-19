-- Revert 0007. Graders become instructors; dropped staff are left out.
CREATE TABLE course_staff (
    course_id TEXT NOT NULL REFERENCES courses (id) ON DELETE CASCADE,
    user_id   BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (course_id, user_id)
);

CREATE TABLE enrollments (
    course_id   TEXT NOT NULL REFERENCES courses (id) ON DELETE CASCADE,
    user_id     BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    enrolled_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    dropped_at  TIMESTAMPTZ,
    PRIMARY KEY (course_id, user_id)
);

INSERT INTO course_staff (course_id, user_id)
SELECT course_id, user_id FROM memberships
 WHERE role IN ('instructor', 'grader') AND dropped_at IS NULL;

INSERT INTO enrollments (course_id, user_id, enrolled_at, dropped_at)
SELECT course_id, user_id, joined_at, dropped_at FROM memberships WHERE role = 'student';

DROP TABLE memberships;

ALTER TABLE users DROP COLUMN last_login_kind;
ALTER TABLE users DROP COLUMN last_login_at;
ALTER TABLE users ADD COLUMN is_admin BOOLEAN NOT NULL DEFAULT false;
UPDATE users SET is_admin = (role = 'admin');
ALTER TABLE users DROP COLUMN role;
