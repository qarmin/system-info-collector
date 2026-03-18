use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use log::info;
use sysinfo::Networks;

use crate::discovery::RuntimeDiscovery;
use crate::settings::CollectSettings;
use crate::shared_state::{NetworkInterfaceSnapshot, SharedState};

/// Polls each discovered network interface independently.
/// Each interface's snapshot is written to `state.latest_networks[iface.iface_index]`.
pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>, discovery: Arc<RuntimeDiscovery>) {
    let mut networks = Networks::new_with_refreshed_list();

    // Per-interface state: prev totals and last tick time.
    let mut prev: Vec<(u64, u64, Instant)> = discovery
        .interfaces
        .iter()
        .map(|iface| {
            let (rx, tx) = networks
                .get(iface.name.as_str())
                .map_or((0, 0), |n| (n.total_received(), n.total_transmitted()));
            (rx, tx, Instant::now())
        })
        .collect();

    let interval_ms = (settings.network_interval_secs * 1000.0) as u64;
    let mut interval = tokio::time::interval(Duration::from_millis(interval_ms.max(50)));

    // consume the instant first tick
    interval.tick().await;

    loop {
        interval.tick().await;

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        networks.refresh(true);

        let mut snapshots: Vec<Option<NetworkInterfaceSnapshot>> = vec![None; discovery.interfaces.len()];

        for iface in &discovery.interfaces {
            let idx = iface.iface_index;
            let (prev_rx, prev_tx, last_tick) = &mut prev[idx];

            let (total_rx, total_tx) = networks
                .get(iface.name.as_str())
                .map_or((*prev_rx, *prev_tx), |n| (n.total_received(), n.total_transmitted()));

            let elapsed = last_tick.elapsed().as_secs_f64().max(f64::EPSILON);
            *last_tick = Instant::now();

            let rx_bps = (total_rx.saturating_sub(*prev_rx)) as f64 / elapsed;
            let tx_bps = (total_tx.saturating_sub(*prev_tx)) as f64 / elapsed;

            *prev_rx = total_rx;
            *prev_tx = total_tx;

            snapshots[idx] = Some(NetworkInterfaceSnapshot {
                rx_bytes_per_sec: rx_bps,
                tx_bytes_per_sec: tx_bps,
                total_rx_bytes: total_rx,
                total_tx_bytes: total_tx,
            });
        }

        {
            let mut guard = state.write().expect("SharedState RwLock poisoned");
            guard.latest_networks = snapshots;
        }
    }

    info!("network_worker stopped");
}
