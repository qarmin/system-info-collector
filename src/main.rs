use std::process;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use crossbeam_channel::Sender;
use handsome_logger::{ColorChoice, ConfigBuilder, TermLogger, TerminalMode};
use log::{error, info};
use sysinfo::System;

use crate::cli::parse_cli;
use crate::collector::collect_data;
use crate::enums::AppMode;
use crate::model::Settings;

mod cli;
mod collector;
mod csv_file_loader;
mod enums;
mod model;
mod ploty_creator;

#[tokio::main]
async fn main() {
    let cli_model = parse_cli();
    let settings: Settings = cli_model.into();

    let config = ConfigBuilder::new().set_level(settings.log_level.into()).build();
    TermLogger::init(config, TerminalMode::Mixed, ColorChoice::Auto).unwrap();

    if [AppMode::COLLECT, AppMode::COLLECT_AND_CONVERT].contains(&settings.app_mode) {
        let creating_start_time = SystemTime::now();
        let mut sys = System::new_all();
        let creating_duration = creating_start_time.elapsed().unwrap();
        let refresh_start_time = SystemTime::now();
        sys.refresh_memory();
        sys.refresh_cpu();
        if settings.need_to_refresh_processes {
            sys.refresh_processes();
        }
        info!(
            "Initial refresh took {:?} (creating sys struct took {:?})",
            refresh_start_time.elapsed().unwrap(),
            creating_duration
        );

        if let Err(e) = collect_data(&mut sys, &settings).await {
            error!("{e}");
            process::exit(1);
        };
    } else {
        // Only convert
        if let Err(e) = ploty_creator::load_results_and_save_plot(&settings) {
            error!("{e}");
            process::exit(1);
        };
    }
    info!("Closing app successfully");
}

pub fn set_ctrl_c_handler(ctx: Sender<()>) {
    let current_ctrl_c = AtomicU32::new(1);
    ctrlc::set_handler(move || {
        ctx.send(()).expect("Could not send signal on channel.");
        if current_ctrl_c.fetch_sub(1, Ordering::SeqCst) == 0 {
            info!("Closing app due clicking Ctrl-C multiple times");
            process::exit(1);
        } else {
            info!("Trying to close app cleanly, if you don't want to wait, click Ctrl-C again");
        }
    })
    .expect("Error when setting Ctrl-C handler");
}
