UPDATE grading_runs SET error_kind = 'internal' WHERE error_kind = 'out_of_disk';
ALTER TABLE grading_runs DROP CONSTRAINT grading_runs_error_kind_check;
ALTER TABLE grading_runs ADD CONSTRAINT grading_runs_error_kind_check
    CHECK (error_kind IN ('load', 'timeout', 'out_of_memory', 'crash', 'no_results', 'invalid', 'internal'));
DROP TABLE maintenance_runs;
ALTER TABLE assignment_builds DROP COLUMN image_deleted_at;
