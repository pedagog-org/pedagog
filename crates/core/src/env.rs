//! Runtime environment selection via `PEDAGOG_ENV`.

/// Environment variable naming the deployment environment.
pub const PEDAGOG_ENV: &str = "PEDAGOG_ENV";

/// Deployment environment. Callers `match` on this rather than `is_dev`/`is_prod`
/// booleans, so adding a variant forces every branch to be revisited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Env {
    Dev,
    Prod,
}

impl Env {
    /// Read [`PEDAGOG_ENV`]. Unset or unrecognized defaults to [`Env::Prod`] —
    /// fail safe, so a misconfigured deployment is never silently treated as dev.
    pub fn current() -> Env {
        Self::parse(std::env::var(PEDAGOG_ENV).ok().as_deref())
    }

    // Split out so tests exercise the mapping without mutating process-global env
    // (`std::env::set_var` is `unsafe` and racy under the 2024 edition).
    fn parse(value: Option<&str>) -> Env {
        match value {
            Some("dev") => Env::Dev,
            Some("prod") => Env::Prod,
            other => {
                tracing::warn!(
                    ?other,
                    "PEDAGOG_ENV unset or unrecognized; defaulting to prod"
                );
                Env::Prod
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_values() {
        assert_eq!(Env::parse(Some("dev")), Env::Dev);
        assert_eq!(Env::parse(Some("prod")), Env::Prod);
    }

    #[test]
    fn defaults_to_prod_when_unset() {
        assert_eq!(Env::parse(None), Env::Prod);
    }

    #[test]
    fn defaults_to_prod_on_unknown() {
        assert_eq!(Env::parse(Some("staging")), Env::Prod);
        assert_eq!(Env::parse(Some("")), Env::Prod);
    }
}
