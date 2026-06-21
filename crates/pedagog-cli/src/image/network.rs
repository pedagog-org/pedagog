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
    /// Compile the manifest's egress policy to the boot-loaded ruleset file.
    Compile {
        /// Path to the manifest.
        #[arg(long, default_value = DEFAULT_MANIFEST)]
        config: PathBuf,
        /// Where to write the nftables ruleset.
        #[arg(long, default_value = DEFAULT_RULESET)]
        out: PathBuf,
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
            NetworkCommand::Compile { config, out } => {
                // A missing manifest compiles to the fail-closed default (this is
                // how the base image bakes its default-deny ruleset); a malformed
                // one is an error, so authors see it.
                let network = if config.exists() {
                    manifest::load(&config)?.network
                } else {
                    eprintln!(
                        "no manifest at {}; compiling fail-closed default",
                        config.display()
                    );
                    NetworkConfig::Default
                };
                std::fs::write(&out, nft::render(&network))
                    .into_diagnostic()
                    .wrap_err_with(|| format!("writing ruleset {}", out.display()))?;
                Ok(())
            }
        }
    }
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
