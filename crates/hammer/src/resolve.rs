use anyhow::Result;

use crate::recipe::{AddonRef, OsDef, PlatformRecipe, ToolchainRecipe};

pub struct ResolvedPlan {
    pub os: OsDef,
    pub platform: PlatformRecipe,
    pub toolchains: Vec<ToolchainRecipe>,
}

pub fn resolve(
    _os_id: &str,
    _platform_id: &str,
    _toolchain_refs: &[(&str, &str)],
    _extra_addon_refs: &[AddonRef],
) -> Result<ResolvedPlan> {
    todo!("recipe resolution")
}
