use std::process;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::SystemTime;

use crossbeam_channel::Sender;
use handsome_logger::{ColorChoice, ConfigBuilder, TermLogger, TerminalMode};
use log::{error, info};
use sysinfo::{System, SystemExt};

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
    let mut settings: Settings = cli_model.into();

    let config = ConfigBuilder::new().set_level(settings.log_level.into()).build();
    TermLogger::init(config, TerminalMode::Mixed, ColorChoice::Auto).unwrap();

    if [AppMode::COLLECT, AppMode::COLLECT_AND_CONVERT].contains(&settings.app_mode) {
        let refresh_start_time = SystemTime::now();
        let mut sys = System::new_all();
        sys.refresh_memory();
        sys.refresh_cpu();
        sys.refresh_processes();
        info!("Initial refresh took {:?}", refresh_start_time.elapsed().unwrap());

        settings.cpu_core_count = sys.cpus().len();
        settings.memory_total = sys.total_memory() as usize;
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
            info!("Trying to save results, if you don't want to save results, press Ctrl-C one more time",);
        }
    })
    .expect("Error when setting Ctrl-C handler");
}
