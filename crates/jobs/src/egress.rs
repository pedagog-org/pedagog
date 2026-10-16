//! `pedagog-jobs await-egress <host:port> <seconds>`: a test-run pod's first
//! initContainer. Network policies apply a few seconds after a pod starts, so
//! until then egress is open; this blocks until a connection to an address
//! every policy denies fails, and fails the pod if that takes too long.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use anyhow::bail;

const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(1);
const RETRY_DELAY: Duration = Duration::from_millis(500);

pub fn await_egress(target: &str, timeout: Duration) -> anyhow::Result<()> {
    let started = Instant::now();
    let mut attempts = 0;
    loop {
        attempts += 1;
        // Resolution needs DNS, which every policy allows; a failure is transient.
        if let Some(addr) = resolve(target)
            && TcpStream::connect_timeout(&addr, ATTEMPT_TIMEOUT).is_err()
        {
            println!(
                "egress is closed ({target} unreachable after {attempts} attempt(s), {:.1}s)",
                started.elapsed().as_secs_f64()
            );
            return Ok(());
        }
        if started.elapsed() >= timeout {
            bail!(
                "egress still open after {}s: {target} is reachable, so the network policies aren't in force",
                timeout.as_secs()
            );
        }
        std::thread::sleep(RETRY_DELAY);
    }
}

fn resolve(target: &str) -> Option<SocketAddr> {
    target.to_socket_addrs().ok()?.next()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::TcpListener;

    #[test]
    fn returns_once_the_probe_is_unreachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = listener.local_addr().unwrap().to_string();
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(700));
            drop(listener);
        });
        let started = Instant::now();
        await_egress(&target, Duration::from_secs(10)).unwrap();
        assert!(
            started.elapsed() >= Duration::from_millis(500),
            "waited while reachable"
        );
        closer.join().unwrap();
    }

    #[test]
    fn fails_if_the_probe_stays_reachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = listener.local_addr().unwrap().to_string();
        let err = await_egress(&target, Duration::from_secs(1)).unwrap_err();
        assert!(err.to_string().contains("still open"), "{err}");
        drop(listener);
    }
}
