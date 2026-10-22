//! End to end on the kind cluster, through the api: personas log in with codes
//! from Mailpit into a fresh course; upload → Kaniko build → validation →
//! verdict; egress; submissions → admission → grading; roles; dates;
//! regrades and their notices; visibility before and after release;
//! activation.
//!
//! `cargo xtask e2e [FILTER…]` runs it (it needs `kind-up`, both images loaded
//! and `kind-deploy`); without `PEDAGOG_E2E` set it skips itself, so
//! `cargo test --workspace` stays offline. Checks run in order and share the
//! course; a filter runs only the checks whose names contain it (plus setup).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use pedagog_e2e::{
    Forward, Persona, Result, flat, kubectl, latest_mail, mail_after, ok, root, run, sql, wait_for,
};

const D: &str = "kind.local";
const EXAMPLE: &str = "recipes/examples/linked-list-python";
const EXAMPLE_ID: &str = "linked-list-python";
const UPLOAD_EXAMPLE: &str = "recipes/examples/linked-list-python-upload";
const UPLOAD_ID: &str = "linked-list-python-upload";
const STUDENTS: [&str; 6] = ["good", "buggy", "zipped", "broken", "late", "browser"];

struct Env {
    course: String,
    work: PathBuf,
    admin: Persona,
    prof: Persona,
    ta: Persona,
    _forwards: (Forward, Forward),
    /// Set by the checks that make them, used by later ones.
    second_build: Option<String>,
    good_first: Option<String>,
}

fn student(name: &str) -> Persona {
    Persona::new(name, &format!("e2e-{name}@{D}"))
}

fn path(rel: &str) -> String {
    root().join(rel).display().to_string()
}

fn at(hours: i64) -> String {
    (time::OffsetDateTime::now_utc() + time::Duration::hours(hours))
        .replace_nanosecond(0)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

fn first_number_after<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    text.split(word)
        .nth(1)?
        .split(|c: char| !c.is_ascii_digit())
        .find(|s| !s.is_empty())
}

fn require(cond: bool, why: impl FnOnce() -> String) -> Result<()> {
    if cond { Ok(()) } else { Err(why()) }
}

impl Env {
    fn setup() -> Result<Self> {
        ok(std::process::Command::new("cargo")
            .args(["build", "-q", "-p", "hammer"])
            .current_dir(root()))?;
        let forwards = Forward::api_and_mail()?;
        let course = format!("e2e-{}", time::OffsetDateTime::now_utc().unix_timestamp());
        let work = root().join(format!("target/kind-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let env = Self {
            admin: Persona::new("admin", &format!("e2e-admin@{D}")),
            prof: Persona::new("prof", &format!("e2e-prof@{D}")),
            ta: Persona::new("ta", &format!("e2e-ta@{D}")),
            course,
            work,
            _forwards: forwards,
            second_build: None,
            good_first: None,
        };
        println!("course {}", env.course);
        Ok(env)
    }

    /// Upload as the instructor, retrying while jobs is unreachable (just
    /// after it restarts, the api can briefly reach no replica).
    fn upload(&self, dir: &str, args: &[&str]) -> Result<(bool, String)> {
        let mut all = vec!["upload", dir, "-c", &self.course, "-q"];
        all.extend_from_slice(args);
        let mut said = self.prof.says(&all)?;
        for _ in 0..12 {
            if !said.1.contains("the build service is") {
                break;
            }
            std::thread::sleep(Duration::from_secs(5));
            said = self.prof.says(&all)?;
        }
        Ok(said)
    }

    fn submit(&self, who: &Persona, assignment: &str, args: &[&str]) -> Result<(bool, String)> {
        let mut all = vec!["submit", assignment];
        all.extend_from_slice(args);
        all.extend_from_slice(&["-c", &self.course]);
        who.says(&all)
    }

    /// The upload example's runs by users whose email is LIKE `pattern`.
    fn runs_of(&self, pattern: &str) -> String {
        format!(
            "SELECT g.status, g.score, g.total, g.notify, g.changes, g.reason, g.build_id, u.email
               FROM grading_runs g JOIN submissions s ON s.id = g.submission_id JOIN users u ON u.id = s.user_id
               JOIN assignments a ON a.id = s.assignment_id
              WHERE a.course_id = '{}' AND a.slug = '{UPLOAD_ID}' AND u.email LIKE '{pattern}'",
            self.course
        )
    }

    fn file(&self, name: &str) -> PathBuf {
        self.work.join(name)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.work);
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    ok(std::process::Command::new("cp").arg("-R").arg(from).arg(to))?;
    Ok(())
}

fn replace_in(file: &Path, from: &str, to: &str) -> Result<()> {
    let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    require(text.contains(from), || {
        format!("{} has no {from:?}", file.display())
    })?;
    std::fs::write(file, text.replace(from, to)).map_err(|e| e.to_string())
}

// ---- the checks, in order -------------------------------------------------

fn personas(e: &mut Env) -> Result<String> {
    ok(&mut kubectl(&[
        "-n",
        "pedagog-system",
        "exec",
        "deploy/pedagog-api",
        "--",
        "/usr/local/bin/pedagog-api",
        "add-admin",
        &e.admin.email,
    ]))?;
    e.admin.session()?;
    let say = |p: &Persona, args: &[&str]| -> Result<()> {
        let (ok, text) = p.says(args)?;
        require(ok, || format!("{} {args:?}: {text}", p.name))
    };
    say(&e.admin, &["users", "role", &e.prof.email, "staff"])?;
    e.prof.session()?;
    say(
        &e.prof,
        &["courses", "create", &e.course, "End-to-end checks"],
    )?;
    say(
        &e.prof,
        &[
            "members",
            "add",
            &e.ta.email,
            "--role",
            "grader",
            "-c",
            &e.course,
        ],
    )?;
    let mut emails: Vec<String> = STUDENTS.iter().map(|s| student(s).email).collect();
    emails.extend((1..=6).map(|n| format!("e2e-burst{n}@{D}")));
    let mut add = vec!["members", "add"];
    add.extend(emails.iter().map(String::as_str));
    add.extend_from_slice(&["-c", &e.course]);
    say(&e.prof, &add)?;
    e.ta.session()?;
    for s in STUDENTS {
        student(s).session()?;
    }
    Ok(format!(
        "admin, instructor, grader and {} students logged in; course created",
        STUDENTS.len()
    ))
}

fn example_validates(e: &mut Env) -> Result<String> {
    let (_, out) = e.upload(&path(EXAMPLE), &["--wait"])?;
    require(out.contains("validation passed: 10.0 / 10.0"), || {
        out.clone()
    })?;
    // Open, so a submission is refused for its platform (later), not its dates.
    e.prof.says(&[
        "dates", "set", EXAMPLE_ID, "--open", "always", "-c", &e.course,
    ])?;
    Ok("example validates (10/10)".into())
}

fn reupload_rebuilds(e: &mut Env) -> Result<String> {
    let current = || {
        sql(&format!(
            "SELECT a.current_build_id FROM assignments a WHERE a.course_id = '{}' AND a.slug = '{EXAMPLE_ID}'",
            e.course
        ))
    };
    let before = current()?;
    let (_, out) = e.upload(&path(EXAMPLE), &["--wait"])?;
    let after = current()?;
    require(out.contains("validation passed") && after != before, || {
        format!("current build {before} -> {after}: {out}")
    })?;
    Ok(format!(
        "identical content builds again (build {before} -> {after})"
    ))
}

fn buggy_solution_fails(e: &mut Env) -> Result<String> {
    let buggy = e.file("buggy");
    copy_dir(&root().join(EXAMPLE), &buggy)?;
    replace_in(
        &buggy.join("solution/linked_list.py"),
        "raise IndexError(\"pop from empty list\")",
        "return None",
    )?;
    let (passed, out) = e.upload(&buggy.display().to_string(), &["--wait"])?;
    require(
        !passed && out.contains("pop-front: expected the solution to pass, but it failed"),
        || out.clone(),
    )?;
    let current = sql(&format!(
        "SELECT b.validation_status FROM assignments a JOIN assignment_builds b ON b.id = a.current_build_id
          WHERE a.course_id = '{}' AND a.slug = '{EXAMPLE_ID}'",
        e.course
    ))?;
    require(current == "passed", || {
        format!("the current build is '{current}'")
    })?;
    Ok("a buggy solution fails validation and doesn't replace the current build".into())
}

fn newer_upload_supersedes(e: &mut Env) -> Result<String> {
    let build = |out: String| first_number_after(&out, "build ").map(str::to_owned);
    let first = build(e.upload(&path(EXAMPLE), &[])?.1).ok_or("no first build")?;
    let second = build(e.upload(&path(EXAMPLE), &[])?.1).ok_or("no second build")?;
    let status = sql(&format!(
        "SELECT build_status FROM assignment_builds WHERE id = {first}"
    ))?;
    require(status == "superseded", || {
        format!("build {first} is '{status}'")
    })?;
    e.second_build = Some(second.clone());
    Ok(format!("build {first} superseded by {second}"))
}

fn build_resumes_after_restart(e: &mut Env) -> Result<String> {
    let second = e.second_build.clone().ok_or("needs the supersede check")?;
    let stage = || {
        sql(&format!(
            "SELECT validation_status FROM assignment_builds WHERE id = {second}"
        ))
        .unwrap_or_default()
    };
    wait_for(Duration::from_secs(300), || stage() == "running");
    let during = stage();
    ok(&mut kubectl(&[
        "-n",
        "pedagog-builds",
        "rollout",
        "restart",
        "deploy/pedagog-jobs",
    ]))?;
    ok(&mut kubectl(&[
        "-n",
        "pedagog-builds",
        "rollout",
        "status",
        "deploy/pedagog-jobs",
        "--timeout=180s",
    ]))?;
    let finished = || {
        sql(&format!("SELECT validation_status FROM assignment_builds WHERE id = {second} AND finished_at IS NOT NULL"))
            .unwrap_or_default()
    };
    wait_for(Duration::from_secs(300), || !finished().is_empty());
    let final_ = finished();
    require(final_ == "passed", || {
        format!("build {second} after the restart: '{final_}'")
    })?;
    Ok(format!(
        "build {second} resumed and passed (restarted during '{during}')"
    ))
}

/// A test-run pod: jobs' await-egress init container, then a shell script.
fn probe(name: &str, network: &str, script: &str) -> Result<String> {
    // Block scalar lines must sit deeper than the `- |` that holds them.
    let script: String = script.lines().map(|l| format!("          {l}\n")).collect();
    let pod = format!(
        "apiVersion: v1
kind: Pod
metadata:
  name: {name}
  labels: {{ pedagog.io/role: validate, pedagog.io/network: {network} }}
spec:
  restartPolicy: Never
  initContainers:
    - name: await-egress
      image: localhost/pedagog/jobs-init:dev
      imagePullPolicy: IfNotPresent
      args: [await-egress, \"kube-dns.kube-system.svc:9153\", \"30\"]
  containers:
    - name: probe
      image: busybox:1.36
      command: [sh, -c]
      args:
        - |
{script}"
    );
    let _ = run(&mut kubectl(&[
        "-n",
        "pedagog-grading",
        "delete",
        "pod",
        name,
        "--ignore-not-found",
    ]));
    let mut apply = kubectl(&["-n", "pedagog-grading", "apply", "-f", "-"]);
    let mut child = apply
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    std::io::Write::write_all(child.stdin.as_mut().ok_or("no stdin")?, pod.as_bytes())
        .map_err(|e| e.to_string())?;
    child.wait().map_err(|e| e.to_string())?;
    ok(&mut kubectl(&[
        "-n",
        "pedagog-grading",
        "wait",
        "--for=jsonpath={.status.phase}=Succeeded",
        &format!("pod/{name}"),
        "--timeout=90s",
    ]))?;
    let logs = ok(&mut kubectl(&[
        "-n",
        "pedagog-grading",
        "logs",
        name,
        "-c",
        "probe",
    ]));
    let _ = run(&mut kubectl(&[
        "-n",
        "pedagog-grading",
        "delete",
        "pod",
        name,
        "--wait=false",
    ]));
    logs
}

fn egress_deny(_: &mut Env) -> Result<String> {
    let out = probe(
        "e2e-netprobe",
        "deny",
        "wget -q -T 5 -O /dev/null http://garage-service.pedagog-data.svc:3900/ 2>&1 | grep -q \"403\" && echo garage-ok
wget -q -T 5 -O /dev/null http://1.1.1.1/ >/dev/null 2>&1 && echo internet-open || echo internet-blocked",
    )?;
    require(
        out.contains("garage-ok") && out.contains("internet-blocked"),
        || out.clone(),
    )?;
    Ok("egress: Garage reachable, internet blocked".into())
}

fn egress_allow(_: &mut Env) -> Result<String> {
    let node = ok(&mut kubectl(&[
        "get",
        "nodes",
        "-o",
        "jsonpath={.items[0].status.addresses[?(@.type==\"InternalIP\")].address}",
    ]))?;
    let out = probe(
        "e2e-netprobe-allow",
        "allow",
        &format!(
            "wget -q -T 5 -O /dev/null http://1.1.1.1/ >/dev/null 2>&1 && echo internet-open || echo internet-blocked
nc -z -w 3 {node} 10250 && echo node-open || echo node-blocked"
        ),
    )?;
    require(
        out.contains("internet-open") && out.contains("node-blocked"),
        || out.clone(),
    )?;
    Ok(format!(
        "egress: internet reachable, node {node} (kubelet) blocked"
    ))
}

fn upload_example_validates(e: &mut Env) -> Result<String> {
    let (_, out) = e.upload(&path(UPLOAD_EXAMPLE), &["--wait"])?;
    let (dated, dates) = e.prof.says(&[
        "dates", "set", UPLOAD_ID, "--open", "always", "-c", &e.course,
    ])?;
    require(
        out.contains("validation passed: 10.0 / 10.0") && dated,
        || format!("{out} {dates}"),
    )?;
    // Submissions the later checks use.
    let buggy = e.file("buggy-sub");
    copy_dir(&root().join(UPLOAD_EXAMPLE).join("solution"), &buggy)?;
    replace_in(
        &buggy.join("linked_list.py"),
        "raise IndexError(\"pop from empty list\")",
        "return None",
    )?;
    std::fs::create_dir_all(e.file("broken")).map_err(|x| x.to_string())?;
    std::fs::write(e.file("broken/linked_list.py"), "class LinkedList(:\n")
        .map_err(|x| x.to_string())?;
    std::fs::write(e.file("other.py"), "x = 1\n").map_err(|x| x.to_string())?;
    Ok("upload example validates (10/10)".into())
}

fn solution(_: &Env) -> String {
    path(&format!("{UPLOAD_EXAMPLE}/solution"))
}

fn submissions_are_graded(e: &mut Env) -> Result<String> {
    let (_, out) = e.submit(&student("good"), UPLOAD_ID, &[&solution(e), "--wait"])?;
    require(
        out.contains("(counts), on time: 10/10 on the tests shown"),
        || out.clone(),
    )?;
    e.good_first = first_number_after(&out, "submission ").map(str::to_owned);
    let buggy = e.file("buggy-sub").display().to_string();
    let (_, out) = e.submit(&student("buggy"), UPLOAD_ID, &[&buggy, "--wait"])?;
    let names_it = out
        .lines()
        .any(|l| l.contains("pop_front") && l.contains("fail"));
    require(out.contains("9/10 on the tests shown") && names_it, || {
        out.clone()
    })?;
    Ok("the solution grades 10/10; a buggy one 9/10, naming the failing test".into())
}

fn finder_zip_is_cleaned(e: &mut Env) -> Result<String> {
    use std::io::Write as _;
    let zip_path = e.file("hw1.zip");
    let file = std::fs::File::create(&zip_path).map_err(|x| x.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    let source = std::fs::read(root().join(UPLOAD_EXAMPLE).join("solution/linked_list.py"))
        .map_err(|x| x.to_string())?;
    for (name, body) in [
        ("hw1/linked_list.py", source.as_slice()),
        ("hw1/notes.txt", b"my notes".as_slice()),
        ("hw1/.DS_Store", b"junk".as_slice()),
        ("__MACOSX/hw1/._linked_list.py", b"junk".as_slice()),
    ] {
        zip.start_file(name, opts).map_err(|x| x.to_string())?;
        zip.write_all(body).map_err(|x| x.to_string())?;
    }
    zip.finish().map_err(|x| x.to_string())?;
    let (_, out) = e.submit(
        &student("zipped"),
        UPLOAD_ID,
        &[&zip_path.display().to_string(), "--wait"],
    )?;
    let all = [
        "dropped the wrapping folder hw1/",
        "ignored by the assignment's rules: notes.txt",
        "dropped 2 OS file",
        "10/10 on the tests shown",
    ];
    require(all.iter().all(|w| out.contains(w)), || out.clone())?;
    Ok("zip: wrapping folder, notes.txt and OS clutter dropped; graded 10/10".into())
}

fn refusals(e: &mut Env) -> Result<String> {
    let (_, out) = e.submit(
        &student("buggy"),
        UPLOAD_ID,
        &[&e.file("other.py").display().to_string()],
    )?;
    let out = flat(&out);
    require(
        out.contains("422") && out.contains("missing required files: linked_list.py"),
        || out.clone(),
    )?;
    let (_, out) = e.submit(&student("good"), EXAMPLE_ID, &[&solution(e)])?;
    let out = flat(&out);
    require(
        out.contains("409") && out.contains("arrives with sessions"),
        || out.clone(),
    )?;
    let (graded, out) = e.submit(
        &student("broken"),
        UPLOAD_ID,
        &[&e.file("broken").display().to_string(), "--wait"],
    )?;
    let out = flat(&out);
    require(
        !graded
            && out.contains("couldn't be graded: the tests couldn't run with your code")
            && out.contains("contact course staff"),
        || out.clone(),
    )?;
    Ok(
        "missing files and workspace uploads are refused; a syntax error is a plain sentence"
            .into(),
    )
}

fn burst_respects_the_quota(e: &mut Env) -> Result<String> {
    let sol = solution(e);
    for n in 1..=6 {
        let email = format!("e2e-burst{n}@{D}");
        e.submit(&e.prof, UPLOAD_ID, &[&sol, "--for", &email])?;
    }
    let mut most = 0;
    let left = || {
        sql(&format!(
            "SELECT count(*) FROM ({}) r WHERE status IN ('queued', 'grading')",
            e.runs_of("e2e-burst%")
        ))
        .unwrap_or_default()
    };
    wait_for(Duration::from_secs(120), || {
        let running = ok(&mut kubectl(&[
            "-n",
            "pedagog-grading",
            "get",
            "pods",
            "-l",
            "pedagog.io/role=grade",
            "--no-headers",
            "--field-selector=status.phase!=Succeeded,status.phase!=Failed",
        ]))
        .map(|o| o.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
        most = most.max(running);
        left() == "0"
    });
    let graded = sql(&format!(
        "SELECT count(*) FROM ({}) r WHERE status = 'graded'",
        e.runs_of("e2e-burst%")
    ))?;
    require(left() == "0" && graded == "6" && most <= 3, || {
        format!("{graded}/6 graded, {} left, {most} at once", left())
    })?;
    Ok(format!(
        "6 submissions drained, at most {most} grading at once (quota: 3)"
    ))
}

fn roles(e: &mut Env) -> Result<String> {
    let denied = |p: &Persona, args: &[&str]| -> Result<bool> {
        let (ok, out) = p.says(args)?;
        Ok(!ok && !out.contains("cannot reach"))
    };
    let good = student("good");
    let checks = [
        (
            denied(
                &good,
                &["upload", &path(UPLOAD_EXAMPLE), "-c", &e.course, "-q"],
            )?,
            "a student uploaded",
        ),
        (
            denied(&good, &["results", "list", UPLOAD_ID, "-c", &e.course])?,
            "a student listed results",
        ),
        (
            denied(
                &good,
                &["courses", "create", &format!("{}-x", e.course), "Nope"],
            )?,
            "a student created a course",
        ),
        (
            denied(&e.ta, &["results", "release", UPLOAD_ID, "-c", &e.course])?,
            "a grader released results",
        ),
        (
            denied(
                &e.ta,
                &["members", "add", &format!("e2e-x@{D}"), "-c", &e.course],
            )?,
            "a grader added a member",
        ),
        (
            e.ta.says(&["results", "list", UPLOAD_ID, "-c", &e.course])?
                .1
                .contains(&good.email),
            "a grader can't list results",
        ),
    ];
    let wrong: Vec<&str> = checks
        .iter()
        .filter(|(ok, _)| !ok)
        .map(|(_, why)| *why)
        .collect();
    require(wrong.is_empty(), || wrong.join("; "))?;
    Ok("students can't manage; graders see results but can't release or add members".into())
}

fn late_closed_and_on_behalf(e: &mut Env) -> Result<String> {
    let late = student("late");
    let sol = solution(e);
    e.prof.says(&[
        "dates",
        "set",
        UPLOAD_ID,
        "--due",
        &at(-1),
        "--late-due",
        &at(1),
        "-c",
        &e.course,
    ])?;
    let (_, out) = e.submit(&late, UPLOAD_ID, &[&sol])?;
    require(out.contains("late by"), || format!("late: {out}"))?;
    e.prof.says(&[
        "dates",
        "set",
        UPLOAD_ID,
        "--due",
        &at(-2),
        "--late-due",
        &at(-1),
        "-c",
        &e.course,
    ])?;
    let (_, out) = e.submit(&late, UPLOAD_ID, &[&sol])?;
    require(flat(&out).contains("deadline has passed"), || {
        format!("closed: {out}")
    })?;
    let (_, out) = e.submit(
        &e.prof,
        UPLOAD_ID,
        &[&sol, "--for", &late.email, "--at", "last", "--wait"],
    )?;
    let (_, mine) = late.says(&["results", "show", UPLOAD_ID, "-c", &e.course])?;
    let result = require(
        out.contains(&format!("submitted for {} by {}", late.email, e.prof.email))
            && mine.contains(&format!("submitted by {} on your behalf", e.prof.email)),
        || format!("on behalf: {out} / {mine}"),
    );
    e.prof.says(&[
        "dates",
        "clear",
        UPLOAD_ID,
        "--due",
        "--late-due",
        "-c",
        &e.course,
    ])?;
    result?;
    Ok("late after due, refused after late due, staff submit on a student's behalf".into())
}

fn regrade_and_notice(e: &mut Env) -> Result<String> {
    let v2 = e.file("v2");
    copy_dir(&root().join(UPLOAD_EXAMPLE), &v2)?;
    let tests = v2.join("tests/test_linked_list.py");
    let mut text = std::fs::read_to_string(&tests).map_err(|x| x.to_string())?;
    text.push_str(
        "\n\n@rubric_item(\"pop_front shrinks the list\", points=1, id=\"pop-shrinks\")
@number(6)
def test_pop_shrinks(score: Score):
    items = LinkedList()
    items.append(1)
    items.append(2)
    items.pop_front()
    assert len(items) == 1
    score.award()
",
    );
    std::fs::write(&tests, text).map_err(|x| x.to_string())?;
    let good = student("good");
    let before = latest_mail(&good.email, "results updated")?;
    let (_, out) = e.upload(
        &v2.display().to_string(),
        &["--regrade", "--notify", "changed", "--wait"],
    )?;
    let build = first_number_after(&out, "build ")
        .ok_or_else(|| out.clone())?
        .to_owned();
    wait_for(Duration::from_secs(180), || {
        sql(&format!("SELECT count(*) FROM grading_runs WHERE build_id = {build} AND status IN ('queued', 'grading')"))
            .is_ok_and(|n| n == "0")
    });
    let regrade = sql(&format!(
        "SELECT score || '/' || total || ' ' || notify || ' ' || (changes->'added')::text
           FROM ({}) r WHERE reason = 'regrade' AND build_id = {build}",
        e.runs_of(&good.email)
    ))?;
    require(regrade == r#"11/11 true ["pop-shrinks"]"#, || {
        format!(
            "regrade of {} against build {build}: '{regrade}'",
            good.email
        )
    })?;
    let mail = mail_after(
        &good.email,
        "results updated",
        before.as_ref(),
        Duration::from_secs(90),
    )?;
    Ok(format!(
        "regraded 11/11 with pop-shrinks added; notice arrived: {}",
        mail.subject
    ))
}

fn visibility_and_release(e: &mut Env) -> Result<String> {
    let good = student("good");
    e.prof.says(&[
        "visibility",
        "set",
        UPLOAD_ID,
        "--test",
        "many",
        "--score",
        "after_publish",
        "-c",
        &e.course,
    ])?;
    let (_, before) = good.says(&["results", "show", UPLOAD_ID, "-c", &e.course])?;
    let released = latest_mail(&good.email, "results released")?;
    let (_, out) = e
        .prof
        .says(&["results", "release", UPLOAD_ID, "-c", &e.course])?;
    let (_, after) = good.says(&["results", "show", UPLOAD_ID, "-c", &e.course])?;
    require(
        before.contains("9/9 on the tests shown")
            && before.contains("hidden until")
            && after.contains("11/11 on the tests shown"),
        || format!("before: {before} / after: {after}"),
    )?;
    let mail = mail_after(
        &good.email,
        "results released",
        released.as_ref(),
        Duration::from_secs(90),
    )?;
    require(out.contains("emailing"), || out.clone())?;
    Ok(format!(
        "hidden before release (9/9), shown after (11/11); {}",
        mail.subject
    ))
}

fn activation(e: &mut Env) -> Result<String> {
    let good = student("good");
    let first = e.good_first.clone().ok_or("needs the grading check")?;
    let buggy = e.file("buggy-sub").display().to_string();
    e.submit(&good, UPLOAD_ID, &[&buggy, "--wait"])?;
    e.prof
        .says(&["submissions", "activate", &first, "-c", &e.course])?;
    let (_, counting) = e.prof.says(&[
        "results", "list", UPLOAD_ID, "--search", "e2e-good", "-c", &e.course,
    ])?;
    let (_, out) = e.submit(&good, UPLOAD_ID, &[&solution(e), "--wait"])?;
    let newer = first_number_after(&out, "submission ")
        .unwrap_or("?")
        .to_owned();
    let (_, latest) = e.prof.says(&[
        "results", "list", UPLOAD_ID, "--search", "e2e-good", "-c", &e.course,
    ])?;
    require(
        counting.contains(&format!("#{first} ")) && latest.contains(&format!("#{newer} ")),
        || format!("activation: {counting} / {latest}"),
    )?;
    Ok(format!(
        "submission {first} counts once activated; {newer} once submitted"
    ))
}

/// Log in through the web UI in headless Chrome: the email, the code from
/// Mailpit, then your courses. Exercises the page boot under its CSP, the
/// session cookie and the X-Pedagog header together.
/// Run `check` against a fresh headless Chrome (chromedriver on port 9515);
/// the page's source is saved to `target/<name>-failure.html` if it fails.
fn in_browser<F>(name: &str, check: impl FnOnce(fantoccini::Client) -> F) -> Result<String>
where
    F: std::future::Future<Output = Result<String>>,
{
    let driver = std::env::var("CHROMEDRIVER")
        .map_err(|_| "CHROMEDRIVER isn't set (run through `cargo xtask e2e`)")?;
    let mut chromedriver = std::process::Command::new(driver)
        .arg("--port=9515")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|x| format!("couldn't start chromedriver: {x}"))?;
    let failure = root().join(format!("target/{name}-failure.html"));
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|x| x.to_string())?
        .block_on(async {
            let c = browser().await?;
            let outcome = check(c.clone()).await;
            if outcome.is_err()
                && let Ok(page) = c.source().await
            {
                let _ = std::fs::write(&failure, page);
            }
            let _ = c.close().await;
            outcome
        });
    let _ = chromedriver.kill();
    result
}

/// Run blocking code (Mailpit's helpers use reqwest's blocking client) from
/// the browser check's async code.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|x| x.to_string())?
}

async fn browser() -> Result<fantoccini::Client> {
    let mut caps = serde_json::Map::new();
    caps.insert(
        "goog:chromeOptions".into(),
        serde_json::json!({"args": ["--headless=new", "--disable-gpu", "--window-size=1280,900"]}),
    );
    for _ in 0..20 {
        match fantoccini::ClientBuilder::rustls()
            .map_err(|x| x.to_string())?
            .capabilities(caps.clone())
            .connect("http://localhost:9515")
            .await
        {
            Ok(c) => return Ok(c),
            Err(_) => tokio::time::sleep(Duration::from_millis(250)).await,
        }
    }
    Err("couldn't reach chromedriver".into())
}

fn base() -> String {
    format!("http://localhost:{}", pedagog_e2e::API_PORT)
}

fn cmd(x: fantoccini::error::CmdError) -> String {
    x.to_string()
}

/// Wait for an element, saying which one in the error.
async fn element(
    c: &fantoccini::Client,
    within: Duration,
    css: &str,
) -> Result<fantoccini::elements::Element> {
    c.wait()
        .at_most(within)
        .for_element(fantoccini::Locator::Css(css))
        .await
        .map_err(|x| format!("waiting for {css}: {x}"))
}

/// Log `who` in through the login page, with the code from Mailpit.
async fn log_in(c: &fantoccini::Client, who: &Persona) -> Result<()> {
    use fantoccini::Locator;
    c.goto(&format!("{}/login", base())).await.map_err(cmd)?;
    let email = element(c, Duration::from_secs(30), "#login-email").await?;
    email.send_keys(&who.email).await.map_err(cmd)?;
    let to = who.email.clone();
    let before = blocking(move || latest_mail(&to, "login code")).await?;
    c.find(Locator::Css("button[type=submit]"))
        .await
        .map_err(cmd)?
        .click()
        .await
        .map_err(cmd)?;
    let code_box = element(c, Duration::from_secs(30), "#login-code").await?;
    let to = who.email.clone();
    let mail =
        blocking(move || mail_after(&to, "login code", before.as_ref(), Duration::from_secs(30)))
            .await?;
    let code = mail
        .subject
        .rsplit(' ')
        .next()
        .unwrap_or_default()
        .to_owned();
    code_box.send_keys(&code).await.map_err(cmd)?;
    c.find(Locator::Css("button[type=submit]"))
        .await
        .map_err(cmd)?
        .click()
        .await
        .map_err(cmd)?;
    c.wait()
        .for_element(Locator::XPath("//h1[text()='Home']"))
        .await
        .map_err(|x| format!("waiting for Home after logging in: {x}"))?;
    Ok(())
}

fn web_login(e: &mut Env) -> Result<String> {
    let course = e.course.clone();
    in_browser("web-login", |c| async move {
        log_in(&c, &student("good")).await?;
        let page = c.source().await.map_err(cmd)?;
        require(page.contains(&course), || {
            format!("the course {course} isn't on Home")
        })?;
        c.goto(&format!("{}/courses/{course}", base()))
            .await
            .map_err(cmd)?;
        c.wait()
            .for_element(fantoccini::Locator::XPath("//h1[text()='Assignments']"))
            .await
            .map_err(|x| format!("waiting for the assignments: {x}"))?;
        element(&c, Duration::from_secs(10), "ul[aria-label=Assignments] li").await?;
        Ok(
            "logged in through the browser (code from Mailpit); courses and assignments shown"
                .into(),
        )
    })
}

fn web_submit(e: &mut Env) -> Result<String> {
    let course = e.course.clone();
    let file = root()
        .join(UPLOAD_EXAMPLE)
        .join("solution/linked_list.py")
        .display()
        .to_string();
    in_browser("web-submit", |c| async move {
        log_in(&c, &student("browser")).await?;
        c.goto(&format!(
            "{}/courses/{course}/assignments/{UPLOAD_ID}",
            base()
        ))
        .await
        .map_err(cmd)?;
        let input = element(
            &c,
            Duration::from_secs(30),
            "input[type=file]:not([webkitdirectory])",
        )
        .await?;
        input.send_keys(&file).await.map_err(cmd)?;
        element(&c, Duration::from_secs(10), "ul.filelist li").await?;
        element(
            &c,
            Duration::from_secs(10),
            "button.btn-primary:not([disabled])",
        )
        .await?
        .click()
        .await
        .map_err(cmd)?;
        let receipt = element(&c, Duration::from_secs(60), ".alert-pass .strong").await?;
        let said = receipt.text().await.map_err(cmd)?;
        require(said.starts_with("Submitted at"), || {
            format!("the receipt says {said:?}")
        })?;
        // Graded while the page is open: the results come in without a reload.
        // Full marks, out of however many points the current build has.
        let score = element(&c, Duration::from_secs(300), ".score .big").await?;
        let got = score.text().await.map_err(cmd)?;
        let of = element(&c, Duration::from_secs(5), ".score .of")
            .await?
            .text()
            .await
            .map_err(cmd)?;
        require(of.starts_with(&format!("/ {got} ")), || {
            format!("the score shown is {got} {of}")
        })?;
        let history = element(&c, Duration::from_secs(30), "#hist-h + ul li").await?;
        let row = history.text().await.map_err(cmd)?;
        require(
            row.contains(&format!("{got}/{got} shown")) && row.contains("Counts"),
            || format!("the history row says {row:?}"),
        )?;
        Ok(format!(
            "a student submits in the browser, watches it grade live, and sees {got}/{got} in results and history"
        ))
    })
}

type Check = fn(&mut Env) -> Result<String>;

/// Each check, and the earlier checks it builds on (a filter runs those too).
const CHECKS: &[(&str, Check, &[&str])] = &[
    ("example_validates", example_validates, &[]),
    (
        "reupload_rebuilds",
        reupload_rebuilds,
        &["example_validates"],
    ),
    (
        "buggy_solution_fails",
        buggy_solution_fails,
        &["example_validates"],
    ),
    (
        "newer_upload_supersedes",
        newer_upload_supersedes,
        &["example_validates"],
    ),
    (
        "build_resumes_after_restart",
        build_resumes_after_restart,
        &["newer_upload_supersedes"],
    ),
    ("egress_deny", egress_deny, &[]),
    ("egress_allow", egress_allow, &[]),
    ("upload_example_validates", upload_example_validates, &[]),
    (
        "submissions_are_graded",
        submissions_are_graded,
        &["upload_example_validates"],
    ),
    (
        "finder_zip_is_cleaned",
        finder_zip_is_cleaned,
        &["upload_example_validates"],
    ),
    (
        "refusals",
        refusals,
        &["example_validates", "upload_example_validates"],
    ),
    (
        "burst_respects_the_quota",
        burst_respects_the_quota,
        &["upload_example_validates"],
    ),
    ("roles", roles, &["submissions_are_graded"]),
    (
        "late_closed_and_on_behalf",
        late_closed_and_on_behalf,
        &["upload_example_validates"],
    ),
    (
        "regrade_and_notice",
        regrade_and_notice,
        &["submissions_are_graded"],
    ),
    (
        "visibility_and_release",
        visibility_and_release,
        &["regrade_and_notice"],
    ),
    ("activation", activation, &["submissions_are_graded"]),
    ("web_login", web_login, &["upload_example_validates"]),
    ("web_submit", web_submit, &["upload_example_validates"]),
];

/// The checks to run: those matching a filter (all without one), and what
/// they build on.
fn selected(filters: &[String]) -> Vec<&'static str> {
    let mut chosen: Vec<&str> = CHECKS
        .iter()
        .map(|(name, _, _)| *name)
        .filter(|name| filters.is_empty() || filters.iter().any(|f| name.contains(f.as_str())))
        .collect();
    let mut i = 0;
    while i < chosen.len() {
        let needs = CHECKS
            .iter()
            .find(|(n, _, _)| *n == chosen[i])
            .map_or(&[][..], |(_, _, needs)| *needs);
        for need in needs {
            if !chosen.contains(need) {
                chosen.push(need);
            }
        }
        i += 1;
    }
    chosen
}

fn main() -> ExitCode {
    if std::env::var_os("PEDAGOG_E2E").is_none() {
        eprintln!("kind e2e skipped: run `cargo xtask e2e` against the kind cluster");
        return ExitCode::SUCCESS;
    }
    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let chosen = selected(&filters);
    let mut e = match Env::setup() {
        Ok(e) => e,
        Err(why) => {
            println!("FAIL  setup: {why}");
            return ExitCode::FAILURE;
        }
    };
    let mut failed = 0;
    let order = CHECKS
        .iter()
        .filter(|(n, _, _)| chosen.contains(n))
        .map(|(n, c, _)| (*n, *c));
    for (name, check) in std::iter::once(("personas", personas as Check)).chain(order) {
        match check(&mut e) {
            Ok(what) => println!("PASS  {name}: {what}"),
            Err(why) => {
                println!("FAIL  {name}: {why}");
                failed += 1;
                if name == "personas" {
                    break;
                }
            }
        }
    }
    println!();
    if failed == 0 {
        println!("all checks passed");
        ExitCode::SUCCESS
    } else {
        println!("{failed} check(s) failed");
        ExitCode::FAILURE
    }
}
