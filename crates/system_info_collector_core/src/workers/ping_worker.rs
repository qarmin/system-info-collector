use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use log::{info, warn};
use system_info_probe::ping::PingTarget;
use system_info_probe::tokio::{AsyncPinger, resolve};
use tokio::time::MissedTickBehavior;

use crate::discovery::RuntimeDiscovery;
use crate::settings::CollectSettings;
use crate::shared_state::SharedState;

const ADDRESS_REFRESH: Duration = Duration::from_secs(10);

/// Checks that an ICMP socket can be opened at all, with a hint on what permission is missing.
pub fn check_permission() -> Result<(), String> {
    AsyncPinger::new().map(drop).map_err(|e| format!("Ping metrics disabled - {e}"))
}

struct Target {
    target: PingTarget,
    address: Option<IpAddr>,
    refreshed_at: Option<Instant>,
}

impl Target {
    async fn current_address(&mut self, timeout: Duration) -> Option<IpAddr> {
        if self.refreshed_at.is_none_or(|t| t.elapsed() >= ADDRESS_REFRESH) {
            let address = resolve(&self.target, timeout).await;
            if address != self.address || self.refreshed_at.is_none() {
                match address {
                    Some(ip) => info!("Ping target {} is {ip}", self.target),
                    None => warn!("Ping target {} has no address, counting its requests as lost", self.target),
                }
            }
            self.address = address;
            self.refreshed_at = Some(Instant::now());
        }
        self.address
    }
}

/// One echo per target every `ping_interval_secs`, all sent together; each result is added to
/// `state.ping_windows[target_index]` as soon as it is known, and file_writer drains the windows
/// per row.
pub async fn run(settings: Arc<CollectSettings>, state: Arc<RwLock<SharedState>>, shutdown: Arc<AtomicBool>, discovery: Arc<RuntimeDiscovery>) {
    let timeout = Duration::from_secs_f32(settings.ping_timeout_secs);
    let mut targets: Vec<Target> = discovery
        .ping_targets
        .iter()
        .map(|t| Target {
            target: t.host.parse().unwrap_or_else(|never| match never {}),
            address: None,
            refreshed_at: None,
        })
        .collect();
    let mut interval = tokio::time::interval(Duration::from_secs_f32(settings.ping_interval_secs));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let mut addressed: Vec<(usize, IpAddr)> = Vec::with_capacity(targets.len());
        for (index, target) in targets.iter_mut().enumerate() {
            match target.current_address(timeout).await {
                Some(address) => addressed.push((index, address)),
                None => record(&state, index, None),
            }
        }
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let mut pinger = match AsyncPinger::new() {
                Ok(pinger) => pinger,
                Err(e) => {
                    warn!("Ping skipped: {e}");
                    return;
                }
            };
            let addresses: Vec<IpAddr> = addressed.iter().map(|(_, address)| *address).collect();
            pinger
                .ping_each(&addresses, timeout, |position, rtt| record(&state, addressed[position].0, rtt))
                .await;
        });
    }
    info!("ping_worker stopped");
}

fn record(state: &RwLock<SharedState>, target_index: usize, rtt: Option<Duration>) {
    let mut guard = state.write().expect("SharedState RwLock poisoned");
    if let Some(window) = guard.ping_windows.get_mut(target_index) {
        window.record(rtt);
    }
}
