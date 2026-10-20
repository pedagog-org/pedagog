-- Why a grading run errored, so students can be told in plain words without
-- seeing logs or problems.
ALTER TABLE grading_runs ADD COLUMN error_kind TEXT
    CHECK (error_kind IN ('load', 'timeout', 'out_of_memory', 'crash', 'no_results', 'invalid', 'internal'));
UPDATE grading_runs SET error_kind = 'internal' WHERE status = 'error';
