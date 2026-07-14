use std::fmt::Write;

use crate::resolve::plan::{BasePlan, BuildPlan, Layer, LayerSource};

pub struct Containerfile {
    pub registry: Option<String>,
}

impl Containerfile {
    fn prefixed<'a>(&self, image: &'a str) -> std::borrow::Cow<'a, str> {
        match &self.registry {
            Some(reg) => format!("{reg}/{image}").into(),
            None => image.into(),
        }
    }
}

impl super::Renderer for Containerfile {
    fn render_build(&self, plan: &BuildPlan) -> String {
        let mut out = String::new();
        writeln!(out, "FROM {}", self.prefixed(&plan.base_image)).unwrap();
        for layer in &plan.layers {
            render_layer(&mut out, layer);
        }
        let entrypoint_args = shell_words(&plan.entrypoint);
        writeln!(out).unwrap();
        writeln!(out, "USER student").unwrap();
        writeln!(out, "ENTRYPOINT [{}]", entrypoint_args.join(", ")).unwrap();
        out
    }

    fn render_base(&self, plan: &BasePlan) -> String {
        render_base(plan)
    }

    fn render_build_with_base(&self, base: &BasePlan, build: &BuildPlan) -> String {
        render_build_with_base(base, build)
    }
}

pub fn render_base(plan: &BasePlan) -> String {
    let mut out = String::new();
    writeln!(out, "FROM {}", plan.upstream).unwrap();
    for layer in &plan.layers {
        render_layer(&mut out, layer);
    }
    out
}

pub fn render_build_with_base(base: &BasePlan, build: &BuildPlan) -> String {
    let mut out = String::new();
    writeln!(out, "FROM {}", base.upstream).unwrap();
    for layer in &base.layers {
        render_layer(&mut out, layer);
    }
    for layer in &build.layers {
        render_layer(&mut out, layer);
    }
    let entrypoint_args = shell_words(&build.entrypoint);
    writeln!(out).unwrap();
    writeln!(out, "USER student").unwrap();
    writeln!(out, "ENTRYPOINT [{}]", entrypoint_args.join(", ")).unwrap();
    out
}

fn render_layer(out: &mut String, layer: &Layer) {
    let (pedagog_type, pedagog_id) = layer_context(&layer.source);
    writeln!(out).unwrap();
    writeln!(out, "# [For {}]", layer_header(&layer.source)).unwrap();
    writeln!(out, "ENV PEDAGOG_TYPE={pedagog_type}").unwrap();
    writeln!(out, "ENV PEDAGOG_ID={pedagog_id}").unwrap();
    for step in &layer.steps {
        if let Some(name) = &step.name {
            writeln!(out, "# [{name}]").unwrap();
        }
        let cmds = &step.commands;
        if cmds.is_empty() {
            continue;
        }
        if cmds.iter().any(|c| c.0.contains('\n')) {
            writeln!(out, "RUN <<'EOF'").unwrap();
            for (i, cmd) in cmds.iter().enumerate() {
                if i == cmds.len() - 1 {
                    writeln!(out, "{}", cmd.0.trim_end()).unwrap();
                } else {
                    writeln!(out, "{} &&", cmd.0.trim_end()).unwrap();
                }
            }
            writeln!(out, "EOF").unwrap();
        } else if cmds.len() == 1 {
            writeln!(out, "RUN {}", cmds[0].0).unwrap();
        } else {
            write!(out, "RUN ").unwrap();
            for (i, cmd) in cmds.iter().enumerate() {
                if i == 0 {
                    writeln!(out, "{} \\", cmd.0).unwrap();
                } else if i == cmds.len() - 1 {
                    writeln!(out, "    && {}", cmd.0).unwrap();
                } else {
                    writeln!(out, "    && {} \\", cmd.0).unwrap();
                }
            }
        }
    }
}

fn layer_context(source: &LayerSource) -> (String, String) {
    match source {
        LayerSource::Os(id) => ("os".into(), id.to_string()),
        LayerSource::Platform(kind) => ("platform".into(), kind.to_string()),
        LayerSource::Toolchain(v) => ("toolchain".into(), format!("{}/{}", v.id, v.version)),
        LayerSource::BuildCleanup => ("build".into(), "cleanup".into()),
    }
}

fn layer_header(source: &LayerSource) -> String {
    match source {
        LayerSource::Os(id) => format!("OS {id}"),
        LayerSource::Platform(kind) => format!("Platform {kind}"),
        LayerSource::Toolchain(v) => format!("Toolchain {v}"),
        LayerSource::BuildCleanup => "Build cleanup".into(),
    }
}

fn shell_words(s: &str) -> Vec<String> {
    s.split_whitespace().map(|w| format!("\"{w}\"")).collect()
}
