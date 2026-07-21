use std::borrow::Cow;
use std::fmt::Write;

use super::{
    BuildPhase, BuildPlan, FromSource, ImageSpec, Layer, LayerSource, Render, RenderOptions,
    ResolvedStep, Runtime,
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
    //   5. os_configure — network.transcribe (Deny allowlist); omitted when Allow
    //   6. os_cleanup   — OS build cleanup; runs last
    // Then runtime metadata: EXPOSE ports, then USER + ENTRYPOINT (see render_runtime).
    fn render(spec: &ImageSpec, opts: &RenderOptions) -> Self {
        let mut to = String::new();
        match spec {
            ImageSpec::Base { upstream, os, .. } => {
                render_from(&mut to, &prefixed(opts.registry.as_deref(), upstream));
                render_phase(&mut to, BuildPhase::Base, &[os]);
            }
            ImageSpec::Full {
                upstream,
                base_image,
                plan,
                runtime,
                ports,
            } => {
                let from = match opts.from {
                    FromSource::Standalone => upstream,
                    FromSource::PrebuiltBase => base_image,
                };
                render_from(&mut to, &prefixed(opts.registry.as_deref(), from));
                render_phases(&mut to, plan, opts.from);

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
    if let Some(configure) = &plan.os_configure {
        render_phase(to, BuildPhase::OsConfigure, &[configure]);
    }
    render_phase(to, BuildPhase::OsCleanup, &[&plan.os_cleanup]);
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
    let _ = writeln!(to, "\n# ----- {} -----", layer_label(&layer.source));
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

fn render_run(to: &mut String, cmd: &str) {
    // Multi-line commands run as a single heredoc script; the quoted delimiter
    // prevents Dockerfile-level expansion of the body.
    if cmd.contains('\n') {
        let _ = writeln!(to, "RUN <<'EOF'");
        for line in cmd.lines() {
            let _ = writeln!(to, "{line}");
        }
        let _ = writeln!(to, "EOF");
    } else {
        let _ = writeln!(to, "RUN {cmd}");
    }
}

fn render_from(to: &mut String, from: &str) {
    let _ = writeln!(to, "FROM {from}");
}

/// Emits `USER student` + exec-form ENTRYPOINT when there is no privileged
/// startup; otherwise stays root and runs `pre_root` before dropping to the user
/// with `gosu` (network.enable needs root, so we cannot switch user first).
fn render_runtime(to: &mut String, rt: &Runtime) {
    let _ = writeln!(to);
    if rt.pre_root.is_empty() {
        let _ = writeln!(to, "USER {}", rt.user);
        let words: Vec<&str> = rt.entrypoint.0.split_whitespace().collect();
        let _ = writeln!(to, "ENTRYPOINT {}", exec_array(&words));
    } else {
        let mut chain: Vec<String> = rt.pre_root.iter().map(|c| c.0.clone()).collect();
        chain.push(format!("exec gosu {u} {ep}", u = rt.user, ep = rt.entrypoint.0));
        let script = chain.join(" && ");
        let _ = writeln!(
            to,
            "ENTRYPOINT {}",
            exec_array(&["/bin/sh", "-c", script.as_str()])
        );
    }
}

fn exec_array(words: &[&str]) -> String {
    serde_json::to_string(words).unwrap_or_default()
}

fn prefixed<'a>(registry: Option<&str>, image: &'a str) -> Cow<'a, str> {
    match registry {
        Some(reg) => format!("{reg}/{image}").into(),
        None => image.into(),
    }
}

fn layer_label(source: &LayerSource) -> String {
    match source {
        LayerSource::Os(id) => format!("OS: {id}"),
        LayerSource::Platform(kind) => format!("Platform: {kind}"),
        LayerSource::Toolchain(tc) => format!("Toolchain: {tc}"),
        LayerSource::Assignment(id) => format!("Assignment: {id}"),
        LayerSource::OsConfigure(id) => format!("OS Configure: {id}"),
        LayerSource::OsCleanup(id) => format!("OS Cleanup: {id}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn full_plan(os_configure: Option<Layer>) -> BuildPlan {
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
            os_configure,
            os_cleanup: Layer {
                source: LayerSource::OsCleanup(id("ubuntu-22")),
                steps: vec![],
            },
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
    fn full_standalone_allow_has_user_and_exec_entrypoint() {
        let spec = ImageSpec::Full {
            upstream: "ubuntu:22.04".to_owned(),
            base_image: "pedagog/ubuntu:22".to_owned(),
            plan: full_plan(None),
            runtime: Runtime {
                user: "student".to_owned(),
                entrypoint: Command("code-server --bind 0.0.0.0:8080".to_owned()),
                pre_root: vec![],
            },
            ports: vec![8080],
        };
        let out = Containerfile::render(&spec, &opts(None, FromSource::Standalone)).to_string();
        assert!(out.contains("FROM ubuntu:22.04"));
        assert!(out.contains("PHASE: BASE"));
        assert!(out.contains("PHASE: PLATFORM"));
        assert!(!out.contains("PHASE: OS CLEANUP"), "empty phase must be skipped");
        assert!(out.contains("EXPOSE 8080"));
        assert!(out.contains("USER student"));
        assert!(
            out.contains(r#"ENTRYPOINT ["code-server","--bind","0.0.0.0:8080"]"#),
            "{out}"
        );
    }

    #[test]
    fn multiline_command_renders_as_heredoc() {
        let spec = ImageSpec::Base {
            upstream: "ubuntu:22.04".to_owned(),
            image: "pedagog/ubuntu:22".to_owned(),
            os: Layer {
                source: LayerSource::Os(id("ubuntu-22")),
                steps: vec![step("multi", &["if true; then\n  echo hi\nfi"])],
            },
        };
        let out = Containerfile::render(&spec, &opts(None, FromSource::Standalone)).to_string();
        assert!(out.contains("RUN <<'EOF'\nif true; then\n  echo hi\nfi\nEOF"), "{out}");
    }

    #[test]
    fn full_prebuilt_deny_drops_privileges_and_skips_os_layer() {
        let configure = Layer {
            source: LayerSource::OsConfigure(id("ubuntu-22")),
            steps: vec![step("restrict", &["iptables -P OUTPUT DROP"])],
        };
        let spec = ImageSpec::Full {
            upstream: "ubuntu:22.04".to_owned(),
            base_image: "pedagog/ubuntu:22".to_owned(),
            plan: full_plan(Some(configure)),
            runtime: Runtime {
                user: "student".to_owned(),
                entrypoint: Command("code-server".to_owned()),
                pre_root: vec![Command("iptables-restore < /etc/pedagog/network.rules".to_owned())],
            },
            ports: vec![],
        };
        let out = Containerfile::render(&spec, &opts(Some("localhost"), FromSource::PrebuiltBase))
            .to_string();
        assert!(out.contains("FROM localhost/pedagog/ubuntu:22"), "{out}");
        assert!(!out.contains("PHASE: BASE"), "os layer baked into base image");
        assert!(out.contains("PHASE: OS CONFIGURE"));
        assert!(!out.contains("USER student"), "stays root to run iptables");
        assert!(out.contains("exec gosu student"), "{out}");
        assert!(
            out.contains(r#"ENTRYPOINT ["/bin/sh","-c","iptables-restore"#),
            "{out}"
        );
    }
}
