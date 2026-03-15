use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use log::info;
use sysinfo::Networks;

use crate::settings::CollectSettings;
use crate::shared_state::{NetworkSnapshot, SharedState};

pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>) {
    let mut networks = Networks::new_with_refreshed_list();

    let mut prev_rx: u64 = networks.values().map(|n| n.total_received()).sum();
    let mut prev_tx: u64 = networks.values().map(|n| n.total_transmitted()).sum();
    let mut last_tick = Instant::now();

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

        let total_rx: u64 = networks.values().map(|n| n.total_received()).sum();
        let total_tx: u64 = networks.values().map(|n| n.total_transmitted()).sum();

        let elapsed = last_tick.elapsed().as_secs_f64().max(f64::EPSILON);
        last_tick = Instant::now();

        let rx_bps = (total_rx.saturating_sub(prev_rx)) as f64 / elapsed;
        let tx_bps = (total_tx.saturating_sub(prev_tx)) as f64 / elapsed;

        prev_rx = total_rx;
        prev_tx = total_tx;

        let snapshot = NetworkSnapshot {
            rx_bytes_per_sec: rx_bps,
            tx_bytes_per_sec: tx_bps,
            total_rx_bytes: total_rx,
            total_tx_bytes: total_tx,
        };

        {
            let mut guard = state.write().expect("SharedState RwLock poisoned");
            guard.latest_network = Some(snapshot);
        }
    }

    info!("network_worker stopped");
}
