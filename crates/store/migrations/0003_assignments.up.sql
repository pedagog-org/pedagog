-- One row per assignment id (from assignment.yml). Ids are global for now;
-- course/tenant scoping arrives with api/auth.
CREATE TABLE assignments (
    id               TEXT PRIMARY KEY,
    name             TEXT NOT NULL,
    -- The latest build that succeeded with validation passed or skipped; what
    -- sessions and grading use. A failed build never replaces it.
    current_build_id BIGINT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A distinct uploaded file tree per assignment; the archive lives in Garage
-- at object_key. Re-uploading the same tree updates uploaded_at.
CREATE TABLE assignment_uploads (
    id            BIGSERIAL PRIMARY KEY,
    assignment_id TEXT NOT NULL REFERENCES assignments (id),
    tree_hash     TEXT NOT NULL,
    object_key    TEXT NOT NULL,
    size_bytes    BIGINT NOT NULL,
    uploaded_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (assignment_id, tree_hash)
);

-- One row per build attempt, including its validation run.
CREATE TABLE assignment_builds (
    id                  BIGSERIAL PRIMARY KEY,
    assignment_id       TEXT NOT NULL REFERENCES assignments (id),
    upload_id           BIGINT NOT NULL REFERENCES assignment_uploads (id),
    image_hash          TEXT NOT NULL,
    image_ref           TEXT NOT NULL,
    -- map to AssignmentBuildStatus / ValidationStatus via strum; CHECKs are the DB backstop
    build_status        TEXT NOT NULL
        CHECK (build_status IN ('running', 'succeeded', 'failed', 'superseded')),
    validation_status   TEXT NOT NULL DEFAULT 'pending'
        CHECK (validation_status IN ('pending', 'running', 'passed', 'failed', 'skipped', 'invalid')),
    -- Parsed results.yaml; NULL until validated, and for invalid results (untrusted).
    results             JSONB,
    -- Format problems, solution mismatches, or infrastructure errors.
    validation_problems TEXT[] NOT NULL DEFAULT '{}',
    error               TEXT,
    started_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Set when the whole attempt (build and validation) is over.
    finished_at         TIMESTAMPTZ
);

-- At most one attempt in flight per assignment: dispatch's supersede rule as a DB invariant.
CREATE UNIQUE INDEX assignment_builds_one_in_flight
    ON assignment_builds (assignment_id)
    WHERE finished_at IS NULL;

CREATE INDEX assignment_builds_by_image_hash ON assignment_builds (assignment_id, image_hash);

ALTER TABLE assignments
    ADD CONSTRAINT assignments_current_build_fk
    FOREIGN KEY (current_build_id) REFERENCES assignment_builds (id);
