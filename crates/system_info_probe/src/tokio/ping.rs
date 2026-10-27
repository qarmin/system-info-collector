use std::net::IpAddr;
use std::time::Duration;

use socket2::Socket;
use tokio::io::unix::AsyncFd;
use tokio::time::Instant;

use crate::error::{ProbeError, ProbeResult};
use crate::ping::icmp::{self, Echoes, Family, SocketKind};
use crate::ping::{PingTarget, default_gateway, prefer_ipv4, socket_error_hint};

struct IcmpSocket {
    fd: AsyncFd<Socket>,
    kind: SocketKind,
}

impl IcmpSocket {
    fn open(family: Family) -> std::io::Result<Self> {
        let (socket, kind) = icmp::open(family)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            fd: AsyncFd::new(socket)?,
            kind,
        })
    }
}

/// [`crate::blocking::Pinger`] on tokio: the same packets and matching, with readiness from the
/// runtime instead of `poll`.
pub struct AsyncPinger {
    v4: IcmpSocket,
    v6: Option<Result<IcmpSocket, String>>,
    echoes: Echoes,
}

impl AsyncPinger {
    /// Must be called inside a tokio runtime.
    pub fn new() -> ProbeResult<Self> {
        let v4 = IcmpSocket::open(Family::V4).map_err(|e| ProbeError::InvalidConfig(format!("Ping unavailable - {}", socket_error_hint(&e))))?;
        Ok(Self {
            v4,
            v6: None,
            echoes: Echoes::new(),
        })
    }

    /// Round trip per address, `None` for no reply within `timeout`; takes at most `timeout`.
    pub async fn ping_all(&mut self, addresses: &[IpAddr], timeout: Duration) -> Vec<Option<Duration>> {
        let mut results = vec![None; addresses.len()];
        self.echoes.clear();
        for (index, address) in addresses.iter().enumerate() {
            let family = Family::of(*address);
            let socket = match family {
                Family::V4 => &self.v4,
                Family::V6 => {
                    let opened = self.v6.get_or_insert_with(|| {
                        IcmpSocket::open(Family::V6)
                            .inspect_err(|e| log::warn!("Could not open an ICMPv6 socket ({e}), IPv6 targets will show as lost"))
                            .map_err(|e| e.to_string())
                    });
                    let Ok(socket) = opened.as_ref() else { continue };
                    socket
                }
            };
            let sequence = self.echoes.send(index, std::time::Instant::now());
            let packet = icmp::encode_request(family, sequence, self.echoes.token());
            if let Err(e) = socket.fd.get_ref().send_to(&packet, &icmp::destination(*address)) {
                log::debug!("echo to {address} not sent: {e}");
            }
        }

        let deadline = Instant::now() + timeout;
        let v6 = self.v6.as_ref().and_then(|v6| v6.as_ref().ok());
        while !self.echoes.is_empty() {
            tokio::select! {
                ready = self.v4.fd.readable() => {
                    let Ok(mut guard) = ready else { break };
                    icmp::drain(self.v4.fd.get_ref(), Family::V4, self.v4.kind, &mut self.echoes, &mut results);
                    guard.clear_ready();
                }
                ready = readable(v6) => {
                    let (Ok(mut guard), Some(socket)) = (ready, v6) else { break };
                    icmp::drain(socket.fd.get_ref(), Family::V6, socket.kind, &mut self.echoes, &mut results);
                    guard.clear_ready();
                }
                () = tokio::time::sleep_until(deadline) => break,
            }
        }
        results
    }

    pub async fn ping(&mut self, address: IpAddr, timeout: Duration) -> Option<Duration> {
        self.ping_all(&[address], timeout).await.pop().flatten()
    }
}

/// Readiness of an optional socket; pends forever when there is none.
async fn readable(socket: Option<&IcmpSocket>) -> std::io::Result<tokio::io::unix::AsyncFdReadyGuard<'_, Socket>> {
    match socket {
        Some(socket) => socket.fd.readable().await,
        None => std::future::pending().await,
    }
}

/// Address of a target right now, giving up on name resolution after `timeout`.
pub async fn resolve(target: &PingTarget, timeout: Duration) -> Option<IpAddr> {
    match target {
        PingTarget::Gateway => default_gateway(),
        PingTarget::Address(ip) => Some(*ip),
        PingTarget::Host(host) => {
            let addresses = tokio::time::timeout(timeout, tokio::net::lookup_host((host.as_str(), 0)))
                .await
                .ok()?
                .ok()?;
            prefer_ipv4(addresses.map(|a| a.ip()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn localhost_answers_and_unroutable_times_out() {
        let Ok(mut pinger) = AsyncPinger::new() else { return };
        let localhost: IpAddr = "127.0.0.1".parse().expect("valid");
        let unroutable: IpAddr = "192.0.2.1".parse().expect("valid");
        let started = std::time::Instant::now();
        let rtts = pinger.ping_all(&[localhost, unroutable, localhost], Duration::from_millis(400)).await;
        assert!(rtts[0].is_some() && rtts[2].is_some(), "{rtts:?}");
        assert_eq!(rtts[1], None);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn resolves_addresses_and_localhost() {
        let ip: IpAddr = "10.1.2.3".parse().expect("valid");
        assert_eq!(resolve(&PingTarget::Address(ip), Duration::from_secs(1)).await, Some(ip));
        assert!(
            resolve(&PingTarget::Host("localhost".to_string()), Duration::from_secs(2))
                .await
                .is_some()
        );
    }
}
