-- People: identified by email (stored lowercased). Admins run the platform.
CREATE TABLE users (
    id         BIGSERIAL PRIMARY KEY,
    email      TEXT NOT NULL UNIQUE CHECK (email = lower(email) AND email LIKE '%_@_%'),
    name       TEXT NOT NULL DEFAULT '',
    is_admin   BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A course offering; the id is a slug used in URLs, commands and image names.
CREATE TABLE courses (
    id         TEXT PRIMARY KEY CHECK (id ~ '^[a-z0-9]([a-z0-9-]{0,38}[a-z0-9])?$'),
    name       TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Instructors of a course (several per course).
CREATE TABLE course_staff (
    course_id TEXT NOT NULL REFERENCES courses (id) ON DELETE CASCADE,
    user_id   BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (course_id, user_id)
);

-- Students of a course. Dropping keeps the row (and their submissions).
CREATE TABLE enrollments (
    course_id   TEXT NOT NULL REFERENCES courses (id) ON DELETE CASCADE,
    user_id     BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    enrolled_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    dropped_at  TIMESTAMPTZ,
    PRIMARY KEY (course_id, user_id)
);

-- Logged-in clients: only a hash of the token is stored.
CREATE TABLE sessions (
    token_hash   TEXT PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    kind         TEXT NOT NULL CHECK (kind IN ('web', 'cli')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL,
    last_used_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX sessions_by_expiry ON sessions (expires_at);

-- The pending login code per email (hashed), with its tries.
CREATE TABLE login_codes (
    email      TEXT PRIMARY KEY,
    code_hash  TEXT NOT NULL,
    attempts   INTEGER NOT NULL DEFAULT 0,
    sent_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);

-- Instructors' own submissions. (Whether one is late isn't stored: it depends
-- on the dates in effect for that student, which can change afterwards.)
ALTER TABLE submissions ADD COLUMN staff BOOLEAN NOT NULL DEFAULT false;

-- When a regrade notice was sent (runs with notify = true).
ALTER TABLE grading_runs ADD COLUMN notified_at TIMESTAMPTZ;
CREATE INDEX grading_runs_unnotified ON grading_runs (id) WHERE notify AND notified_at IS NULL;
