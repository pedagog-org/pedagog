//! `pedagog image network …` — inspect the student egress policy.

use std::path::PathBuf;

use clap::Subcommand;
use miette::{IntoDiagnostic, Result, WrapErr};
use pedagog_core::image::manifest::{Action, NetworkConfig};
use pedagog_core::image::nft;
use tabled::settings::object::Rows;
use tabled::settings::style::{BorderSpanCorrection, HorizontalLine};
use tabled::settings::{Alignment, Panel, Style};
use tabled::{Table, Tabled};

use crate::manifest;

/// Default manifest location inside the image.
const DEFAULT_MANIFEST: &str = "/pedagog/source/pedagog.toml";

/// Default location of the compiled ruleset, loaded at boot by `nft -f`.
const DEFAULT_RULESET: &str = "/pedagog/config/nftables.conf";

#[derive(Debug, Subcommand)]
pub enum NetworkCommand {
    /// Summarize the student egress policy from the manifest.
    Status {
        /// Path to the manifest.
        #[arg(long, default_value = DEFAULT_MANIFEST)]
        config: PathBuf,
        /// Print the raw nftables ruleset instead of the summary.
        #[arg(long)]
        nft: bool,
    },
    /// Render the manifest's egress policy and apply it live (`nft -f -`), or
    /// write it to the boot-loaded ruleset file with `--compile-only`.
    Load {
        /// Path to the manifest.
        #[arg(long, default_value = DEFAULT_MANIFEST)]
        config: PathBuf,
        /// Where to write the ruleset (with `--compile-only`).
        #[arg(long, default_value = DEFAULT_RULESET)]
        out: PathBuf,
        /// Write the ruleset file instead of applying it live (build-time path).
        #[arg(long)]
        compile_only: bool,
    },
}

/// One row of the egress summary: an action applied to a destination.
#[derive(Tabled)]
struct PolicyRow {
    #[tabled(rename = "Action")]
    action: &'static str,
    #[tabled(rename = "Destination")]
    destination: String,
}

impl NetworkCommand {
    pub fn run(self) -> Result<()> {
        match self {
            NetworkCommand::Status { config, nft } => {
                let network = manifest::load(&config)?.network;
                if nft {
                    print!("{}", nft::render(&network));
                } else {
                    print_summary(&network);
                }
                Ok(())
            }
            NetworkCommand::Load {
                config,
                out,
                compile_only,
            } => {
                // A missing manifest renders the fail-closed default (this is how
                // the base image bakes its default-deny ruleset); a malformed one
                // is an error, so authors see it.
                let network = if config.exists() {
                    manifest::load(&config)?.network
                } else {
                    eprintln!(
                        "no manifest at {}; using fail-closed default",
                        config.display()
                    );
                    NetworkConfig::Default
                };
                let ruleset = nft::render(&network);
                if compile_only {
                    std::fs::write(&out, &ruleset)
                        .into_diagnostic()
                        .wrap_err_with(|| format!("writing ruleset {}", out.display()))?;
                } else {
                    apply(&ruleset)?;
                }
                Ok(())
            }
        }
    }
}

/// Apply a rendered ruleset to the live netns by piping it to `nft -f -`.
///
/// Requires `CAP_NET_ADMIN`, so this only succeeds in an instructor session (the
/// editor's ambient cap, inherited by its terminal), not a locked student one. It
/// deliberately does not write the baked boot ruleset, so live edits stay ephemeral.
fn apply(ruleset: &str) -> Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("nft")
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .spawn()
        .into_diagnostic()
        .wrap_err("spawning nft (is nftables installed?)")?;
    // Scope the stdin handle so it drops (closing the pipe, signalling EOF) before
    // we wait; the ruleset is small, so writing before reading can't deadlock.
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| miette::miette!("nft stdin unavailable"))?;
        stdin
            .write_all(ruleset.as_bytes())
            .into_diagnostic()
            .wrap_err("writing ruleset to nft")?;
    }
    let status = child.wait().into_diagnostic().wrap_err("waiting for nft")?;
    if !status.success() {
        return Err(miette::miette!(
            "nft exited with {status}; a live apply needs CAP_NET_ADMIN (instructor sessions only)"
        ));
    }
    Ok(())
}

/// Print the human-readable egress summary as a bordered table.
fn print_summary(network: &NetworkConfig) {
    let (rules, terminal) = network.lower();
    let mut rows: Vec<PolicyRow> = rules
        .iter()
        .map(|rule| PolicyRow {
            action: word(rule.action),
            destination: rule.target.to_string(),
        })
        .collect();
    rows.push(PolicyRow {
        action: word(terminal),
        destination: "(default)".to_owned(),
    });

    let line = HorizontalLine::inherit(Style::modern());
    let mut table = Table::new(rows);
    table
        .with(Panel::header(format!("egress: {}", mode(network))))
        .with(Style::rounded().horizontals([(1, line), (2, line)]))
        .with(BorderSpanCorrection)
        .modify(Rows::first(), Alignment::center());
    println!("{table}");
}

/// The mode keyword for a policy.
fn mode(network: &NetworkConfig) -> &'static str {
    match network {
        NetworkConfig::Default => "default",
        NetworkConfig::Block { .. } => "block",
        NetworkConfig::Open { .. } => "open",
        NetworkConfig::Custom { .. } => "custom",
    }
}

/// The summary keyword for an action.
fn word(action: Action) -> &'static str {
    match action {
        Action::Allow => "allow",
        Action::Block => "block",
    }
}
