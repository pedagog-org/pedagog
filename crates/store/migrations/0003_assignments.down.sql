-- Revert 0003.
ALTER TABLE assignments DROP CONSTRAINT assignments_current_build_fk;
DROP TABLE assignment_builds;
DROP TABLE assignment_uploads;
DROP TABLE assignments;
