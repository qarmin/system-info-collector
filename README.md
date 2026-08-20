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

During testing on i7-4770, app used stable 15-20MB Ram and most of the time, cpu usage was lower than 0.1% - but it may increase when using more resource intensive options like tracking top N processes.

Running with `--serve` adds the live buffer on top of that; see [The live buffer](#the-live-buffer).

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

Collect disk activity (iostat-style busy% and read/write throughput) for all disks

```
./system_info_collector collect -m disk-busy -m disk-read -m disk-write --all-disks
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
./system_info_collector collect --serve --port 5998 --buffer-seconds 86400
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
or `--all-disks` to track all non-virtual disks - one entry per device, so bind mounts and subvolumes of the same
disk are not counted several times. Run with `--list-disks` to see available disks.

Disk activity is read from the same counters `iostat` uses (`/proc/diskstats`, Linux only) and is available as three
metrics per tracked disk:

- `disk-busy` — share of wall time the device had at least one request in flight, i.e. iostat's `%util`
- `disk-read` / `disk-write` — throughput in MB/s

They are averaged over `--disk-io-interval` (1 s by default) rather than the main `--check-interval`. On NVMe drives,
which serve many requests in parallel, `disk-busy` reaches 100% long before the drive is actually saturated - read it
together with the throughput columns.

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

Run with the `--serve` option (optionally with `--port` and `--buffer-seconds`):

```
./system_info_collector collect --serve --port 5998 --buffer-seconds 86400
```

After starting, open your browser and go to `http://localhost:5998/`.

### Web interface features
- **CPU chart** – total CPU usage and/or per-core breakdown (one line per core)
- **RAM chart**
- **GPU charts** – utilization, VRAM and temperature (one chart each, one line per GPU)
- **Swap, network and disk charts**
- **Top-N process charts** – when `--top-n-processes` is used
- **Time range** – pick how much history to show, from 5 minutes up to 1 day
- **Live updates over a websocket** – the full history is downloaded once on page load, after that the server
  pushes each new tick over a connection that stays open; the browser appends it to the existing charts, so
  nothing is polled, re-fetched or re-rendered from scratch
- **Redraw interval** – data keeps arriving at the collection rate, but the charts are only repainted every
  N seconds (1 / 2 / 5 / 10 / 30 / 60, default 5) because repainting is where the browser CPU goes
- **Data table** – hidden behind a checkbox, because rendering thousands of rows is what makes the page
  feel slow; when enabled only the newest 1000 rows are rendered
- **Remembered view settings** – series hidden by clicking a legend entry stay hidden through a reload, a
  time-range change and a page refresh (as do the time range, redraw interval, table and width choices);
  **Show all series** brings every hidden series back
- **Export** – server-side generated report downloaded as a file, from this run or from an older data file
  (see below)
- **Dark mode**
- **No external dependencies** – static files (e.g. Chart.js) are embedded in the binary and served by the backend

### The live buffer

The web view keeps recent samples in memory. Its size is set as a **duration**, not a sample count:

```
./system_info_collector collect --serve --buffer-seconds 86400   # 24 h, the default
```

The number of samples is derived from `--check-interval`, so changing the sampling rate does not require
touching the buffer setting (24 h is 86400 samples at 1 s, 43200 at 2 s). It is capped at 1 000 000 samples;
if the requested duration needs more, a warning at startup says how much is actually kept.

Memory scales with the buffer: roughly 0.6-0.8 kB per sample depending on how many metrics are collected,
so a full 24 h buffer at 1 s is in the 30-70 MB range. Shorten `--buffer-seconds` if that matters.

**This limit only applies to the live view.** Exports read the CSV file, so they are never truncated by it.

### Exporting from the web interface

The **Export** button generates the file on the server and downloads it. Two formats are available:

- **Plotly report** – the same HTML plot the `convert` command produces (including the top-N process charts,
  embedded in the same document). Built from the **CSV file on disk**, so it covers the whole recorded
  history regardless of `--buffer-seconds`
- **Dashboard snapshot** – a self-contained copy of the live page with the data baked in and Chart.js
  inlined. This one is a copy of what the page shows, so it *is* limited to the live buffer of the current run

The **Data file** selector lists every CSV next to the output file, not just the one this run writes:
the backups of earlier runs (`system_data__1.csv` …, see `--backup-number`) and the files created by
size-based rotation. Each entry shows the period it covers, its duration, how many points it holds and how
big it is, so data recorded before a restart stays reachable. `All files` writes one report per file into a
single `.zip`. Only the current file can be exported as a dashboard snapshot, because that format is a copy
of the live page.

The period can be everything recorded, a relative window (15 minutes … 7 days) or one specific day / ISO week;
the offered days and weeks are the ones actually present in the selected file.

A plotly report can additionally be split into **one file per day or per ISO week** (the same grouping as
`convert --split-mode`), delivered as a single `.zip`.

Exports over very long periods are thinned to at most 100 000 points, spread evenly so the report still
covers the whole period instead of just its tail. The reduction is logged.

### HTTP API

| Endpoint | Description |
| --- | --- |
| `GET /api/metadata` | hardware info, column headers, buffer size, check interval |
| `GET /api/snapshot?seconds=<n>&limit=<n>` | history for the requested window plus buffer counters |
| `GET /api/ws` | websocket upgrade - see below |
| `GET /api/export/report?mode=full\|last\|day\|week&seconds=&date=&week=&split=day\|week&source=` | plotly report download, `split` returns a zip of per-period files, `source` selects the data file (`all` for every file, default is the current one) |
| `GET /api/export/html?mode=…` | dashboard snapshot download |
| `GET /api/export/sources` | data files available for export, with the period, point count and size of each |

### Live update protocol

`/api/ws` is requested **once** to upgrade the connection; from then on the connection stays open and the
server pushes on its own — the client never asks for anything again. One frame is sent per collection tick,
containing everything gathered in that tick:

```json
{"timestamp":4.1,"data":["4.1","100","15027.1"],"top":{"cpu":[{"name":"firefox","value":31.2}],"ram":[…]}}
```

`top` is omitted when `--top-n-processes` is not used.

The frame is built and serialized **once per tick**, not once per client: all connected browsers share the
same buffer and sending to each is only a socket write. When no browser is connected nothing is serialized
at all. Measured on a 12-thread machine with `-c 0.1` and per-core CPU tracking: 0.24% CPU with nobody
watching, 0.40% with one browser, 1.36% with sixteen.

A client that cannot keep up is allowed to fall 64 frames behind, after which it skips to the newest data
instead of replaying a backlog.

## License

MIT License

Copyright (c) 2023-2026 Rafał Mikrut and contributors