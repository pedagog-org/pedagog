//! Student egress network policy.

use ipnet::IpNet;

/// What to do with traffic to a destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Allow,
    Block,
}

/// A single egress rule: an action for traffic to a destination network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub action: Action,
    pub target: IpNet,
}

/// The verdict for student traffic that matches no rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    Drop,
}

/// Student egress policy. Other uids (pedagog, root) are unaffected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkMode {
    /// All student egress blocked (fail-closed default).
    Default,
    /// Blocked except these destinations.
    Block { allow: Vec<IpNet> },
    /// Allowed except these destinations.
    Open { block: Vec<IpNet> },
    /// Ordered, first-match rules; unmatched traffic is dropped.
    Custom { rules: Vec<Rule> },
}

impl NetworkMode {
    /// Lower to the unified form every mode shares: an ordered list of student
    /// rules plus the terminal verdict for traffic that matches none of them.
    pub fn lower(&self) -> (Vec<Rule>, Verdict) {
        match self {
            NetworkMode::Default => (Vec::new(), Verdict::Drop),
            NetworkMode::Block { allow } => (
                allow
                    .iter()
                    .map(|&target| Rule {
                        action: Action::Allow,
                        target,
                    })
                    .collect(),
                Verdict::Drop,
            ),
            NetworkMode::Open { block } => (
                block
                    .iter()
                    .map(|&target| Rule {
                        action: Action::Block,
                        target,
                    })
                    .collect(),
                Verdict::Accept,
            ),
            NetworkMode::Custom { rules } => (rules.clone(), Verdict::Drop),
        }
    }
}
