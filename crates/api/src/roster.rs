//! Roster files: a simple `email,name` CSV, or Canvas's gradebook export
//! (recognized by its `SIS Login ID` column; logins become
//! `<login>@<login domain>`). Rows that can't be used are reported, not fatal.

/// One student to enroll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub email: String,
    pub name: String,
}

/// What a roster file held.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub entries: Vec<Entry>,
    /// `(line, why)` for rows left out.
    pub skipped: Vec<(usize, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Simple { email: usize, name: Option<usize> },
    Canvas { name: usize, login: usize },
}

pub fn parse(text: &str, login_domain: &str) -> Result<Parsed, String> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.trim_start_matches('\u{feff}').as_bytes());
    let headers = reader
        .headers()
        .map_err(|e| format!("unreadable CSV: {e}"))?
        .clone();
    let find = |name: &str| headers.iter().position(|h| h.eq_ignore_ascii_case(name));
    let format = match (find("SIS Login ID"), find("Student"), find("email")) {
        (Some(login), Some(name), _) => Format::Canvas { name, login },
        (_, _, Some(email)) => Format::Simple {
            email,
            name: find("name"),
        },
        _ => {
            return Err(
                "unrecognized roster: expected an `email` column (with an optional `name`), or Canvas's gradebook export"
                    .to_owned(),
            );
        }
    };
    let mut parsed = Parsed::default();
    let mut seen = std::collections::HashSet::new();
    for (i, row) in reader.records().enumerate() {
        let line = i + 2;
        let row = match row {
            Ok(r) => r,
            Err(e) => {
                parsed.skipped.push((line, format!("unreadable: {e}")));
                continue;
            }
        };
        let field = |n: usize| row.get(n).unwrap_or("").to_owned();
        let entry = match format {
            Format::Simple { email, name } => Entry {
                email: field(email).to_lowercase(),
                name: name.map(field).unwrap_or_default(),
            },
            Format::Canvas { name, login } => {
                let student = field(name);
                // Canvas's own rows: points possible, and its test student.
                if student.eq_ignore_ascii_case("points possible") {
                    continue;
                }
                let login = field(login);
                if login.is_empty() {
                    parsed.skipped.push((
                        line,
                        format!("{student:?} has no SIS login (e.g. Canvas's test student)"),
                    ));
                    continue;
                }
                let email = if login.contains('@') {
                    login.to_lowercase()
                } else {
                    format!("{}@{login_domain}", login.to_lowercase())
                };
                Entry {
                    email,
                    name: canvas_name(&student),
                }
            }
        };
        if !looks_like_email(&entry.email) {
            parsed
                .skipped
                .push((line, format!("{:?} isn't an email address", entry.email)));
            continue;
        }
        if !seen.insert(entry.email.clone()) {
            parsed
                .skipped
                .push((line, format!("{} is listed twice", entry.email)));
            continue;
        }
        parsed.entries.push(entry);
    }
    Ok(parsed)
}

/// Canvas writes "Last, First"; people read "First Last".
fn canvas_name(name: &str) -> String {
    match name.split_once(',') {
        Some((last, first)) => format!("{} {}", first.trim(), last.trim()),
        None => name.to_owned(),
    }
}

fn looks_like_email(s: &str) -> bool {
    match s.split_once('@') {
        Some((local, domain)) => !local.is_empty() && domain.contains('.') && !s.contains(' '),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_simple_csv_needs_an_email_column() {
        let p = parse(
            "email,name\nAlbert.Gator@ufl.edu,Albert Gator\nbob@ufl.edu,\n",
            "ufl.edu",
        )
        .unwrap();
        assert_eq!(
            p.entries,
            [
                Entry {
                    email: "albert.gator@ufl.edu".into(),
                    name: "Albert Gator".into()
                },
                Entry {
                    email: "bob@ufl.edu".into(),
                    name: String::new()
                },
            ]
        );
        assert!(parse("e-mail,full name\nx@ufl.edu,X\n", "ufl.edu").is_err());
        let only = parse("Email\nx@ufl.edu\n", "ufl.edu").unwrap();
        assert_eq!(only.entries[0].email, "x@ufl.edu", "names are optional");
    }

    #[test]
    fn a_canvas_export_uses_sis_logins_and_skips_its_own_rows() {
        let canvas = "\u{feff}Student,ID,SIS User ID,SIS Login ID,Section,Lab 1 (123)\n\
                      \"    Points Possible\",,,,,10\n\
                      \"Gator, Albert\",101,12345678,albert.gator,COP3502 001,9\n\
                      \"Swamp, Sally\",102,23456789,SSwamp,COP3502 002,10\n\
                      \"Student, Test\",999,,,COP3502 001,0\n";
        let p = parse(canvas, "ufl.edu").unwrap();
        assert_eq!(
            p.entries,
            [
                Entry {
                    email: "albert.gator@ufl.edu".into(),
                    name: "Albert Gator".into()
                },
                Entry {
                    email: "sswamp@ufl.edu".into(),
                    name: "Sally Swamp".into()
                },
            ]
        );
        assert_eq!(p.skipped.len(), 1);
        assert!(p.skipped[0].1.contains("test student"), "{:?}", p.skipped);
    }

    #[test]
    fn bad_and_repeated_rows_are_reported_with_their_line() {
        let p = parse(
            "email,name\nnot-an-email,X\na@ufl.edu,A\nA@ufl.edu,A again\n",
            "ufl.edu",
        )
        .unwrap();
        assert_eq!(p.entries.len(), 1);
        assert_eq!(p.skipped[0].0, 2);
        assert!(p.skipped[0].1.contains("isn't an email"));
        assert_eq!(p.skipped[1], (4, "a@ufl.edu is listed twice".to_owned()));
    }
}
