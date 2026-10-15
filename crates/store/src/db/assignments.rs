//! `assignments`, `assignment_uploads` and `assignment_builds` queries.
//!
//! A build attempt moves through `build_status` (running → succeeded | failed |
//! superseded) and then `validation_status` (pending → running → passed | failed |
//! skipped | invalid); `finished_at` marks the whole attempt as over. Writes that
//! finish a stage only apply to attempts still in flight, so a superseded attempt
//! can't be overwritten by its late-finishing Job.

use std::str::FromStr;

use sqlx::PgPool;

use crate::{AssignmentBuild, AssignmentBuildStatus, AssignmentUpload, ValidationStatus};

/// Create the assignment, or update its name.
pub async fn upsert_assignment(pool: &PgPool, id: &str, name: &str) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO assignments (id, name) VALUES ($1, $2)
         ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, updated_at = now()",
        id,
        name
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Record an uploaded tree (re-uploading the same tree refreshes `uploaded_at`).
pub async fn record_upload(
    pool: &PgPool,
    assignment_id: &str,
    tree_hash: &str,
    object_key: &str,
    size_bytes: i64,
) -> sqlx::Result<AssignmentUpload> {
    let row = sqlx::query!(
        "INSERT INTO assignment_uploads (assignment_id, tree_hash, object_key, size_bytes)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (assignment_id, tree_hash) DO UPDATE
           SET object_key = EXCLUDED.object_key,
               size_bytes = EXCLUDED.size_bytes,
               uploaded_at = now()
         RETURNING id",
        assignment_id,
        tree_hash,
        object_key,
        size_bytes
    )
    .fetch_one(pool)
    .await?;
    Ok(AssignmentUpload {
        id: row.id,
        assignment_id: assignment_id.into(),
        tree_hash: tree_hash.into(),
        object_key: object_key.into(),
        size_bytes,
    })
}

/// The latest attempt for this image hash that succeeded with a usable
/// validation (passed or skipped): an upload that needs no rebuild.
pub async fn usable_build_for(
    pool: &PgPool,
    assignment_id: &str,
    image_hash: &str,
) -> sqlx::Result<Option<i64>> {
    let row = sqlx::query!(
        "SELECT id FROM assignment_builds
          WHERE assignment_id = $1 AND image_hash = $2
            AND build_status = 'succeeded' AND validation_status IN ('passed', 'skipped')
          ORDER BY id DESC
          LIMIT 1",
        assignment_id,
        image_hash
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.id))
}

/// Open a build attempt (`build_status = running`, validation pending).
///
/// Fails with a unique violation if the assignment already has an attempt in
/// flight; supersede it first.
pub async fn start_build(
    pool: &PgPool,
    assignment_id: &str,
    upload_id: i64,
    image_hash: &str,
    image_ref: &str,
) -> sqlx::Result<i64> {
    let row = sqlx::query!(
        "INSERT INTO assignment_builds (assignment_id, upload_id, image_hash, image_ref, build_status)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id",
        assignment_id,
        upload_id,
        image_hash,
        image_ref,
        AssignmentBuildStatus::Running.to_string()
    )
    .fetch_one(pool)
    .await?;
    Ok(row.id)
}

/// Record the image build's outcome. A failed build also ends the attempt.
/// Returns false if the attempt is no longer in flight (e.g. superseded).
pub async fn finish_build(
    pool: &PgPool,
    build: i64,
    succeeded: bool,
    error: Option<&str>,
) -> sqlx::Result<bool> {
    let status = if succeeded {
        AssignmentBuildStatus::Succeeded
    } else {
        AssignmentBuildStatus::Failed
    };
    let done = sqlx::query!(
        "UPDATE assignment_builds
            SET build_status = $2, error = $3,
                finished_at = CASE WHEN $4 THEN NULL ELSE now() END
          WHERE id = $1 AND finished_at IS NULL AND build_status = 'running'",
        build,
        status.to_string(),
        error,
        succeeded
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Mark validation as running. Returns false if the attempt is no longer in flight.
pub async fn start_validation(pool: &PgPool, build: i64) -> sqlx::Result<bool> {
    let done = sqlx::query!(
        "UPDATE assignment_builds SET validation_status = $2
          WHERE id = $1 AND finished_at IS NULL AND build_status = 'succeeded'",
        build,
        ValidationStatus::Running.to_string()
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Record validation's outcome and end the attempt. `results` is the parsed
/// results as JSON text (`None` for skipped and invalid). A usable outcome
/// (passed or skipped) makes this the assignment's current build, atomically.
/// Returns false if the attempt is no longer in flight.
pub async fn finish_validation(
    pool: &PgPool,
    build: i64,
    status: ValidationStatus,
    results: Option<&str>,
    problems: &[String],
) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let row = sqlx::query!(
        "UPDATE assignment_builds
            SET validation_status = $2, results = $3::text::jsonb,
                validation_problems = $4, finished_at = now()
          WHERE id = $1 AND finished_at IS NULL AND build_status = 'succeeded'
          RETURNING assignment_id",
        build,
        status.to_string(),
        results,
        problems
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(false);
    };
    if status.is_usable() {
        sqlx::query!(
            "UPDATE assignments SET current_build_id = $2, updated_at = now() WHERE id = $1",
            row.assignment_id,
            build
        )
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Cancel an in-flight attempt because a newer upload replaces it.
/// Returns false if it had already finished.
pub async fn supersede(pool: &PgPool, build: i64) -> sqlx::Result<bool> {
    let done = sqlx::query!(
        "UPDATE assignment_builds SET build_status = $2, finished_at = now()
          WHERE id = $1 AND finished_at IS NULL",
        build,
        AssignmentBuildStatus::Superseded.to_string()
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// The assignment's attempt in flight, if any.
pub async fn in_flight_for(
    pool: &PgPool,
    assignment_id: &str,
) -> sqlx::Result<Option<AssignmentBuild>> {
    sqlx::query_as!(
        BuildRow,
        r#"SELECT id, assignment_id, upload_id, image_hash, image_ref, build_status,
                  validation_status, results::text AS "results?", validation_problems,
                  error, finished_at IS NOT NULL AS "finished!"
             FROM assignment_builds
            WHERE assignment_id = $1 AND finished_at IS NULL"#,
        assignment_id
    )
    .fetch_optional(pool)
    .await?
    .map(AssignmentBuild::try_from)
    .transpose()
}

/// Every attempt in flight: what reconcile adopts after a restart.
pub async fn in_flight(pool: &PgPool) -> sqlx::Result<Vec<AssignmentBuild>> {
    sqlx::query_as!(
        BuildRow,
        r#"SELECT id, assignment_id, upload_id, image_hash, image_ref, build_status,
                  validation_status, results::text AS "results?", validation_problems,
                  error, finished_at IS NOT NULL AS "finished!"
             FROM assignment_builds
            WHERE finished_at IS NULL
            ORDER BY started_at"#
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(AssignmentBuild::try_from)
    .collect()
}

/// One attempt of an assignment (for the status endpoint).
pub async fn get_build(
    pool: &PgPool,
    assignment_id: &str,
    build: i64,
) -> sqlx::Result<Option<AssignmentBuild>> {
    sqlx::query_as!(
        BuildRow,
        r#"SELECT id, assignment_id, upload_id, image_hash, image_ref, build_status,
                  validation_status, results::text AS "results?", validation_problems,
                  error, finished_at IS NOT NULL AS "finished!"
             FROM assignment_builds
            WHERE assignment_id = $1 AND id = $2"#,
        assignment_id,
        build
    )
    .fetch_optional(pool)
    .await?
    .map(AssignmentBuild::try_from)
    .transpose()
}

/// Uploads that never got a build attempt (jobs stopped between storing and
/// dispatching): reconcile dispatches them.
pub async fn uploads_without_builds(pool: &PgPool) -> sqlx::Result<Vec<AssignmentUpload>> {
    let rows = sqlx::query_as!(
        AssignmentUpload,
        "SELECT u.id, u.assignment_id, u.tree_hash, u.object_key, u.size_bytes
           FROM assignment_uploads u
          WHERE NOT EXISTS (SELECT 1 FROM assignment_builds b WHERE b.upload_id = u.id)
          ORDER BY u.uploaded_at"
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// One upload by id.
pub async fn get_upload(pool: &PgPool, upload: i64) -> sqlx::Result<Option<AssignmentUpload>> {
    sqlx::query_as!(
        AssignmentUpload,
        "SELECT id, assignment_id, tree_hash, object_key, size_bytes
           FROM assignment_uploads WHERE id = $1",
        upload
    )
    .fetch_optional(pool)
    .await
}

/// The assignment's current (usable) build, if any.
pub async fn current_build_id(pool: &PgPool, assignment_id: &str) -> sqlx::Result<Option<i64>> {
    let row = sqlx::query!(
        "SELECT current_build_id FROM assignments WHERE id = $1",
        assignment_id
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|r| r.current_build_id))
}

/// `assignment_builds` as stored; statuses are parsed into their enums.
struct BuildRow {
    id: i64,
    assignment_id: String,
    upload_id: i64,
    image_hash: String,
    image_ref: String,
    build_status: String,
    validation_status: String,
    results: Option<String>,
    validation_problems: Vec<String>,
    error: Option<String>,
    finished: bool,
}

impl TryFrom<BuildRow> for AssignmentBuild {
    type Error = sqlx::Error;

    fn try_from(row: BuildRow) -> sqlx::Result<Self> {
        let decode = |e: strum::ParseError| sqlx::Error::Decode(Box::new(e));
        Ok(Self {
            id: row.id,
            assignment_id: row.assignment_id,
            upload_id: row.upload_id,
            image_hash: row.image_hash,
            image_ref: row.image_ref,
            build_status: AssignmentBuildStatus::from_str(&row.build_status).map_err(decode)?,
            validation_status: ValidationStatus::from_str(&row.validation_status)
                .map_err(decode)?,
            results: row.results,
            validation_problems: row.validation_problems,
            error: row.error,
            finished: row.finished,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn uploaded(pool: &PgPool, id: &str, tree: &str) -> sqlx::Result<AssignmentUpload> {
        upsert_assignment(pool, id, "Linked List").await?;
        record_upload(
            pool,
            id,
            tree,
            &format!("assignments/{id}/{tree}.tgz"),
            1234,
        )
        .await
    }

    #[sqlx::test]
    async fn uploads_are_one_row_per_tree(pool: PgPool) -> sqlx::Result<()> {
        let a = uploaded(&pool, "ll", "tree-a").await?;
        let again = uploaded(&pool, "ll", "tree-a").await?;
        let b = uploaded(&pool, "ll", "tree-b").await?;
        assert_eq!(a.id, again.id);
        assert_eq!(get_upload(&pool, b.id).await?.as_ref(), Some(&b));
        assert_eq!(get_upload(&pool, 999).await?, None);
        assert_ne!(a.id, b.id);
        assert_eq!(uploads_without_builds(&pool).await?.len(), 2);
        start_build(&pool, "ll", a.id, "img-a", "reg/a:1").await?;
        let orphans = uploads_without_builds(&pool).await?;
        assert_eq!(orphans, [b]);
        Ok(())
    }

    #[sqlx::test]
    async fn passing_validation_becomes_current(pool: PgPool) -> sqlx::Result<()> {
        let up = uploaded(&pool, "ll", "tree-a").await?;
        let build = start_build(&pool, "ll", up.id, "img-a", "reg/a:1").await?;
        let running = in_flight_for(&pool, "ll").await?.unwrap();
        assert_eq!(
            (
                running.build_status,
                running.validation_status,
                running.finished
            ),
            (
                AssignmentBuildStatus::Running,
                ValidationStatus::Pending,
                false
            )
        );

        assert!(finish_build(&pool, build, true, None).await?);
        assert!(start_validation(&pool, build).await?);
        let results = r#"{"summary": {"score": 10, "total": 10}}"#;
        assert!(
            finish_validation(&pool, build, ValidationStatus::Passed, Some(results), &[]).await?
        );

        let done = get_build(&pool, "ll", build).await?.unwrap();
        assert!(done.finished);
        assert_eq!(done.validation_status, ValidationStatus::Passed);
        assert!(done.results.unwrap().contains(r#""total": 10"#));
        assert_eq!(current_build_id(&pool, "ll").await?, Some(build));
        assert_eq!(usable_build_for(&pool, "ll", "img-a").await?, Some(build));
        assert!(in_flight(&pool).await?.is_empty());
        Ok(())
    }

    #[sqlx::test]
    async fn failures_never_replace_the_current_build(pool: PgPool) -> sqlx::Result<()> {
        let up = uploaded(&pool, "ll", "tree-a").await?;
        let good = start_build(&pool, "ll", up.id, "img-a", "reg/a:1").await?;
        finish_build(&pool, good, true, None).await?;
        finish_validation(&pool, good, ValidationStatus::Skipped, None, &[]).await?;

        // A failed validation keeps its results and problems but isn't usable.
        let bad = start_build(&pool, "ll", up.id, "img-b", "reg/a:2").await?;
        finish_build(&pool, bad, true, None).await?;
        let problems = vec!["append: expected the solution to pass, but it failed".to_string()];
        finish_validation(&pool, bad, ValidationStatus::Failed, Some("{}"), &problems).await?;
        let row = get_build(&pool, "ll", bad).await?.unwrap();
        assert_eq!(row.validation_problems, problems);
        assert_eq!(usable_build_for(&pool, "ll", "img-b").await?, None);

        // A failed image build ends the attempt without validating.
        let broken = start_build(&pool, "ll", up.id, "img-c", "reg/a:3").await?;
        finish_build(&pool, broken, false, Some("kaniko: exit 1")).await?;
        let row = get_build(&pool, "ll", broken).await?.unwrap();
        assert!(row.finished);
        assert_eq!(row.validation_status, ValidationStatus::Pending);
        assert_eq!(row.error.as_deref(), Some("kaniko: exit 1"));

        assert_eq!(current_build_id(&pool, "ll").await?, Some(good));
        Ok(())
    }

    #[sqlx::test]
    async fn one_attempt_in_flight_and_supersede(pool: PgPool) -> sqlx::Result<()> {
        let up = uploaded(&pool, "ll", "tree-a").await?;
        let first = start_build(&pool, "ll", up.id, "img-a", "reg/a:1").await?;
        assert!(
            start_build(&pool, "ll", up.id, "img-b", "reg/a:2")
                .await
                .is_err()
        );

        assert!(supersede(&pool, first).await?);
        assert!(!supersede(&pool, first).await?, "already finished");
        // The superseded attempt's late Job can't overwrite it.
        assert!(!finish_build(&pool, first, true, None).await?);
        assert!(!start_validation(&pool, first).await?);
        assert!(!finish_validation(&pool, first, ValidationStatus::Passed, None, &[]).await?);
        let row = get_build(&pool, "ll", first).await?.unwrap();
        assert_eq!(row.build_status, AssignmentBuildStatus::Superseded);
        assert_eq!(current_build_id(&pool, "ll").await?, None);

        let second = start_build(&pool, "ll", up.id, "img-b", "reg/a:2").await?;
        assert_eq!(in_flight(&pool).await?[0].id, second);
        Ok(())
    }

    #[sqlx::test]
    async fn statuses_outside_the_check_are_rejected(pool: PgPool) -> sqlx::Result<()> {
        let up = uploaded(&pool, "ll", "tree-a").await?;
        let bad = sqlx::query(
            "INSERT INTO assignment_builds (assignment_id, upload_id, image_hash, image_ref, build_status)
             VALUES ('ll', $1, 'h', 'r', 'bogus')",
        )
        .bind(up.id)
        .execute(&pool)
        .await;
        assert!(bad.is_err());
        Ok(())
    }
}
