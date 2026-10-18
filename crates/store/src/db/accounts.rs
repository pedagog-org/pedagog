//! Users, courses, course staff and enrollments, sessions and login codes.
//!
//! Emails are stored lowercased; callers may pass any case. Session tokens
//! and login codes arrive already hashed: the database never sees them.

use sqlx::PgPool;

use crate::{Course, Role, SessionKind, User};

fn normalize(email: &str) -> String {
    email.trim().to_lowercase()
}

/// Create the user, or update their name (an empty name keeps the old one).
pub async fn upsert_user(pool: &PgPool, email: &str, name: &str) -> sqlx::Result<User> {
    sqlx::query_as!(
        User,
        "INSERT INTO users (email, name) VALUES ($1, $2)
         ON CONFLICT (email) DO UPDATE
           SET name = CASE WHEN EXCLUDED.name = '' THEN users.name ELSE EXCLUDED.name END
         RETURNING id, email, name, is_admin",
        normalize(email),
        name.trim()
    )
    .fetch_one(pool)
    .await
}

pub async fn user_by_email(pool: &PgPool, email: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_as!(
        User,
        "SELECT id, email, name, is_admin FROM users WHERE email = $1",
        normalize(email)
    )
    .fetch_optional(pool)
    .await
}

/// Make a user an admin or not, creating them if needed. Returns the user.
pub async fn set_admin(pool: &PgPool, email: &str, admin: bool) -> sqlx::Result<User> {
    sqlx::query_as!(
        User,
        "INSERT INTO users (email, is_admin) VALUES ($1, $2)
         ON CONFLICT (email) DO UPDATE SET is_admin = EXCLUDED.is_admin
         RETURNING id, email, name, is_admin",
        normalize(email),
        admin
    )
    .fetch_one(pool)
    .await
}

pub async fn admins(pool: &PgPool) -> sqlx::Result<Vec<User>> {
    sqlx::query_as!(
        User,
        "SELECT id, email, name, is_admin FROM users WHERE is_admin ORDER BY email"
    )
    .fetch_all(pool)
    .await
}

/// Create a course. Returns false if the id is taken.
pub async fn create_course(pool: &PgPool, id: &str, name: &str) -> sqlx::Result<bool> {
    let done = sqlx::query!(
        "INSERT INTO courses (id, name) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING",
        id,
        name
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn course(pool: &PgPool, id: &str) -> sqlx::Result<Option<Course>> {
    sqlx::query_as!(Course, "SELECT id, name FROM courses WHERE id = $1", id)
        .fetch_optional(pool)
        .await
}

pub async fn all_courses(pool: &PgPool) -> sqlx::Result<Vec<Course>> {
    sqlx::query_as!(Course, "SELECT id, name FROM courses ORDER BY id")
        .fetch_all(pool)
        .await
}

pub async fn add_staff(pool: &PgPool, course: &str, user: i64) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO course_staff (course_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        course,
        user
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Returns false if they weren't staff.
pub async fn remove_staff(pool: &PgPool, course: &str, user: i64) -> sqlx::Result<bool> {
    let done = sqlx::query!(
        "DELETE FROM course_staff WHERE course_id = $1 AND user_id = $2",
        course,
        user
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

pub async fn staff(pool: &PgPool, course: &str) -> sqlx::Result<Vec<User>> {
    sqlx::query_as!(
        User,
        "SELECT u.id, u.email, u.name, u.is_admin
           FROM course_staff s JOIN users u ON u.id = s.user_id
          WHERE s.course_id = $1 ORDER BY u.email",
        course
    )
    .fetch_all(pool)
    .await
}

/// Enroll a student (again, if they were dropped). Returns true if they
/// weren't enrolled before.
pub async fn enroll(pool: &PgPool, course: &str, user: i64) -> sqlx::Result<bool> {
    let row = sqlx::query!(
        r#"INSERT INTO enrollments (course_id, user_id) VALUES ($1, $2)
           ON CONFLICT (course_id, user_id) DO UPDATE SET dropped_at = NULL
           RETURNING (xmax = 0) AS "new!""#,
        course,
        user
    )
    .fetch_one(pool)
    .await?;
    Ok(row.new)
}

/// Drop a student (they keep their submissions). Returns false if they
/// weren't enrolled.
pub async fn drop_student(pool: &PgPool, course: &str, user: i64) -> sqlx::Result<bool> {
    let done = sqlx::query!(
        "UPDATE enrollments SET dropped_at = now()
          WHERE course_id = $1 AND user_id = $2 AND dropped_at IS NULL",
        course,
        user
    )
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// A course's enrolled students (dropped ones left out).
pub async fn roster(pool: &PgPool, course: &str) -> sqlx::Result<Vec<User>> {
    sqlx::query_as!(
        User,
        "SELECT u.id, u.email, u.name, u.is_admin
           FROM enrollments e JOIN users u ON u.id = e.user_id
          WHERE e.course_id = $1 AND e.dropped_at IS NULL ORDER BY u.email",
        course
    )
    .fetch_all(pool)
    .await
}

/// What a user is in a course: staff wins over an enrollment.
pub async fn role_in(pool: &PgPool, course: &str, user: i64) -> sqlx::Result<Option<Role>> {
    let row = sqlx::query!(
        r#"SELECT EXISTS (SELECT 1 FROM course_staff WHERE course_id = $1 AND user_id = $2) AS "staff!",
                  EXISTS (SELECT 1 FROM enrollments
                           WHERE course_id = $1 AND user_id = $2 AND dropped_at IS NULL) AS "student!""#,
        course,
        user
    )
    .fetch_one(pool)
    .await?;
    Ok(if row.staff {
        Some(Role::Staff)
    } else if row.student {
        Some(Role::Student)
    } else {
        None
    })
}

/// The courses a user teaches or is enrolled in, with their role.
pub async fn courses_for(pool: &PgPool, user: i64) -> sqlx::Result<Vec<(Course, Role)>> {
    let rows = sqlx::query!(
        r#"SELECT c.id, c.name, 'staff' AS "role!"
             FROM course_staff s JOIN courses c ON c.id = s.course_id WHERE s.user_id = $1
           UNION
           SELECT c.id, c.name, 'student'
             FROM enrollments e JOIN courses c ON c.id = e.course_id
            WHERE e.user_id = $1 AND e.dropped_at IS NULL
              AND NOT EXISTS (SELECT 1 FROM course_staff s
                               WHERE s.course_id = c.id AND s.user_id = $1)
           ORDER BY 1"#,
        user
    )
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|r| {
            let role = r
                .role
                .parse()
                .map_err(|e: strum::ParseError| sqlx::Error::Decode(Box::new(e)))?;
            Ok((
                Course {
                    id: r.id.unwrap_or_default(),
                    name: r.name.unwrap_or_default(),
                },
                role,
            ))
        })
        .collect()
}

/// Open a session for `token_hash`, valid for `ttl_secs`.
pub async fn create_session(
    pool: &PgPool,
    token_hash: &str,
    user: i64,
    kind: SessionKind,
    ttl_secs: i64,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO sessions (token_hash, user_id, kind, expires_at)
         VALUES ($1, $2, $3, now() + make_interval(secs => $4))",
        token_hash,
        user,
        kind.to_string(),
        ttl_secs as f64
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// The user behind an unexpired session (and mark it used).
pub async fn session_user(pool: &PgPool, token_hash: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_as!(
        User,
        "UPDATE sessions s SET last_used_at = now()
           FROM users u
          WHERE s.token_hash = $1 AND s.expires_at > now() AND u.id = s.user_id
          RETURNING u.id, u.email, u.name, u.is_admin",
        token_hash
    )
    .fetch_optional(pool)
    .await
}

/// End a session (logout). Returns false if there was none.
pub async fn delete_session(pool: &PgPool, token_hash: &str) -> sqlx::Result<bool> {
    let done = sqlx::query!("DELETE FROM sessions WHERE token_hash = $1", token_hash)
        .execute(pool)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// Forget expired sessions and login codes; returns how many went.
pub async fn delete_expired(pool: &PgPool) -> sqlx::Result<u64> {
    let sessions = sqlx::query!("DELETE FROM sessions WHERE expires_at <= now()")
        .execute(pool)
        .await?
        .rows_affected();
    let codes = sqlx::query!("DELETE FROM login_codes WHERE expires_at <= now()")
        .execute(pool)
        .await?
        .rows_affected();
    Ok(sessions + codes)
}

/// Whether a new login code was stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Issued {
    Stored,
    /// One was sent less than `min_interval_secs` ago.
    TooSoon,
}

/// Store a new login code for `email` (replacing an older one, and its
/// tries), unless one was sent too recently.
pub async fn issue_code(
    pool: &PgPool,
    email: &str,
    code_hash: &str,
    ttl_secs: i64,
    min_interval_secs: i64,
) -> sqlx::Result<Issued> {
    let done = sqlx::query!(
        "INSERT INTO login_codes (email, code_hash, expires_at)
         VALUES ($1, $2, now() + make_interval(secs => $3))
         ON CONFLICT (email) DO UPDATE
           SET code_hash = EXCLUDED.code_hash, attempts = 0, sent_at = now(),
               expires_at = EXCLUDED.expires_at
         WHERE login_codes.sent_at <= now() - make_interval(secs => $4)",
        normalize(email),
        code_hash,
        ttl_secs as f64,
        min_interval_secs as f64
    )
    .execute(pool)
    .await?;
    Ok(if done.rows_affected() == 1 {
        Issued::Stored
    } else {
        Issued::TooSoon
    })
}

/// The outcome of trying a login code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeCheck {
    /// Right: the code is used up.
    Valid,
    /// Wrong; tries left before the code is void.
    Wrong { left: i32 },
    /// No code, expired, or out of tries: start again.
    Void,
}

/// Try a login code. A right one is consumed; a wrong one counts against
/// `max_attempts`, after which the code is void.
pub async fn check_code(
    pool: &PgPool,
    email: &str,
    code_hash: &str,
    max_attempts: i32,
) -> sqlx::Result<CodeCheck> {
    let email = normalize(email);
    let mut tx = pool.begin().await?;
    let row = sqlx::query!(
        r#"SELECT code_hash, attempts, expires_at > now() AS "live!"
             FROM login_codes WHERE email = $1 FOR UPDATE"#,
        email
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        return Ok(CodeCheck::Void);
    };
    let outcome = if !row.live || row.attempts >= max_attempts {
        CodeCheck::Void
    } else if row.code_hash == code_hash {
        CodeCheck::Valid
    } else {
        let left = max_attempts - row.attempts - 1;
        if left > 0 {
            sqlx::query!(
                "UPDATE login_codes SET attempts = attempts + 1 WHERE email = $1",
                email
            )
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(CodeCheck::Wrong { left });
        }
        CodeCheck::Void
    };
    sqlx::query!("DELETE FROM login_codes WHERE email = $1", email)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn users_are_found_by_email_in_any_case(pool: PgPool) -> sqlx::Result<()> {
        let u = upsert_user(&pool, " Albert.Gator@UFL.edu ", "Albert Gator").await?;
        assert_eq!(u.email, "albert.gator@ufl.edu");
        let again = upsert_user(&pool, "albert.gator@ufl.edu", "").await?;
        assert_eq!(
            (again.id, again.name.as_str()),
            (u.id, "Albert Gator"),
            "an empty name keeps the old one"
        );
        assert_eq!(
            user_by_email(&pool, "ALBERT.GATOR@ufl.edu").await?,
            Some(again)
        );
        assert_eq!(user_by_email(&pool, "nobody@ufl.edu").await?, None);
        Ok(())
    }

    #[sqlx::test]
    async fn admins_are_set_and_listed(pool: PgPool) -> sqlx::Result<()> {
        let a = set_admin(&pool, "conder@ufl.edu", true).await?;
        assert!(a.is_admin);
        upsert_user(&pool, "student@ufl.edu", "S").await?;
        assert_eq!(admins(&pool).await?, std::slice::from_ref(&a));
        set_admin(&pool, "conder@ufl.edu", false).await?;
        assert!(admins(&pool).await?.is_empty());
        Ok(())
    }

    #[sqlx::test]
    async fn courses_staff_and_roster(pool: PgPool) -> sqlx::Result<()> {
        assert!(create_course(&pool, "cop3502-f26", "COP3502 (Fall 2026)").await?);
        assert!(!create_course(&pool, "cop3502-f26", "again").await?);
        let prof = upsert_user(&pool, "prof@ufl.edu", "Prof").await?;
        let alice = upsert_user(&pool, "alice@ufl.edu", "Alice").await?;
        add_staff(&pool, "cop3502-f26", prof.id).await?;
        assert!(enroll(&pool, "cop3502-f26", alice.id).await?);
        assert!(
            !enroll(&pool, "cop3502-f26", alice.id).await?,
            "already enrolled"
        );

        assert_eq!(
            role_in(&pool, "cop3502-f26", prof.id).await?,
            Some(Role::Staff)
        );
        assert_eq!(
            role_in(&pool, "cop3502-f26", alice.id).await?,
            Some(Role::Student)
        );
        assert_eq!(
            staff(&pool, "cop3502-f26").await?,
            std::slice::from_ref(&prof)
        );
        assert_eq!(
            roster(&pool, "cop3502-f26").await?,
            std::slice::from_ref(&alice)
        );
        let mine = courses_for(&pool, alice.id).await?;
        assert_eq!(mine[0].1, Role::Student);
        assert_eq!(mine[0].0.id, "cop3502-f26");

        assert!(drop_student(&pool, "cop3502-f26", alice.id).await?);
        assert_eq!(role_in(&pool, "cop3502-f26", alice.id).await?, None);
        assert!(roster(&pool, "cop3502-f26").await?.is_empty());
        assert!(courses_for(&pool, alice.id).await?.is_empty());
        assert!(
            !enroll(&pool, "cop3502-f26", alice.id).await?,
            "re-enrolled, not new"
        );
        assert_eq!(
            role_in(&pool, "cop3502-f26", alice.id).await?,
            Some(Role::Student)
        );
        Ok(())
    }

    #[sqlx::test]
    async fn course_ids_are_slugs(pool: PgPool) -> sqlx::Result<()> {
        for bad in ["Bad Id", "-cop", "cop_3502", ""] {
            assert!(create_course(&pool, bad, "x").await.is_err(), "{bad:?}");
        }
        assert!(create_course(&pool, "cop3502-f26", "ok").await?);
        Ok(())
    }

    #[sqlx::test]
    async fn sessions_find_their_user_until_they_expire(pool: PgPool) -> sqlx::Result<()> {
        let u = upsert_user(&pool, "alice@ufl.edu", "Alice").await?;
        create_session(&pool, "hash-a", u.id, SessionKind::Cli, 3600).await?;
        create_session(&pool, "hash-old", u.id, SessionKind::Web, -1).await?;
        assert_eq!(
            session_user(&pool, "hash-a").await?.map(|u| u.id),
            Some(u.id)
        );
        assert_eq!(session_user(&pool, "hash-old").await?, None, "expired");
        assert_eq!(session_user(&pool, "unknown").await?, None);
        assert_eq!(delete_expired(&pool).await?, 1);
        assert!(delete_session(&pool, "hash-a").await?);
        assert_eq!(session_user(&pool, "hash-a").await?, None);
        Ok(())
    }

    #[sqlx::test]
    async fn codes_are_rate_limited_and_used_once(pool: PgPool) -> sqlx::Result<()> {
        assert_eq!(
            issue_code(&pool, "alice@ufl.edu", "h1", 600, 60).await?,
            Issued::Stored
        );
        assert_eq!(
            issue_code(&pool, "ALICE@ufl.edu", "h2", 600, 60).await?,
            Issued::TooSoon
        );
        assert_eq!(
            check_code(&pool, "alice@ufl.edu", "h1", 5).await?,
            CodeCheck::Valid
        );
        assert_eq!(
            check_code(&pool, "alice@ufl.edu", "h1", 5).await?,
            CodeCheck::Void,
            "used up"
        );
        Ok(())
    }

    #[sqlx::test]
    async fn wrong_codes_count_down_then_void(pool: PgPool) -> sqlx::Result<()> {
        issue_code(&pool, "alice@ufl.edu", "right", 600, 0).await?;
        assert_eq!(
            check_code(&pool, "alice@ufl.edu", "x", 3).await?,
            CodeCheck::Wrong { left: 2 }
        );
        assert_eq!(
            check_code(&pool, "alice@ufl.edu", "x", 3).await?,
            CodeCheck::Wrong { left: 1 }
        );
        assert_eq!(
            check_code(&pool, "alice@ufl.edu", "x", 3).await?,
            CodeCheck::Void
        );
        assert_eq!(
            check_code(&pool, "alice@ufl.edu", "right", 3).await?,
            CodeCheck::Void,
            "voided"
        );

        issue_code(&pool, "bob@ufl.edu", "right", -1, 0).await?;
        assert_eq!(
            check_code(&pool, "bob@ufl.edu", "right", 3).await?,
            CodeCheck::Void,
            "expired"
        );
        Ok(())
    }
}
