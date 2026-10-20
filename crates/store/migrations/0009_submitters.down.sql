-- Revert 0009: users' submissions keep their email as the student id.
DROP INDEX submissions_by_user;
ALTER TABLE submissions DROP CONSTRAINT submissions_one_owner;
UPDATE submissions s SET student_id = u.email FROM users u WHERE u.id = s.user_id;
ALTER TABLE submissions ALTER COLUMN student_id SET NOT NULL;
ALTER TABLE submissions DROP COLUMN counted_at;
ALTER TABLE submissions DROP COLUMN submitted_by;
ALTER TABLE submissions DROP COLUMN user_id;
