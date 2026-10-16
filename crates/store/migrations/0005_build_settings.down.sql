-- Revert 0005.
ALTER TABLE assignment_builds
    DROP COLUMN platform,
    DROP COLUMN isolation,
    DROP COLUMN egress,
    DROP COLUMN submission;
