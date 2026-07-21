use std::fmt::Write;

use crate::recipe::{primitives::command::Command, render::{BuildPhase::{self}, ImageSpec, Layer, LayerSource, Render, RenderStartpoint::{self, BaseImage, Scratch}, ResolvedStep}};

pub struct Containerfile(String);
    // pub registry: Option<String>,

impl Containerfile {
    fn prefixed<'a>(&self, registry:Option<&'a str>,  image: &'a str) -> std::borrow::Cow<'a, str> {
        match registry {
            Some(reg) => format!("{reg}/{image}").into(),
            None => image.into(),
        }
    }
}

impl ToString for Containerfile {
    fn to_string(&self) -> String {
        self.0.clone()
    }
}


impl Containerfile {
    fn insert_comment(to: &mut String, text: &str) {
        writeln!(to, "# {}", text);
    }

    fn insert_divider_comment(to: &mut String, text: &str) {
        writeln!(to, "\n# ----- {} -----", text);
    }

    fn insert_phase_comment(to: &mut String, phase: BuildPhase) {
        Self::insert_divider_comment(to, format!("PHASE: {}", phase).as_str());
    }

    fn insert_layer_comment(to: &mut String, layer: &LayerSource) {
        Self::insert_divider_comment(to,
            match layer {
                LayerSource::Os(id) => format!("OS: {}", {id}),
                LayerSource::Platform(kind) => format!("Platform: {}", kind),
                LayerSource::Toolchain(ver_id) => format!("Toolchain: {}", {ver_id}),
                LayerSource::Assignment(id) => format!("Assignment: {}", {id})
            }.as_str()
        );
    }

    fn render_phase(to: &mut String, phase: BuildPhase, layers: &Vec<&Layer>) {
        Self::insert_phase_comment(to, phase);
        for l in layers {
            Self::render_layer(to, l);
        }
    }

    fn render_layer(to: &mut String, layer: &Layer) {
        Self::insert_layer_comment(to, &layer.source);
        for res_s in &layer.steps {
            Self::render_resolved_step(to, &res_s);
        }
    }

    fn render_resolved_step(to: &mut String, resolved_step: &ResolvedStep) {
        if let Some(step_name) = &resolved_step.name {
            Self::insert_comment(to, &step_name);
        }
        for cmd in &resolved_step.commands {
            Self::render_run(to, cmd);
    }
    }

    // Containerfile primitives
    fn  render_from(to: &mut String, from: &str) {
        writeln!(to, "FROM {}\n", from);
    }

    fn render_run(to: &mut String, cmd: &Command) {
        writeln!(to, "RUN {}", cmd);
    }

    fn render_entrypoint(to: &mut String, cmd: &Command) {
        writeln!(to, "ENTRYPOINT {}", cmd);
    }
}

impl Render<()> for Containerfile {
    fn render(spec: &ImageSpec, from: RenderStartpoint, options: ()) -> Self {
        let bp = &spec.build_plan;
        let mut to = String::new();

        // Derive
        let from_image = match from {
            Scratch => &spec.upstream_image,
            BaseImage => &spec.base_image,
        };
        Self::render_from(&mut to, from_image);

        // Phases
        Self::render_phase(&mut to, BuildPhase::Base, &vec![&bp.base].as_ref());
        Self::render_phase(&mut to, BuildPhase::Platform, &vec![&bp.platform].as_ref());
        Self::render_phase(&mut to, BuildPhase::Toolchain, &bp.toolchain.iter().collect());
        Self::render_phase(&mut to, BuildPhase::Assignment, &vec![&bp.assignment].as_ref());

        // Runtime
        Self::render_entrypoint(&mut to, &spec.entrypoint);

        // Done!
        Containerfile(to)
    }


    // fn render_build(&self, plan: &BuildPlan) -> String {
    //     let mut out = String::new();
    //     writeln!(out, "FROM {}", self.prefixed(&plan.base_image)).unwrap();
    //     for layer in &plan.layers {
    //         render_layer(&mut out, layer);
    //     }
    //     let entrypoint_args = shell_words(&plan.entrypoint);
    //     writeln!(out).unwrap();
    //     writeln!(out, "USER student").unwrap();
    //     writeln!(out, "ENTRYPOINT [{}]", entrypoint_args.join(", ")).unwrap();
    //     out
    // }

    // fn render_base(&self, plan: &BasePlan) -> String {
    //     render_base(plan)
    // }

    // fn render_build_with_base(&self, base: &BasePlan, build: &BuildPlan) -> String {
    //     render_build_with_base(base, build)
    // }
}

// pub fn render_base(plan: &BasePlan) -> String {
//     let mut out = String::new();
//     writeln!(out, "FROM {}", plan.upstream).unwrap();
//     for layer in &plan.layers {
//         render_layer(&mut out, layer);
//     }
//     out
// }

// pub fn render_build_with_base(base: &BasePlan, build: &BuildPlan) -> String {
//     let mut out = String::new();
//     writeln!(out, "FROM {}", base.upstream).unwrap();
//     for layer in &base.layers {
//         render_layer(&mut out, layer);
//     }
//     for layer in &build.layers {
//         render_layer(&mut out, layer);
//     }
//     let entrypoint_args = shell_words(&build.entrypoint);
//     writeln!(out).unwrap();
//     writeln!(out, "USER student").unwrap();
//     writeln!(out, "ENTRYPOINT [{}]", entrypoint_args.join(", ")).unwrap();
//     out
// }

// fn render_layer(out: &mut String, layer: &Layer) {
//     let (pedagog_type, pedagog_id) = layer_context(&layer.source);
//     writeln!(out).unwrap();
//     writeln!(out, "# [For {}]", layer_header(&layer.source)).unwrap();
//     writeln!(out, "ENV PEDAGOG_TYPE={pedagog_type}").unwrap();
//     writeln!(out, "ENV PEDAGOG_ID={pedagog_id}").unwrap();
//     for step in &layer.steps {
//         if let Some(name) = &step.name {
//             writeln!(out, "# [{name}]").unwrap();
//         }
//         let cmds = &step.commands;
//         if cmds.is_empty() {
//             continue;
//         }
//         if cmds.iter().any(|c| c.0.contains('\n')) {
//             writeln!(out, "RUN <<'EOF'").unwrap();
//             for (i, cmd) in cmds.iter().enumerate() {
//                 if i == cmds.len() - 1 {
//                     writeln!(out, "{}", cmd.0.trim_end()).unwrap();
//                 } else {
//                     writeln!(out, "{} &&", cmd.0.trim_end()).unwrap();
//                 }
//             }
//             writeln!(out, "EOF").unwrap();
//         } else if cmds.len() == 1 {
//             writeln!(out, "RUN {}", cmds[0].0).unwrap();
//         } else {
//             write!(out, "RUN ").unwrap();
//             for (i, cmd) in cmds.iter().enumerate() {
//                 if i == 0 {
//                     writeln!(out, "{} \\", cmd.0).unwrap();
//                 } else if i == cmds.len() - 1 {
//                     writeln!(out, "    && {}", cmd.0).unwrap();
//                 } else {
//                     writeln!(out, "    && {} \\", cmd.0).unwrap();
//                 }
//             }
//         }
//     }
// }

// fn layer_context(source: &LayerSource) -> (String, String) {
//     match source {
//         LayerSource::Os(id) => ("os".into(), id.to_string()),
//         LayerSource::Platform(kind) => ("platform".into(), kind.to_string()),
//         LayerSource::Toolchain(v) => ("toolchain".into(), format!("{}/{}", v.id, v.version)),
//         LayerSource::BuildCleanup => ("build".into(), "cleanup".into()),
//     }
// }

// fn layer_header(source: &LayerSource) -> String {
//     match source {
//         LayerSource::Os(id) => format!("OS {id}"),
//         LayerSource::Platform(kind) => format!("Platform {kind}"),
//         LayerSource::Toolchain(v) => format!("Toolchain {v}"),
//         LayerSource::BuildCleanup => "Build cleanup".into(),
//     }
// }

// fn shell_words(s: &str) -> Vec<String> {
//     s.split_whitespace().map(|w| format!("\"{w}\"")).collect()
// }
