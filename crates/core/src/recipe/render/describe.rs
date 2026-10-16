use anstyle::{AnsiColor, Style};

use super::{
    AssignmentFiles, BuildPhase, BuildPlan, FromSource, ImageSpec, Layer, LayerSource, Render,
    RenderOptions, Runtime, SOLUTION_DIR, STARTER_DIR, TESTS_DIR,
};
use crate::recipe::assignment::Testing;
use crate::recipe::network::NetworkSpec;
use crate::recipe::primitives::ToolchainRef;

/// Human-readable tree of what an image contains. Always styled with ANSI codes;
/// callers strip them when the output isn't a terminal.
pub struct Describe(String);

impl std::fmt::Display for Describe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

const HEADER: Style = Style::new().bold();
const LAYER: Style = AnsiColor::Cyan.on_default();
const FROM: Style = AnsiColor::Green.on_default();
const STEP: Style = Style::new().italic();
const DIM: Style = Style::new().dimmed();
const NOTE: Style = AnsiColor::Yellow.on_default();

impl Render for Describe {
    fn render(spec: &ImageSpec, opts: &RenderOptions) -> Self {
        let root = match spec {
            ImageSpec::Base {
                upstream,
                image,
                os,
            } => {
                let mut root = Node::new(paint(
                    HEADER,
                    &format!("Base image {}", opts.prefixed(image)),
                ));
                root.push(Node::new(paint(FROM, &format!("FROM {upstream}"))));
                root.push(layer_node(os));
                root
            }
            ImageSpec::Full {
                upstream,
                base_image,
                plan,
                runtime,
                ports,
                network,
                testing,
                files,
            } => {
                let mut root = Node::new(paint(HEADER, &plan.assignment.source.label()));
                root.push(Node::new(paint(
                    FROM,
                    &format!("FROM {}", opts.full_from(upstream, base_image)),
                )));
                for (phase, layers) in phases(plan, opts.from) {
                    if layers.iter().all(|l| l.steps.is_empty()) {
                        continue;
                    }
                    let mut node = Node::new(paint(HEADER, &phase.to_string()));
                    for layer in layers.iter().filter(|l| !l.steps.is_empty()) {
                        node.push(layer_node(layer));
                    }
                    root.push(node);
                }
                if let Some(node) = files_node(files) {
                    root.push(node);
                }
                root.push(runtime_node(runtime, ports, network, testing.as_ref()));
                if let Some(notes) = notes_node(plan) {
                    root.push(notes);
                }
                root
            }
        };
        Describe(render_tree(&root))
    }
}

fn phases(plan: &BuildPlan, from: FromSource) -> Vec<(BuildPhase, Vec<&Layer>)> {
    let mut out = Vec::new();
    // Under PrebuiltBase the os layer is already in the base image.
    if matches!(from, FromSource::Standalone) {
        out.push((BuildPhase::Base, vec![&plan.os]));
    }
    out.push((BuildPhase::Platform, vec![&plan.platform]));
    out.push((BuildPhase::Toolchain, plan.toolchain.iter().collect()));
    out.push((BuildPhase::Assignment, vec![&plan.assignment]));
    out.push((BuildPhase::OsCleanup, vec![&plan.os_cleanup]));
    out
}

fn layer_node(layer: &Layer) -> Node {
    let mut node = Node::new(paint(LAYER, &layer.source.label()));
    for (n, step) in layer.steps.iter().enumerate() {
        let label = match &step.name {
            Some(name) => paint(STEP, name),
            None => paint(DIM, &format!("Step {}", n + 1)),
        };
        node.push(Node::new(label));
    }
    node
}

fn files_node(files: &AssignmentFiles) -> Option<Node> {
    if files.is_empty() {
        return None;
    }
    let mut node = Node::new(paint(HEADER, &BuildPhase::AssignmentFiles.to_string()));
    for (src, dest, note) in [
        (&files.starter, STARTER_DIR, "read-only"),
        (&files.tests, TESTS_DIR, "grader only"),
        (&files.solution, SOLUTION_DIR, "grader only"),
    ] {
        if let Some(src) = src {
            node.push(Node::new(format!(
                "{src}/ → {dest} {}",
                paint(DIM, &format!("({note})"))
            )));
        }
    }
    Some(node)
}

fn runtime_node(
    runtime: &Runtime,
    ports: &[u16],
    network: &NetworkSpec,
    testing: Option<&Testing>,
) -> Node {
    let mut node = Node::new(paint(HEADER, "RUNTIME"));
    node.push(field("user", &runtime.user));
    if runtime.entrypoint.is_empty() {
        node.push(field("entrypoint", "none (no session)"));
    } else {
        node.push(field("entrypoint", &shell_words(&runtime.entrypoint)));
    }
    if !ports.is_empty() {
        let ports: Vec<String> = ports.iter().map(ToString::to_string).collect();
        node.push(field("ports", &ports.join(", ")));
    }
    node.push(field("network", &network.to_string()));
    if let Some(t) = testing {
        node.push(field(
            "testing",
            &format!("{} (isolation: {})", t.framework, t.isolation),
        ));
    }
    node
}

/// Calls out toolchains installed at more than one version.
fn notes_node(plan: &BuildPlan) -> Option<Node> {
    let refs: Vec<&ToolchainRef> = plan
        .toolchain
        .iter()
        .filter_map(|l| match &l.source {
            LayerSource::Toolchain { tc, .. } => Some(tc),
            _ => None,
        })
        .collect();
    let mut notes = Vec::new();
    for (i, tc) in refs.iter().enumerate() {
        if refs[..i].iter().any(|t| t.id == tc.id) {
            continue;
        }
        let versions: Vec<String> = refs
            .iter()
            .filter(|t| t.id == tc.id)
            .map(|t| t.version.to_string())
            .collect();
        if versions.len() > 1 {
            notes.push(format!(
                "{} is installed at versions {}",
                tc.id,
                versions.join(", ")
            ));
        }
    }
    if notes.is_empty() {
        return None;
    }
    let mut node = Node::new(paint(HEADER, "NOTES"));
    for note in notes {
        node.push(Node::new(paint(NOTE, &note)));
    }
    Some(node)
}

fn field(name: &str, value: &str) -> Node {
    Node::new(format!("{} {value}", paint(DIM, &format!("{name}:"))))
}

/// Joins argv for display, quoting items that contain whitespace.
fn shell_words(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.chars().any(char::is_whitespace) {
                format!("{a:?}")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn paint(style: Style, text: &str) -> String {
    format!("{style}{text}{style:#}")
}

// ---- Tree renderer -----------------------------------------------------------

struct Node {
    label: String,
    children: Vec<Node>,
}

impl Node {
    fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            children: vec![],
        }
    }

    fn push(&mut self, child: Node) {
        self.children.push(child);
    }
}

fn render_tree(root: &Node) -> String {
    let mut buf = String::new();
    buf.push_str(&root.label);
    buf.push('\n');
    write_children(&mut buf, &root.children, "");
    buf
}

fn write_children(buf: &mut String, children: &[Node], prefix: &str) {
    for (i, child) in children.iter().enumerate() {
        let last = i + 1 == children.len();
        let (connector, cont) = if last {
            ("└── ", "    ")
        } else {
            ("├── ", "│   ")
        };
        buf.push_str(&format!("{prefix}{connector}{}\n", child.label));
        write_children(buf, &child.children, &format!("{prefix}{cont}"));
    }
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    use super::*;
    use crate::recipe::platform::PlatformKind;
    use crate::recipe::primitives::{Command, Id};
    use crate::recipe::render::{ResolvedStep, ToolchainOrigin};

    fn plain(d: Describe) -> String {
        Regex::new("\x1b\\[[0-9;]*m")
            .unwrap()
            .replace_all(&d.to_string(), "")
            .into_owned()
    }

    fn id(s: &str) -> Id {
        Id::try_from(s.to_owned()).unwrap()
    }

    fn tc(s: &str) -> ToolchainRef {
        ToolchainRef::try_from(s.to_owned()).unwrap()
    }

    fn layer(source: LayerSource, steps: &[Option<&str>]) -> Layer {
        Layer {
            source,
            steps: steps
                .iter()
                .map(|n| ResolvedStep {
                    name: n.map(str::to_owned),
                    commands: vec![Command("true".to_owned())],
                })
                .collect(),
        }
    }

    fn toolchain(r: &str, origins: Vec<ToolchainOrigin>) -> Layer {
        layer(
            LayerSource::Toolchain { tc: tc(r), origins },
            &[Some("Install")],
        )
    }

    fn full(toolchain: Vec<Layer>, network: NetworkSpec) -> ImageSpec {
        ImageSpec::Full {
            upstream: "ubuntu:22.04".to_owned(),
            base_image: "pedagog/ubuntu:22".to_owned(),
            plan: BuildPlan {
                os: layer(LayerSource::Os(id("ubuntu-22")), &[Some("init")]),
                platform: layer(
                    LayerSource::Platform(PlatformKind::Workspace),
                    &[Some("Install code-server"), None],
                ),
                toolchain,
                assignment: layer(LayerSource::Assignment(id("hw1")), &[]),
                os_cleanup: layer(LayerSource::OsCleanup(id("ubuntu-22")), &[Some("clean")]),
            },
            runtime: Runtime {
                user: "student".to_owned(),
                entrypoint: vec!["code-server".to_owned(), "/home/student/my work".to_owned()],
            },
            ports: vec![8080],
            network,
            testing: None,
            files: AssignmentFiles::default(),
        }
    }

    fn opts(registry: Option<&str>, from: FromSource) -> RenderOptions {
        RenderOptions {
            registry: registry.map(str::to_owned),
            from,
        }
    }

    #[test]
    fn full_prebuilt_tree() {
        let spec = full(
            vec![
                toolchain("gcc:13", vec![ToolchainOrigin::TopLevel]),
                toolchain("gdb:14", vec![ToolchainOrigin::AddonOf(tc("gcc:13"))]),
            ],
            NetworkSpec::Allow,
        );
        let out = plain(Describe::render(
            &spec,
            &opts(Some("localhost"), FromSource::PrebuiltBase),
        ));
        assert_eq!(
            out,
            "\
Assignment: hw1
├── FROM localhost/pedagog/ubuntu:22
├── PLATFORM
│   └── Platform: workspace
│       ├── Install code-server
│       └── Step 2
├── TOOLCHAIN
│   ├── Toolchain: gcc:13
│   │   └── Install
│   └── Toolchain: gdb:14 (addon of gcc:13)
│       └── Install
├── OS CLEANUP
│   └── OS Cleanup: ubuntu-22
│       └── clean
└── RUNTIME
    ├── user: student
    ├── entrypoint: code-server \"/home/student/my work\"
    ├── ports: 8080
    └── network: unrestricted
"
        );
    }

    #[test]
    fn full_standalone_includes_base_phase_from_upstream() {
        let out = plain(Describe::render(
            &full(vec![], NetworkSpec::Allow),
            &opts(Some("localhost"), FromSource::Standalone),
        ));
        assert!(out.contains("├── FROM ubuntu:22.04\n"), "{out}");
        assert!(out.contains("├── BASE\n│   └── OS: ubuntu-22\n"), "{out}");
    }

    #[test]
    fn base_shows_upstream_and_produced_tag() {
        let spec = ImageSpec::Base {
            upstream: "ubuntu:22.04".to_owned(),
            image: "pedagog/ubuntu:22".to_owned(),
            os: layer(LayerSource::Os(id("ubuntu-22")), &[Some("init")]),
        };
        let out = plain(Describe::render(
            &spec,
            &opts(Some("localhost"), FromSource::Standalone),
        ));
        assert_eq!(
            out,
            "Base image localhost/pedagog/ubuntu:22\n├── FROM ubuntu:22.04\n└── OS: ubuntu-22\n    └── init\n"
        );
    }

    #[test]
    fn network_policy_and_multi_version_notes_are_shown() {
        let network: NetworkSpec =
            serde_yaml::from_str("mode: deny\nallow:\n  - cidr: 10.0.0.0/8\n").unwrap();
        let spec = full(
            vec![
                toolchain("gdb:15", vec![ToolchainOrigin::TopLevel]),
                toolchain("gdb:13", vec![ToolchainOrigin::TopLevel]),
            ],
            network,
        );
        let out = plain(Describe::render(
            &spec,
            &opts(None, FromSource::PrebuiltBase),
        ));
        assert!(
            out.contains("network: deny by default; allow 10.0.0.0/8 (all ports)"),
            "{out}"
        );
        assert!(
            out.ends_with("└── NOTES\n    └── gdb is installed at versions 15, 13\n"),
            "{out}"
        );
    }

    #[test]
    fn output_is_styled() {
        let out = Describe::render(
            &full(vec![], NetworkSpec::Allow),
            &opts(None, FromSource::PrebuiltBase),
        )
        .to_string();
        assert!(out.contains('\x1b'), "expected ANSI styling");
    }

    #[test]
    fn shows_assignment_files_and_testing() {
        let mut spec = full(vec![], NetworkSpec::Allow).with_files(AssignmentFiles {
            starter: Some("src".into()),
            tests: Some("tests".into()),
            solution: None,
        });
        if let ImageSpec::Full { testing, .. } = &mut spec {
            *testing = Some(serde_yaml::from_str("framework: pytest:1\n").unwrap());
        }
        let out = plain(Describe::render(
            &spec,
            &opts(None, FromSource::PrebuiltBase),
        ));
        assert!(
            out.contains(
                "├── ASSIGNMENT FILES\n│   ├── src/ → /pedagog/starter (read-only)\n│   └── tests/ → /pedagog/tests (grader only)\n"
            ),
            "{out}"
        );
        assert!(
            out.contains("testing: pytest:1 (isolation: process)"),
            "{out}"
        );
    }
}
