// use console::style;

// use crate::resolve::plan::{BasePlan, BuildPlan, Layer, LayerSource, ResolvedStep};

// pub struct Describe;

// impl super::Render for Describe {
//     fn render_build(&self, plan: &BuildPlan) -> String {
//         render_build(plan)
//     }

//     fn render_base(&self, plan: &BasePlan) -> String {
//         render_base(plan)
//     }

//     fn render_build_with_base(&self, base: &BasePlan, build: &BuildPlan) -> String {
//         let mut merged_layers = base.layers.clone();
//         merged_layers.extend(build.layers.iter().cloned());
//         let merged = BuildPlan {
//             id: build.id.clone(),
//             name: build.name.clone(),
//             base_image: base.upstream.clone(),
//             layers: merged_layers,
//             entrypoint: build.entrypoint.clone(),
//         };
//         render_build(&merged)
//     }
// }

// pub fn render_build(plan: &BuildPlan) -> String {
//     let label = format!(
//         "{} {}",
//         style(format!("[{}]", plan.id)).dim(),
//         style(&plan.name).bold(),
//     );
//     let mut root = Node::new(label);

//     root.push(Node::leaf(style(format!("FROM {}", plan.base_image)).green().to_string()));

//     let os_layers: Vec<_> = plan
//         .layers
//         .iter()
//         .filter(|l| matches!(l.source, LayerSource::Os(_)))
//         .collect();
//     let build_layers: Vec<_> = plan
//         .layers
//         .iter()
//         .filter(|l| !matches!(l.source, LayerSource::Os(_)))
//         .collect();


//     if !os_layers.is_empty() {
//         let mut base_node = Node::new(style("Base").bold().to_string());
//         for layer in os_layers {
//             base_node.push(layer_node(layer));
//         }
//         root.push(base_node);
//     }

//     if !build_layers.is_empty() {
//         let mut build_node = Node::new(style("Build").bold().to_string());
//         for layer in build_layers {
//             build_node.push(layer_node(layer));
//         }
//         root.push(build_node);
//     }

//     let mut runtime = Node::new(style("Runtime").bold().to_string());
//     let platform_kind = plan.layers.iter().find_map(|l| match &l.source {
//         LayerSource::Platform(k) => Some(k),
//         _ => None,
//     });
//     if let Some(kind) = platform_kind {
//         let mut platform_node = Node::new(style(format!("Platform: {kind}")).cyan().to_string());
//         platform_node.push(entrypoint_node(&plan.entrypoint));
//         runtime.push(platform_node);
//     } else {
//         runtime.push(entrypoint_node(&plan.entrypoint));
//     }
//     root.push(runtime);

//     format!("{}\n", render_tree(&root))
// }

// pub fn render_base(plan: &BasePlan) -> String {
//     let mut root = Node::new(style(format!("Base Image: {}", plan.os_id)).bold().to_string());
//     root.push(Node::leaf(
//         style(format!("FROM {} → {}", plan.upstream, plan.image))
//             .green()
//             .to_string(),
//     ));
//     for layer in &plan.layers {
//         root.push(layer_node(layer));
//     }
//     format!("{}\n", render_tree(&root))
// }

// fn layer_node(layer: &Layer) -> Node {
//     let mut node = Node::new(style(layer_header(&layer.source)).cyan().to_string());
//     for (n, step) in layer.steps.iter().enumerate() {
//         node.push(step_node(step, n));
//     }
//     node
// }

// fn step_node(step: &ResolvedStep, n: usize) -> Node {
//     let step_name = step
//         .name
//         .as_deref()
//         .map(|s| style(s).italic().to_string())
//         .unwrap_or_else(|| style(format!("Step {}", n + 1)).dim().to_string());

//     let width = line_num_width(step.commands.len());
//     // Continuation lines within a command are padded to align with command content.
//     let cont_pad = " ".repeat(width + 2);

//     let mut lines: Vec<String> = vec![step_name];
//     for (i, cmd) in step.commands.iter().enumerate() {
//         let num = format!("{:>width$}", i + 1);
//         let mut cmd_lines = cmd.0.lines();
//         if let Some(first) = cmd_lines.next() {
//             lines.push(style(format!("{num}  {first}")).dim().to_string());
//             for cont in cmd_lines {
//                 lines.push(style(format!("{cont_pad}{cont}")).dim().to_string());
//             }
//         }
//     }

//     Node::leaf(lines.join("\n"))
// }

// fn entrypoint_node(cmd: &str) -> Node {
//     let mut lines: Vec<String> = vec![style("Entrypoint").italic().to_string()];
//     for (i, line) in cmd.lines().enumerate() {
//         let num = format!("{:>1}", i + 1);
//         lines.push(style(format!("{num}  {line}")).dim().to_string());
//     }
//     Node::leaf(lines.join("\n"))
// }

// // ── Custom tree renderer ───────────────────────────────────────────────────

// struct Node {
//     label: String,
//     children: Vec<Node>,
// }

// impl Node {
//     fn new(label: impl Into<String>) -> Self {
//         Self { label: label.into(), children: vec![] }
//     }

//     fn leaf(label: impl Into<String>) -> Self {
//         Self::new(label)
//     }

//     fn push(&mut self, child: Node) -> &mut Self {
//         self.children.push(child);
//         self
//     }
// }

// fn render_tree(root: &Node) -> String {
//     let mut buf = String::new();
//     write_label(&mut buf, &root.label, "", "");
//     write_children(&mut buf, &root.children, "");
//     buf
// }

// fn write_children(buf: &mut String, children: &[Node], prefix: &str) {
//     for (i, child) in children.iter().enumerate() {
//         let last = i == children.len() - 1;
//         let connector = if last { "└── " } else { "├── " };
//         let cont = if last { "    " } else { "│   " };
//         let child_prefix = format!("{prefix}{cont}");
//         write_label(buf, &child.label, &format!("{prefix}{connector}"), &child_prefix);
//         write_children(buf, &child.children, &child_prefix);
//     }
// }

// fn write_label(buf: &mut String, label: &str, first_prefix: &str, cont_prefix: &str) {
//     let mut iter = label.lines().peekable();
//     match iter.next() {
//         None => buf.push_str(&format!("{first_prefix}\n")),
//         Some(first) => {
//             buf.push_str(&format!("{first_prefix}{first}\n"));
//             for line in iter {
//                 buf.push_str(&format!("{cont_prefix}{line}\n"));
//             }
//         }
//     }
// }

// // ── Helpers ────────────────────────────────────────────────────────────────

// fn line_num_width(n: usize) -> usize {
//     if n < 10 { 1 } else if n < 100 { 2 } else { 3 }
// }

// fn layer_header(source: &LayerSource) -> String {
//     match source {
//         LayerSource::Os(id) => format!("OS: {id}"),
//         LayerSource::Platform(kind) => format!("Platform: {kind}"),
//         LayerSource::Toolchain(v) => format!("Toolchain: {v}"),
//         LayerSource::BuildCleanup => "Build: cleanup".into(),
//     }
// }
