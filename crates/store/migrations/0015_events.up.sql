-- Live updates: every grading run's and build's status change is announced on
-- `pedagog_events`, so the api can push it to the pages watching. A trigger, so
-- no code path that changes a status can forget to.
CREATE FUNCTION pedagog_run_event() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'INSERT' OR NEW.status IS DISTINCT FROM OLD.status THEN
        PERFORM pg_notify('pedagog_events', json_build_object(
            'kind', 'run',
            'run', NEW.id,
            'submission', NEW.submission_id,
            'assignment', s.assignment_id,
            'user', s.user_id,
            'email', u.email,
            'status', NEW.status,
            'score', NEW.score,
            'total', NEW.total
        )::text)
        FROM submissions s LEFT JOIN users u ON u.id = s.user_id
        WHERE s.id = NEW.submission_id;
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER grading_runs_event AFTER INSERT OR UPDATE OF status ON grading_runs
    FOR EACH ROW EXECUTE FUNCTION pedagog_run_event();

CREATE FUNCTION pedagog_build_event() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'INSERT' OR NEW.build_status IS DISTINCT FROM OLD.build_status
        OR NEW.validation_status IS DISTINCT FROM OLD.validation_status THEN
        PERFORM pg_notify('pedagog_events', json_build_object(
            'kind', 'build',
            'build', NEW.id,
            'assignment', NEW.assignment_id,
            'build_status', NEW.build_status,
            'validation_status', NEW.validation_status
        )::text);
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER assignment_builds_event
    AFTER INSERT OR UPDATE OF build_status, validation_status ON assignment_builds
    FOR EACH ROW EXECUTE FUNCTION pedagog_build_event();
