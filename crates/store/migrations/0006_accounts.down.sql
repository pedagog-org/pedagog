-- Revert 0006.
DROP INDEX grading_runs_unnotified;
ALTER TABLE grading_runs DROP COLUMN notified_at;
ALTER TABLE submissions DROP COLUMN late, DROP COLUMN staff;
DROP TABLE login_codes;
DROP TABLE sessions;
DROP TABLE enrollments;
DROP TABLE course_staff;
DROP TABLE courses;
DROP TABLE users;
