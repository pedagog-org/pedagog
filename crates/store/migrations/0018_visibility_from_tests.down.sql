-- (The overrides dropped on the way up don't come back.)
ALTER TABLE assignments ADD COLUMN visibility_overrides JSONB NOT NULL DEFAULT '{}';
