//! Shared manifest loading for the `image` verbs.

use std::path::Path;

use miette::{IntoDiagnostic, Result, WrapErr};
use pedagog_core::image::manifest::Manifest;

/// Read and parse the manifest at `path`.
pub fn load(path: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("reading manifest {}", path.display()))?;
    text.parse::<Manifest>()
        .into_diagnostic()
        .wrap_err_with(|| format!("parsing manifest {}", path.display()))
}
