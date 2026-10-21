//! `cargo xtask`: development tasks (the dev database, the kind cluster,
//! personas, end-to-end tests, pre-commit checks), so none of them is a shell
//! script. `just` recipes call these.

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use clap::{Parser, Subcommand};
use pedagog_e2e::{
    API_PORT, CONTEXT, Forward, GLITCHTIP_PORT, MAILPIT_PORT, Persona, Result, kubectl, ok, root,
};

const DEV_DB: &str = "postgres://pedagog:pedagog@localhost:5432/pedagog";
const PREPARE_DB: &str = "postgres://pedagog:pedagog@localhost:5432/pedagog_prepare";
const MIGRATIONS: &str = "crates/store/migrations";

#[derive(Parser)]
#[command(about = "Pedagog's development tasks")]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Start the dev Postgres in podman (postgres:16-alpine, db/user `pedagog`).
    DbUp,
    /// Stop and remove it (the `pedagog-pgdata` volume stays).
    DbDown,
    /// Migrate the dev database and refresh `.sqlx/` against an empty one, as CI does.
    DbPrepare,
    /// Check `.sqlx/` against an empty, migrated database.
    DbPrepareCheck,
    /// fmt, clippy, and (when a query or migration is staged) the sqlx cache.
    Precommit,
    /// (Re)create the kind cluster with this checkout's recipes mounted.
    KindUp,
    /// Delete the kind cluster.
    KindDown,
    /// Build an image with podman and load it into the kind node.
    KindLoad {
        #[arg(value_parser = ["jobs", "api", "all"])]
        image: String,
    },
    /// Apply the kind overlay, then let the node pull from the in-cluster registry.
    KindDeploy,
    /// Make someone an admin on kind.
    KindAddAdmin { email: String },
    /// Port-forward the api, Mailpit and GlitchTip until interrupted.
    KindApi,
    /// Log a persona in with the code from Mailpit (session in target/kind/NAME).
    KindLogin { email: String, name: Option<String> },
    /// Run hammer on kind as a persona, e.g. `kind-as prof upload recipes/examples/linked-list-python --wait`.
    KindAs {
        name: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Rebuild the OS images through jobs' endpoint, e.g. `kind-rebuild force=true`.
    KindRebuild {
        query: Option<String>,
        /// Wait until no OS build is running, and fail if the latest failed.
        #[arg(long)]
        wait: bool,
    },
    /// The end-to-end tests on kind (all, or those whose names contain FILTER).
    E2e { filters: Vec<String> },
    /// Point git at the tracked hooks in .githooks/ (once per clone).
    InstallHooks,
    /// A psql shell on the dev database.
    DbShell,
    /// Serve the web UI on http://localhost:3000, rebuilding on change (/api/ goes to `kind-api`).
    WebDev,
    /// Build the web UI's static files into target/web.
    WebBuild,
    /// The web UI's component tests in headless Chrome (fetches a matching chromedriver if needed).
    WebTest,
    /// Build the web UI and check its first load against the budget (450 KB gzipped).
    WebBudget,
    /// Download and cache recipe ingredients (hammer vend; filters: --os/--platform/--toolchain).
    Vend {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Print an assignment's Containerfile (FROM the pre-built base image).
    Plan { assignment: String },
    /// Print an assignment's build as a tree: phases, layers, steps, runtime.
    Describe { assignment: String },
    /// Build a base OS image locally, e.g. `build-base ubuntu-22 pedagog/ubuntu:22`.
    BuildBase { os: String, tag: String },
    /// Build an assignment's image locally with podman (vendored ingredients if present).
    Build { assignment: String },
    /// Squash an assignment image's layers in place.
    Squash { assignment: String },
    /// Build an assignment's image, then run it.
    Dev {
        assignment: String,
        #[arg(long, default_value = ".")]
        workdir: String,
        #[arg(long, default_value_t = 8080)]
        port: u16,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Run an assignment image: WORKDIR (default .) at /workspace, PORT (default 8080) exposed.
    Run {
        assignment: String,
        #[arg(long, default_value = ".")]
        workdir: String,
        #[arg(long, default_value_t = 8080)]
        port: u16,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Local images: the registry prefix and the per-container limits (sized for
/// small student projects on an RK1 node).
const REGISTRY: &str = "localhost";
const MEMORY: &str = "512m";

fn main() -> ExitCode {
    match run(Cli::parse().task) {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("error: {why}");
            ExitCode::FAILURE
        }
    }
}

/// The container engine: podman, or `PEDAGOG_ENGINE` (docker on CI runners).
fn engine() -> String {
    std::env::var("PEDAGOG_ENGINE").unwrap_or_else(|_| "podman".into())
}

/// A command run in the repository's root, its output shown as it goes.
fn here(program: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(root());
    cmd
}

fn shown(cmd: &mut Command) -> Result<()> {
    let status = cmd
        .status()
        .map_err(|e| format!("couldn't run {:?}: {e}", cmd.get_program()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{:?} failed ({status})", cmd.get_program()))
    }
}

fn kind(args: &[&str]) -> Command {
    let mut cmd = here("kind", args);
    cmd.env("KIND_EXPERIMENTAL_PROVIDER", engine());
    cmd
}

fn run(task: Task) -> Result<()> {
    match task {
        Task::DbUp => {
            shown(&mut here(
                &engine(),
                &[
                    "run",
                    "-d",
                    "--replace",
                    "--name",
                    "pedagog-db",
                    "-e",
                    "POSTGRES_DB=pedagog",
                    "-e",
                    "POSTGRES_USER=pedagog",
                    "-e",
                    "POSTGRES_PASSWORD=pedagog",
                    "-p",
                    "5432:5432",
                    "-v",
                    "pedagog-pgdata:/var/lib/postgresql/data",
                    "docker.io/library/postgres:16-alpine",
                ],
            ))?;
            println!("dev Postgres up: {DEV_DB}");
            Ok(())
        }
        Task::DbDown => shown(&mut here(&engine(), &["rm", "-f", "pedagog-db"])),
        Task::DbPrepare => {
            empty_db()?;
            shown(
                here("cargo", &["sqlx", "migrate", "run", "--source", MIGRATIONS])
                    .env("DATABASE_URL", DEV_DB),
            )?;
            shown(
                here(
                    "cargo",
                    &[
                        "sqlx",
                        "prepare",
                        "--workspace",
                        "--",
                        "--all-targets",
                        "--all-features",
                    ],
                )
                .env("DATABASE_URL", PREPARE_DB),
            )
        }
        Task::DbPrepareCheck => prepare_check(),
        Task::Precommit => precommit(),
        Task::KindUp => {
            let clusters = ok(&mut kind(&["get", "clusters"])).unwrap_or_default();
            if clusters.lines().any(|c| c == "pedagog") {
                shown(&mut kind(&["delete", "cluster", "--name", "pedagog"]))?;
            }
            let config = std::fs::read_to_string(root().join("deploy/kind/cluster.yaml"))
                .map_err(|e| e.to_string())?
                .replace("@RECIPES@", &root().join("recipes").display().to_string());
            let mut child = kind(&["create", "cluster", "--config", "-"])
                .stdin(Stdio::piped())
                .spawn()
                .map_err(|e| e.to_string())?;
            std::io::Write::write_all(child.stdin.as_mut().ok_or("no stdin")?, config.as_bytes())
                .map_err(|e| e.to_string())?;
            drop(child.stdin.take());
            let status = child.wait().map_err(|e| e.to_string())?;
            if status.success() {
                Ok(())
            } else {
                Err("kind create cluster failed".into())
            }
        }
        Task::KindDown => shown(&mut kind(&["delete", "cluster", "--name", "pedagog"])),
        Task::KindLoad { image } => {
            if image == "jobs" || image == "all" {
                load(
                    "crates/jobs/Containerfile",
                    None,
                    "localhost/pedagog/jobs:dev",
                    "jobs-image.tar",
                )?;
                load(
                    "crates/jobs/Containerfile",
                    Some("init"),
                    "localhost/pedagog/jobs-init:dev",
                    "jobs-init-image.tar",
                )?;
            }
            if image == "api" || image == "all" {
                load(
                    "crates/api/Containerfile",
                    None,
                    "localhost/pedagog/api:dev",
                    "api-image.tar",
                )?;
            }
            Ok(())
        }
        Task::KindDeploy => {
            shown(kubectl(&["apply", "-k", "deploy/overlays/kind"]).current_dir(root()))?;
            registry_access()
        }
        Task::KindAddAdmin { email } => shown(&mut kubectl(&[
            "-n",
            "pedagog-system",
            "exec",
            "deploy/pedagog-api",
            "--",
            "/usr/local/bin/pedagog-api",
            "add-admin",
            &email,
        ])),
        Task::KindApi => {
            Forward::follow("pedagog-system", "pedagog-api", API_PORT, 8080)?;
            Forward::follow("pedagog-data", "mailpit", MAILPIT_PORT, 8025)?;
            Forward::follow("pedagog-data", "glitchtip", GLITCHTIP_PORT, 8000)?;
            println!(
                "api: http://localhost:{API_PORT}   mail: http://localhost:{MAILPIT_PORT}   errors: http://localhost:{GLITCHTIP_PORT}   (Ctrl-C to stop)"
            );
            loop {
                std::thread::park();
            }
        }
        Task::KindLogin { email, name } => {
            build_hammer()?;
            let _forwards = Forward::api_and_mail()?;
            let name = name.unwrap_or_else(|| email.split('@').next().unwrap_or(&email).to_owned());
            let persona = Persona::new(&name, &email);
            persona.login()?;
            shown(&mut persona.hammer(&["whoami"]))
        }
        Task::KindAs { name, args } => {
            build_hammer()?;
            let _forwards = Forward::api_and_mail()?;
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            // The email only matters for logging in.
            shown(&mut Persona::new(&name, "").hammer(&args))
        }
        Task::KindRebuild { query, wait } => {
            let _jobs = Forward::open("pedagog-builds", "pedagog-jobs", 18081, 8080)?;
            let url = format!(
                "http://localhost:18081/internal/recipes/rebuild?{}",
                query.unwrap_or_default()
            );
            let reply = reqwest::blocking::Client::new()
                .post(url)
                .send()
                .and_then(|r| r.text())
                .map_err(|e| e.to_string())?;
            println!("{reply}");
            if wait {
                let running = || {
                    pedagog_e2e::sql("SELECT count(*) FROM build_runs WHERE status = 'running'")
                        .unwrap_or_default()
                };
                if !pedagog_e2e::wait_for(std::time::Duration::from_secs(45 * 60), || {
                    running() == "0"
                }) {
                    return Err("OS builds still running after 45 minutes".into());
                }
                let failed = pedagog_e2e::sql(
                    "SELECT count(*) FROM (SELECT DISTINCT ON (os_id) status FROM build_runs
                      ORDER BY os_id, id DESC) latest WHERE status = 'failed'",
                )?;
                if failed != "0" {
                    return Err(format!("{failed} OS build(s) failed"));
                }
                println!("OS images built");
            }
            Ok(())
        }
        Task::InstallHooks => {
            shown(&mut here("git", &["config", "core.hooksPath", ".githooks"]))?;
            println!("git hooks installed (core.hooksPath = .githooks)");
            Ok(())
        }
        Task::DbShell => shown(&mut here(
            &engine(),
            &[
                "exec",
                "-it",
                "pedagog-db",
                "psql",
                "-U",
                "pedagog",
                "-d",
                "pedagog",
            ],
        )),
        Task::WebDev => shown(here("trunk", &["serve"]).current_dir(root().join("crates/web"))),
        Task::WebBuild => {
            shown(here("trunk", &["build", "--release"]).current_dir(root().join("crates/web")))
        }
        Task::WebTest => web_test(),
        Task::WebBudget => web_budget(),
        Task::Vend { args } => {
            let args: Vec<&str> = std::iter::once("vend")
                .chain(args.iter().map(String::as_str))
                .collect();
            shown(&mut hammer(&args))
        }
        Task::Plan { assignment } => shown(&mut hammer(&[
            "plan",
            "-a",
            &assignment,
            "--registry",
            REGISTRY,
        ])),
        Task::Describe { assignment } => shown(&mut hammer(&[
            "plan",
            "-a",
            &assignment,
            "--registry",
            REGISTRY,
            "-f",
            "describe",
        ])),
        Task::BuildBase { os, tag } => {
            let plan = ok(&mut hammer(&["plan", "--os", &os]))?;
            let recipes = std::env::var("HAMMER_RECIPES").unwrap_or_default();
            podman_build_stdin(
                &plan,
                &[
                    "--volume",
                    &format!("{recipes}:/pedagog/recipes:ro,z"),
                    "-t",
                    &format!("{REGISTRY}/{tag}"),
                    "-",
                ],
            )
        }
        Task::Build { assignment } => {
            let plan = ok(&mut hammer(&[
                "plan",
                "-a",
                &assignment,
                "--registry",
                REGISTRY,
            ]))?;
            let recipes = std::env::var("HAMMER_RECIPES").unwrap_or_default();
            let dir = Path::new(&assignment)
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| ".".into());
            podman_build_stdin(
                &plan,
                &[
                    "--volume",
                    &format!("{recipes}/ingredients:/pedagog/ingredients:ro,z"),
                    "-t",
                    &image_tag(&assignment),
                    "-f",
                    "-",
                    &dir,
                ],
            )
        }
        Task::Squash { assignment } => {
            let tag = image_tag(&assignment);
            podman_build_stdin(&format!("FROM {tag}\n"), &["--squash-all", "-t", &tag, "-"])
        }
        Task::Dev {
            assignment,
            workdir,
            port,
            args,
        } => {
            run(Task::Build {
                assignment: assignment.clone(),
            })?;
            run(Task::Run {
                assignment,
                workdir,
                port,
                args,
            })
        }
        Task::Run {
            assignment,
            workdir,
            port,
            args,
        } => {
            let workdir = std::fs::canonicalize(&workdir).map_err(|e| format!("{workdir}: {e}"))?;
            let mut cmd = here(
                &engine(),
                &[
                    "run",
                    "--rm",
                    "-it",
                    "--memory",
                    MEMORY,
                    "--memory-swap",
                    MEMORY,
                    "--cpus",
                    "1",
                    "--pids-limit",
                    "256",
                ],
            );
            cmd.arg("-v")
                .arg(format!("{}:/workspace", workdir.display()));
            cmd.args(["-p", &format!("{port}:{port}"), &image_tag(&assignment)])
                .args(&args);
            shown(&mut cmd)
        }
        Task::E2e { filters } => {
            let mut cmd = here(
                "cargo",
                &["test", "-q", "-p", "pedagog-e2e", "--test", "kind", "--"],
            );
            cmd.args(&filters).env("PEDAGOG_E2E", "1");
            // For the browser checks.
            cmd.env("CHROMEDRIVER", find_chromedriver()?);
            shown(&mut cmd)
        }
    }
}

fn hammer(args: &[&str]) -> Command {
    let mut cmd = here("cargo", &["run", "-q", "-p", "hammer", "--"]);
    cmd.args(args);
    cmd
}

/// `localhost/pedagog/<the assignment file's stem>`.
fn image_tag(assignment: &str) -> String {
    let stem = Path::new(assignment)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{REGISTRY}/pedagog/{stem}")
}

/// `podman build ARGS` with `containerfile` on stdin.
fn podman_build_stdin(containerfile: &str, args: &[&str]) -> Result<()> {
    let mut child = here(&engine(), &["build"])
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    std::io::Write::write_all(
        child.stdin.as_mut().ok_or("no stdin")?,
        containerfile.as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    drop(child.stdin.take());
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("podman build failed".into())
    }
}

fn build_hammer() -> Result<()> {
    shown(&mut here("cargo", &["build", "-q", "-p", "hammer"]))
}

fn empty_db() -> Result<()> {
    ok(&mut here(
        &engine(),
        &[
            "exec",
            "pedagog-db",
            "psql",
            "-U",
            "pedagog",
            "-q",
            "-c",
            "DROP DATABASE IF EXISTS pedagog_prepare",
            "-c",
            "CREATE DATABASE pedagog_prepare",
        ],
    ))?;
    ok(
        here("cargo", &["sqlx", "migrate", "run", "--source", MIGRATIONS])
            .env("DATABASE_URL", PREPARE_DB),
    )
    .map(drop)
}

fn prepare_check() -> Result<()> {
    empty_db()?;
    shown(
        here(
            "cargo",
            &[
                "sqlx",
                "prepare",
                "--workspace",
                "--check",
                "--",
                "--all-targets",
                "--all-features",
            ],
        )
        .env("DATABASE_URL", PREPARE_DB),
    )
}

/// Whether a staged change can affect the sqlx cache: a migration or cache
/// file, or a Rust file with query macros.
fn touches_queries(staged: &[String], read: impl Fn(&str) -> Option<String>) -> bool {
    staged.iter().any(|f| {
        f.starts_with(MIGRATIONS) || f.starts_with(".sqlx/") || {
            f.ends_with(".rs")
                && read(f).is_some_and(|t| {
                    ["query!(", "query_as!(", "query_scalar!("]
                        .iter()
                        .any(|m| t.contains(m))
                })
        }
    })
}

fn precommit() -> Result<()> {
    println!("pre-commit: cargo fmt --check");
    shown(&mut here("cargo", &["fmt", "--check"]))?;
    println!("pre-commit: cargo clippy --workspace --all-targets --all-features -D warnings");
    shown(&mut here(
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    ))?;
    println!("pre-commit: cargo clippy -p pedagog-web --target wasm32-unknown-unknown -D warnings");
    shown(&mut here(
        "cargo",
        &[
            "clippy",
            "-p",
            "pedagog-web",
            "--target",
            "wasm32-unknown-unknown",
            "--",
            "-D",
            "warnings",
        ],
    ))?;
    let staged = ok(&mut here(
        "git",
        &["diff", "--cached", "--name-only", "--diff-filter=d"],
    ))?;
    let staged: Vec<String> = staged.lines().map(str::to_owned).collect();
    if touches_queries(&staged, |f| std::fs::read_to_string(root().join(f)).ok()) {
        println!("pre-commit: sqlx offline cache");
        prepare_check().map_err(|_| {
            "the .sqlx/ cache is stale, or the dev DB is down: `cargo xtask db-up`, then `cargo xtask db-prepare` and stage .sqlx/".to_owned()
        })?;
    }
    Ok(())
}

fn load(containerfile: &str, target: Option<&str>, tag: &str, archive: &str) -> Result<()> {
    let mut build = vec!["build", "-f", containerfile, "-t", tag];
    if let Some(t) = target {
        build.extend_from_slice(&["--target", t]);
    }
    build.push(".");
    shown(&mut here(&engine(), &build))?;
    let archive = Path::new("target").join(archive).display().to_string();
    shown(&mut here(
        &engine(),
        &["save", "--format", "docker-archive", "-o", &archive, tag],
    ))?;
    shown(&mut kind(&[
        "load",
        "image-archive",
        &archive,
        "--name",
        "pedagog",
    ]))
}

/// Let the node pull from the in-cluster registry by its Service name over
/// plain HTTP (validation pods run built images). Idempotent.
fn registry_access() -> Result<()> {
    let ip = ok(&mut kubectl(&[
        "-n",
        "pedagog-data",
        "get",
        "svc",
        "registry-service",
        "-o",
        "jsonpath={.spec.clusterIP}",
    ]))?;
    let node = "pedagog-control-plane";
    let enable = r#"grep -q config_path /etc/containerd/config.toml || {
  printf '\n[plugins."io.containerd.grpc.v1.cri".registry]\n  config_path = "/etc/containerd/certs.d"\n' >> /etc/containerd/config.toml
  systemctl restart containerd
}"#;
    ok(&mut here(&engine(), &["exec", node, "sh", "-c", enable]))?;
    let dir = "/etc/containerd/certs.d/registry-service.pedagog-data.svc:5000";
    let hosts = format!(
        "server = \"http://{ip}:5000\"\n\n[host.\"http://{ip}:5000\"]\n  capabilities = [\"pull\", \"resolve\"]\n"
    );
    ok(&mut here(
        &engine(),
        &[
            "exec",
            node,
            "sh",
            "-c",
            &format!("mkdir -p '{dir}' && cat > '{dir}/hosts.toml' <<'EOF'\n{hosts}EOF"),
        ],
    ))?;
    println!(
        "kind node ({CONTEXT}) pulls registry-service.pedagog-data.svc:5000 from http://{ip}:5000"
    );
    Ok(())
}

/// The first load's budget: wasm, js, css and the preloaded fonts, gzipped.
const WEB_BUDGET: u64 = 450 * 1024;

fn web_budget() -> Result<()> {
    run(Task::WebBuild)?;
    let dist = root().join("target/web");
    let index = std::fs::read_to_string(dist.join("index.html")).map_err(|e| e.to_string())?;
    let mut files: Vec<String> = std::fs::read_dir(&dist)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|f| [".wasm", ".js", ".css"].iter().any(|x| f.ends_with(x)))
        .collect();
    files.extend(preloaded(&index));
    let mut total = 0;
    for f in &files {
        let path = dist.join(f.trim_start_matches('/'));
        // woff2 is compressed already; gzip the rest as the api serves them.
        let size = if f.ends_with(".woff2") {
            std::fs::metadata(&path)
                .map(|m| m.len())
                .map_err(|e| format!("{f}: {e}"))?
        } else {
            ok(Command::new("sh")
                .arg("-c")
                .arg(format!("gzip -9c '{}' | wc -c", path.display())))?
            .trim()
            .parse::<u64>()
            .map_err(|e| e.to_string())?
        };
        println!("{:>8} KB  {f}", size / 1024);
        total += size;
    }
    println!(
        "{:>8} KB  first load (budget {} KB)",
        total / 1024,
        WEB_BUDGET / 1024
    );
    if total > WEB_BUDGET {
        return Err(format!(
            "the web UI's first load is {} KB, over its {} KB budget",
            total / 1024,
            WEB_BUDGET / 1024
        ));
    }
    Ok(())
}

/// The fonts `index.html` preloads (`<link rel="preload" href="…" as="font">`).
fn preloaded(index: &str) -> Vec<String> {
    index
        .split("<link")
        .filter(|l| l.contains("rel=\"preload\"") && l.contains("as=\"font\""))
        .filter_map(|l| {
            l.split("href=\"")
                .nth(1)?
                .split('"')
                .next()
                .map(str::to_owned)
        })
        .collect()
}

fn web_test() -> Result<()> {
    let wanted = wasm_bindgen_version()?;
    let have = ok(&mut here("wasm-bindgen-test-runner", &["--version"])).unwrap_or_default();
    if !have.contains(&wanted) {
        println!("installing wasm-bindgen-cli {wanted} (it must match Cargo.lock)");
        shown(&mut here(
            "cargo",
            &[
                "install",
                "wasm-bindgen-cli",
                "--version",
                &wanted,
                "--locked",
            ],
        ))?;
    }
    let driver = find_chromedriver()?;
    shown(
        here(
            "cargo",
            &[
                "test",
                "-p",
                "pedagog-web",
                "--target",
                "wasm32-unknown-unknown",
            ],
        )
        .env("CHROMEDRIVER", driver),
    )
}

/// The `wasm-bindgen` version in Cargo.lock.
fn wasm_bindgen_version() -> Result<String> {
    let lock = std::fs::read_to_string(root().join("Cargo.lock")).map_err(|e| e.to_string())?;
    lock.split("[[package]]")
        .find(|p| p.contains("name = \"wasm-bindgen\"\n"))
        .and_then(|p| {
            p.split("version = \"")
                .nth(1)?
                .split('"')
                .next()
                .map(str::to_owned)
        })
        .ok_or_else(|| "no wasm-bindgen in Cargo.lock".into())
}

/// A chromedriver: `CHROMEDRIVER`, the GitHub runner's (`CHROMEWEBDRIVER`),
/// one on PATH, or one fetched for this machine's Chrome.
fn find_chromedriver() -> Result<String> {
    if let Ok(d) = std::env::var("CHROMEDRIVER") {
        return Ok(d);
    }
    if let Ok(dir) = std::env::var("CHROMEWEBDRIVER") {
        let d = Path::new(&dir).join("chromedriver");
        if d.exists() {
            return Ok(d.display().to_string());
        }
    }
    match ok(&mut here("which", &["chromedriver"])) {
        Ok(found) if !found.trim().is_empty() => Ok(found.trim().to_owned()),
        _ => chromedriver(),
    }
}

/// A chromedriver for this machine's Chrome, from Chrome for Testing, kept in target/tools.
fn chromedriver() -> Result<String> {
    let chrome = if cfg!(target_os = "macos") {
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
    } else {
        "google-chrome"
    };
    let version = ok(Command::new(chrome).arg("--version"))
        .map_err(|_| "Chrome isn't installed (or set CHROMEDRIVER)".to_owned())?;
    let full = version
        .split_whitespace()
        .last()
        .unwrap_or_default()
        .to_owned();
    let build = full
        .rsplit_once('.')
        .map(|(b, _)| b.to_owned())
        .unwrap_or(full);
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "mac-arm64",
        ("macos", _) => "mac-x64",
        _ => "linux64",
    };
    let tools = root().join("target/tools");
    let driver = tools.join(format!("chromedriver-{platform}/chromedriver"));
    let stamp = tools.join(format!("chromedriver-{platform}/for-chrome"));
    if driver.exists() && std::fs::read_to_string(&stamp).is_ok_and(|s| s == build) {
        return Ok(driver.display().to_string());
    }
    let index = reqwest::blocking::get(
        "https://googlechromelabs.github.io/chrome-for-testing/latest-patch-versions-per-build-with-downloads.json",
    )
    .and_then(|r| r.text())
    .map_err(|e| format!("Chrome for Testing: {e}"))?;
    let url = chromedriver_url(&index, &build, platform)
        .ok_or_else(|| format!("no chromedriver for Chrome {build} ({platform})"))?;
    std::fs::create_dir_all(&tools).map_err(|e| e.to_string())?;
    let zip = tools.join("chromedriver.zip");
    let bytes = reqwest::blocking::get(&url)
        .and_then(|r| r.bytes())
        .map_err(|e| e.to_string())?;
    std::fs::write(&zip, &bytes).map_err(|e| e.to_string())?;
    ok(Command::new("unzip")
        .arg("-oq")
        .arg(&zip)
        .arg("-d")
        .arg(&tools))?;
    std::fs::write(&stamp, &build).map_err(|e| e.to_string())?;
    println!("chromedriver for Chrome {build}: {}", driver.display());
    Ok(driver.display().to_string())
}

/// The download URL for `platform`'s chromedriver for Chrome `build` (major.minor.build).
fn chromedriver_url(index: &str, build: &str, platform: &str) -> Option<String> {
    let index: serde_json::Value = serde_json::from_str(index).ok()?;
    index["builds"][build]["downloads"]["chromedriver"]
        .as_array()?
        .iter()
        .find(|d| d["platform"] == platform)?["url"]
        .as_str()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budgets_count_preloaded_fonts_and_drivers_match_chrome() {
        let index = r#"<link rel="preload" href="/fonts/a.woff2" as="font" type="font/woff2" crossorigin />
            <link rel="stylesheet" href="/x.css" />"#;
        assert_eq!(preloaded(index), ["/fonts/a.woff2"]);
        let cft = r#"{"builds": {"155.0.8059": {"downloads": {"chromedriver": [
            {"platform": "linux64", "url": "https://x/linux64.zip"},
            {"platform": "mac-arm64", "url": "https://x/mac-arm64.zip"}]}}}}"#;
        assert_eq!(
            chromedriver_url(cft, "155.0.8059", "mac-arm64").as_deref(),
            Some("https://x/mac-arm64.zip")
        );
        assert_eq!(chromedriver_url(cft, "154.0.1", "linux64"), None);
    }

    #[test]
    fn images_are_named_after_their_file() {
        assert_eq!(
            image_tag("recipes/examples/linked-list.yaml"),
            "localhost/pedagog/linked-list"
        );
    }

    #[test]
    fn query_changes_are_spotted() {
        let read = |f: &str| match f {
            "crates/store/src/db/x.rs" => Some("sqlx::query!(\"SELECT 1\")".to_owned()),
            _ => Some("fn main() {}".to_owned()),
        };
        let s = |files: &[&str]| files.iter().map(|f| f.to_string()).collect::<Vec<_>>();
        assert!(touches_queries(
            &s(&["crates/store/migrations/0016_x.up.sql"]),
            read
        ));
        assert!(touches_queries(&s(&[".sqlx/query-1.json"]), read));
        assert!(touches_queries(&s(&["crates/store/src/db/x.rs"]), read));
        assert!(!touches_queries(
            &s(&["crates/web/src/main.rs", "README.md"]),
            read
        ));
    }
}
