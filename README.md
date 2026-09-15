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

During testing on i7-4770, app used stable 15-20MB Ram and most of the time, cpu usage was lower than 0.1% - but it may increase when using more resource intensive options like per-process tracking.

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

Find out what is loading the machine right now, across every process

```
./system_info_collector session --duration 60 --open
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

## Column labels

Per-disk and per-interface columns are named `DISK_N_*` / `NET_N_*` in the CSV, but charts, the live view and the raw
data table show what the index actually is - `/home (nvme1n1 915 GB)` and `wlan0 (WiFi - Wi-Fi 6 AX201)`. The live
view also lists every tracked disk and interface in its System Information panel.

The labels are written to the CSV metadata line as `DISK_LABEL_N` / `NET_LABEL_N`, so `convert` reproduces them
later; files written by older versions fall back to the bare mount point and interface name.

## Network monitoring

Network RX/TX rates are collected in MB/s. Use `--network <iface>` to track specific interfaces or `--all-networks`
to track all non-virtual ones. Run with `--list-networks` to see available interfaces.

`--all-networks` already skips loopback and the usual container/virtualisation interfaces by name (`lo`, `docker*`,
`veth*`, `br-*`, `virbr*`, `tun*`, `tap*`, `dummy*`). Anything else - a VPN, a bond, a bridge under a custom name -
can be dropped with `--exclude-network <iface>`.

Each interface is labelled with its connection type, adapter model (from the PCI database, falling back to the kernel
driver name) and negotiated link speed, so charts read `wlan0 (WiFi - Wi-Fi 6 AX201)` rather than `NET_0`. The
detection of type, model and speed is Linux-only (`/sys/class/net`); elsewhere only the interface name is shown.

## Disk monitoring

Disk used/available space is collected in GB. Use `--disk <mount>` to track specific mount points (e.g. `/`, `/home`)
or `--all-disks` to track all non-virtual disks - one entry per device, so bind mounts and subvolumes of the same
disk are not counted several times. Run with `--list-disks` to see available disks and what `--all-disks` would pick.

`--all-disks` leaves out virtual filesystems (tmpfs, overlay, squashfs, …) and boot partitions (`/boot`, `/boot/efi`,
`/efi`) - they are tiny, never change and only add noise. Anything else can be dropped with `--exclude-disk`, by mount
point (which also covers mounts below it) or by device name:

```
./system_info_collector collect -m disk-used --all-disks --exclude-disk /mnt/backup --exclude-disk /dev/sdb1
```

Exclusions only apply to `--all-disks`; an explicitly requested `--disk /boot` is always tracked.

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

## Process sessions

`collect` answers "how did the machine behave over the last few hours" for a chosen set of metrics. A session answers a
different question - "what is loading the machine *right now*" - by recording **every** process for a bounded period.

```
./system_info_collector session --duration 60 --open
```

That records for 60 s, writes `sessions/session_<timestamp>.json`, and opens a self-contained HTML viewer next to it -
one file, no server and no network needed. `Ctrl-C` stops early and keeps whatever was collected.

With `--serve` running there is a **Record processes** button on the dashboard at `http://localhost:5998/`, next to
`Export`. Pick a duration, press Start, and a countdown shows how much is left with a Stop button that keeps whatever
has been collected so far. When it finishes the result downloads as a single HTML file (or opens in the viewer at
`/session`, which also lists every earlier recording). Recording happens in the collector process, so closing the page
or reloading it does not interrupt one that is running.

Recorded per process, per sample: CPU%, RSS, `rchar`/`wchar` (syscall level) and `read_bytes`/`write_bytes` (block device
level), status, thread count, context switches, PID, parent PID, command line, and the exact ticks it was alive for. Each
process also carries its cumulative all-time read/write counters - the figures a task manager shows - and the recording
as a whole carries byte totals for the device and for processes, so writes the disk performed that no process claimed
are stated outright instead of leaving you to work out that the table does not add up.
Processes that appear and exit mid-recording are tracked, as are kernel threads. For processes that were writing, the
regular files they had open are captured too, so you learn *what* is being written and not just by whom.

The viewer stacks three machine-wide charts on top - CPU total, disk read/write at the device, and what processes
wrote through syscalls. Drag across any of them to select a period: the band is drawn on all three with its start and
end times on the edges, and the process table below recomputes for exactly that range. Setting a quiet period as the
**baseline** switches the table to differences, which cancels out steady background load and leaves whatever actually
changed. Each row also carries a shape column per metric - CPU, write, write-to-device, read and RSS - scaled to that
row, so a single spike is distinguishable from an even stream that adds up to the same total. There are also a process
tree (with subtree totals) and a lifetimes view, where a process respawned on a timer shows up as a regular comb of
short bars. Grouping by executable folds every instance of a program into one row: the figures are its instances added
together, and the peak columns are the highest they reached *together* - a program respawned hundreds of times never
coexists with itself, so adding up the peaks it reached one at a time would describe a moment that never happened. Kernel threads are hidden from the table by default and there is a checkbox to bring them back; every
column is explained in a glossary at the bottom of the page.

### Sampling rate and what it cannot see

The maximum rate is **4 Hz**, and a higher `--hz` is rejected rather than accepted. `sysinfo` will not re-read
`/proc/stat` more often than every 200 ms while still refreshing each process's own counters, so a faster rate leaves
per-process CPU% divided by a stale denominator - the numbers stay plausible while being wrong, which is worse than
refusing. Sampling exactly at that 200 ms floor is not safe either: the gate is checked against the wall clock, so a
5 Hz tick clears it only about half the time, and a busy core owing 4.17% per tick was recorded as `4.17, 4.17, 2.09`
repeating. 4 Hz keeps 50 ms of margin and measures flat.

A process living less than one sampling interval is never observed directly. It is usually still visible, because Linux
folds a reaped child's I/O accounting into its parent: the bytes surface against whatever spawned it. Where the child
*was* sampled, its contribution is measured and reported in a separate "Write by children" column instead of being
credited to the parent; where it was not, the parent's own total unavoidably includes it, and the row is tagged with how
many children it started so the ambiguity is visible rather than hidden.

Syscall-level I/O, open files and kernel threads come from `/proc`, so on Windows and macOS a recording falls back to
what `sysinfo` reports (device-level bytes only) and says so in the viewer.

### Run it with privilege, or most of the I/O is invisible

`/proc/<pid>/io` is mode 0400 and needs permission over the target process. Recorded as an ordinary user you get
counters for your own processes and nothing else - no system daemons, no kernel threads, so none of the writeback that
buffered writes are charged to. Those rows are tagged `no I/O data` and a notice appears at the top of the viewer,
because a zero there means *unknown*, not *idle*, and the gap between what the disk did and what processes claim is
then meaningless.

Two capabilities fix it: `cap_dac_read_search` opens the 0400 file and `cap_sys_ptrace` passes the access check the
kernel additionally applies. Either alone still fails.

For a recording started from the terminal, grant them to the binary once:

```
sudo setcap cap_dac_read_search,cap_sys_ptrace+ep ./system_info_collector
```

For recordings started from the web UI - which run inside the `collect --serve` process - the shipped systemd unit
already asks for them via `AmbientCapabilities`, so a deployed service has full coverage without any `setcap` at all.
That matters because file capabilities are extended attributes and do not survive `scp`; the `just full_send`,
`full_send_arm` and `full_install` recipes reapply them after every deploy anyway, and `just grant_caps <ip>` does it
on its own for a binary copied over by hand.

Or simply run the whole thing as root:

```
sudo ./system_info_collector session --duration 60
```

### Where recordings are written

`--session-dir` (default `sessions/`) for both the command and the web UI, or `--output` for one explicit path. Files are
listed newest first in the web UI, so two recordings - one while the machine misbehaves, one while it is fine - can be
compared. Nothing is written until a recording finishes or is stopped: it is held in memory, so a crash mid-recording
loses it.

> The session endpoints are the only ones in this server that *do* something rather than just read data, and like the
> rest of the server they are unauthenticated. Anyone who can reach the port can start a recording and read process
> command lines and file paths, which do sometimes contain secrets. Bind it somewhere you trust.

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

- **Plotly report** – the same HTML plot the `convert` command produces (
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
| `GET /session` | the process session viewer |
| `POST /api/session/start` | body `{"seconds": 60, "hz": 4}` - starts a recording; `409` when one is already running, `400` when the rate is above the 4 Hz maximum |
| `POST /api/session/stop` | stops the running recording and keeps what it collected; `404` when none is running |
| `GET /api/session/status` | whether a recording is active, elapsed and remaining seconds, samples taken, and the file name once it finishes |
| `GET /api/session/list` | saved recordings, newest first, each with its metadata |
| `GET /api/export/json?mode=…` | live buffer as JSON, downloaded as a file |
| `GET /api/session/file/<name>` | one recording as JSON, inline; add `?download=1` to save it as a file |
| `GET /api/session/export/<name>` | one recording as a self-contained HTML viewer |

### Live update protocol

`/api/ws` is requested **once** to upgrade the connection; from then on the connection stays open and the
server pushes on its own — the client never asks for anything again. One frame is sent per collection tick,
containing everything gathered in that tick:

```json
{"timestamp":4.1,"data":["4.1","100","15027.1"]}
```

The frame is built and serialized **once per tick**, not once per client: all connected browsers share the
same buffer and sending to each is only a socket write. When no browser is connected nothing is serialized
at all. Measured on a 12-thread machine with `-c 0.1` and per-core CPU tracking: 0.24% CPU with nobody
watching, 0.40% with one browser, 1.36% with sixteen.

A client that cannot keep up is allowed to fall 64 frames behind, after which it skips to the newest data
instead of replaying a backlog.

## License

MIT License

Copyright (c) 2023-2026 Rafał Mikrut and contributors