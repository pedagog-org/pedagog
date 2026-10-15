use std::fmt::Write;

use super::{
    AssignmentFiles, BuildPhase, BuildPlan, FromSource, GRADER_USER, ImageSpec, Layer, LayerSource,
    Render, RenderOptions, ResolvedStep, Runtime, SOLUTION_DIR, STARTER_DIR, TESTS_DIR,
};

pub struct Containerfile(String);

// Display gives the `ToString` impl the `Render` trait requires, for free.
impl std::fmt::Display for Containerfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Render for Containerfile {
    // Phase order (Full build). Each phase is one or more RUN layers, in order:
    //   1. os           — base OS package-manager init (only when FROM upstream)
    //   2. platform     — platform build steps (e.g. install code-server)
    //   3. toolchain    — each requested toolchain, in listed order
    //   4. assignment   — the assignment's own build steps
    //   5. os_cleanup   — OS build cleanup
    //   6. files        — archive starter/tests/solution; last, so editing tests
    //                     only rebuilds this layer
    // Then runtime metadata: EXPOSE ports, then USER + ENTRYPOINT (see render_runtime).
    fn render(spec: &ImageSpec, opts: &RenderOptions) -> Self {
        let mut to = String::new();
        match spec {
            ImageSpec::Base { upstream, os, .. } => {
                render_from(&mut to, upstream);
                render_phase(&mut to, BuildPhase::Base, &[os]);
            }
            ImageSpec::Full {
                upstream,
                base_image,
                plan,
                runtime,
                ports,
                files,
                ..
            } => {
                render_from(&mut to, &opts.full_from(upstream, base_image));
                render_phases(&mut to, plan, opts.from);
                render_files(&mut to, files);

                for port in ports {
                    let _ = writeln!(to, "EXPOSE {port}");
                }
                render_runtime(&mut to, runtime);
            }
        }
        Containerfile(to)
    }
}

fn render_phases(to: &mut String, plan: &BuildPlan, from: FromSource) {
    // The os layer is inline only when building standalone; under PrebuiltBase
    // it already lives in the base image.
    if matches!(from, FromSource::Standalone) {
        render_phase(to, BuildPhase::Base, &[&plan.os]);
    }
    render_phase(to, BuildPhase::Platform, &[&plan.platform]);
    render_phase(
        to,
        BuildPhase::Toolchain,
        &plan.toolchain.iter().collect::<Vec<_>>(),
    );
    render_phase(to, BuildPhase::Assignment, &[&plan.assignment]);
    render_phase(to, BuildPhase::OsCleanup, &[&plan.os_cleanup]);
}

/// COPY the archive's directories from the build context. Tests and solution go
/// to grader-owned 0700 dirs (no one else can traverse them); starter is
/// root-owned and read-only.
fn render_files(to: &mut String, files: &AssignmentFiles) {
    if files.is_empty() {
        return;
    }
    let _ = writeln!(to, "\n# ===== PHASE: {} =====", BuildPhase::AssignmentFiles);
    let mut fixups = Vec::new();
    if let Some(src) = &files.starter {
        let _ = writeln!(to, "COPY {src}/ {STARTER_DIR}/");
        fixups.push(format!("chmod -R go-w {STARTER_DIR}"));
    }
    let mut private = Vec::new();
    for (src, dest) in [(&files.tests, TESTS_DIR), (&files.solution, SOLUTION_DIR)] {
        if let Some(src) = src {
            let _ = writeln!(
                to,
                "COPY --chown={GRADER_USER}:{GRADER_USER} {src}/ {dest}/"
            );
            private.push(dest);
        }
    }
    if !private.is_empty() {
        fixups.push(format!("chmod 0700 {}", private.join(" ")));
    }
    render_run(to, &fixups.join(" && "));
}

fn render_phase(to: &mut String, phase: BuildPhase, layers: &[&Layer]) {
    if layers.iter().all(|l| l.steps.is_empty()) {
        return;
    }
    let _ = writeln!(to, "\n# ===== PHASE: {phase} =====");
    for layer in layers {
        render_layer(to, layer);
    }
}

fn render_layer(to: &mut String, layer: &Layer) {
    if layer.steps.is_empty() {
        return;
    }
    let _ = writeln!(to, "\n# ----- {} -----", layer.source.label());
    // The `vend` helper resolves ingredient paths from these per-layer vars.
    let (ptype, pid) = layer_env(&layer.source);
    let _ = writeln!(to, "ENV PEDAGOG_TYPE={ptype}");
    let _ = writeln!(to, "ENV PEDAGOG_ID={pid}");
    for step in &layer.steps {
        render_step(to, step);
    }
}

fn render_step(to: &mut String, step: &ResolvedStep) {
    if let Some(name) = &step.name {
        let _ = writeln!(to, "# {name}");
    }
    for cmd in &step.commands {
        render_run(to, &cmd.0);
    }
}

/// Where a multi-line step's script is written inside the build.
const STEP_SCRIPT: &str = "/tmp/pedagog-step.sh";

fn render_run(to: &mut String, cmd: &str) {
    // Multi-line commands run as one script with `sh -e`. Kaniko doesn't support
    // heredoc RUNs (it silently runs nothing), so the script travels base64-encoded
    // and is shown above the RUN as comments.
    if cmd.contains('\n') {
        for line in cmd.lines() {
            let _ = writeln!(to, "#   {line}");
        }
        let script = format!("{}\n", cmd.trim_end_matches('\n'));
        let _ = writeln!(
            to,
            "RUN echo '{}' | base64 -d > {STEP_SCRIPT} && sh -e {STEP_SCRIPT}; s=$?; rm -f {STEP_SCRIPT}; exit $s",
            base64(script.as_bytes())
        );
    } else {
        let _ = writeln!(to, "RUN {cmd}");
    }
}

/// Standard base64 with padding (RFC 4648), as `base64 -d` reads it.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn render_from(to: &mut String, from: &str) {
    let _ = writeln!(to, "FROM {from}");
}

fn render_runtime(to: &mut String, rt: &Runtime) {
    let _ = writeln!(to);
    let _ = writeln!(to, "USER {}", rt.user);
    let _ = writeln!(to, "ENTRYPOINT {}", exec_array(&rt.entrypoint));
}

fn exec_array(words: &[String]) -> String {
    serde_json::to_string(words).unwrap_or_default()
}

/// (PEDAGOG_TYPE, PEDAGOG_ID) for a layer — the `vend` helper joins these into
/// the ingredient path `/pedagog/ingredients/<type>/<id>/<file>`, matching how
/// `hammer vend` lays ingredients out on disk.
fn layer_env(source: &LayerSource) -> (String, String) {
    match source {
        LayerSource::Os(id) => ("os".to_owned(), id.to_string()),
        LayerSource::Platform(kind) => ("platform".to_owned(), kind.to_string()),
        LayerSource::Toolchain { tc, .. } => {
            ("toolchain".to_owned(), format!("{}/{}", tc.id, tc.version))
        }
        LayerSource::Assignment(id) => ("assignment".to_owned(), id.to_string()),
        LayerSource::OsCleanup(id) => ("os".to_owned(), id.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::network::NetworkSpec;
    use crate::recipe::platform::PlatformKind;
    use crate::recipe::primitives::{Command, Id};

    fn id(s: &str) -> Id {
        Id::try_from(s.to_owned()).unwrap()
    }

    fn step(name: &str, cmds: &[&str]) -> ResolvedStep {
        ResolvedStep {
            name: Some(name.to_owned()),
            commands: cmds.iter().map(|c| Command((*c).to_owned())).collect(),
        }
    }

    fn os_layer() -> Layer {
        Layer {
            source: LayerSource::Os(id("ubuntu-22")),
            steps: vec![step("init", &["apt-get update"])],
        }
    }

    fn full_plan() -> BuildPlan {
        BuildPlan {
            os: os_layer(),
            platform: Layer {
                source: LayerSource::Platform(PlatformKind::Interactive),
                steps: vec![step("code-server", &["install cs"])],
            },
            toolchain: vec![],
            assignment: Layer {
                source: LayerSource::Assignment(id("hw1")),
                steps: vec![],
            },
            os_cleanup: Layer {
                source: LayerSource::OsCleanup(id("ubuntu-22")),
                steps: vec![],
            },
        }
    }

    fn full_spec(entrypoint: &[&str], ports: Vec<u16>, network: NetworkSpec) -> ImageSpec {
        ImageSpec::Full {
            upstream: "ubuntu:22.04".to_owned(),
            base_image: "pedagog/ubuntu:22".to_owned(),
            plan: full_plan(),
            runtime: Runtime {
                user: "student".to_owned(),
                entrypoint: entrypoint.iter().map(|s| (*s).to_owned()).collect(),
            },
            ports,
            network,
            testing: None,
            files: AssignmentFiles::default(),
        }
    }

    fn opts(registry: Option<&str>, from: FromSource) -> RenderOptions {
        RenderOptions {
            registry: registry.map(|r| r.to_owned()),
            from,
        }
    }

    #[test]
    fn base_renders_from_upstream_without_runtime() {
        let spec = ImageSpec::Base {
            upstream: "ubuntu:22.04".to_owned(),
            image: "pedagog/ubuntu:22".to_owned(),
            os: os_layer(),
        };
        let out = Containerfile::render(&spec, &opts(None, FromSource::Standalone)).to_string();
        assert!(out.starts_with("FROM ubuntu:22.04\n"), "{out}");
        assert!(out.contains("RUN apt-get update"));
        assert!(!out.contains("ENTRYPOINT"));
        assert!(!out.contains("USER"));
    }

    #[test]
    fn full_standalone_has_user_and_exec_entrypoint() {
        let spec = full_spec(
            &[
                "code-server",
                "--bind",
                "0.0.0.0:8080",
                "/home/student/my work",
            ],
            vec![8080],
            NetworkSpec::Allow,
        );
        let out = Containerfile::render(&spec, &opts(None, FromSource::Standalone)).to_string();
        assert!(out.contains("FROM ubuntu:22.04"));
        assert!(out.contains("PHASE: BASE"));
        assert!(out.contains("PHASE: PLATFORM"));
        assert!(
            !out.contains("PHASE: OS CLEANUP"),
            "empty phase must be skipped"
        );
        assert!(out.contains("EXPOSE 8080"));
        assert!(out.contains("USER student"));
        assert!(
            out.contains(
                r#"ENTRYPOINT ["code-server","--bind","0.0.0.0:8080","/home/student/my work"]"#
            ),
            "{out}"
        );
    }

    #[test]
    fn full_prebuilt_skips_os_layer() {
        let spec = full_spec(&["code-server"], vec![], NetworkSpec::Allow);
        let out = Containerfile::render(&spec, &opts(Some("localhost"), FromSource::PrebuiltBase))
            .to_string();
        assert!(out.contains("FROM localhost/pedagog/ubuntu:22"), "{out}");
        assert!(
            !out.contains("PHASE: BASE"),
            "os layer baked into base image"
        );
    }

    #[test]
    fn registry_never_prefixes_upstream() {
        let base = ImageSpec::Base {
            upstream: "ubuntu:22.04".to_owned(),
            image: "pedagog/ubuntu:22".to_owned(),
            os: os_layer(),
        };
        let out = Containerfile::render(&base, &opts(Some("localhost"), FromSource::Standalone))
            .to_string();
        assert!(out.starts_with("FROM ubuntu:22.04\n"), "{out}");

        let full = full_spec(&["code-server"], vec![], NetworkSpec::Allow);
        let out = Containerfile::render(&full, &opts(Some("localhost"), FromSource::Standalone))
            .to_string();
        assert!(out.starts_with("FROM ubuntu:22.04\n"), "{out}");
    }

    #[test]
    fn prebuilt_without_registry_is_unprefixed() {
        let spec = full_spec(&["code-server"], vec![], NetworkSpec::Allow);
        let out = Containerfile::render(&spec, &opts(None, FromSource::PrebuiltBase)).to_string();
        assert!(out.starts_with("FROM pedagog/ubuntu:22\n"), "{out}");
    }

    #[test]
    fn network_policy_does_not_change_the_image() {
        let render = |network| {
            Containerfile::render(
                &full_spec(&["code-server"], vec![], network),
                &opts(None, FromSource::Standalone),
            )
            .to_string()
        };
        assert_eq!(
            render(NetworkSpec::Allow),
            render(NetworkSpec::Deny { allow: vec![] })
        );
    }

    #[test]
    fn layers_emit_pedagog_env_for_vend() {
        let spec = full_spec(&["code-server"], vec![], NetworkSpec::Allow);
        let out = Containerfile::render(&spec, &opts(None, FromSource::PrebuiltBase)).to_string();
        assert!(out.contains("ENV PEDAGOG_TYPE=platform"), "{out}");
        assert!(out.contains("ENV PEDAGOG_ID=interactive"), "{out}");
    }

    #[test]
    fn multiline_command_runs_as_an_encoded_script() {
        let spec = ImageSpec::Base {
            upstream: "ubuntu:22.04".to_owned(),
            image: "pedagog/ubuntu:22".to_owned(),
            os: Layer {
                source: LayerSource::Os(id("ubuntu-22")),
                steps: vec![step("multi", &["if true; then\n  echo hi\nfi"])],
            },
        };
        let out = Containerfile::render(&spec, &opts(None, FromSource::Standalone)).to_string();
        assert!(
            !out.contains("<<"),
            "no heredocs (Kaniko skips them): {out}"
        );
        assert!(
            out.contains("#   if true; then\n#     echo hi\n#   fi\n"),
            "the script is shown as comments: {out}"
        );
        let encoded = base64(b"if true; then\n  echo hi\nfi\n");
        assert!(
            out.contains(&format!(
                "RUN echo '{encoded}' | base64 -d > /tmp/pedagog-step.sh && sh -e /tmp/pedagog-step.sh; s=$?; rm -f /tmp/pedagog-step.sh; exit $s"
            )),
            "{out}"
        );
    }

    #[test]
    fn base64_matches_rfc_4648() {
        for (input, expected) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
            (&[0xff, 0xfe, 0x00][..], "//4A"),
        ] {
            assert_eq!(base64(input), expected);
        }
    }

    fn render_with(files: AssignmentFiles) -> String {
        let spec = full_spec(&["code-server"], vec![], NetworkSpec::Allow).with_files(files);
        Containerfile::render(&spec, &opts(None, FromSource::PrebuiltBase)).to_string()
    }

    #[test]
    fn assignment_files_phase_bakes_archive_dirs_before_runtime() {
        let out = render_with(AssignmentFiles {
            starter: Some("starter".into()),
            tests: Some("tests".into()),
            solution: Some("solution".into()),
        });
        assert!(
            out.contains(
                "# ===== PHASE: ASSIGNMENT FILES =====\n\
                 COPY starter/ /pedagog/starter/\n\
                 COPY --chown=grader:grader tests/ /pedagog/tests/\n\
                 COPY --chown=grader:grader solution/ /pedagog/solution/\n\
                 RUN chmod -R go-w /pedagog/starter && chmod 0700 /pedagog/tests /pedagog/solution\n"
            ),
            "{out}"
        );
        let files_at = out.find("ASSIGNMENT FILES").unwrap();
        assert!(files_at < out.find("USER student").unwrap(), "{out}");
    }

    #[test]
    fn only_present_dirs_are_copied() {
        let out = render_with(AssignmentFiles {
            tests: Some("tests".into()),
            ..Default::default()
        });
        assert!(!out.contains("COPY starter/"), "{out}");
        assert!(!out.contains("solution/"), "{out}");
        assert!(out.contains("RUN chmod 0700 /pedagog/tests\n"), "{out}");
    }

    #[test]
    fn configured_sources_copy_to_fixed_image_paths() {
        let out = render_with(AssignmentFiles {
            starter: Some("src".into()),
            tests: Some("grading/hidden".into()),
            solution: Some("reference".into()),
        });
        assert!(out.contains("COPY src/ /pedagog/starter/\n"), "{out}");
        assert!(
            out.contains("COPY --chown=grader:grader grading/hidden/ /pedagog/tests/\n"),
            "{out}"
        );
        assert!(
            out.contains("COPY --chown=grader:grader reference/ /pedagog/solution/\n"),
            "{out}"
        );
    }

    #[test]
    fn no_files_means_no_phase() {
        assert!(!render_with(AssignmentFiles::default()).contains("ASSIGNMENT FILES"));
    }

    fn assignment(extra: &str) -> crate::recipe::assignment::AssignmentYaml {
        serde_yaml::from_str(&format!(
            "id: t\nname: T\nenvironment:\n  os: ubuntu-22\n  platform:\n    kind: interactive\n{extra}"
        ))
        .unwrap()
    }

    #[test]
    fn testing_requires_tests_but_starter_and_solution_are_optional() {
        let with_testing =
            assignment("testing:\n  framework: pytest:1\nfiles:\n  tests: grading\n");
        let none = AssignmentFiles::default();
        assert_eq!(
            none.problems(&with_testing),
            ["testing.framework is pytest:1 but the archive has no grading/ directory"]
        );
        let tests_only = AssignmentFiles {
            tests: Some("grading".into()),
            ..Default::default()
        };
        assert!(tests_only.problems(&with_testing).is_empty());
        assert!(
            none.problems(&assignment("")).is_empty(),
            "no testing, nothing required"
        );
    }
}
