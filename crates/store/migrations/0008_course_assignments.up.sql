-- Assignments belong to a course: a numeric id is their key, and a slug (the
-- id in assignment.yml) is unique within the course. Existing assignments
-- move into course `default`.
INSERT INTO courses (id, name)
SELECT 'default', 'Default' WHERE EXISTS (SELECT 1 FROM assignments)
ON CONFLICT (id) DO NOTHING;

ALTER TABLE assignments RENAME COLUMN id TO slug;
ALTER TABLE assignments ADD COLUMN course_id TEXT REFERENCES courses (id) ON UPDATE CASCADE;
UPDATE assignments SET course_id = 'default';
ALTER TABLE assignments ALTER COLUMN course_id SET NOT NULL;
ALTER TABLE assignments ADD COLUMN new_id BIGSERIAL;

ALTER TABLE assignment_uploads ADD COLUMN new_assignment_id BIGINT;
UPDATE assignment_uploads t SET new_assignment_id = a.new_id FROM assignments a WHERE a.slug = t.assignment_id;
ALTER TABLE assignment_uploads DROP COLUMN assignment_id;
ALTER TABLE assignment_uploads RENAME COLUMN new_assignment_id TO assignment_id;
ALTER TABLE assignment_uploads ALTER COLUMN assignment_id SET NOT NULL;

ALTER TABLE assignment_builds ADD COLUMN new_assignment_id BIGINT;
UPDATE assignment_builds t SET new_assignment_id = a.new_id FROM assignments a WHERE a.slug = t.assignment_id;
ALTER TABLE assignment_builds DROP COLUMN assignment_id;
ALTER TABLE assignment_builds RENAME COLUMN new_assignment_id TO assignment_id;
ALTER TABLE assignment_builds ALTER COLUMN assignment_id SET NOT NULL;

ALTER TABLE submissions ADD COLUMN new_assignment_id BIGINT;
UPDATE submissions t SET new_assignment_id = a.new_id FROM assignments a WHERE a.slug = t.assignment_id;
ALTER TABLE submissions DROP COLUMN assignment_id;
ALTER TABLE submissions RENAME COLUMN new_assignment_id TO assignment_id;
ALTER TABLE submissions ALTER COLUMN assignment_id SET NOT NULL;

ALTER TABLE regrade_batches ADD COLUMN new_assignment_id BIGINT;
UPDATE regrade_batches t SET new_assignment_id = a.new_id FROM assignments a WHERE a.slug = t.assignment_id;
ALTER TABLE regrade_batches DROP COLUMN assignment_id;
ALTER TABLE regrade_batches RENAME COLUMN new_assignment_id TO assignment_id;
ALTER TABLE regrade_batches ALTER COLUMN assignment_id SET NOT NULL;

ALTER TABLE assignments DROP CONSTRAINT assignments_pkey;
ALTER TABLE assignments RENAME COLUMN new_id TO id;
ALTER TABLE assignments ADD PRIMARY KEY (id);
ALTER TABLE assignments ADD CONSTRAINT assignments_course_slug UNIQUE (course_id, slug);
ALTER TABLE assignments ADD CONSTRAINT assignments_slug_check
    CHECK (slug ~ '^[a-z]([a-z0-9-]{0,38}[a-z0-9])?$') NOT VALID;

ALTER TABLE assignment_uploads ADD FOREIGN KEY (assignment_id) REFERENCES assignments (id);
ALTER TABLE assignment_uploads ADD UNIQUE (assignment_id, tree_hash);
ALTER TABLE assignment_builds ADD FOREIGN KEY (assignment_id) REFERENCES assignments (id);
ALTER TABLE submissions ADD FOREIGN KEY (assignment_id) REFERENCES assignments (id);
ALTER TABLE regrade_batches ADD FOREIGN KEY (assignment_id) REFERENCES assignments (id);

CREATE UNIQUE INDEX assignment_builds_one_in_flight
    ON assignment_builds (assignment_id) WHERE finished_at IS NULL;
CREATE INDEX assignment_builds_by_image_hash ON assignment_builds (assignment_id, image_hash);
CREATE INDEX submissions_by_student ON submissions (assignment_id, student_id, id DESC);

-- The file's `assignment:` policy (dates, visibility defaults), as uploaded
-- with this build. NULL (builds from before) reads as the default: no dates.
ALTER TABLE assignment_builds ADD COLUMN policy JSONB;

-- Course slugs can be renamed: references follow, and the old slug says
-- where it went.
ALTER TABLE memberships DROP CONSTRAINT memberships_course_id_fkey;
ALTER TABLE memberships ADD CONSTRAINT memberships_course_id_fkey
    FOREIGN KEY (course_id) REFERENCES courses (id) ON DELETE CASCADE ON UPDATE CASCADE;

CREATE TABLE course_renames (
    old_id     TEXT PRIMARY KEY,
    new_id     TEXT NOT NULL,
    renamed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
