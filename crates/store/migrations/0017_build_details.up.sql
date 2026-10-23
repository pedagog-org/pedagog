-- What the staff build pages show: who started each attempt, what it rebuilt,
-- when each phase ran, and how long each phase's log (kept in Garage) is.
ALTER TABLE assignment_builds
    ADD COLUMN uploaded_by BIGINT REFERENCES users (id) ON DELETE SET NULL,
    -- Making an old build current whose image was cleaned up builds its upload again.
    ADD COLUMN rebuild_of BIGINT REFERENCES assignment_builds (id),
    ADD COLUMN built_at TIMESTAMPTZ,
    ADD COLUMN validation_started_at TIMESTAMPTZ,
    -- Set when that phase's log is in Garage (logs/builds/{id}/{phase}.log.gz);
    -- cleared when the log is deleted.
    ADD COLUMN build_log_lines INTEGER,
    ADD COLUMN validation_log_lines INTEGER;

-- When the current build became current (validated, or made current by hand).
ALTER TABLE assignments ADD COLUMN current_since TIMESTAMPTZ;
UPDATE assignments a SET current_since = b.finished_at
  FROM assignment_builds b WHERE b.id = a.current_build_id;

-- `error` holds only a failure's short reason from now on; the whole pod log it
-- used to hold is cut to its last non-empty line (those builds have no log kept).
UPDATE assignment_builds b
   SET error = (SELECT t.line FROM regexp_split_to_table(b.error, E'\n') WITH ORDINALITY AS t(line, n)
                 WHERE btrim(t.line) <> '' ORDER BY t.n DESC LIMIT 1)
 WHERE b.error LIKE E'%\n%';

-- A build made current (by validation or by hand) is announced like a status change.
CREATE FUNCTION pedagog_current_event() RETURNS trigger AS $$
BEGIN
    IF NEW.current_build_id IS DISTINCT FROM OLD.current_build_id THEN
        PERFORM pg_notify('pedagog_events', json_build_object(
            'kind', 'current',
            'assignment', NEW.id,
            'build', NEW.current_build_id
        )::text);
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER assignments_current_event AFTER UPDATE OF current_build_id ON assignments
    FOR EACH ROW EXECUTE FUNCTION pedagog_current_event();
