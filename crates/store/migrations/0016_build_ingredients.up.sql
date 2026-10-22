-- The ingredient files (sha256) a build declared, so the sync that deletes
-- unpinned ingredients spares those a build still in flight needs.
ALTER TABLE assignment_builds ADD COLUMN ingredients text[] NOT NULL DEFAULT '{}';
ALTER TABLE build_runs ADD COLUMN ingredients text[] NOT NULL DEFAULT '{}';
