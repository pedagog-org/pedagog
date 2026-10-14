use std::fmt;

use crate::recipe::platform::PlatformKind;
use crate::recipe::primitives::{OsId, ToolchainRef};

#[derive(Debug)]
pub enum ResolveError {
    OsNotFound(OsId),
    PlatformNotFound { kind: PlatformKind, os: OsId },
    ToolchainNotFound { tc: ToolchainRef, os: OsId },
    InvalidParams(String),
    Interpolation { context: String, message: String },
    InvalidPort { raw: String },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::OsNotFound(os) => write!(f, "os {:?} not found", os.as_str()),
            ResolveError::PlatformNotFound { kind, os } => write!(
                f,
                "no {:?} platform recipe found for os {:?}",
                kind.as_str(),
                os.as_str()
            ),
            ResolveError::ToolchainNotFound { tc, os } => {
                write!(f, "toolchain {tc} not found for os {:?}", os.as_str())
            }
            ResolveError::InvalidParams(msg) => write!(f, "invalid platform params: {msg}"),
            ResolveError::Interpolation { context, message } => {
                write!(f, "interpolation error in {context}: {message}")
            }
            ResolveError::InvalidPort { raw } => {
                write!(f, "invalid port {raw:?}: expected a number from 1 to 65535")
            }
        }
    }
}

impl std::error::Error for ResolveError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::primitives::Id;

    #[test]
    fn toolchain_not_found_names_ref_and_os() {
        let err = ResolveError::ToolchainNotFound {
            tc: ToolchainRef::try_from("gcc:13".to_owned()).unwrap(),
            os: Id::try_from("ubuntu-22".to_owned()).unwrap(),
        };
        assert_eq!(err.to_string(), r#"toolchain gcc:13 not found for os "ubuntu-22""#);
    }
}
