use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "hammer", about = "Pedagog image build tool")]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Resolve recipes and print the ordered build plan without building
    Plan {
        /// Path to assignment.yml
        assignment: PathBuf,
    },
    /// Submit a Kaniko job to build the instructor image for this assignment
    Build {
        /// Path to assignment.yml
        assignment: PathBuf,
    },
    /// Submit a Kaniko job to build a pedagog OS base image
    BuildOs {
        /// OS id (e.g. ubuntu-22)
        os_id: String,
    },
}

impl Cli {
    pub fn run(self) -> Result<()> {
        match self.command {
            Command::Plan { assignment } => crate::plan::run(&assignment),
            Command::Build { assignment } => crate::build::run_assignment(&assignment),
            Command::BuildOs { os_id } => crate::build::run_os(&os_id),
        }
    }
}
