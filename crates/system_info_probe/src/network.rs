//! Network interface traffic.

use std::time::Instant;

/// Name prefixes of virtual and container interfaces.
pub const VIRTUAL_PREFIXES: &[&str] = &["docker", "veth", "br-", "virbr", "tun", "tap", "dummy", "lo"];

pub fn is_virtual_name(name: &str) -> bool {
    VIRTUAL_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum InterfaceKind {
    Wifi,
    Ethernet,
    Virtual,
    Unknown,
}

impl InterfaceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Wifi => "WiFi",
            Self::Ethernet => "Ethernet",
            Self::Virtual => "Virtual",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Interface {
    pub name: String,
    pub kind: InterfaceKind,
    /// Adapter model from the PCI database, or the kernel driver name.
    pub model: Option<String>,
    /// Negotiated link speed, when the driver reports one (it does not while the link is down).
    pub speed_mbps: Option<u64>,
}

impl Interface {
    /// Kind, model and speed are best effort and only known on Linux.
    pub fn describe(name: &str) -> Self {
        #[cfg(target_os = "linux")]
        {
            let sys_path = std::path::Path::new("/sys/class/net").join(name);
            let device_path = sys_path.join("device");
            // Wireless cards are ARPHRD_ETHER like wired ones; the wireless directories tell them apart.
            let kind = if sys_path.join("wireless").exists() || sys_path.join("phy80211").exists() {
                InterfaceKind::Wifi
            } else if device_path.exists() {
                InterfaceKind::Ethernet
            } else {
                InterfaceKind::Virtual
            };
            let speed_mbps = std::fs::read_to_string(sys_path.join("speed"))
                .ok()
                .and_then(|s| s.trim().parse::<i64>().ok())
                .and_then(|speed| u64::try_from(speed).ok())
                .filter(|&speed| speed > 0);
            Self {
                name: name.to_string(),
                kind,
                model: crate::pci::device_name(&device_path).or_else(|| crate::pci::driver_name(&device_path)),
                speed_mbps,
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self {
                name: name.to_string(),
                kind: InterfaceKind::Unknown,
                model: None,
                speed_mbps: None,
            }
        }
    }
}

/// Names of every interface, sorted.
pub fn interface_names() -> Vec<String> {
    #[cfg(target_os = "linux")]
    let mut names: Vec<String> = std::fs::read_dir("/sys/class/net")
        .map(|entries| entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    #[cfg(not(target_os = "linux"))]
    let mut names: Vec<String> = sysinfo::Networks::new_with_refreshed_list().keys().cloned().collect();
    names.sort_unstable();
    names
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InterfaceQuery {
    /// Every non-virtual interface except the excluded names.
    All {
        excluded: Vec<String>,
    },
    Requested(Vec<String>),
}

/// Chosen interfaces, sorted by name, and the requested names that do not exist.
pub fn select(query: &InterfaceQuery) -> (Vec<Interface>, Vec<String>) {
    let real: Vec<String> = interface_names().into_iter().filter(|n| !is_virtual_name(n)).collect();
    let (mut names, missing) = match query {
        InterfaceQuery::All { excluded } => (real.into_iter().filter(|n| !excluded.contains(n)).collect::<Vec<_>>(), Vec::new()),
        InterfaceQuery::Requested(requested) => {
            let (found, missing): (Vec<String>, Vec<String>) = requested.iter().cloned().partition(|r| real.contains(r));
            (found, missing)
        }
    };
    names.sort_unstable();
    (names.iter().map(|n| Interface::describe(n)).collect(), missing)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NetworkCounters {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NetworkRates {
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

impl NetworkRates {
    pub fn between(earlier: NetworkCounters, later: NetworkCounters, elapsed_secs: f64) -> Option<Self> {
        (elapsed_secs > 0.0).then(|| Self {
            rx_bytes_per_sec: later.rx_bytes.saturating_sub(earlier.rx_bytes) as f64 / elapsed_secs,
            tx_bytes_per_sec: later.tx_bytes.saturating_sub(earlier.tx_bytes) as f64 / elapsed_secs,
        })
    }
}

/// Counters of the tracked interfaces, aligned with [`NetworkSampler::interfaces`]; `None` for an
/// interface that disappeared.
#[derive(Clone, Debug)]
pub struct NetworkSample {
    pub taken_at: Instant,
    pub counters: Vec<Option<NetworkCounters>>,
}

impl NetworkSample {
    pub fn rates_since(&self, earlier: &Self) -> Vec<Option<NetworkRates>> {
        let elapsed = self.taken_at.saturating_duration_since(earlier.taken_at).as_secs_f64();
        self.counters
            .iter()
            .zip(&earlier.counters)
            .map(|(now, before)| NetworkRates::between((*before)?, (*now)?, elapsed))
            .collect()
    }
}

pub struct NetworkSampler {
    interfaces: Vec<Interface>,
    #[cfg(target_os = "linux")]
    scratch: (String, String),
    #[cfg(not(target_os = "linux"))]
    networks: sysinfo::Networks,
}

impl NetworkSampler {
    pub fn new(interfaces: Vec<Interface>) -> Self {
        Self {
            interfaces,
            #[cfg(target_os = "linux")]
            scratch: (String::new(), String::new()),
            #[cfg(not(target_os = "linux"))]
            networks: sysinfo::Networks::new_with_refreshed_list(),
        }
    }

    pub fn interfaces(&self) -> &[Interface] {
        &self.interfaces
    }

    /// Two small sysfs reads per tracked interface, nothing for the others.
    #[cfg(target_os = "linux")]
    pub fn sample(&mut self) -> NetworkSample {
        use std::fmt::Write as _;
        let taken_at = Instant::now();
        let (path, text) = &mut self.scratch;
        let mut read = |iface: &str, file: &str| -> Option<u64> {
            path.clear();
            let _ = write!(path, "/sys/class/net/{iface}/statistics/{file}");
            crate::backend::read_text_into(&*path, text).ok()?;
            text.trim().parse().ok()
        };
        let counters = self
            .interfaces
            .iter()
            .map(|iface| {
                Some(NetworkCounters {
                    rx_bytes: read(&iface.name, "rx_bytes")?,
                    tx_bytes: read(&iface.name, "tx_bytes")?,
                })
            })
            .collect();
        NetworkSample { taken_at, counters }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn sample(&mut self) -> NetworkSample {
        let taken_at = Instant::now();
        self.networks.refresh(true);
        let counters = self
            .interfaces
            .iter()
            .map(|iface| {
                self.networks.get(&iface.name).map(|n| NetworkCounters {
                    rx_bytes: n.total_received(),
                    tx_bytes: n.total_transmitted(),
                })
            })
            .collect();
        NetworkSample { taken_at, counters }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_container_and_loopback_interfaces() {
        for name in ["lo", "docker0", "veth12ab", "br-1234", "virbr0", "tun0", "tap1", "dummy0"] {
            assert!(is_virtual_name(name), "{name}");
        }
        for name in ["eth0", "wlan0", "enp3s0", "wlp2s0"] {
            assert!(!is_virtual_name(name), "{name}");
        }
    }

    #[test]
    fn computes_rates_and_tolerates_counter_resets() {
        let earlier = NetworkCounters {
            rx_bytes: 1000,
            tx_bytes: 500,
        };
        let later = NetworkCounters {
            rx_bytes: 3000,
            tx_bytes: 100,
        };
        let rates = NetworkRates::between(earlier, later, 2.0).expect("positive window");
        assert!((rates.rx_bytes_per_sec - 1000.0).abs() < 1e-9);
        assert!(rates.tx_bytes_per_sec.abs() < 1e-9);
        assert!(NetworkRates::between(earlier, later, 0.0).is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_loopback_counters() {
        let mut sampler = NetworkSampler::new(vec![Interface::describe("lo"), Interface::describe("definitely-missing0")]);
        let sample = sampler.sample();
        assert!(sample.counters[0].is_some());
        assert!(sample.counters[1].is_none());
    }
}
