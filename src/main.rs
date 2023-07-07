mod collector;
mod csv_file_loader;
mod model;
mod ploty_creator;

use crate::collector::save_system_info_to_file;
use crate::csv_file_loader::load_csv_results;
use crate::ploty_creator::save_plot_into_file;
use crossbeam_channel::{unbounded, Sender};
use handsome_logger::{ColorChoice, Config, TermLogger, TerminalMode};
use log::{error, info};
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::process;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime};
use sysinfo::{System, SystemExt};
use tokio::time::interval;

const CSV_FILE_NAME: &str = "data.csv";
const RESULTS_HTML: &str = "results.html";

#[tokio::main]
async fn main() {
    TermLogger::init(Config::default(), TerminalMode::Mixed, ColorChoice::Auto).unwrap();

    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.refresh_cpu();
    // sys.refresh_processes();

    let csv_file = OpenOptions::new().write(true).create(true).truncate(true).open(CSV_FILE_NAME).unwrap();
    let mut csv_file = BufWriter::new(csv_file);
    let string_header = "unix_timestamp,memory_used,memory_available,memory_free,memory_total,cpu_total,cpu_usage_per_core".to_string();
    writeln!(csv_file, "{string_header}").unwrap();

    let html_file = OpenOptions::new().write(true).create(true).truncate(true).open(RESULTS_HTML).unwrap();
    let _html_file = BufWriter::new(html_file);

    let time_interval_miliseconds = 1000;
    let mut interv = interval(Duration::from_millis(time_interval_miliseconds));
    interv.tick().await; // This will instantly finish, so next time will take required amount of seconds

    let (ctx, crx) = unbounded::<()>();
    set_ctrl_c_handler(ctx);

    info!("Started collecting data...");
    loop {
        save_system_info_to_file(&mut sys, &mut csv_file);

        if crx.try_recv().is_ok() {
            info!("Trying to create html file...");
            let time_start = SystemTime::now();
            // Both save csv file and then load it from disk, to test if it works
            // For now it is not necessary to have the best performance
            drop(csv_file);
            let loaded_results = match load_csv_results() {
                Ok(loaded_results) => loaded_results,
                Err(e) => {
                    error!("{e}");
                    process::exit(1);
                }
            };
            info!("Loading data from csv took {:?}", time_start.elapsed().unwrap());
            let time_start = SystemTime::now();
            if loaded_results.memory_total.is_empty() {
                info!("There is nothing to save");
            } else {
                match save_plot_into_file(&loaded_results) {
                    Ok(file) => {
                        info!("Creating plot took {:?}", time_start.elapsed().unwrap());
                        info!("Opening file {file}");
                        if let Err(e) = open::that(&file) {
                            error!("Failed to open {file}, reason - {e}");
                            process::exit(1);
                        }
                    }
                    Err(e) => {
                        error!("{e}");
                        process::exit(1);
                    }
                }
                if let Err(e) = save_plot_into_file(&loaded_results) {
                    error!("{e}");
                    process::exit(1);
                }
            }
            process::exit(0);
        }

        interv.tick().await;
    }
}

pub fn set_ctrl_c_handler(ctx: Sender<()>) {
    let current_ctrl_c = AtomicU32::new(1);
    ctrlc::set_handler(move || {
        ctx.send(()).expect("Could not send signal on channel.");
        if current_ctrl_c.fetch_sub(1, Ordering::SeqCst) == 0 {
            info!("Closing app");
            process::exit(1);
        } else {
            info!("Trying to save results, if you don't want to save results, press Ctrl-C one more time",);
        }
    })
    .expect("Error when setting Ctrl-C handler");
}
