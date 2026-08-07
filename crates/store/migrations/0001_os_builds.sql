-- Current successful build per OS — the change-detection source of truth.
CREATE TABLE os_builds (
    os_id              TEXT PRIMARY KEY,
    containerfile_hash TEXT NOT NULL,
    image_ref          TEXT NOT NULL,
    built_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);
