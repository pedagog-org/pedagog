//! Render a student egress policy to an nftables ruleset script.

use super::ids::{PEDAGOG_UID, STUDENT_UID};
use super::network::{Action, NetworkMode, Verdict};
use ipnet::IpNet;

/// Render the nftables script that enforces `mode` for the student uid.
///
/// Loopback and the pedagog broker always keep egress; only the student uid is
/// constrained. The script is loaded at boot with `nft -f`.
pub fn render(mode: &NetworkMode) -> String {
    let (rules, terminal) = mode.lower();

    let mut lines = vec![
        "table inet pedagog {".to_owned(),
        "\tchain output {".to_owned(),
        "\t\ttype filter hook output priority 0; policy accept;".to_owned(),
        "\t\toifname \"lo\" accept".to_owned(),
        format!("\t\tmeta skuid {PEDAGOG_UID} accept"),
    ];

    for rule in &rules {
        lines.push(format!(
            "\t\tmeta skuid {STUDENT_UID} {} daddr {} {}",
            family(rule.target),
            rule.target,
            verdict(action_verdict(rule.action)),
        ));
    }
    lines.push(format!(
        "\t\tmeta skuid {STUDENT_UID} {}",
        verdict(terminal)
    ));
    lines.push("\t}".to_owned());
    lines.push("}".to_owned());

    lines.join("\n") + "\n"
}

/// nft address-family keyword for a destination match.
fn family(net: IpNet) -> &'static str {
    match net {
        IpNet::V4(_) => "ip",
        IpNet::V6(_) => "ip6",
    }
}

/// nft verdict keyword.
fn verdict(v: Verdict) -> &'static str {
    match v {
        Verdict::Accept => "accept",
        Verdict::Drop => "drop",
    }
}

/// A rule action as the verdict it applies.
fn action_verdict(a: Action) -> Verdict {
    match a {
        Action::Allow => Verdict::Accept,
        Action::Block => Verdict::Drop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::network::Rule;

    fn net(s: &str) -> IpNet {
        s.parse().unwrap()
    }

    #[test]
    fn default_drops_all_student_egress() {
        let out = render(&NetworkMode::Default);
        assert!(out.contains(&format!("meta skuid {PEDAGOG_UID} accept")));
        assert!(out.contains(&format!("meta skuid {STUDENT_UID} drop")));
        assert!(!out.contains("daddr"));
    }

    #[test]
    fn block_allows_then_drops() {
        let out = render(&NetworkMode::Block {
            allow: vec![net("10.0.0.5/32")],
        });
        let allow_at = out
            .find(&format!(
                "meta skuid {STUDENT_UID} ip daddr 10.0.0.5/32 accept"
            ))
            .unwrap();
        let drop_at = out.find(&format!("meta skuid {STUDENT_UID} drop")).unwrap();
        assert!(allow_at < drop_at, "allow must precede the terminal drop");
    }

    #[test]
    fn open_blocks_then_accepts() {
        let out = render(&NetworkMode::Open {
            block: vec![net("192.168.0.0/16")],
        });
        assert!(out.contains(&format!(
            "meta skuid {STUDENT_UID} ip daddr 192.168.0.0/16 drop"
        )));
        assert!(out.contains(&format!("meta skuid {STUDENT_UID} accept")));
    }

    #[test]
    fn custom_preserves_order_then_drops() {
        let out = render(&NetworkMode::Custom {
            rules: vec![
                Rule {
                    action: Action::Allow,
                    target: net("10.0.0.5/32"),
                },
                Rule {
                    action: Action::Block,
                    target: net("10.0.0.0/8"),
                },
            ],
        });
        let a = out.find("10.0.0.5/32 accept").unwrap();
        let b = out.find("10.0.0.0/8 drop").unwrap();
        let term = out
            .rfind(&format!("meta skuid {STUDENT_UID} drop"))
            .unwrap();
        assert!(a < b && b < term);
    }

    #[test]
    fn ipv6_uses_ip6_family() {
        let out = render(&NetworkMode::Block {
            allow: vec![net("2001:db8::/32")],
        });
        assert!(out.contains("ip6 daddr 2001:db8::/32 accept"));
    }
}
