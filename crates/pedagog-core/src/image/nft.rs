//! Render a student egress policy to an nftables ruleset script.

use super::ids::{PEDAGOG_UID, STUDENT_UID};
use super::manifest::{Action, NetworkConfig};
use ipnet::IpNet;

/// Render the nftables script that enforces `config` for the student uid.
///
/// Loopback and the pedagog broker always keep egress; only the student uid is
/// constrained. The script is loaded at boot with `nft -f`.
pub fn render(config: &NetworkConfig) -> String {
    let (rules, terminal) = config.lower();

    let mut lines = vec![
        // Replace, don't append: ensure the table exists, delete it, then recreate
        // it. `nft -f` applies the whole script as one atomic transaction, so a
        // live reload swaps the policy cleanly instead of stacking onto the old
        // chain (at boot the table doesn't exist yet, where this is a harmless no-op).
        "table inet pedagog".to_owned(),
        "delete table inet pedagog".to_owned(),
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
            verdict(rule.action),
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

/// nft verdict keyword for an action.
fn verdict(action: Action) -> &'static str {
    match action {
        Action::Allow => "accept",
        Action::Block => "drop",
    }
}

#[cfg(test)]
mod tests {
    use super::super::manifest::Rule;
    use super::*;

    fn net(s: &str) -> IpNet {
        s.parse().unwrap()
    }

    #[test]
    fn default_drops_all_student_egress() {
        let out = render(&NetworkConfig::Default);
        assert!(out.contains(&format!("meta skuid {PEDAGOG_UID} accept")));
        assert!(out.contains(&format!("meta skuid {STUDENT_UID} drop")));
        assert!(!out.contains("daddr"));
    }

    #[test]
    fn block_allows_then_drops() {
        let out = render(&NetworkConfig::Block {
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
        let out = render(&NetworkConfig::Open {
            block: vec![net("192.168.0.0/16")],
        });
        assert!(out.contains(&format!(
            "meta skuid {STUDENT_UID} ip daddr 192.168.0.0/16 drop"
        )));
        assert!(out.contains(&format!("meta skuid {STUDENT_UID} accept")));
    }

    #[test]
    fn custom_preserves_order_then_drops() {
        let out = render(&NetworkConfig::Custom {
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
    fn replaces_table_so_reload_does_not_append() {
        let out = render(&NetworkConfig::Default);
        // The delete must precede the (re)definition so a live reload swaps the
        // policy atomically instead of stacking onto the existing chain.
        let del = out.find("delete table inet pedagog").unwrap();
        let def = out.find("table inet pedagog {").unwrap();
        assert!(del < def, "delete must precede the table definition");
    }

    #[test]
    fn ipv6_uses_ip6_family() {
        let out = render(&NetworkConfig::Block {
            allow: vec![net("2001:db8::/32")],
        });
        assert!(out.contains("ip6 daddr 2001:db8::/32 accept"));
    }
}
