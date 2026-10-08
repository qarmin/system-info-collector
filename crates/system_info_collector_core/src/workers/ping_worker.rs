use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use log::{info, warn};
use surge_ping::{Client, Config, ICMP, PingIdentifier, PingSequence};
use tokio::time::MissedTickBehavior;

use crate::discovery::RuntimeDiscovery;
use crate::settings::CollectSettings;
use crate::shared_state::SharedState;

pub const GATEWAY_TARGET: &str = "gateway";
const ADDRESS_REFRESH: Duration = Duration::from_secs(10);
const PAYLOAD: [u8; 32] = [0; 32];
const RTF_UP: u32 = 0x1;
const RTF_GATEWAY: u32 = 0x2;

/// Tries an unprivileged ICMP socket first and falls back to a raw one (root / CAP_NET_RAW).
pub fn open_client() -> Result<Client, String> {
    Client::new(&Config::default()).map_err(|e| {
        format!(
            "Ping metrics disabled - could not open an ICMP socket ({}). Grant CAP_NET_RAW (`setcap cap_net_raw+ep <binary>`) or allow unprivileged ICMP (`sysctl -w net.ipv4.ping_group_range=\"0 2147483647\"`).",
            e.kind()
        )
    })
}

struct TargetState {
    host: String,
    address: Option<IpAddr>,
    refreshed_at: Option<Instant>,
}

impl TargetState {
    async fn current_address(&mut self, timeout: Duration) -> Option<IpAddr> {
        if self.refreshed_at.is_none_or(|t| t.elapsed() >= ADDRESS_REFRESH) {
            let address = resolve(&self.host, timeout).await;
            if address != self.address || self.refreshed_at.is_none() {
                match address {
                    Some(ip) => info!("Ping target {} is {ip}", self.host),
                    None => warn!("Ping target {} has no address, counting its requests as lost", self.host),
                }
            }
            self.address = address;
            self.refreshed_at = Some(Instant::now());
        }
        self.address
    }
}

/// Sends one echo request per target every `ping_interval_secs`.  Each result is
/// added to `state.ping_windows[target_index]` once it is known, and file_writer
/// drains the windows when it writes a row.
pub async fn run(
    settings: Arc<CollectSettings>,
    state: Arc<RwLock<SharedState>>,
    shutdown: Arc<AtomicBool>,
    discovery: Arc<RuntimeDiscovery>,
    client_v4: Client,
) {
    let timeout = Duration::from_secs_f32(settings.ping_timeout_secs);
    let mut targets: Vec<TargetState> = discovery
        .ping_targets
        .iter()
        .map(|t| TargetState {
            host: t.host.clone(),
            address: None,
            refreshed_at: None,
        })
        .collect();
    let mut client_v6: Option<Option<Client>> = None;
    let ident = PingIdentifier(std::process::id() as u16);
    let mut seq: u16 = 0;

    let mut interval = tokio::time::interval(Duration::from_secs_f32(settings.ping_interval_secs));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        for (target_index, target) in targets.iter_mut().enumerate() {
            let Some(address) = target.current_address(timeout).await else {
                record(&state, target_index, None);
                continue;
            };

            let client = match address {
                IpAddr::V4(_) => &client_v4,
                IpAddr::V6(_) => {
                    let client = client_v6.get_or_insert_with(|| {
                        Client::new(&Config::builder().kind(ICMP::V6).build())
                            .inspect_err(|e| warn!("Could not open an ICMPv6 socket ({e}), IPv6 ping targets will show as lost"))
                            .ok()
                    });
                    let Some(client) = client.as_ref() else {
                        record(&state, target_index, None);
                        continue;
                    };
                    client
                }
            };

            // One counter shared by all targets keeps (address, seq) unique even when
            // two targets resolve to the same host.
            seq = seq.wrapping_add(1);
            let sequence = PingSequence(seq);
            let mut pinger = client.pinger(address, ident).await;
            pinger.timeout(timeout);
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                let rtt = pinger.ping(sequence, &PAYLOAD).await.ok().map(|(_, rtt)| rtt);
                record(&state, target_index, rtt);
            });
        }
    }

    info!("ping_worker stopped");
}

fn record(state: &RwLock<SharedState>, target_index: usize, rtt: Option<Duration>) {
    let mut guard = state.write().expect("SharedState RwLock poisoned");
    if let Some(window) = guard.ping_windows.get_mut(target_index) {
        match rtt {
            Some(rtt) => {
                window.replies += 1;
                window.rtt_sum_ms += rtt.as_secs_f64() * 1000.0;
            }
            None => window.lost += 1,
        }
    }
}

async fn resolve(host: &str, timeout: Duration) -> Option<IpAddr> {
    if host == GATEWAY_TARGET {
        let route_table = std::fs::read_to_string("/proc/net/route").ok()?;
        return parse_default_gateway(&route_table).map(IpAddr::V4);
    }
    if let Ok(ip) = host.parse() {
        return Some(ip);
    }
    let addresses: Vec<IpAddr> = tokio::time::timeout(timeout, tokio::net::lookup_host((host, 0)))
        .await
        .ok()?
        .ok()?
        .map(|a| a.ip())
        .collect();
    addresses.iter().find(|a| a.is_ipv4()).or(addresses.first()).copied()
}

/// Default route with the lowest metric from `/proc/net/route`, whose addresses
/// are hex dumps of the in-memory (network order) value.
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
    fn ignores_routes_without_gateway_flag() {
        let table = format!("{HEADER}eth0\t00000000\t0101A8C0\t0001\t0\t0\t100\t00000000\t0\t0\t0\n");
        assert_eq!(parse_default_gateway(&table), None);
    }

    #[test]
    fn no_default_route() {
        assert_eq!(parse_default_gateway(HEADER), None);
    }
}
