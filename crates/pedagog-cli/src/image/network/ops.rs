//! Inspect and apply the student egress policy — the logic behind
//! `pedagog image network …` (clap surface in the parent module).

use std::path::Path;

use miette::{IntoDiagnostic, Result, WrapErr};
use pedagog_core::image::manifest::{Action, Manifest, NetworkConfig};
use pedagog_core::image::nft;
use tabled::settings::object::Rows;
use tabled::settings::style::{BorderSpanCorrection, HorizontalLine};
use tabled::settings::{Alignment, Panel, Style};
use tabled::{Table, Tabled};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table as TomlTable, value};

use crate::image::manifest;

/// One row of the egress summary: an action applied to a destination.
#[derive(Tabled)]
struct PolicyRow {
    #[tabled(rename = "Action")]
    action: &'static str,
    #[tabled(rename = "Destination")]
    destination: String,
}

/// Summarize the manifest's egress policy, or print its raw nftables ruleset.
pub fn status(config: &Path, nft: bool) -> Result<()> {
    let network = manifest::load(config)?.image.network;
    if nft {
        print!("{}", nft::render(&network));
    } else {
        print_summary(&network);
    }
    Ok(())
}

/// Render the manifest's egress policy and apply it live, or write the boot
/// ruleset file with `compile_only`.
pub fn load(config: &Path, out: &Path, compile_only: bool) -> Result<()> {
    // A missing manifest renders the fail-closed default (this is how the base
    // image bakes its default-deny ruleset); a malformed one is an error, so
    // authors see it.
    let network = if config.exists() {
        manifest::load(config)?.image.network
    } else {
        eprintln!(
            "no manifest at {}; using fail-closed default",
            config.display()
        );
        NetworkConfig::Default
    };
    let ruleset = nft::render(&network);
    if compile_only {
        std::fs::write(out, &ruleset)
            .into_diagnostic()
            .wrap_err_with(|| format!("writing ruleset {}", out.display()))?;
    } else {
        apply(&ruleset)?;
    }
    Ok(())
}

/// Rewrite the manifest's `[network]` table into an equivalent `custom` rule list.
/// Only that table is touched — the rest of the file (comments, other tables,
/// ordering) is preserved — and the result is re-parsed to validate before it
/// replaces the original atomically.
pub fn convert(config: &Path) -> Result<()> {
    let original = std::fs::read_to_string(config)
        .into_diagnostic()
        .wrap_err_with(|| format!("reading manifest {}", config.display()))?;
    let network = original
        .parse::<Manifest>()
        .into_diagnostic()
        .wrap_err_with(|| format!("parsing manifest {}", config.display()))?
        .image
        .network;

    if let NetworkConfig::Custom { .. } = network {
        eprintln!("manifest is already mode = \"custom\"; nothing to convert");
        return Ok(());
    }
    let was = mode(&network);
    let rules = network.to_custom_rules();

    let mut doc = original
        .parse::<DocumentMut>()
        .into_diagnostic()
        .wrap_err("re-parsing manifest for editing")?;
    let mut table = TomlTable::new();
    table.insert("mode", value("custom"));
    let mut arr = Array::new();
    for rule in &rules {
        let mut item = InlineTable::new();
        item.insert("action", word(rule.action).into());
        item.insert("to", rule.target.to_string().into());
        arr.push(item);
    }
    table.insert("rules", value(arr));
    doc["image"]["network"] = Item::Table(table);
    let edited = doc.to_string();

    // Never persist a manifest that won't load.
    edited
        .parse::<Manifest>()
        .into_diagnostic()
        .wrap_err("converted manifest failed to re-parse")?;

    // Atomic replace: write a sibling temp file, then rename over the original.
    let tmp = config.with_extension("toml.tmp");
    std::fs::write(&tmp, &edited)
        .into_diagnostic()
        .wrap_err_with(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, config)
        .into_diagnostic()
        .wrap_err_with(|| format!("replacing {}", config.display()))?;

    eprintln!("converted {was} -> custom ({} rules)", rules.len());
    if was == "open" {
        eprintln!(
            "note: appended catch-all allow rules (0.0.0.0/0, ::/0) for the open terminal accept"
        );
    }
    Ok(())
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
