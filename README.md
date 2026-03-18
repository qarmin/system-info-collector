# System info collector

This is simple app to collect data about system cpu, memory, swap, network, GPU and disk usage over time.

After collecting results into csv file, html file can be created with plot.


<img width="800" alt="Screenshot From 2026-03-18 07-31-03" src="https://github.com/user-attachments/assets/58edba97-fa62-4615-bb7d-f8e77aac3fc3" />

<img width="800" alt="Screenshot From 2026-03-18 07-30-43" src="https://github.com/user-attachments/assets/2bce0c34-b82d-40ab-9fab-60697e0167c4" />

## Why?

I needed a simple and fast application to collect basic information about the amount of RAM used and CPU consumption on
a slow(4x1Ghz) 32 bit ARM computer which uses custom Linux OS build with Yocto.

I looked at a few applications like grafana, but they are too heavy or work in a client server architecture
which in this case I would prefer to avoid.

## How to use it?
You need to install it via cargo or download precompiled binary from releases and then run it in terminal.
```

```


This is console app, so that means that you need to use terminal to use it.

```
./system_info_collector collect --convert-after -o
```

should once per second print debug message about refreshed CPU and memory usage.

After a while you can click `Ctrl+C` once to stop collecting data and wait for automatic preparing and opening prepared
html plot data.

https://github.com/qarmin/system-info-collector/assets/41945903/7ac510b5-babf-4d04-9624-34d83b8f1866

## Performance and memory usage

During testing on i7-4770, app used stable 15-20MB Ram and most of the time, cpu usage was lower than 0.1%.

Sys-info library which I use have quite big overhead(usually few ms) when finding cpu/ram usage for processes due
opening unnecessary files, so I plan to do some computations manually. So if you want to use as little resources as
possible, you should use only collect basic os info without any processes(this is default mode).

In collect mode, app only needs to read cpu/ram usage and then save it to file, so that is why it uses so little
resources.

Converting csv file to html file is more resource intensive, so should be done on more powerful computer.

Example of first lines of csv file (by default compact mode is used: repeated values written as empty, filled back in on read):

```
INTERVAL_SECONDS=1,CPU_CORE_COUNT=8,MEMORY_TOTAL=23943.89,SWAP_TOTAL=2048.00,UNIX_TIMESTAMP_START_TIME=1690142980.2999594,APP_VERSION=0.7.0,CUSTOM_0=FIREFOX,NET_0=enp8s0,GPU_0=NVIDIA GeForce RTX 3080
SECONDS_SINCE_START,MEMORY_USED,CPU_USAGE_TOTAL,CPU_USAGE_PER_CORE,CUSTOM_0_CPU,CUSTOM_0_MEMORY,NET_0_RX_BPS,NET_0_TX_BPS,GPU_0_UTIL,GPU_0_VRAM_MB,GPU_0_TEMP_C
0.24,11031.2,49.7,55.1;62.3;44.2;38.7;51.0;48.9;60.1;42.5,0,1111.3,0,0,23,4012,61
1.24,11037.6,,16.8;12.1;18.3;14.5;20.1;17.8;22.3;15.9,2.1,1111.3,0.5,0.1,,4015,
```

## Example commands
You can look at justfile, for more examples of usage.

Collect used memory and CPU usage in interval of 1 second and save it to system_data.csv file

```
./system_info_collector collect
```

Collect and convert csv data and automatically open html file in browser, additionally will show more detailed logs

```
./system_info_collector collect  --convert-after -o
```

Convert csv data file into html document with plot and open it in browser

```
./system_info_collector convert -d /home/user/data.csv -p /home/user/plot.html -o
```

Collect all basic data with interval of 0.2 seconds

```
./system_info_collector collect --convert-after -o -m memory-used -m memory-free -m memory-available -m cpu-usage-total -m cpu-usage-per-core -c 0.2
```

Collect memory and CPU usage of selected processes - will try to find process with command containing `firefox` in
name - `FIREFOX` name will be used later in plot.

App can only track 1 process with certain name at once, so if two or more processes contain `firefox` in name, only
info about the first will be collected

```
./system_info_collector collect -e "FIREFOX|firefox" -e "Event Handler|/usr/bin/event_handler --timeout"
```

Track the top 10 most CPU-hungry and RAM-hungry processes (grouped by executable path, only those using >1% CPU are logged).
This writes two extra files: `system_data_top_cpu.csv` and `system_data_top_ram.csv`.
Note: this option is very resource-intensive as it refreshes all processes on every tick.

```
./system_info_collector collect --top-n-processes 10
```

Convert main data file together with top-N process files into a single HTML plot

```
./system_info_collector convert -d system_data.csv -d system_data_top_cpu.csv -d system_data_top_ram.csv -o
```

Collect network RX/TX for a specific interface

```
./system_info_collector collect -m network-rx -m network-tx --network enp8s0
```

Collect network stats for all non-virtual interfaces

```
./system_info_collector collect -m network-rx -m network-tx --all-networks
```

List all available network interfaces and exit

```
./system_info_collector collect --list-networks
```

Collect disk used/available space for specific mount points

```
./system_info_collector collect -m disk-used -m disk-available --disk / --disk /home
```

Collect disk stats for all non-virtual disks

```
./system_info_collector collect -m disk-used --all-disks
```

List all available disks and exit

```
./system_info_collector collect --list-disks
```

Collect GPU utilization, VRAM usage and temperature (NVIDIA via NVML, AMD/Intel via sysfs)

```
./system_info_collector collect -m gpu-utilization -m gpu-memory-used -m gpu-temperature
```

Start live HTTP server on port 5998 (open `http://localhost:5998/` in browser)

```
./system_info_collector collect --serve --port 5998 --max-results 1000
```

Shows help about available arguments

```
./system_info_collector --help
```

## Running app when OS starts (Linux)

Simple way to collect OS data from start is to create a simple systemd service.

To do this, copy app into `/usr/bin` folder and create folder for collected data

```
sudo cp system_info_collector /usr/bin/system_info_collector
sudo mkdir -p /opt/system_info_collector/ # To collect reports
```

creating service content

```
sudo touch /etc/systemd/system/system-info-collector.service
sudo gedit /etc/systemd/system/system-info-collector.service # open it with any text editor - I used gedit
```

paste this code with modified arguments:

```
[Unit]
Description=System Data Collector

[Service]
ExecStart=/usr/bin/system_info_collector collect -d /opt/system_info_collector/data.csv

[Install]
WantedBy=default.target
```

now start service

```
sudo systemctl daemon-reload
sudo systemctl start system-info-collector
sudo systemctl status system-info-collector # This should print "active (running)" if everything works fine, if there is failure, check log to see what happened
sudo systemctl enable system-info-collector # To enable running service when OS starts
```

now you can convert collected data with simple command

```
system_info_collector convert -d /opt/system_info_collector/data.csv -p /tmp/plot.html -o
```

## CPU/Memory/Swap results

Cpu usage is shown in range between 0 and 100%, if computer has more than 1 core, cpu usage is divided by number of
cores, to get value in proper range.

`cpu-usage-per-core` stores all core values semicolon-separated in a single CSV column. The HTML plot and the live
web interface both expand it into one line per core automatically.

Memory and swap usage are shown in MiB, with range from 0 to total memory/swap size.

When checking for processes -1 is visible both in cpu/memory plot if searched process is not found.

## Top-N processes

When `--top-n-processes N` is used, two extra CSV files are written alongside the main data file:

- `system_data_top_cpu.csv` — top N processes by CPU%, sorted descending
- `system_data_top_ram.csv` — top N processes by RAM usage, sorted descending

Processes are grouped by their executable path (not the OS process name), so multiple instances of the same binary
(e.g. Firefox content processes) are aggregated into a single entry. Only processes using more than 1% of total CPU
are included in the CPU ranking. Kernel and userland threads are excluded.

Pass both extra files to `convert` with `-d` to include them as additional charts in the HTML plot.

## Network monitoring

Network RX/TX rates are collected in MB/s. Use `--network <iface>` to track specific interfaces or `--all-networks`
to track all non-virtual ones. Run with `--list-networks` to see available interfaces.

## Disk monitoring

Disk used/available space is collected in GB. Use `--disk <mount>` to track specific mount points (e.g. `/`, `/home`)
or `--all-disks` to track all non-virtual disks. Run with `--list-disks` to see available disks.

## GPU monitoring

GPU utilization (%), VRAM usage (MB) and temperature (°C) are collected automatically when any of the GPU modes are
selected. NVIDIA GPUs are accessed via NVML; AMD and Intel GPUs are accessed via sysfs. Multiple GPUs are supported,
each appearing as a separate column.

## Data file compatibility

Compatibility between different versions of app is not guaranteed, so if you want to collect or create graphs from csv
file, be sure that you use the same version of app (csv file contains inside info which version of app was used).

Usually incompatibilities are quite easy to workaround by manually adding/removing records from csv file.

## OS Support

Currently, fully supported is only Linux, due to using manually reading `/proc` files (performance reasons).

App should also fully work on Mac, but on Windows capturing process cpu/memory usage is not supported (except that,
everything should work fine).

## Live HTTP Server

The application has a built-in HTTP server that allows you to view live data in your web browser.

### How to start the server?

Run with the `--serve` option (optionally with `--port` and `--max-results`):

```
./system_info_collector collect --serve --port 5998 --max-results 1000
```

After starting, open your browser and go to `http://localhost:5998/`.

### Web interface features
- **CPU chart** – total CPU usage and/or per-core breakdown (one line per core)
- **RAM chart**
- **GPU charts** – utilization, VRAM and temperature (one chart each, one line per GPU)
- **Data table** – with text information about current metrics
- **Auto-refresh** – ability to set the refresh interval or leave it to manual refresh
- **Dark mode**
- **No external dependencies** – static files (e.g. Chart.js) are embedded in the binary and served by the backend

## License

MIT License

Copyright (c) 2023-2026 Rafał Mikrut and contributors