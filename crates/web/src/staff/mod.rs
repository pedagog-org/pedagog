//! The staff pages: a course's assignments as a table, an assignment's
//! overview (its builds, making one current), uploading a version, and a
//! build's page. Instructors change things; graders see the same pages.

pub mod build;
pub mod describe;
pub mod list;
mod make_current;
pub mod overview;
pub mod upload;

use crate::session::Session;

/// Whether the viewer may change `course`'s assignments (instructors, admins).
pub fn manages(session: &Session, course: &str) -> bool {
    session.is_admin()
        || session
            .course(course)
            .is_some_and(|c| c.role == "instructor")
}

/// Whether the viewer sees `course` as staff.
pub fn is_staff(session: &Session, course: &str) -> bool {
    session.is_admin() || session.course(course).is_some_and(|c| c.role != "student")
}
