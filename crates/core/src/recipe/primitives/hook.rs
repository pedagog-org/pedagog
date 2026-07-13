use std::collections::{BTreeSet, HashMap};

use serde::Deserialize;

use super::param::ParamDef;
use super::step::Step;

/// Hook whose inputs are hammer-provided args (OS hooks).
/// `A` is a per-hook enum of valid arg names; serde rejects unknown variants.
#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "A: Ord + for<'de2> Deserialize<'de2>"))]
pub struct ArgHook<A: Ord> {
    #[serde(default)]
    pub args: BTreeSet<A>,
    pub steps: Vec<Step>,
}

/// Hook whose inputs are user-defined params (platform/toolchain hooks).
/// Params are declared here with types and defaults; assignments may override.
#[derive(Debug, Deserialize)]
pub struct ParamHook {
    #[serde(default)]
    pub params: HashMap<String, ParamDef>,
    pub steps: Vec<Step>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arghook_no_args_field_defaults_to_empty() {
        use crate::recipe::os::NoArg;
        let yaml = "steps:\n  - run: ['echo hi']\n";
        let hook: ArgHook<NoArg> = serde_yaml::from_str(yaml).unwrap();
        assert!(hook.args.is_empty());
    }

    #[test]
    fn arghook_rejects_invalid_variant() {
        use crate::recipe::os::PkgArg;
        let yaml = "args: [cidrs]\nsteps: []\n";
        assert!(serde_yaml::from_str::<ArgHook<PkgArg>>(yaml).is_err());
    }

    #[test]
    fn arghook_deduplicates_via_btreeset() {
        use crate::recipe::os::PkgArg;
        let yaml = "args: [packages, packages]\nsteps: []\n";
        let hook: ArgHook<PkgArg> = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(hook.args.len(), 1);
    }

    #[test]
    fn paramhook_no_params_defaults_to_empty() {
        let yaml = "steps:\n  - run: ['echo hi']\n";
        let hook: ParamHook = serde_yaml::from_str(yaml).unwrap();
        assert!(hook.params.is_empty());
    }

    #[test]
    fn paramhook_deserializes_params() {
        let yaml = "params:\n  terminal:\n    type: bool\n    default: true\nsteps: []\n";
        let hook: ParamHook = serde_yaml::from_str(yaml).unwrap();
        assert!(hook.params.contains_key("terminal"));
    }
}
