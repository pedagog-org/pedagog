//! `pedagog image network …` — clap surface for the egress-policy verbs; the
//! logic lives in [`ops`].

use std::path::PathBuf;

use clap::Subcommand;
use miette::Result;
use pedagog_core::image::manifest::DEFAULT_MANIFEST;
use pedagog_core::image::nft::DEFAULT_RULESET;

mod ops;

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
    /// Rewrite the manifest's `[network]` table into an equivalent `custom` rule
    /// list, so its ordered rules can be hand-edited.
    Convert {
        /// Path to the manifest.
        #[arg(long, default_value = DEFAULT_MANIFEST)]
        config: PathBuf,
    },
}

impl NetworkCommand {
    pub fn run(self) -> Result<()> {
        match self {
            NetworkCommand::Status { config, nft } => ops::status(&config, nft),
            NetworkCommand::Load {
                config,
                out,
                compile_only,
            } => ops::load(&config, &out, compile_only),
            NetworkCommand::Convert { config } => ops::convert(&config),
        }
    }
}
