use std::net::IpAddr;
use std::num::NonZeroU16;
use std::sync::LazyLock;

use ipnet::IpNet;
use regex::Regex;
use serde::Deserialize;

const LABEL: &str = r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?";
const MAX_HOST_LEN: usize = 253;

#[allow(clippy::expect_used)]
static HOST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^{LABEL}(\.{LABEL})*$")).expect("valid host pattern"));

/// Egress policy, enforced by the platform at runtime (not baked into the image).
/// Omitted, it denies all egress.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RawNetworkSpec")]
pub enum NetworkSpec {
    Allow,
    Deny {
        allow: Vec<AllowRule>,
    },
}

impl Default for NetworkSpec {
    fn default() -> Self {
        NetworkSpec::Deny { allow: Vec::new() }
    }
}

impl NetworkSpec {
    /// Rules naming hosts, which need the egress proxy to be enforced; until it
    /// exists, those hosts stay unreachable.
    pub fn host_rules(&self) -> Vec<&AllowRule> {
        match self {
            NetworkSpec::Allow => Vec::new(),
            NetworkSpec::Deny { allow } => allow
                .iter()
                .filter(|r| matches!(r.target, EgressTarget::Host(_)))
                .collect(),
        }
    }
}

// Serde ignores unknown keys on unit variants of tagged enums, so an `allow:`
// list under `mode: allow` would be silently dropped; parse loosely and check.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNetworkSpec {
    mode: Mode,
    allow: Option<Vec<AllowRule>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Allow,
    Deny,
}

impl TryFrom<RawNetworkSpec> for NetworkSpec {
    type Error = String;

    fn try_from(raw: RawNetworkSpec) -> Result<Self, Self::Error> {
        match (raw.mode, raw.allow) {
            (Mode::Allow, None) => Ok(NetworkSpec::Allow),
            (Mode::Allow, Some(_)) => Err(
                "`allow:` entries only apply to `mode: deny`; `mode: allow` is unrestricted"
                    .to_owned(),
            ),
            (Mode::Deny, allow) => Ok(NetworkSpec::Deny {
                allow: allow.unwrap_or_default(),
            }),
        }
    }
}

/// One allowed egress destination. Empty `ports` means all ports; TCP is assumed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "RawAllowRule")]
pub struct AllowRule {
    pub target: EgressTarget,
    pub ports: Vec<NonZeroU16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressTarget {
    Host(HostPattern),
    Cidr(IpNet),
}

// `flatten` can't be combined with `deny_unknown_fields`, so entries parse into
// this loose shape first and are checked for exactly one of host/cidr.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAllowRule {
    host: Option<String>,
    cidr: Option<String>,
    #[serde(default)]
    ports: Vec<NonZeroU16>,
}

impl TryFrom<RawAllowRule> for AllowRule {
    type Error = String;

    fn try_from(raw: RawAllowRule) -> Result<Self, Self::Error> {
        let target = match (raw.host, raw.cidr) {
            (Some(host), None) => EgressTarget::Host(HostPattern::try_from(host)?),
            (None, Some(cidr)) => EgressTarget::Cidr(cidr.parse().map_err(|_| {
                format!("invalid cidr {cidr:?}: expected e.g. \"10.0.0.0/8\" or \"2001:db8::/32\"")
            })?),
            (Some(_), Some(_)) => {
                return Err("allow entry has both `host` and `cidr`; use one per entry".to_owned());
            }
            (None, None) => return Err("allow entry needs one of `host` or `cidr`".to_owned()),
        };
        Ok(AllowRule {
            target,
            ports: raw.ports,
        })
    }
}

impl std::fmt::Display for EgressTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EgressTarget::Host(h) => h.fmt(f),
            EgressTarget::Cidr(n) => n.fmt(f),
        }
    }
}

// e.g. "*.github.com:443", "mirror.internal:80,443", "10.0.0.0/8 (all ports)"
impl std::fmt::Display for AllowRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ports.is_empty() {
            return write!(f, "{} (all ports)", self.target);
        }
        let ports: Vec<String> = self.ports.iter().map(ToString::to_string).collect();
        write!(f, "{}:{}", self.target, ports.join(","))
    }
}

impl std::fmt::Display for NetworkSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkSpec::Allow => f.write_str("unrestricted"),
            NetworkSpec::Deny { allow } if allow.is_empty() => f.write_str("deny all egress"),
            NetworkSpec::Deny { allow } => {
                let rules: Vec<String> = allow.iter().map(ToString::to_string).collect();
                write!(f, "deny by default; allow {}", rules.join(", "))
            }
        }
    }
}

/// RFC 1123 hostname, optionally prefixed with a single `*.` wildcard label.
/// Stored lowercased.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPattern(String);

impl HostPattern {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_wildcard(&self) -> bool {
        self.0.starts_with("*.")
    }
}

impl TryFrom<String> for HostPattern {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        let lower = s.to_ascii_lowercase();
        if lower.parse::<IpAddr>().is_ok() {
            return Err(format!(
                "host {s:?} is an IP address; use `cidr:` instead (e.g. \"{lower}/32\" or \"/128\")"
            ));
        }
        let name = lower.strip_prefix("*.").unwrap_or(&lower);
        if name.len() > MAX_HOST_LEN || !HOST_RE.is_match(name) {
            return Err(format!(
                "invalid host {s:?}: expected a hostname like \"mirror.internal\", optionally prefixed with \"*.\""
            ));
        }
        Ok(HostPattern(lower))
    }
}

impl std::fmt::Display for HostPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(yaml: &str) -> Result<NetworkSpec, String> {
        serde_yaml::from_str(yaml).map_err(|e| e.to_string())
    }

    fn deny_rules(yaml: &str) -> Vec<AllowRule> {
        match spec(yaml).unwrap() {
            NetworkSpec::Deny { allow } => allow,
            NetworkSpec::Allow => panic!("expected deny"),
        }
    }

    fn host(s: &str) -> Result<HostPattern, String> {
        HostPattern::try_from(s.to_owned())
    }

    #[test]
    fn deny_with_host_and_cidr_entries() {
        let rules = deny_rules(
            "mode: deny\nallow:\n  - host: Mirror.Internal\n    ports: [80, 443]\n  - cidr: 10.0.0.0/8\n  - cidr: 2001:db8::/32\n",
        );
        assert_eq!(rules.len(), 3);
        assert!(
            matches!(&rules[0].target, EgressTarget::Host(h) if h.as_str() == "mirror.internal")
        );
        assert_eq!(
            rules[0].ports.iter().map(|p| p.get()).collect::<Vec<_>>(),
            [80, 443]
        );
        assert!(matches!(&rules[1].target, EgressTarget::Cidr(n) if n.to_string() == "10.0.0.0/8"));
        assert!(rules[1].ports.is_empty(), "omitted ports means all ports");
        assert!(matches!(&rules[2].target, EgressTarget::Cidr(IpNet::V6(_))));
    }

    #[test]
    fn deny_defaults_to_empty_allowlist() {
        assert!(deny_rules("mode: deny\n").is_empty());
    }

    #[test]
    fn entry_with_both_host_and_cidr_is_rejected() {
        let err = spec("mode: deny\nallow:\n  - host: a.com\n    cidr: 1.2.3.4/32\n").unwrap_err();
        assert!(err.contains("both"), "{err}");
    }

    #[test]
    fn entry_with_neither_host_nor_cidr_is_rejected() {
        let err = spec("mode: deny\nallow:\n  - ports: [443]\n").unwrap_err();
        assert!(err.contains("one of"), "{err}");
    }

    #[test]
    fn misspelled_entry_key_is_rejected() {
        assert!(spec("mode: deny\nallow:\n  - host: a.com\n    prots: [443]\n").is_err());
    }

    #[test]
    fn allowlist_under_allow_mode_is_rejected() {
        let err = spec("mode: allow\nallow:\n  - host: a.com\n").unwrap_err();
        assert!(err.contains("mode: deny"), "{err}");
    }

    #[test]
    fn unknown_mode_is_rejected() {
        assert!(spec("mode: block\n").is_err());
    }

    #[test]
    fn port_zero_is_rejected() {
        assert!(spec("mode: deny\nallow:\n  - host: a.com\n    ports: [0]\n").is_err());
    }

    #[test]
    fn invalid_cidr_is_rejected() {
        let err = spec("mode: deny\nallow:\n  - cidr: 10.0.0.0/33\n").unwrap_err();
        assert!(err.contains("invalid cidr"), "{err}");
    }

    #[test]
    fn host_accepts_leading_wildcard_only() {
        assert!(host("*.github.com").unwrap().is_wildcard());
        assert!(host("api-*.example.com").is_err());
        assert!(host("*.*.example.com").is_err());
        assert!(host("example.*").is_err());
        assert!(host("*").is_err());
    }

    #[test]
    fn host_rejects_bad_labels() {
        assert!(host("-bad.com").is_err());
        assert!(host("bad-.com").is_err());
        assert!(host("a..com").is_err());
        assert!(host("under_score.com").is_err());
        assert!(host(&format!("{}.com", "a".repeat(64))).is_err());
        assert!(host("localhost").is_ok());
    }

    #[test]
    fn host_rejects_ip_addresses_with_cidr_hint() {
        let err = host("10.0.0.1").unwrap_err();
        assert!(err.contains("cidr"), "{err}");
        assert!(host("2001:db8::1").is_err());
    }

    #[test]
    fn default_denies_all_egress() {
        assert!(matches!(NetworkSpec::default(), NetworkSpec::Deny { allow } if allow.is_empty()));
    }

    #[test]
    fn host_rules_are_listed_for_warnings() {
        let s = spec("mode: deny\nallow:\n  - host: '*.github.com'\n    ports: [443]\n  - cidr: 10.0.0.0/8\n")
            .unwrap();
        let hosts: Vec<String> = s.host_rules().iter().map(ToString::to_string).collect();
        assert_eq!(hosts, ["*.github.com:443"]);
        assert!(NetworkSpec::Allow.host_rules().is_empty());
    }

    #[test]
    fn network_summary_display() {
        assert_eq!(NetworkSpec::Allow.to_string(), "unrestricted");
        assert_eq!(spec("mode: deny\n").unwrap().to_string(), "deny all egress");
        let s = spec("mode: deny\nallow:\n  - host: '*.github.com'\n    ports: [80, 443]\n  - cidr: 10.0.0.0/8\n")
            .unwrap();
        assert_eq!(
            s.to_string(),
            "deny by default; allow *.github.com:80,443, 10.0.0.0/8 (all ports)"
        );
    }
}
