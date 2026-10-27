//! ICMP echo latency and loss.  The blocking and tokio pingers share everything here and differ
//! only in how they wait for the socket.

#[cfg(target_os = "linux")]
pub(crate) mod icmp;

use std::net::{IpAddr, Ipv4Addr};
use std::str::FromStr;
use std::time::Duration;

/// Echo results accumulated over a window, e.g. between two rows of output.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PingWindow {
    pub replies: u32,
    pub lost: u32,
    pub rtt_sum: Duration,
}

impl PingWindow {
    pub fn record(&mut self, rtt: Option<Duration>) {
        match rtt {
            Some(rtt) => {
                self.replies += 1;
                self.rtt_sum += rtt;
            }
            None => self.lost += 1,
        }
    }

    pub fn average_rtt(&self) -> Option<Duration> {
        (self.replies > 0).then(|| self.rtt_sum / self.replies)
    }

    pub fn loss_percent(&self) -> Option<f64> {
        let total = self.replies + self.lost;
        (total > 0).then(|| f64::from(self.lost) * 100.0 / f64::from(total))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum PingTarget {
    /// The default route's gateway, re-read on every resolution so it follows network changes.
    Gateway,
    Address(IpAddr),
    Host(String),
}

impl FromStr for PingTarget {
    type Err = std::convert::Infallible;

    /// `gateway`, an IP address, or anything else as a host name.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(if s == "gateway" {
            Self::Gateway
        } else if let Ok(ip) = s.parse() {
            Self::Address(ip)
        } else {
            Self::Host(s.to_string())
        })
    }
}

impl std::fmt::Display for PingTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gateway => f.write_str("gateway"),
            Self::Address(ip) => write!(f, "{ip}"),
            Self::Host(host) => f.write_str(host),
        }
    }
}

const RTF_UP: u32 = 0x1;
const RTF_GATEWAY: u32 = 0x2;

/// Default route with the lowest metric from `/proc/net/route`, whose addresses are hex dumps of
/// the in-memory (network order) value.
pub fn parse_default_gateway(route_table: &str) -> Option<Ipv4Addr> {
    route_table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (destination, gateway, flags, metric) = (fields.get(1)?, fields.get(2)?, fields.get(3)?, fields.get(6)?);
            let flags = u32::from_str_radix(flags, 16).ok()?;
            if *destination != "00000000" || flags & RTF_UP == 0 || flags & RTF_GATEWAY == 0 {
                return None;
            }
            let gateway = u32::from_str_radix(gateway, 16).ok()?;
            Some((metric.parse::<u32>().ok()?, Ipv4Addr::from(gateway.to_ne_bytes())))
        })
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, gateway)| gateway)
}

/// From `/proc/net/route`, so `None` outside Linux.
pub fn default_gateway() -> Option<IpAddr> {
    parse_default_gateway(&std::fs::read_to_string("/proc/net/route").ok()?).map(IpAddr::V4)
}

/// IPv4 is preferred when a name has both, as most networks route it more reliably.
#[cfg(target_os = "linux")]
pub(crate) fn prefer_ipv4(addresses: impl IntoIterator<Item = IpAddr>) -> Option<IpAddr> {
    let all: Vec<IpAddr> = addresses.into_iter().collect();
    all.iter().find(|a| a.is_ipv4()).or(all.first()).copied()
}

#[cfg(target_os = "linux")]
pub(crate) fn socket_error_hint(e: &std::io::Error) -> String {
    format!(
        "could not open an ICMP socket ({}). Grant CAP_NET_RAW (`setcap cap_net_raw+ep <binary>`) or allow unprivileged ICMP \
         (`sysctl -w net.ipv4.ping_group_range=\"0 2147483647\"`).",
        e.kind()
    )
}

#[cfg(all(test, target_endian = "little"))]
mod tests {
    use super::*;

    const HEADER: &str = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n";

    #[test]
    fn picks_default_route_with_lowest_metric() {
        let table = format!(
            "{HEADER}wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
             eth0\t00000000\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
             eth0\t0000000A\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n"
        );
        assert_eq!(parse_default_gateway(&table), Some(Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn ignores_routes_without_gateway_flag_or_no_default() {
        let table = format!("{HEADER}eth0\t00000000\t0101A8C0\t0001\t0\t0\t100\t00000000\t0\t0\t0\n");
        assert_eq!(parse_default_gateway(&table), None);
        assert_eq!(parse_default_gateway(HEADER), None);
    }

    #[test]
    fn parses_targets() {
        assert_eq!("gateway".parse::<PingTarget>(), Ok(PingTarget::Gateway));
        assert_eq!(
            "1.1.1.1".parse::<PingTarget>(),
            Ok(PingTarget::Address(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))))
        );
        assert_eq!("example.com".parse::<PingTarget>(), Ok(PingTarget::Host("example.com".to_string())));
    }

    #[test]
    fn window_averages_and_counts_loss() {
        let mut window = PingWindow::default();
        assert_eq!(window.average_rtt(), None);
        window.record(Some(Duration::from_millis(10)));
        window.record(Some(Duration::from_millis(30)));
        window.record(None);
        window.record(None);
        assert_eq!(window.average_rtt(), Some(Duration::from_millis(20)));
        assert_eq!(window.loss_percent(), Some(50.0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn prefers_ipv4() {
        let v6: IpAddr = "::1".parse().expect("valid");
        let v4: IpAddr = "127.0.0.1".parse().expect("valid");
        assert_eq!(prefer_ipv4([v6, v4]), Some(v4));
        assert_eq!(prefer_ipv4([v6]), Some(v6));
        assert_eq!(prefer_ipv4([]), None);
    }
}
