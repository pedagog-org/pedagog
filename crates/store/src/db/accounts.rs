//! Users and global roles, courses, memberships, sessions and login codes.
//!
//! Emails are stored lowercased; callers may pass any case. Session tokens
//! and login codes arrive already hashed: the database never sees them.
//! Times come back as RFC 3339 strings in UTC.

use sqlx::PgPool;

use crate::{Course, CourseRole, GlobalRole, JoinSource, Membership, SessionKind, User};

fn normalize(email: &str) -> String {
    email.trim().to_lowercase()
}

fn decode(e: strum::ParseError) -> sqlx::Error {
    sqlx::Error::Decode(Box::new(e))
}

/// A case-insensitive `LIKE` pattern matching `q` anywhere.
fn anywhere(q: &str) -> String {
    let escaped = q
        .trim()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

struct UserRow {
    id: i64,
    email: String,
    name: String,
    role: String,
}

impl TryFrom<UserRow> for User {
    type Error = sqlx::Error;

    fn try_from(r: UserRow) -> sqlx::Result<Self> {
        Ok(User {
            id: r.id,
            email: r.email,
            name: r.name,
            role: r.role.parse().map_err(decode)?,
        })
    }
}

/// Create the user, or update their name (an empty name keeps the old one).
pub async fn upsert_user(pool: &PgPool, email: &str, name: &str) -> sqlx::Result<User> {
    sqlx::query_as!(
        UserRow,
        "INSERT INTO users (email, name) VALUES ($1, $2)
         ON CONFLICT (email) DO UPDATE
           SET name = CASE WHEN EXCLUDED.name = '' THEN users.name ELSE EXCLUDED.name END
         RETURNING id, email, name, role",
        normalize(email),
        name.trim()
    )
    .fetch_one(pool)
    .await?
    .try_into()
}

pub async fn user_by_email(pool: &PgPool, email: &str) -> sqlx::Result<Option<User>> {
    sqlx::query_as!(
        UserRow,
        "SELECT id, email, name, role FROM users WHERE email = $1",
        normalize(email)
    )
    .fetch_optional(pool)
    .await?
    .map(User::try_from)
    .transpose()
}

/// What setting a global role did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoleSet {
    /// `from` is `None` if the user was created.
    Set {
        user: User,
        from: Option<GlobalRole>,
    },
    /// Refused: they're the last admin.
    LastAdmin,
}

/// Give a user a global role, creating them if needed. The last admin stays.
pub async fn set_role(pool: &PgPool, email: &str, role: GlobalRole) -> sqlx::Result<RoleSet> {
    let email = normalize(email);
    let mut tx = pool.begin().await?;
    super::lock(&mut tx, "admins").await?;
    let before = sqlx::query_scalar!("SELECT role FROM users WHERE email = $1", email)
        .fetch_optional(&mut *tx)
        .await?
        .map(|r| r.parse::<GlobalRole>().map_err(decode))
        .transpose()?;
    if before == Some(GlobalRole::Admin) && role != GlobalRole::Admin {
        let admins =
            sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM users WHERE role = 'admin'"#)
                .fetch_one(&mut *tx)
                .await?;
        if admins <= 1 {
            return Ok(RoleSet::LastAdmin);
        }
    }
    let user: User = sqlx::query_as!(
        UserRow,
        "INSERT INTO users (email, role) VALUES ($1, $2)
         ON CONFLICT (email) DO UPDATE SET role = EXCLUDED.role
         RETURNING id, email, name, role",
        email,
        role.to_string()
    )
    .fetch_one(&mut *tx)
    .await?
    .try_into()?;
    tx.commit().await?;
    Ok(RoleSet::Set { user, from: before })
}

/// One page of users, optionally with one global role and matching `search`
/// (part of an email or name; an exact email comes first), and how many
/// match in all.
pub async fn find_users(
    pool: &PgPool,
    role: Option<GlobalRole>,
    search: Option<&str>,
    limit: i64,
    offset: i64,
) -> sqlx::Result<(Vec<User>, i64)> {
    let role = role.map(|r| r.to_string());
    let (exact, pattern) = (search.map(normalize), search.map(anywhere));
    let page = sqlx::query_as!(
        UserRow,
        r#"SELECT id, email, name, role FROM users
            WHERE ($1::text IS NULL OR role = $1)
              AND ($2::text IS NULL OR email = $2
                   OR email ILIKE $3 ESCAPE '\' OR name ILIKE $3 ESCAPE '\')
            ORDER BY COALESCE(email = $2, false) DESC, email
            LIMIT $4 OFFSET $5"#,
        role,
        exact,
        pattern,
        limit,
        offset
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(User::try_from)
    .collect::<sqlx::Result<_>>()?;
    let total = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM users
            WHERE ($1::text IS NULL OR role = $1)
              AND ($2::text IS NULL OR email = $2
                   OR email ILIKE $3 ESCAPE '\' OR name ILIKE $3 ESCAPE '\')"#,
        role,
        exact,
        pattern
    )
    .fetch_one(pool)
    .await?;
    Ok((page, total))
}

/// A user's account and logins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    pub created_at: String,
    pub last_login_at: Option<String>,
    pub last_login_kind: Option<SessionKind>,
    pub web_sessions: i64,
    pub cli_sessions: i64,
}

pub async fn activity(pool: &PgPool, user: i64) -> sqlx::Result<Activity> {
    let r = sqlx::query!(
        r#"SELECT to_char(u.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS "created_at!",
                  to_char(u.last_login_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS last_login_at,
                  u.last_login_kind,
                  (SELECT count(*) FROM sessions s
                    WHERE s.user_id = u.id AND s.kind = 'web' AND s.expires_at > now()) AS "web!",
                  (SELECT count(*) FROM sessions s
                    WHERE s.user_id = u.id AND s.kind = 'cli' AND s.expires_at > now()) AS "cli!"
             FROM users u WHERE u.id = $1"#,
        user
    )
    .fetch_one(pool)
    .await?;
    Ok(Activity {
        created_at: r.created_at,
        last_login_at: r.last_login_at,
        last_login_kind: r
            .last_login_kind
            .map(|k| k.parse().map_err(decode))
            .transpose()?,
        web_sessions: r.web,
        cli_sessions: r.cli,
    })
}

/// Note a login (when sessions open).
pub async fn record_login(pool: &PgPool, user: i64, kind: SessionKind) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE users SET last_login_at = now(), last_login_kind = $2 WHERE id = $1",
        user,
        kind.to_string()
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Sign a user out everywhere; returns how many sessions ended.
pub async fn end_sessions(pool: &PgPool, user: i64) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "DELETE FROM sessions WHERE user_id = $1 AND expires_at > now()",
        user
    )
    .execute(pool)
    .await?
    .rows_affected())
}

/// Create a course with its first instructors. Returns false if the id is taken.
pub async fn create_course(
    pool: &PgPool,
    id: &str,
    name: &str,
    by: i64,
    instructors: &[i64],
) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let done = sqlx::query!(
        "INSERT INTO courses (id, name) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING",
        id,
        name
    )
    .execute(&mut *tx)
    .await?;
    if done.rows_affected() == 0 {
        return Ok(false);
    }
    sqlx::query!(
        "INSERT INTO memberships (course_id, user_id, role, source, added_by)
         SELECT $1, u, 'instructor', 'created', $3 FROM UNNEST($2::bigint[]) AS u
         ON CONFLICT DO NOTHING",
        id,
        instructors,
        by
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn course(pool: &PgPool, id: &str) -> sqlx::Result<Option<Course>> {
    sqlx::query_as!(Course, "SELECT id, name FROM courses WHERE id = $1", id)
        .fetch_optional(pool)
        .await
}

/// One page of every course (optionally matching part of the id or name),
/// and how many match in all.
pub async fn all_courses(
    pool: &PgPool,
    search: Option<&str>,
    limit: i64,
    offset: i64,
) -> sqlx::Result<(Vec<Course>, i64)> {
    let pattern = search.map(anywhere);
    let page = sqlx::query_as!(
        Course,
        r#"SELECT id, name FROM courses
            WHERE $1::text IS NULL OR id ILIKE $1 ESCAPE '\' OR name ILIKE $1 ESCAPE '\'
            ORDER BY id LIMIT $2 OFFSET $3"#,
        pattern,
        limit,
        offset
    )
    .fetch_all(pool)
    .await?;
    let total = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM courses
            WHERE $1::text IS NULL OR id ILIKE $1 ESCAPE '\' OR name ILIKE $1 ESCAPE '\'"#,
        pattern
    )
    .fetch_one(pool)
    .await?;
    Ok((page, total))
}

/// The courses a user is an active member of, with their role.
pub async fn courses_for(pool: &PgPool, user: i64) -> sqlx::Result<Vec<(Course, CourseRole)>> {
    sqlx::query!(
        "SELECT c.id, c.name, m.role
           FROM memberships m JOIN courses c ON c.id = m.course_id
          WHERE m.user_id = $1 AND m.dropped_at IS NULL
          ORDER BY c.id",
        user
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| {
        Ok((
            Course {
                id: r.id,
                name: r.name,
            },
            r.role.parse().map_err(decode)?,
        ))
    })
    .collect()
}

/// A user's active role in a course.
pub async fn role_in(pool: &PgPool, course: &str, user: i64) -> sqlx::Result<Option<CourseRole>> {
    sqlx::query_scalar!(
        "SELECT role FROM memberships
          WHERE course_id = $1 AND user_id = $2 AND dropped_at IS NULL",
        course,
        user
    )
    .fetch_optional(pool)
    .await?
    .map(|r| r.parse().map_err(decode))
    .transpose()
}

struct MemberRow {
    course_id: String,
    course_name: String,
    user_id: i64,
    email: String,
    name: String,
    user_role: String,
    role: String,
    source: String,
    added_by: Option<String>,
    joined_at: String,
    dropped_at: Option<String>,
}

impl TryFrom<MemberRow> for Membership {
    type Error = sqlx::Error;

    fn try_from(r: MemberRow) -> sqlx::Result<Self> {
        Ok(Membership {
            course: Course {
                id: r.course_id,
                name: r.course_name,
            },
            user: User {
                id: r.user_id,
                email: r.email,
                name: r.name,
                role: r.user_role.parse().map_err(decode)?,
            },
            role: r.role.parse().map_err(decode)?,
            source: r.source.parse().map_err(decode)?,
            added_by: r.added_by,
            joined_at: r.joined_at,
            dropped_at: r.dropped_at,
        })
    }
}

macro_rules! select_members {
    ($tail:literal $(, $arg:expr)* $(,)?) => {
        sqlx::query_as!(
            MemberRow,
            r#"SELECT m.course_id, c.name AS course_name, u.id AS user_id, u.email, u.name,
                      u.role AS user_role, m.role, m.source, a.email AS "added_by?",
                      to_char(m.joined_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS "joined_at!",
                      to_char(m.dropped_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS dropped_at
                 FROM memberships m
                 JOIN courses c ON c.id = m.course_id
                 JOIN users u ON u.id = m.user_id
                 LEFT JOIN users a ON a.id = m.added_by "# + $tail
            $(, $arg)*
        )
    };
}

fn memberships(rows: Vec<MemberRow>) -> sqlx::Result<Vec<Membership>> {
    rows.into_iter().map(Membership::try_from).collect()
}

/// A course's members, optionally of one role, by email; dropped ones only
/// with `with_dropped`.
pub async fn members(
    pool: &PgPool,
    course: &str,
    role: Option<CourseRole>,
    with_dropped: bool,
) -> sqlx::Result<Vec<Membership>> {
    memberships(
        select_members!(
            "WHERE m.course_id = $1 AND ($2::text IS NULL OR m.role = $2)
               AND ($3 OR m.dropped_at IS NULL)
             ORDER BY u.email",
            course,
            role.map(|r| r.to_string()),
            with_dropped
        )
        .fetch_all(pool)
        .await?,
    )
}

/// Members (dropped ones too) matching part of an email or name; an exact
/// email match comes first.
pub async fn find_members(
    pool: &PgPool,
    course: &str,
    query: &str,
    limit: i64,
) -> sqlx::Result<Vec<Membership>> {
    memberships(
        select_members!(
            r#"WHERE m.course_id = $1
                 AND (u.email = $2 OR u.email ILIKE $3 ESCAPE '\' OR u.name ILIKE $3 ESCAPE '\')
               ORDER BY (u.email = $2) DESC, u.email
               LIMIT $4"#,
            course,
            normalize(query),
            anywhere(query),
            limit
        )
        .fetch_all(pool)
        .await?,
    )
}

/// Every course a user is or was in.
pub async fn memberships_of(pool: &PgPool, user: i64) -> sqlx::Result<Vec<Membership>> {
    memberships(
        select_members!(
            "WHERE m.user_id = $1 ORDER BY m.dropped_at IS NOT NULL, c.id",
            user
        )
        .fetch_all(pool)
        .await?,
    )
}

/// Who's changing a membership, for the instructor rules.
#[derive(Clone, Copy, Debug)]
pub struct Actor {
    pub id: i64,
    pub admin: bool,
}

/// Why a membership change was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The course keeps at least one active instructor.
    LastInstructor,
    /// Only admins remove or demote another instructor.
    OtherInstructor,
}

/// What adding a member did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Joined {
    New,
    /// They had been dropped.
    Restored,
    Changed {
        from: CourseRole,
    },
    Unchanged,
}

/// The instructor rules, for removing or demoting `user`, an active
/// instructor. Call with the course's members lock held.
async fn may_unseat(
    conn: &mut sqlx::PgConnection,
    course: &str,
    user: i64,
    actor: Actor,
) -> sqlx::Result<Option<Refused>> {
    if user != actor.id && !actor.admin {
        return Ok(Some(Refused::OtherInstructor));
    }
    let instructors = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM memberships
            WHERE course_id = $1 AND role = 'instructor' AND dropped_at IS NULL"#,
        course
    )
    .fetch_one(conn)
    .await?;
    Ok((instructors <= 1).then_some(Refused::LastInstructor))
}

/// Add `user` to a course as `role`: new, restored (if dropped), or with
/// their role changed.
pub async fn add_member(
    pool: &PgPool,
    course: &str,
    user: i64,
    role: CourseRole,
    source: JoinSource,
    actor: Actor,
) -> sqlx::Result<Result<Joined, Refused>> {
    let mut tx = pool.begin().await?;
    super::lock(&mut tx, &format!("members:{course}")).await?;
    let current = sqlx::query!(
        r#"SELECT role, dropped_at IS NOT NULL AS "dropped!" FROM memberships
            WHERE course_id = $1 AND user_id = $2"#,
        course,
        user
    )
    .fetch_optional(&mut *tx)
    .await?;
    let joined = match current {
        None => {
            sqlx::query!(
                "INSERT INTO memberships (course_id, user_id, role, source, added_by)
                 VALUES ($1, $2, $3, $4, $5)",
                course,
                user,
                role.to_string(),
                source.to_string(),
                actor.id
            )
            .execute(&mut *tx)
            .await?;
            Joined::New
        }
        Some(c) if c.dropped => {
            sqlx::query!(
                "UPDATE memberships
                    SET role = $3, source = $4, added_by = $5, joined_at = now(), dropped_at = NULL
                  WHERE course_id = $1 AND user_id = $2",
                course,
                user,
                role.to_string(),
                source.to_string(),
                actor.id
            )
            .execute(&mut *tx)
            .await?;
            Joined::Restored
        }
        Some(c) => {
            let from: CourseRole = c.role.parse().map_err(decode)?;
            if from == role {
                return Ok(Ok(Joined::Unchanged));
            }
            if from == CourseRole::Instructor
                && let Some(refused) = may_unseat(&mut tx, course, user, actor).await?
            {
                return Ok(Err(refused));
            }
            sqlx::query!(
                "UPDATE memberships SET role = $3 WHERE course_id = $1 AND user_id = $2",
                course,
                user,
                role.to_string()
            )
            .execute(&mut *tx)
            .await?;
            Joined::Changed { from }
        }
    };
    tx.commit().await?;
    Ok(Ok(joined))
}

/// Drop a member (they keep their history). `None` if they weren't an
/// active member; otherwise the role they had.
pub async fn drop_member(
    pool: &PgPool,
    course: &str,
    user: i64,
    actor: Actor,
) -> sqlx::Result<Result<Option<CourseRole>, Refused>> {
    let mut tx = pool.begin().await?;
    super::lock(&mut tx, &format!("members:{course}")).await?;
    let Some(role) = sqlx::query_scalar!(
        "SELECT role FROM memberships
          WHERE course_id = $1 AND user_id = $2 AND dropped_at IS NULL",
        course,
        user
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok(Ok(None));
    };
    let role: CourseRole = role.parse().map_err(decode)?;
    if role == CourseRole::Instructor
        && let Some(refused) = may_unseat(&mut tx, course, user, actor).await?
    {
        return Ok(Err(refused));
    }
    sqlx::query!(
        "UPDATE memberships SET dropped_at = now() WHERE course_id = $1 AND user_id = $2",
        course,
        user
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(Some(role)))
}

/// What enrolling someone as a student did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enrolled {
    New,
    Restored,
    Already,
    /// An active instructor or grader: left alone.
    Staff,
}

/// Enroll a student (imports): new, or restored if dropped. Active members
/// keep their role.
pub async fn enroll(
    pool: &PgPool,
    course: &str,
    user: i64,
    by: i64,
    source: JoinSource,
) -> sqlx::Result<Enrolled> {
    let mut tx = pool.begin().await?;
    let current = sqlx::query!(
        r#"SELECT role, dropped_at IS NOT NULL AS "dropped!" FROM memberships
            WHERE course_id = $1 AND user_id = $2 FOR UPDATE"#,
        course,
        user
    )
    .fetch_optional(&mut *tx)
    .await?;
    let enrolled = match current {
        None => {
            let done = sqlx::query!(
                "INSERT INTO memberships (course_id, user_id, role, source, added_by)
                 VALUES ($1, $2, 'student', $3, $4) ON CONFLICT DO NOTHING",
                course,
                user,
                source.to_string(),
                by
            )
            .execute(&mut *tx)
            .await?;
            if done.rows_affected() == 1 {
                Enrolled::New
            } else {
                Enrolled::Already
            }
        }
        Some(c) if c.dropped => {
            sqlx::query!(
                "UPDATE memberships
                    SET role = 'student', source = $3, added_by = $4, joined_at = now(),
                        dropped_at = NULL
                  WHERE course_id = $1 AND user_id = $2",
                course,
                user,
                source.to_string(),
                by
            )
            .execute(&mut *tx)
            .await?;
            Enrolled::Restored
        }
        Some(c) if c.role == "student" => Enrolled::Already,
        Some(_) => Enrolled::Staff,
    };
    tx.commit().await?;
    Ok(enrolled)
}

/// Drop these students (a roster sync); staff among them are left alone.
pub async fn drop_students(pool: &PgPool, course: &str, users: &[i64]) -> sqlx::Result<u64> {
    Ok(sqlx::query!(
        "UPDATE memberships SET dropped_at = now()
          WHERE course_id = $1 AND user_id = ANY($2) AND role = 'student' AND dropped_at IS NULL",
        course,
        users
    )
    .execute(pool)
    .await?
    .rows_affected())
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
        UserRow,
        "UPDATE sessions s SET last_used_at = now()
           FROM users u
          WHERE s.token_hash = $1 AND s.expires_at > now() AND u.id = s.user_id
          RETURNING u.id, u.email, u.name, u.role",
        token_hash
    )
    .fetch_optional(pool)
    .await?
    .map(User::try_from)
    .transpose()
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
    async fn roles_are_set_and_the_last_admin_stays(pool: PgPool) -> sqlx::Result<()> {
        let RoleSet::Set { user: a, from } =
            set_role(&pool, "conder@ufl.edu", GlobalRole::Admin).await?
        else {
            panic!("refused");
        };
        assert_eq!((a.role, from), (GlobalRole::Admin, None), "created");
        assert_eq!(
            set_role(&pool, "conder@ufl.edu", GlobalRole::Student).await?,
            RoleSet::LastAdmin
        );
        upsert_user(&pool, "prof@ufl.edu", "Prof").await?;
        let RoleSet::Set { user: p, from } =
            set_role(&pool, "PROF@ufl.edu", GlobalRole::Admin).await?
        else {
            panic!("refused");
        };
        assert_eq!((p.name.as_str(), from), ("Prof", Some(GlobalRole::Student)));
        assert!(matches!(
            set_role(&pool, "conder@ufl.edu", GlobalRole::Staff).await?,
            RoleSet::Set { .. }
        ));
        assert!(
            user_by_email(&pool, "conder@ufl.edu")
                .await?
                .unwrap()
                .creates_courses()
        );
        Ok(())
    }

    #[sqlx::test]
    async fn users_are_found_by_role_and_part_of_a_name(pool: PgPool) -> sqlx::Result<()> {
        upsert_user(&pool, "albert.gator@ufl.edu", "Albert Gator").await?;
        upsert_user(&pool, "alberta.gator@ufl.edu", "Alberta Gator").await?;
        upsert_user(&pool, "a@ufl.edu", "").await?;
        upsert_user(&pool, "ba@ufl.edu", "").await?;
        upsert_user(&pool, "x_y@ufl.edu", "").await?;
        set_role(&pool, "sgator@ufl.edu", GlobalRole::Staff).await?;
        let emails =
            |(us, _): (Vec<User>, i64)| us.into_iter().map(|u| u.email).collect::<Vec<_>>();

        assert_eq!(
            emails(find_users(&pool, None, Some("GATOR"), 20, 0).await?),
            [
                "albert.gator@ufl.edu",
                "alberta.gator@ufl.edu",
                "sgator@ufl.edu"
            ]
        );
        assert_eq!(
            emails(find_users(&pool, None, Some("albert gator"), 20, 0).await?),
            ["albert.gator@ufl.edu"],
            "names match too"
        );
        assert_eq!(
            emails(find_users(&pool, None, Some("a@ufl.edu"), 20, 0).await?)[0],
            "a@ufl.edu",
            "an exact email comes first"
        );
        assert_eq!(
            emails(find_users(&pool, None, Some("x_"), 20, 0).await?),
            ["x_y@ufl.edu"],
            "wildcards are literal"
        );
        assert_eq!(
            emails(find_users(&pool, Some(GlobalRole::Staff), None, 20, 0).await?),
            ["sgator@ufl.edu"]
        );
        let (page, total) = find_users(&pool, None, None, 2, 2).await?;
        assert_eq!((page.len(), total), (2, 6));
        Ok(())
    }

    #[sqlx::test]
    async fn courses_start_with_their_instructors_and_page(pool: PgPool) -> sqlx::Result<()> {
        let prof = upsert_user(&pool, "prof@ufl.edu", "Prof").await?;
        let co = upsert_user(&pool, "co@ufl.edu", "Co").await?;
        assert!(
            create_course(
                &pool,
                "cop3502-f26",
                "Programming 1",
                prof.id,
                &[prof.id, co.id]
            )
            .await?
        );
        assert!(!create_course(&pool, "cop3502-f26", "again", prof.id, &[prof.id]).await?);
        create_course(&pool, "cop3502-f25", "Programming 1", prof.id, &[prof.id]).await?;
        create_course(&pool, "cap4053-s26", "AI for Games", prof.id, &[prof.id]).await?;

        let ms = members(&pool, "cop3502-f26", None, false).await?;
        assert_eq!(ms.len(), 2);
        assert_eq!(
            (
                ms[1].user.email.as_str(),
                ms[1].role,
                ms[1].source,
                ms[1].added_by.as_deref()
            ),
            (
                "prof@ufl.edu",
                CourseRole::Instructor,
                JoinSource::Created,
                Some("prof@ufl.edu")
            )
        );
        assert_eq!(
            courses_for(&pool, co.id).await?,
            [(
                Course {
                    id: "cop3502-f26".into(),
                    name: "Programming 1".into()
                },
                CourseRole::Instructor
            )]
        );

        let (page, total) = all_courses(&pool, Some("cop3502"), 1, 0).await?;
        assert_eq!((page[0].id.as_str(), total), ("cop3502-f25", 2));
        let (page, _) = all_courses(&pool, Some("cop3502"), 1, 1).await?;
        assert_eq!(page[0].id, "cop3502-f26");
        assert_eq!(all_courses(&pool, Some("games"), 50, 0).await?.1, 1);
        assert_eq!(all_courses(&pool, None, 50, 0).await?.1, 3);
        Ok(())
    }

    async fn course_with(pool: &PgPool, instructors: &[&str]) -> sqlx::Result<Vec<User>> {
        let mut users = Vec::new();
        for email in instructors {
            users.push(upsert_user(pool, email, "").await?);
        }
        let ids: Vec<i64> = users.iter().map(|u| u.id).collect();
        create_course(pool, "c", "C", ids[0], &ids).await?;
        Ok(users)
    }

    fn as_member(u: &User) -> Actor {
        Actor {
            id: u.id,
            admin: false,
        }
    }

    #[sqlx::test]
    async fn members_join_change_role_and_drop_softly(pool: PgPool) -> sqlx::Result<()> {
        let prof = &course_with(&pool, &["prof@ufl.edu"]).await?[0];
        let ta = upsert_user(&pool, "ta@ufl.edu", "TA").await?;
        let me = as_member(prof);
        assert_eq!(
            add_member(
                &pool,
                "c",
                ta.id,
                CourseRole::Student,
                JoinSource::Manual,
                me
            )
            .await?,
            Ok(Joined::New)
        );
        assert_eq!(
            add_member(
                &pool,
                "c",
                ta.id,
                CourseRole::Student,
                JoinSource::Manual,
                me
            )
            .await?,
            Ok(Joined::Unchanged)
        );
        assert_eq!(
            add_member(
                &pool,
                "c",
                ta.id,
                CourseRole::Grader,
                JoinSource::Manual,
                me
            )
            .await?,
            Ok(Joined::Changed {
                from: CourseRole::Student
            })
        );
        assert_eq!(role_in(&pool, "c", ta.id).await?, Some(CourseRole::Grader));

        assert_eq!(
            drop_member(&pool, "c", ta.id, me).await?,
            Ok(Some(CourseRole::Grader))
        );
        assert_eq!(
            drop_member(&pool, "c", ta.id, me).await?,
            Ok(None),
            "already dropped"
        );
        assert_eq!(role_in(&pool, "c", ta.id).await?, None);
        assert!(courses_for(&pool, ta.id).await?.is_empty());
        assert_eq!(members(&pool, "c", None, false).await?.len(), 1);
        let all = members(&pool, "c", None, true).await?;
        assert!(
            all.iter()
                .any(|m| m.user.id == ta.id && m.dropped_at.is_some()),
            "kept"
        );

        assert_eq!(
            add_member(
                &pool,
                "c",
                ta.id,
                CourseRole::Grader,
                JoinSource::Manual,
                me
            )
            .await?,
            Ok(Joined::Restored)
        );
        let found = find_members(&pool, "c", "TA", 20).await?;
        assert_eq!((found.len(), found[0].dropped_at.as_deref()), (1, None));
        assert_eq!(memberships_of(&pool, ta.id).await?[0].course.id, "c");
        Ok(())
    }

    #[sqlx::test]
    async fn instructors_keep_each_other_and_the_course(pool: PgPool) -> sqlx::Result<()> {
        let us = course_with(&pool, &["prof@ufl.edu", "grad@ufl.edu"]).await?;
        let (prof, grad) = (&us[0], &us[1]);
        let admin = Actor { id: 0, admin: true };
        assert_eq!(
            drop_member(&pool, "c", prof.id, as_member(grad)).await?,
            Err(Refused::OtherInstructor)
        );
        assert_eq!(
            add_member(
                &pool,
                "c",
                prof.id,
                CourseRole::Grader,
                JoinSource::Manual,
                as_member(grad)
            )
            .await?,
            Err(Refused::OtherInstructor),
            "demoting counts"
        );
        assert_eq!(
            drop_member(&pool, "c", grad.id, as_member(grad)).await?,
            Ok(Some(CourseRole::Instructor)),
            "yourself"
        );
        assert_eq!(
            drop_member(&pool, "c", prof.id, as_member(prof)).await?,
            Err(Refused::LastInstructor)
        );
        assert_eq!(
            add_member(
                &pool,
                "c",
                prof.id,
                CourseRole::Student,
                JoinSource::Manual,
                admin
            )
            .await?,
            Err(Refused::LastInstructor),
            "admins too"
        );
        add_member(
            &pool,
            "c",
            grad.id,
            CourseRole::Instructor,
            JoinSource::Manual,
            as_member(prof),
        )
        .await?
        .unwrap();
        assert_eq!(
            drop_member(&pool, "c", grad.id, admin).await?,
            Ok(Some(CourseRole::Instructor))
        );
        Ok(())
    }

    #[sqlx::test]
    async fn imports_enroll_students_and_leave_staff_alone(pool: PgPool) -> sqlx::Result<()> {
        let prof = &course_with(&pool, &["prof@ufl.edu"]).await?[0];
        let alice = upsert_user(&pool, "alice@ufl.edu", "Alice").await?;
        let bob = upsert_user(&pool, "bob@ufl.edu", "Bob").await?;
        let by = prof.id;
        assert_eq!(
            enroll(&pool, "c", alice.id, by, JoinSource::Import).await?,
            Enrolled::New
        );
        assert_eq!(
            enroll(&pool, "c", alice.id, by, JoinSource::Import).await?,
            Enrolled::Already
        );
        assert_eq!(
            enroll(&pool, "c", prof.id, by, JoinSource::Import).await?,
            Enrolled::Staff
        );
        assert_eq!(
            role_in(&pool, "c", prof.id).await?,
            Some(CourseRole::Instructor)
        );
        enroll(&pool, "c", bob.id, by, JoinSource::Import).await?;

        assert_eq!(
            drop_students(&pool, "c", &[bob.id, prof.id]).await?,
            1,
            "staff stay"
        );
        assert_eq!(
            role_in(&pool, "c", prof.id).await?,
            Some(CourseRole::Instructor)
        );
        assert_eq!(
            enroll(&pool, "c", bob.id, by, JoinSource::Import).await?,
            Enrolled::Restored
        );
        let m = &find_members(&pool, "c", "bob@ufl.edu", 1).await?[0];
        assert_eq!(
            (m.source, m.dropped_at.as_deref()),
            (JoinSource::Import, None)
        );
        Ok(())
    }

    #[sqlx::test]
    async fn logins_and_sessions_show_in_activity(pool: PgPool) -> sqlx::Result<()> {
        let u = upsert_user(&pool, "alice@ufl.edu", "Alice").await?;
        let fresh = activity(&pool, u.id).await?;
        assert_eq!((fresh.last_login_at, fresh.web_sessions), (None, 0));
        assert!(fresh.created_at.ends_with('Z'), "{}", fresh.created_at);
        create_session(&pool, "w", u.id, SessionKind::Web, 3600).await?;
        create_session(&pool, "c1", u.id, SessionKind::Cli, 3600).await?;
        create_session(&pool, "c2", u.id, SessionKind::Cli, -1).await?;
        record_login(&pool, u.id, SessionKind::Cli).await?;
        let a = activity(&pool, u.id).await?;
        assert_eq!(
            (a.last_login_kind, a.web_sessions, a.cli_sessions),
            (Some(SessionKind::Cli), 1, 1)
        );
        assert!(a.last_login_at.is_some());
        assert_eq!(end_sessions(&pool, u.id).await?, 2);
        assert_eq!(session_user(&pool, "w").await?, None);
        Ok(())
    }

    #[sqlx::test]
    async fn course_ids_are_slugs(pool: PgPool) -> sqlx::Result<()> {
        let u = upsert_user(&pool, "prof@ufl.edu", "").await?;
        for bad in ["Bad Id", "-cop", "cop_3502", ""] {
            assert!(
                create_course(&pool, bad, "x", u.id, &[u.id]).await.is_err(),
                "{bad:?}"
            );
        }
        assert!(create_course(&pool, "cop3502-f26", "ok", u.id, &[u.id]).await?);
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
