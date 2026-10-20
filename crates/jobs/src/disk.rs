//! How full the cluster's disks are: each node's (the `disk-report` DaemonSet),
//! the registry's volume (after its nightly GC) and Postgres's (by its
//! databases' size). Above the thresholds they're logged as warnings or errors,
//! which reach GlitchTip.

use std::time::Duration;

/// Used and total bytes of one filesystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Usage {
    pub used: u64,
    pub capacity: u64,
}

impl Usage {
    pub fn percent(self) -> f64 {
        if self.capacity == 0 {
            0.0
        } else {
            self.used as f64 * 100.0 / self.capacity as f64
        }
    }
}

/// Percent used at which a disk is a warning, then an error.
#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub warn: f64,
    pub error: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Error,
}

impl Thresholds {
    pub fn level(self, usage: Usage) -> Level {
        let p = usage.percent();
        if p >= self.error {
            Level::Error
        } else if p >= self.warn {
            Level::Warn
        } else {
            Level::Ok
        }
    }

    /// Log `what`'s usage at its level (info when fine).
    pub fn report(self, what: &str, usage: Usage) {
        let percent = format!("{:.0}%", usage.percent());
        let (used, capacity) = (gib(usage.used), gib(usage.capacity));
        match self.level(usage) {
            Level::Error => {
                tracing::error!(%what, %percent, %used, %capacity, "a disk is nearly full")
            }
            Level::Warn => {
                tracing::warn!(%what, %percent, %used, %capacity, "a disk is filling up")
            }
            Level::Ok => tracing::info!(%what, %percent, %used, %capacity, "disk usage"),
        }
    }
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

/// The usage in a `df -Pk` data line (`fs 1024-blocks used available
/// capacity mounted-on`).
pub fn parse_df(line: &str) -> Option<Usage> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let kib = |i: usize| fields.get(i)?.parse::<u64>().ok().map(|k| k * 1024);
    Some(Usage {
        used: kib(2)?,
        capacity: kib(1)?,
    })
}

/// A Kubernetes quantity of bytes (`1Gi`, `500M`, `1073741824`).
pub fn bytes(quantity: &str) -> Option<u64> {
    let q = quantity.trim();
    let split = q.find(|c: char| !c.is_ascii_digit()).unwrap_or(q.len());
    let (n, suffix) = q.split_at(split);
    let n: u64 = n.parse().ok()?;
    let scale: u64 = match suffix {
        "" => 1,
        "Ki" => 1 << 10,
        "Mi" => 1 << 20,
        "Gi" => 1 << 30,
        "Ti" => 1 << 40,
        "k" => 1_000,
        "M" => 1_000_000,
        "G" => 1_000_000_000,
        "T" => 1_000_000_000_000,
        _ => return None,
    };
    n.checked_mul(scale)
}

/// `disk-report`: every `every`, measure each `name=path` (hostPath mounts of
/// this node's filesystems) and report it, for the life of the process.
pub fn report_forever(node: &str, paths: &[(String, String)], t: Thresholds, every: Duration) {
    loop {
        for (name, path) in paths {
            match measure(path) {
                Some(usage) => t.report(&format!("node {node} {name}"), usage),
                None => tracing::warn!(node, %name, %path, "couldn't measure a disk"),
            }
        }
        std::thread::sleep(every);
    }
}

fn measure(path: &str) -> Option<Usage> {
    let out = std::process::Command::new("df")
        .args(["-Pk", path])
        .output()
        .ok()?;
    parse_df(String::from_utf8_lossy(&out.stdout).lines().last()?)
}

/// `name=path,name=path` (e.g. `node=/host/root,images=/host/images`).
pub fn parse_paths(spec: &str) -> Vec<(String, String)> {
    spec.split(',')
        .filter_map(|p| {
            let (name, path) = p.split_once('=')?;
            Some((name.trim().to_owned(), path.trim().to_owned()))
        })
        .filter(|(n, p)| !n.is_empty() && !p.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Thresholds = Thresholds {
        warn: 80.0,
        error: 90.0,
    };

    #[test]
    fn levels_follow_the_thresholds() {
        let at = |used| {
            T.level(Usage {
                used,
                capacity: 100,
            })
        };
        assert_eq!(
            [at(50), at(80), at(89), at(90), at(100)],
            [
                Level::Ok,
                Level::Warn,
                Level::Warn,
                Level::Error,
                Level::Error
            ]
        );
        assert_eq!(
            T.level(Usage {
                used: 5,
                capacity: 0
            }),
            Level::Ok
        );
    }

    #[test]
    fn df_lines_and_quantities_parse() {
        assert_eq!(
            parse_df("/dev/vda4  104857600  73400320  31457280  70% /var/lib/registry"),
            Some(Usage {
                used: 73_400_320 * 1024,
                capacity: 104_857_600 * 1024
            })
        );
        assert_eq!(
            parse_df("Filesystem 1024-blocks Used Available Capacity Mounted on"),
            None
        );
        assert_eq!(bytes("1Gi"), Some(1 << 30));
        assert_eq!(bytes("500M"), Some(500_000_000));
        assert_eq!(bytes("10Gx"), None);
    }

    #[test]
    fn paths_are_named() {
        assert_eq!(
            parse_paths("node=/host/root, images=/host/images,bad,=x"),
            [
                ("node".to_owned(), "/host/root".to_owned()),
                ("images".to_owned(), "/host/images".to_owned())
            ]
        );
    }
}
