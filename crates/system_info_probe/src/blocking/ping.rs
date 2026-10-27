use std::net::{IpAddr, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use socket2::Socket;

use crate::error::{ProbeError, ProbeResult};
use crate::ping::icmp::{self, Echoes, Family, SocketKind};
use crate::ping::{PingTarget, default_gateway, prefer_ipv4, socket_error_hint};

/// Sends echoes over one socket per address family and waits on both with `poll`.
pub struct Pinger {
    v4: (Socket, SocketKind),
    /// Opened on first use; `Err` keeps IPv6 targets reported as lost without retrying every time.
    v6: Option<Result<(Socket, SocketKind), String>>,
    echoes: Echoes,
}

impl Pinger {
    /// Fails when no ICMP socket can be opened, with a hint on which permission is missing.
    pub fn new() -> ProbeResult<Self> {
        let v4 = icmp::open(Family::V4).map_err(|e| ProbeError::InvalidConfig(format!("Ping unavailable - {}", socket_error_hint(&e))))?;
        Ok(Self {
            v4,
            v6: None,
            echoes: Echoes::new(),
        })
    }

    /// Round trip per address, `None` for no reply within `timeout`.  All echoes are sent first,
    /// so the call takes at most `timeout` however many addresses there are.
    pub fn ping_all(&mut self, addresses: &[IpAddr], timeout: Duration) -> Vec<Option<Duration>> {
        let mut results = vec![None; addresses.len()];
        self.echoes.clear();
        for (index, address) in addresses.iter().enumerate() {
            let family = Family::of(*address);
            let Some((socket, _)) = socket_for(&self.v4, &mut self.v6, family) else {
                continue;
            };
            let sequence = self.echoes.send(index, Instant::now());
            let packet = icmp::encode_request(family, sequence, self.echoes.token());
            if let Err(e) = socket.send_to(&packet, &icmp::destination(*address)) {
                log::debug!("echo to {address} not sent: {e}");
            }
        }

        let deadline = Instant::now() + timeout;
        while !self.echoes.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let mut fds: Vec<libc::pollfd> = [Some(&self.v4), self.v6.as_ref().and_then(|v6| v6.as_ref().ok())]
                .into_iter()
                .flatten()
                .map(|(socket, _)| libc::pollfd {
                    fd: socket.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                })
                .collect();
            let timeout_ms = i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX).max(1);
            // SAFETY: `fds` is a valid array of `fds.len()` pollfd entries.
            if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) } <= 0 {
                continue;
            }
            for fd in fds.iter().filter(|fd| fd.revents & libc::POLLIN != 0) {
                let (family, kind, socket) = if fd.fd == self.v4.0.as_raw_fd() {
                    (Family::V4, self.v4.1, &self.v4.0)
                } else if let Some(Ok((socket, kind))) = &self.v6 {
                    (Family::V6, *kind, socket)
                } else {
                    continue;
                };
                icmp::drain(socket, family, kind, &mut self.echoes, &mut results);
            }
        }
        results
    }

    pub fn ping(&mut self, address: IpAddr, timeout: Duration) -> Option<Duration> {
        self.ping_all(&[address], timeout).pop().flatten()
    }
}

fn socket_for<'a>(
    v4: &'a (Socket, SocketKind),
    v6: &'a mut Option<Result<(Socket, SocketKind), String>>,
    family: Family,
) -> Option<&'a (Socket, SocketKind)> {
    match family {
        Family::V4 => Some(v4),
        Family::V6 => v6
            .get_or_insert_with(|| {
                icmp::open(Family::V6)
                    .inspect_err(|e| log::warn!("Could not open an ICMPv6 socket ({e}), IPv6 targets will show as lost"))
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .ok(),
    }
}

/// Address of a target right now.  Host names go through the system resolver, which has no
/// timeout of its own.
pub fn resolve(target: &PingTarget) -> Option<IpAddr> {
    match target {
        PingTarget::Gateway => default_gateway(),
        PingTarget::Address(ip) => Some(*ip),
        PingTarget::Host(host) => prefer_ipv4((host.as_str(), 0).to_socket_addrs().ok()?.map(|a| a.ip())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localhost_answers() {
        let Ok(mut pinger) = Pinger::new() else { return };
        let localhost: IpAddr = "127.0.0.1".parse().expect("valid");
        let rtts = pinger.ping_all(&[localhost, localhost], Duration::from_secs(2));
        assert!(rtts.iter().all(Option::is_some), "{rtts:?}");
        assert!(rtts.iter().flatten().all(|rtt| *rtt < Duration::from_secs(1)));
    }

    #[test]
    fn unroutable_address_times_out() {
        let Ok(mut pinger) = Pinger::new() else { return };
        // TEST-NET-1, reserved for documentation and never routed.
        let started = Instant::now();
        assert_eq!(pinger.ping("192.0.2.1".parse().expect("valid"), Duration::from_millis(300)), None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
