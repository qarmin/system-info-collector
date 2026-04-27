#![allow(clippy::collapsible_else_if)]
#![allow(clippy::type_complexity)]
#![allow(clippy::needless_late_init)]
#![allow(clippy::too_many_arguments)]
#![warn(clippy::unwrap_used)]
#![warn(clippy::print_stderr)]
#![warn(clippy::print_stdout)]
#![warn(clippy::dbg_macro)]
#![warn(clippy::string_slice)]

use std::sync::atomic::Ordering;
use std::thread::available_parallelism;
use std::{env, process};

use handsome_logger::{ColorChoice, ConfigBuilder, TermLogger, TerminalMode};
use log::{error, info, warn};
use sysinfo::System;
use system_info_collector_core::discovery::{list_real_disks, list_real_interfaces};
use system_info_collector_core::engine::CollectorEngine;
use system_info_collector_core::enums::{DataType, SimpleDataCollectionMode};
use system_info_collector_core::workers::sysinfo_worker::bytes_to_mb;

use crate::cli::{Commands, parse_cli};
use crate::converting::ploty_creator::load_results_and_save_plot;
use crate::serving::data_buffer::{DataBuffer, DataPoint, SystemInfo, SystemMetadata};
use crate::settings::{build_collect_settings, build_convert_settings};

mod cli;
mod converting;
mod serving;
mod settings;

#[tokio::main]
async fn main() {
    let _ = TermLogger::init(ConfigBuilder::default().build(), TerminalMode::Mixed, ColorChoice::Auto);

    print_version_mode();

    let args = parse_cli();

    match args.command {
        Commands::Collect(collect_args) => {
            // Handle --list-* flags before doing anything else.
            if collect_args.list_disks {
                list_real_disks();
                return;
            }
            if collect_args.list_networks {
                list_real_interfaces();
                return;
            }

            let settings = std::sync::Arc::new(build_collect_settings(collect_args));
            let convert_settings = settings.convert.clone();
            let convert_after = settings.convert_after;

            // Always collect and log system hardware info.
            let mut meta_sys = System::new();
            meta_sys.refresh_memory();
            meta_sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());

            let cpu_logical_cores = meta_sys.cpus().len();
            let cpu_physical_cores = sysinfo::System::physical_core_count().unwrap_or(0);
            let cpu_model = meta_sys.cpus().first().map(|c| c.brand().to_string()).unwrap_or_default();
            let total_memory_mb = bytes_to_mb(meta_sys.total_memory());
            let total_swap_mb = bytes_to_mb(meta_sys.total_swap());

            info!("CPU: {cpu_model}, {cpu_physical_cores} physical cores / {cpu_logical_cores} threads");
            info!("Memory: {total_memory_mb:.0} MB total RAM, {total_swap_mb:.0} MB swap");

            // Create the engine — this runs hardware discovery exactly once.
            let engine = CollectorEngine::new(std::sync::Arc::clone(&settings));

            let gpu_names: Vec<String> = engine.discovery().gpus.iter().map(|g| g.display_name().to_string()).collect();
            if gpu_names.is_empty() {
                info!("GPU: none detected");
            }

            let shutdown = engine.shutdown_handle();

            // Build the HTTP data buffer before starting the engine so we can
            // pass a clone to the on_row callback.
            let data_buffer: Option<DataBuffer> = if settings.serve {
                let buffer = DataBuffer::new(settings.max_results);

                let interfaces = engine.discovery().interfaces.clone();
                let disks = engine.discovery().disks.clone();

                // Build column headers that match the expanded per-GPU / per-interface
                // format produced by file_writer, so the JS chart detector can find them.
                let mut column_headers = vec!["Timestamp".to_string()];
                for mode in &settings.collection_mode {
                    match mode {
                        SimpleDataCollectionMode::GPU_UTILIZATION => {
                            for (idx, name) in gpu_names.iter().enumerate() {
                                column_headers.push(DataType::GPU_N_UTIL((idx, name.clone())).column_name());
                            }
                        }
                        SimpleDataCollectionMode::GPU_MEMORY_USED => {
                            for (idx, name) in gpu_names.iter().enumerate() {
                                column_headers.push(DataType::GPU_N_VRAM_MB((idx, name.clone())).column_name());
                            }
                        }
                        SimpleDataCollectionMode::GPU_TEMPERATURE => {
                            for (idx, name) in gpu_names.iter().enumerate() {
                                column_headers.push(DataType::GPU_N_TEMP_C((idx, name.clone())).column_name());
                            }
                        }
                        SimpleDataCollectionMode::NETWORK_RX_BYTES_PER_SEC => {
                            for iface in &interfaces {
                                column_headers.push(DataType::NET_N_RX_BPS((iface.iface_index, iface.name.clone())).column_name());
                            }
                        }
                        SimpleDataCollectionMode::NETWORK_TX_BYTES_PER_SEC => {
                            for iface in &interfaces {
                                column_headers.push(DataType::NET_N_TX_BPS((iface.iface_index, iface.name.clone())).column_name());
                            }
                        }
                        SimpleDataCollectionMode::DISK_USED => {
                            for disk in &disks {
                                column_headers.push(DataType::DISK_N_USED_GB((disk.disk_index, disk.mount_point.clone())).column_name());
                            }
                        }
                        SimpleDataCollectionMode::DISK_AVAILABLE => {
                            for disk in &disks {
                                column_headers.push(DataType::DISK_N_AVAIL_GB((disk.disk_index, disk.mount_point.clone())).column_name());
                            }
                        }
                        other => column_headers.push(other.to_string()),
                    }
                }
                for p in &settings.process_cmd_to_search {
                    column_headers.push(format!("{} CPU", p.graph_name));
                    column_headers.push(format!("{} Memory", p.graph_name));
                }

                let metadata = SystemMetadata {
                    system_info: SystemInfo {
                        total_memory_mb,
                        total_swap_mb,
                        cpu_cores: cpu_logical_cores,
                        cpu_physical_cores,
                        cpu_model: cpu_model.clone(),
                        gpu_names: gpu_names.clone(),
                        start_time: settings.start_time,
                        app_version: env!("CARGO_PKG_VERSION").to_string(),
                    },
                    column_headers,
                    max_buffer_size: settings.max_results,
                };
                buffer.set_metadata(metadata);

                // Start the HTTP server in its own OS thread with an independent
                // Tokio runtime so it never blocks data collection.
                let server_buffer = buffer.clone();
                let port = settings.port;
                std::thread::spawn(move || {
                    info!("Starting HTTP server thread on port {port}");
                    let runtime = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime for server");
                    runtime.block_on(async move {
                        if let Err(e) = crate::serving::server::start_server(port, server_buffer).await {
                            error!("Server error: {e}");
                        }
                    });
                });

                Some(buffer)
            } else {
                None
            };
            // Build top-N live callback (only when serve is enabled).
            let on_top_row: Option<std::sync::Arc<dyn Fn(f64, Vec<(String, f32)>, Vec<(String, f64)>) + Send + Sync>> =
                if let Some(ref buf) = data_buffer {
                    let top_buf = buf.clone();
                    Some(std::sync::Arc::new(move |ts, cpu, ram| {
                        top_buf.add_top_point(ts, cpu, ram);
                    }))
                } else {
                    None
                };

            let _ = (cpu_model, gpu_names); // suppress unused warnings when !serve

            // Register Ctrl-C handler: first press → graceful stop, second → immediate exit.
            let shutdown_for_ctrlc = shutdown.clone();
            let mut ctrlc_count = 0u32;
            ctrlc::set_handler(move || {
                ctrlc_count += 1;
                if ctrlc_count == 1 {
                    info!("Trying to close app cleanly, press Ctrl-C again to force quit");
                    shutdown_for_ctrlc.store(true, Ordering::Relaxed);
                } else {
                    info!("Forcing quit");
                    process::exit(1);
                }
            })
            .expect("Error setting Ctrl-C handler");

            if let Err(e) = engine
                .run(
                    env!("CARGO_PKG_VERSION"),
                    move |row| {
                        if let Some(ref buf) = data_buffer {
                            buf.add_data_point(DataPoint::from_row(&row));
                        }
                    },
                    on_top_row,
                )
                .await
            {
                error!("{e}");
                process::exit(1);
            }

            if convert_after && let Err(e) = load_results_and_save_plot(&convert_settings) {
                error!("{e}");
                process::exit(1);
            }
        }

        Commands::Convert(convert_args) => {
            let settings = build_convert_settings(convert_args);
            if let Err(e) = load_results_and_save_plot(&settings) {
                error!("{e}");
                process::exit(1);
            }
        }
    }

    info!("Closing app successfully");
}

// This is unused depending on build features
#[allow(clippy::allow_attributes)]
#[allow(unused_mut)]
pub fn print_version_mode() {
    let debug_release = if cfg!(debug_assertions) { "debug" } else { "release" };
    let processors = available_parallelism().map(std::num::NonZero::get).unwrap_or(1);
    let info = os_info::get();

    let mut app_cpu_version = "Baseline";
    let mut os_cpu_version = "Baseline";
    if cfg!(target_feature = "sse2") {
        app_cpu_version = "x86-64-v1 (SSE2)";
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("sse2") {
        os_cpu_version = "x86-64-v1 (SSE2)";
    }
    if cfg!(target_feature = "popcnt") {
        app_cpu_version = "x86-64-v2 (SSE4.2 + POPCNT)";
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("popcnt") {
        os_cpu_version = "x86-64-v2 (SSE4.2 + POPCNT)";
    }
    if cfg!(target_feature = "avx2") {
        app_cpu_version = "x86-64-v3 (AVX2)";
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("avx2") {
        os_cpu_version = "x86-64-v3 (AVX2)";
    }
    if cfg!(target_feature = "avx512f") {
        app_cpu_version = "x86-64-v4 (AVX-512)";
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if is_x86_feature_detected!("avx512f") {
        os_cpu_version = "x86-64-v4 (AVX-512)";
    }

    info!(
        "System Info Collector, version: {}, {debug_release} mode, os {} {} ({} {}), {processors} cpu/threads, app cpu: {app_cpu_version}, os cpu: {os_cpu_version}",
        env!("CARGO_PKG_VERSION"),
        info.os_type(),
        info.version(),
        env::consts::ARCH,
        info.bitness(),
    );
    info!("Process ID is {}", process::id());

    if cfg!(debug_assertions) {
        warn!("You are running debug version of app which is a lot of slower than release version.");
    }
}
