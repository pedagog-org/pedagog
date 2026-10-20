-- The nightly registry cleanup deletes images nothing uses; a build whose image
-- is gone is never reused (the same content builds again).
ALTER TABLE assignment_builds ADD COLUMN image_deleted_at TIMESTAMPTZ;
