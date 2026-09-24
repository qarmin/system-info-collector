# System info collector

A small, fast tool that records how a computer is doing - CPU, memory, swap, network, disks and GPU - and turns that
into charts you can watch live in a browser or open later as a single HTML file.

It writes one CSV file and nothing else. No database, no agent, no account, no server to set up.

<img width="800" alt="The live dashboard showing CPU, GPU and RAM usage" src="https://github.com/user-attachments/assets/336c72af-c046-49e6-bba7-bb4161f3cab9" />

The live dashboard, served by the app itself: CPU, GPU and RAM usage over time, with the machine's hardware and OS
details above the charts. Everything you see is in the binary - no internet access and no external services involved.

## What it does

There are three commands:

| Command | What it is for |
| --- | --- |
| `collect` | Samples the machine every second and appends a row to a CSV file |
| `convert` | Turns that CSV into one self-contained HTML report |
| `session` | Records **every** process for a minute or two, to find out what is loading the machine *right now* |

Add `-s` to `collect` and you also get a live dashboard in your browser at `http://localhost:5998`.

## Install

### Linux - as a background service (the easiest way)

One command builds the app, installs it, creates a systemd service that starts on boot, and starts it:

```
git clone https://github.com/qarmin/system-info-collector
cd system-info-collector
just full_install
```

From then on the machine is recorded continuously into `~/data_collector/data.csv` and the live dashboard is at
`http://localhost:5998`. Nothing else to run.

You need [`just`](https://github.com/casey/just), `cargo-zigbuild` with [`zig`](https://ziglang.org/download/) (the
build targets an older glibc so the same binary also runs on older distros), systemd, and permission to `sudo`:

```
cargo install just cargo-zigbuild
```

To pick what is collected, pass the metrics and the disks (both space-separated, `all` means every real disk):

```
just full_install "cpu-usage-total memory-used memory-available" "/ /home"
```

To remove it again: `sudo systemctl disable --now system-info-collector`.

Installing on *another* Linux machine works the same way over ssh - `just full_send 192.168.1.50` for a normal x86_64
machine, `just full_send_arm 192.168.1.60` for an ARM device reachable as root.

### Just the binary

```
cargo install system_info_collector
```

or, from a checkout, `just install`. Precompiled binaries for Linux, macOS and Windows are also built by CI - they are
attached as artifacts to every [Actions run](https://github.com/qarmin/system-info-collector/actions).

## Quick start

Record CPU and memory once a second, into `system_data.csv`:

```
./system_info_collector collect
```

Press `Ctrl+C` to stop. Then turn the file into a chart and open it:

```
./system_info_collector convert -d system_data.csv -p plot.html -o
```

Or do both at once - collect until `Ctrl+C`, then build the report and open it:

```
./system_info_collector collect --convert-after -o
```

Watch it live instead, in a browser at `http://localhost:5998`:

```
./system_info_collector collect -s
```

Find out what is loading the machine right now - records every process for 60 seconds, then opens the result:

```
./system_info_collector session --duration 60 --open
```

`./system_info_collector --help` lists everything.

## What it can collect

Pick metrics with `-m`. The default is `cpu-usage-total memory-used`.

| Metric | Unit | Notes |
| --- | --- | --- |
| `cpu-usage-total` | % | 0-100% regardless of core count |
| `cpu-usage-per-core` | % | One line per core. The most expensive metric to collect and to plot |
| `memory-used`, `memory-available`, `memory-free` | MiB | Prefer `memory-available` - on Linux `memory-free` ignores the page cache and reads far too low |
| `swap-used`, `swap-free` | MiB | |
| `network-rx`, `network-tx` | MiB/s | Needs `--network <iface>` or `--all-networks` |
| `network-total` | MiB | Cumulative bytes since the interface came up |
| `disk-used`, `disk-available` | GiB | Needs `--disk <mount>` or `--all-disks` |
| `disk-busy` | % | iostat's `%util`. Linux only. On NVMe it reaches 100% long before the drive is saturated - read it next to the throughput columns |
| `disk-read`, `disk-write` | MiB/s | Linux only |
| `gpu-utilization`, `gpu-memory-used`, `gpu-temperature` | %, MiB, °C | NVIDIA via NVML, AMD and Intel via sysfs. Multiple GPUs supported |

A named process can be tracked too - its CPU and memory land in their own columns:

```
./system_info_collector collect -e "FIREFOX|firefox" -e "Event Handler|/usr/bin/event_handler"
```

The part before `|` is the label used in charts, the part after is matched against the command line. Only the first
matching process is tracked; `-1` in the chart means it was not running at that moment.

### Choosing disks and interfaces

`--all-disks` and `--all-networks` pick everything real, skipping virtual filesystems, boot partitions, loopback and
the usual container/VM interfaces. Drop anything else with `--exclude-disk` / `--exclude-network`. To see what is
available before deciding:

```
./system_info_collector collect --list-disks
./system_info_collector collect --list-networks
```

Charts and tables label each one with what it actually is - `/home (nvme1n1 915 GB)`, `wlan0 (WiFi - Wi-Fi 6 AX201)` -
not `DISK_0` and `NET_0`.

## Live dashboard

Run `collect` with `-s` (optionally `--port`):

```
./system_info_collector collect -s --port 5998
```

Then open `http://localhost:5998`. It shows:

- Charts for CPU (total and per core), RAM, swap, network, disks and GPUs
- Hardware and OS details, and every attached filesystem with its size, type and drive model
- A time range from 30 seconds up to a full day
- Live updates pushed over a websocket - nothing is polled or re-downloaded
- Hidden series, chosen time range and other view settings that survive a page reload
- An optional raw data table, dark mode, and no external dependencies - everything is embedded in the binary

https://github.com/user-attachments/assets/d21faa76-f42c-4b0e-8ea6-eb3a6672a3cb

The dashboard updating live, and then **Record processes** - a five second recording of every process running on the
machine, opened in the session viewer as soon as it finishes. See [Process sessions](#process-sessions) below.

### Export

The **Export** button builds a file on the server and downloads it:

- **Plotly report** - the same HTML report `convert` produces. Built from the CSV on disk, so it covers the whole
  recorded history, and can be split into one file per day or per ISO week inside a `.zip`
- **Dashboard snapshot** - a copy of the live page with the data baked in

You can export everything recorded, a relative window (15 minutes … 7 days), or one specific day or week. Older data
files from previous runs are offered as well, each with the period it covers and its size. Very long periods are
thinned to at most 100 000 points, spread evenly.

### Memory use

The live view keeps recent samples in memory - 24 hours by default, set with `--buffer-seconds`. That costs roughly
0.6-0.8 kB per sample, so a full day at one sample per second is in the 30-70 MB range. Shorten it if that matters.
Exports are unaffected: they read the CSV file.

> The server is unauthenticated and the session endpoints can start a recording, which exposes process command lines
> and file paths. Bind it somewhere you trust.

## Process sessions

`collect` answers "how did this machine behave over the last few hours". A session answers a different question -
"what is loading it *right now*" - by recording every process for a bounded period.

```
./system_info_collector session --duration 60 --open
```

That records for 60 seconds, writes `sessions/session_<timestamp>.json`, and opens a self-contained HTML viewer next to
it. One file, no server needed. `Ctrl-C` stops early and keeps what was collected. With `--serve` running there is a
**Record processes** button on the dashboard that does the same thing, and `/session` lists every earlier recording.

For each process the viewer shows CPU, memory, how much it read and wrote (both through syscalls and at the actual
device), thread count, status, command line and the files it had open while writing. Drag across a chart to select a
period and the table recomputes for exactly that range; mark a quiet period as the baseline and the table switches to
differences, which cancels out steady background load. There are also a process tree and a lifetimes view, so a process
respawned on a timer shows up as a regular comb of short bars.

### Run it with privilege, or most of the I/O is invisible

Reading another process's I/O counters needs permission. As an ordinary user you get your own processes and nothing
else - no daemons, no kernel threads - and those rows are tagged `no I/O data` in the viewer, because zero there means
*unknown*, not *idle*.

The simplest fix is to grant the binary the two capabilities it needs, once:

```
sudo setcap cap_dac_read_search,cap_sys_ptrace+ep ./system_info_collector
```

Or just run it as root. A service installed with `just full_install` already has them - the systemd unit asks for them
directly, so recordings started from the web UI see everything.

## Data files

The CSV has one metadata line, one header line, then one row per sample:

```
INTERVAL_SECONDS=1,CPU_CORE_COUNT=24,MEMORY_TOTAL=30676.21,SWAP_TOTAL=8192.00,UNIX_TIMESTAMP_START_TIME=1790274288.04,APP_VERSION=0.8.0,CUSTOM_0=FIREFOX,NET_0=eth0,NET_LABEL_0=eth0 (Ethernet - 1000 Mb/s),CPU_MODEL=AMD Ryzen 9 7900,DISK_0=/home,DISK_LABEL_0=/home (nvme1n1 915 GB)
SECONDS_SINCE_START,CPU_USAGE_TOTAL,MEMORY_USED,NET_0_RX_BPS,NET_0_TX_BPS,DISK_0_USED_GB,CUSTOM_0_CPU,CUSTOM_0_MEMORY
5,12.4,10152.5,0.4,0.1,681,2.1,1111.3
6,9.8,10178.7,,,,,
7,,10172.1,0.2,,,3.4,
```

Empty means "same as the row above" - that is compact mode, on by default, and the reader fills the values back in.
Turn it off with `--no-compact` if you want to process the file with something else. A `-1` means the value could not
be measured, which for a tracked process means it was not running.

Nothing is ever overwritten. When a run starts, an existing data file is kept as a backup (`--backup-number`, 5 by
default), and when a file grows past `--maximum-data-file-size-mb` (200 MB) it is rotated aside and a new one started.
All of them stay available for export from the web interface.

CSV files are not guaranteed to be readable across versions - the version that wrote a file is recorded in its first
line. Differences are usually easy to fix by hand.

## Running on boot

On Linux, `just full_install` (above) does all of this for you. To do it by hand, copy the binary to `/usr/bin`, create
`/etc/systemd/system/system-info-collector.service`:

```
[Unit]
Description=System Info Collector

[Service]
ExecStart=/usr/bin/system_info_collector collect -d /opt/system_info_collector/data.csv

[Install]
WantedBy=multi-user.target
```

and enable it:

```
sudo systemctl daemon-reload
sudo systemctl enable --now system-info-collector
sudo systemctl status system-info-collector
```

`system-info-collector.service` in this repository is the fuller version the `just` recipes use.

## OS support

Linux is fully supported and gets the most detail, because several things are read straight from `/proc`. macOS works
too, except for disk activity and the `/proc`-based parts of process sessions. On Windows per-process CPU and memory
are not collected; everything else works.

## Resource use

Measured on an i7-4770: a steady 15-20 MB of RAM and below 0.1% CPU while collecting. Per-core CPU tracking and
process sessions cost noticeably more. Building the HTML report is the expensive part, so do it on a capable machine.

## Details

<details>
<summary>HTTP API</summary>

| Endpoint | Description |
| --- | --- |
| `GET /api/metadata` | hardware info, column headers, buffer size, check interval |
| `GET /api/snapshot?seconds=<n>&limit=<n>` | history for the requested window plus buffer counters |
| `GET /api/ws` | websocket upgrade, see below |
| `GET /api/export/report?mode=full\|last\|day\|week&seconds=&date=&week=&split=day\|week&source=` | plotly report download; `split` returns a zip of per-period files, `source` selects the data file (`all` for every file) |
| `GET /api/export/html?mode=…` | dashboard snapshot download |
| `GET /api/export/json?mode=…` | live buffer as JSON |
| `GET /api/export/sources` | data files available for export, with the period, point count and size of each |
| `GET /session` | the process session viewer |
| `POST /api/session/start` | body `{"seconds": 60, "hz": 4}` - starts a recording; `409` if one is already running, `400` above the 4 Hz maximum |
| `POST /api/session/stop` | stops the running recording and keeps what it collected; `404` if none is running |
| `GET /api/session/status` | whether a recording is active, elapsed and remaining seconds, samples taken, file name once finished |
| `GET /api/session/list` | saved recordings, newest first |
| `GET /api/session/file/<name>` | one recording as JSON; add `?download=1` to save it |
| `GET /api/session/export/<name>` | one recording as a self-contained HTML viewer |

</details>

<details>
<summary>How live updates are sent</summary>

`/api/ws` is requested once to upgrade the connection; after that the server pushes on its own and the client never
asks for anything again. One frame per collection tick:

```json
{"timestamp":4.1,"data":["4.1","100","15027.1"]}
```

The frame is built and serialized once per tick, not once per client, so extra viewers are only a socket write each.
When nobody is connected, nothing is serialized at all. Measured on a 12-thread machine at `-c 0.1` with per-core CPU
tracking: 0.24% CPU with nobody watching, 0.40% with one browser, 1.36% with sixteen. A client that falls more than 64
frames behind skips to the newest data instead of replaying the backlog.

</details>

<details>
<summary>Session sampling rate, and what a recording cannot see</summary>

The maximum rate is 4 Hz, and a higher `--hz` is rejected rather than accepted. `sysinfo` will not re-read `/proc/stat`
more often than every 200 ms while still refreshing each process's counters, so a faster rate would divide per-process
CPU by a stale denominator - numbers that stay plausible while being wrong. Sampling exactly at that floor is not safe
either: the check is against the wall clock, so a 5 Hz tick clears it only about half the time.

A process living less than one sampling interval is never observed directly. It is usually still visible, because Linux
folds a reaped child's I/O into its parent - the bytes surface against whatever spawned it. Where the child *was*
sampled, its contribution is reported in a separate "Write by children" column; where it was not, the parent's total
unavoidably includes it, and the row is tagged with how many children it started.

Syscall-level I/O, open files and kernel threads come from `/proc`, so on Windows and macOS a recording falls back to
device-level bytes only, and the viewer says so.

</details>

<details>
<summary>Why this exists</summary>

I needed a simple, fast way to see how much RAM and CPU were being used on a slow (4x1 GHz) 32-bit ARM machine running
a custom Yocto Linux build. Grafana and friends were either too heavy or wanted a client/server setup I would rather
avoid.

</details>

## Contributing and development

`just --groups` lists the recipe groups, `just --list` shows them all:

| Group | What is in it |
| --- | --- |
| `build` | compiling for this machine and for other targets |
| `dev` | formatting, lints, dependency updates |
| `release` | `cargo install` here, `cargo publish` to crates.io |
| `run` | running the collector locally with various metric sets |
| `session` | recording every process for a short period |
| `plots` | turning collected CSVs into HTML reports |
| `deploy` | installing the systemd service here or on another machine |

## License

MIT License

Copyright (c) 2023-2026 Rafał Mikrut and contributors

> **Note:** this project was developed with AI assistance (Claude Code).
