-- Revert 0008: back to global text ids. Refused once another course has
-- assignments (their slugs could clash).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM assignments WHERE course_id <> 'default') THEN
        RAISE EXCEPTION 'assignments exist outside course default; move or delete them first';
    END IF;
END $$;

ALTER TABLE assignment_builds DROP COLUMN policy;

DROP TABLE course_renames;
ALTER TABLE memberships DROP CONSTRAINT memberships_course_id_fkey;
ALTER TABLE memberships ADD CONSTRAINT memberships_course_id_fkey
    FOREIGN KEY (course_id) REFERENCES courses (id) ON DELETE CASCADE;

DROP INDEX submissions_by_student;
DROP INDEX assignment_builds_by_image_hash;
DROP INDEX assignment_builds_one_in_flight;

ALTER TABLE assignment_uploads ADD COLUMN old_assignment_id TEXT;
UPDATE assignment_uploads t SET old_assignment_id = a.slug FROM assignments a WHERE a.id = t.assignment_id;
ALTER TABLE assignment_uploads DROP COLUMN assignment_id;
ALTER TABLE assignment_uploads RENAME COLUMN old_assignment_id TO assignment_id;
ALTER TABLE assignment_uploads ALTER COLUMN assignment_id SET NOT NULL;

ALTER TABLE assignment_builds ADD COLUMN old_assignment_id TEXT;
UPDATE assignment_builds t SET old_assignment_id = a.slug FROM assignments a WHERE a.id = t.assignment_id;
ALTER TABLE assignment_builds DROP COLUMN assignment_id;
ALTER TABLE assignment_builds RENAME COLUMN old_assignment_id TO assignment_id;
ALTER TABLE assignment_builds ALTER COLUMN assignment_id SET NOT NULL;

ALTER TABLE submissions ADD COLUMN old_assignment_id TEXT;
UPDATE submissions t SET old_assignment_id = a.slug FROM assignments a WHERE a.id = t.assignment_id;
ALTER TABLE submissions DROP COLUMN assignment_id;
ALTER TABLE submissions RENAME COLUMN old_assignment_id TO assignment_id;
ALTER TABLE submissions ALTER COLUMN assignment_id SET NOT NULL;

ALTER TABLE regrade_batches ADD COLUMN old_assignment_id TEXT;
UPDATE regrade_batches t SET old_assignment_id = a.slug FROM assignments a WHERE a.id = t.assignment_id;
ALTER TABLE regrade_batches DROP COLUMN assignment_id;
ALTER TABLE regrade_batches RENAME COLUMN old_assignment_id TO assignment_id;
ALTER TABLE regrade_batches ALTER COLUMN assignment_id SET NOT NULL;

ALTER TABLE assignments DROP CONSTRAINT assignments_slug_check;
ALTER TABLE assignments DROP CONSTRAINT assignments_course_slug;
ALTER TABLE assignments DROP CONSTRAINT assignments_pkey;
ALTER TABLE assignments DROP COLUMN id;
ALTER TABLE assignments DROP COLUMN course_id;
ALTER TABLE assignments RENAME COLUMN slug TO id;
ALTER TABLE assignments ADD PRIMARY KEY (id);

ALTER TABLE assignment_uploads ADD FOREIGN KEY (assignment_id) REFERENCES assignments (id);
ALTER TABLE assignment_uploads ADD UNIQUE (assignment_id, tree_hash);
ALTER TABLE assignment_builds ADD FOREIGN KEY (assignment_id) REFERENCES assignments (id);
ALTER TABLE submissions ADD FOREIGN KEY (assignment_id) REFERENCES assignments (id);
ALTER TABLE regrade_batches ADD FOREIGN KEY (assignment_id) REFERENCES assignments (id);

CREATE UNIQUE INDEX assignment_builds_one_in_flight
    ON assignment_builds (assignment_id) WHERE finished_at IS NULL;
CREATE INDEX assignment_builds_by_image_hash ON assignment_builds (assignment_id, image_hash);
CREATE INDEX submissions_by_student ON submissions (assignment_id, student_id, id DESC);

DELETE FROM courses c WHERE c.id = 'default'
   AND NOT EXISTS (SELECT 1 FROM memberships m WHERE m.course_id = c.id);
