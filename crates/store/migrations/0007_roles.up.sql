-- Global roles: admins run the platform; staff (and admins) create courses.
ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'student'
    CHECK (role IN ('admin', 'staff', 'student'));
UPDATE users SET role = 'admin' WHERE is_admin;
ALTER TABLE users DROP COLUMN is_admin;
ALTER TABLE users ADD COLUMN last_login_at TIMESTAMPTZ;
ALTER TABLE users ADD COLUMN last_login_kind TEXT CHECK (last_login_kind IN ('web', 'cli'));

-- Everyone in a course, with one role each. Dropping keeps the row (and its
-- history); adding again restores it.
CREATE TABLE memberships (
    course_id  TEXT NOT NULL REFERENCES courses (id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role       TEXT NOT NULL CHECK (role IN ('instructor', 'grader', 'student')),
    source     TEXT NOT NULL CHECK (source IN ('created', 'manual', 'import')),
    added_by   BIGINT REFERENCES users (id) ON DELETE SET NULL,
    joined_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    dropped_at TIMESTAMPTZ,
    PRIMARY KEY (course_id, user_id)
);

CREATE INDEX memberships_by_user ON memberships (user_id);

INSERT INTO memberships (course_id, user_id, role, source, joined_at, dropped_at)
SELECT course_id, user_id, 'student', 'import', enrolled_at, dropped_at FROM enrollments;

INSERT INTO memberships (course_id, user_id, role, source)
SELECT course_id, user_id, 'instructor', 'manual' FROM course_staff
ON CONFLICT (course_id, user_id) DO UPDATE SET role = 'instructor', dropped_at = NULL;

DROP TABLE course_staff;
DROP TABLE enrollments;
