use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use log::info;
use system_info_probe::network::NetworkSampler;

use crate::discovery::RuntimeDiscovery;
use crate::settings::CollectSettings;
use crate::shared_state::{NetworkInterfaceSnapshot, SharedState};

/// Traffic of each discovered interface, into `state.latest_networks[iface_index]`.
pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>, discovery: Arc<RuntimeDiscovery>) {
    let mut sampler = NetworkSampler::new(discovery.interfaces.iter().map(|i| i.interface.clone()).collect());
    let interval_ms = (settings.network_interval_secs * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(50)));
    let mut previous = sampler.sample();
    interval.tick().await;

    loop {
        interval.tick().await;
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let current = sampler.sample();
        let snapshots = current
            .rates_since(&previous)
            .into_iter()
            .zip(&current.counters)
            .map(|(rates, totals)| {
                let (rates, totals) = (rates?, (*totals)?);
                Some(NetworkInterfaceSnapshot {
                    rx_bytes_per_sec: rates.rx_bytes_per_sec,
                    tx_bytes_per_sec: rates.tx_bytes_per_sec,
                    total_rx_bytes: totals.rx_bytes,
                    total_tx_bytes: totals.tx_bytes,
                })
            })
            .collect();
        state.write().expect("SharedState RwLock poisoned").latest_networks = snapshots;
        previous = current;
    }
    info!("network_worker stopped");
}
