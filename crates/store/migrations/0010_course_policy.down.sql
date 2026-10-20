-- Revert 0010.
ALTER TABLE assignment_builds DROP COLUMN take_dates;
ALTER TABLE assignments DROP COLUMN released_at;
ALTER TABLE assignments DROP COLUMN visibility_overrides;
ALTER TABLE assignments DROP COLUMN date_overrides;
