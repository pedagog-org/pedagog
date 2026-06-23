//! Read and write the build ledger (`/pedagog/config/ledger.toml`) — the resolved
//! record of what `pkg`, `toolchain`, and `build` have installed.

use std::path::Path;

use miette::{IntoDiagnostic, Result, WrapErr};
use pedagog_core::image::ledger::Ledger;

pub use pedagog_core::image::ledger::DEFAULT_LEDGER;

/// Load the ledger, treating a missing file as an empty (fresh) ledger.
pub fn load(path: &Path) -> Result<Ledger> {
    if !path.exists() {
        return Ok(Ledger::default());
    }
    let text = std::fs::read_to_string(path)
        .into_diagnostic()
        .wrap_err_with(|| format!("reading ledger {}", path.display()))?;
    text.parse::<Ledger>()
        .into_diagnostic()
        .wrap_err_with(|| format!("parsing ledger {}", path.display()))
}

/// Write the ledger atomically (sibling temp file + rename).
pub fn save(path: &Path, state: &Ledger) -> Result<()> {
    let text = state
        .to_toml()
        .into_diagnostic()
        .wrap_err("serializing ledger")?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &text)
        .into_diagnostic()
        .wrap_err_with(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .into_diagnostic()
        .wrap_err_with(|| format!("replacing {}", path.display()))
}
