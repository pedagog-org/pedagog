-- What a build's test runs need, saved at dispatch so admission and intake
-- don't re-derive it from the stored archive. NULL for builds from before
-- this migration until jobs backfills them.
ALTER TABLE assignment_builds
    ADD COLUMN platform   TEXT CHECK (platform IN ('workspace', 'upload')),
    ADD COLUMN isolation  TEXT CHECK (isolation IN ('process', 'in-process')),
    -- {"mode": "deny"} | {"mode": "internet"} | {"mode": "cidrs", "cidrs": [[cidr, [ports]]]}
    ADD COLUMN egress     JSONB,
    -- The assignment's parsed submission: rules.
    ADD COLUMN submission JSONB;
