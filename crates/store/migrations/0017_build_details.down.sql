DROP TRIGGER assignments_current_event ON assignments;
DROP FUNCTION pedagog_current_event();
ALTER TABLE assignments DROP COLUMN current_since;
-- (The old rows' full error text isn't restored.)
ALTER TABLE assignment_builds
    DROP COLUMN validation_log_lines,
    DROP COLUMN build_log_lines,
    DROP COLUMN validation_started_at,
    DROP COLUMN built_at,
    DROP COLUMN rebuild_of,
    DROP COLUMN uploaded_by;
