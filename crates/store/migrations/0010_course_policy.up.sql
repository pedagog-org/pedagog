-- The course's policy for an assignment, over the file's: date edits (a
-- Dates, field by field), visibility overrides (a ResultsPolicy), and when
-- results were released. Course edits apply at once; they never build.
ALTER TABLE assignments ADD COLUMN date_overrides JSONB NOT NULL DEFAULT '{}';
ALTER TABLE assignments ADD COLUMN visibility_overrides JSONB NOT NULL DEFAULT '{}';
ALTER TABLE assignments ADD COLUMN released_at TIMESTAMPTZ;

-- Uploaded with --take-dates: once this build is current, the course's date
-- edits are cleared and the file's dates apply.
ALTER TABLE assignment_builds ADD COLUMN take_dates BOOLEAN NOT NULL DEFAULT false;
