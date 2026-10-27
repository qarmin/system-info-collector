//! ICMP echo packets and the bookkeeping of echoes in flight.
//!
//! An unprivileged `SOCK_DGRAM` ICMP socket is tried first, then a raw one (root or
//! `CAP_NET_RAW`).  The kernel rewrites the identifier of datagram echoes and a raw socket
//! receives every ICMP packet on the machine, so replies are matched on sequence number plus a
//! token in the payload rather than on the identifier.

use std::collections::HashMap;
use std::mem::MaybeUninit;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, SockAddr, Socket, Type};

const PAYLOAD_LEN: usize = 32;
const HEADER_LEN: usize = 8;
pub const PACKET_LEN: usize = HEADER_LEN + PAYLOAD_LEN;
const ECHO_REQUEST_V4: u8 = 8;
const ECHO_REPLY_V4: u8 = 0;
const ECHO_REQUEST_V6: u8 = 128;
const ECHO_REPLY_V6: u8 = 129;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    pub fn of(address: IpAddr) -> Self {
        if address.is_ipv4() { Self::V4 } else { Self::V6 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SocketKind {
    Datagram,
    Raw,
}

pub fn open(family: Family) -> std::io::Result<(Socket, SocketKind)> {
    let (domain, protocol) = match family {
        Family::V4 => (Domain::IPV4, Protocol::ICMPV4),
        Family::V6 => (Domain::IPV6, Protocol::ICMPV6),
    };
    match Socket::new(domain, Type::DGRAM, Some(protocol)) {
        Ok(socket) => Ok((socket, SocketKind::Datagram)),
        Err(_) => Socket::new(domain, Type::RAW, Some(protocol)).map(|socket| (socket, SocketKind::Raw)),
    }
}

pub fn destination(address: IpAddr) -> SockAddr {
    SockAddr::from(SocketAddr::new(address, 0))
}

/// The ICMPv6 checksum covers a pseudo-header only the kernel knows, so it fills it in; ICMPv4
/// carries its own.
pub fn encode_request(family: Family, sequence: u16, token: u64) -> [u8; PACKET_LEN] {
    let mut packet = [0u8; PACKET_LEN];
    packet[0] = match family {
        Family::V4 => ECHO_REQUEST_V4,
        Family::V6 => ECHO_REQUEST_V6,
    };
    packet[4..6].copy_from_slice(&(std::process::id() as u16).to_be_bytes());
    packet[6..8].copy_from_slice(&sequence.to_be_bytes());
    packet[HEADER_LEN..HEADER_LEN + 8].copy_from_slice(&token.to_be_bytes());
    if family == Family::V4 {
        let checksum = internet_checksum(&packet);
        packet[2..4].copy_from_slice(&checksum.to_be_bytes());
    }
    packet
}

/// Sequence number and token of an echo reply; raw IPv4 sockets deliver the IP header too.
pub fn decode_reply(family: Family, kind: SocketKind, packet: &[u8]) -> Option<(u16, u64)> {
    let icmp = match (family, kind) {
        (Family::V4, SocketKind::Raw) => packet.get(usize::from(packet.first()? & 0x0f) * 4..)?,
        _ => packet,
    };
    let expected = match family {
        Family::V4 => ECHO_REPLY_V4,
        Family::V6 => ECHO_REPLY_V6,
    };
    if *icmp.first()? != expected || icmp.len() < HEADER_LEN + 8 {
        return None;
    }
    let sequence = u16::from_be_bytes([icmp[6], icmp[7]]);
    let token = u64::from_be_bytes(icmp[HEADER_LEN..HEADER_LEN + 8].try_into().ok()?);
    Some((sequence, token))
}

fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = data
        .chunks(2)
        .map(|pair| u32::from(u16::from_be_bytes([pair[0], pair.get(1).copied().unwrap_or(0)])))
        .sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Echoes sent and not yet answered, for any number of targets sharing one socket.
pub struct Echoes {
    token: u64,
    next_sequence: u16,
    pending: HashMap<u16, (usize, Instant)>,
}

impl Echoes {
    pub fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        Self {
            token: nanos ^ (u64::from(std::process::id()) << 32) ^ 0x9e37_79b9_7f4a_7c15,
            next_sequence: 0,
            pending: HashMap::new(),
        }
    }

    pub fn token(&self) -> u64 {
        self.token
    }

    /// Sequence number for target `index`, sent at `now`.
    pub fn send(&mut self, index: usize, now: Instant) -> u16 {
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.pending.insert(self.next_sequence, (index, now));
        self.next_sequence
    }

    /// Target index and round trip of the echo a reply answers, ignoring strangers and duplicates.
    pub fn receive(&mut self, sequence: u16, token: u64, now: Instant) -> Option<(usize, Duration)> {
        if token != self.token {
            return None;
        }
        let (index, sent) = self.pending.remove(&sequence)?;
        Some((index, now.duration_since(sent)))
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

/// Reads every queued packet without blocking.
pub fn drain(socket: &Socket, family: Family, kind: SocketKind, echoes: &mut Echoes, results: &mut [Option<Duration>]) {
    let mut buffer = [MaybeUninit::<u8>::uninit(); 128 + PACKET_LEN];
    while let Ok(length) = socket.recv_with_flags(&mut buffer, libc::MSG_DONTWAIT) {
        // SAFETY: recv initialized the first `length` bytes.
        let packet = unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), length) };
        if let Some((sequence, token)) = decode_reply(family, kind, packet)
            && let Some((index, rtt)) = echoes.receive(sequence, token, Instant::now())
            && let Some(slot) = results.get_mut(index)
        {
            *slot = Some(rtt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4_request_checksums_to_zero() {
        let packet = encode_request(Family::V4, 7, 42);
        assert_eq!(packet[0], ECHO_REQUEST_V4);
        assert_eq!(internet_checksum(&packet), 0);
    }

    #[test]
    fn decodes_replies_from_datagram_and_raw_sockets() {
        let mut reply = encode_request(Family::V4, 9, 1234);
        reply[0] = ECHO_REPLY_V4;
        assert_eq!(decode_reply(Family::V4, SocketKind::Datagram, &reply), Some((9, 1234)));

        let mut with_ip_header = vec![0x45u8];
        with_ip_header.extend_from_slice(&[0; 19]);
        with_ip_header.extend_from_slice(&reply);
        assert_eq!(decode_reply(Family::V4, SocketKind::Raw, &with_ip_header), Some((9, 1234)));

        let request = encode_request(Family::V4, 9, 1234);
        assert_eq!(decode_reply(Family::V4, SocketKind::Datagram, &request), None);
        assert_eq!(decode_reply(Family::V4, SocketKind::Datagram, &reply[..10]), None);
    }

    #[test]
    fn matches_replies_by_sequence_and_token() {
        let mut echoes = Echoes::new();
        let start = Instant::now();
        let first = echoes.send(0, start);
        let second = echoes.send(1, start);
        let later = start + Duration::from_millis(5);
        assert_eq!(echoes.receive(second, echoes.token(), later), Some((1, Duration::from_millis(5))));
        assert_eq!(echoes.receive(second, echoes.token(), later), None, "duplicate");
        assert_eq!(echoes.receive(first, echoes.token() ^ 1, later), None, "someone else's echo");
        assert!(!echoes.is_empty());
    }
}
