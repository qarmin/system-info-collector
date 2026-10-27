## Unreleased

### Libraries and benchmarking - new

- `system_info_probe`: sampling of processes, CPU, memory, disks, network, GPUs and ping as a library, blocking or on
  tokio - process trees and patterns follow processes started later, and tree CPU includes reaped children
- `system_info_report`: summaries, tables, CSV/JSON export, SVG and HTML charts, and benchmarks over repeated runs
- `system_info_bench`: a small tool to measure a command, a running process, or a matrix of variants from a TOML file,
  with numbers read from the program's output by regex

### Collecting

- Charts leave a gap where a value could not be read instead of dropping to -1
- `-e` tracks every matching process, summed, instead of only the one with the shortest command line; the collector
  and the processes that started it are no longer matched by their own arguments
- GPU figures a driver does not expose (memory of Intel iGPUs, unreadable RC6) are written as -1 instead of 0
- Pings record each reply as it arrives, so an unreachable target no longer delays the others
- Process sessions attribute I/O in the first sample instead of folding it into the starting totals, and read 44%
  less from `/proc`
- Fewer reads overall: watched processes are re-read only when they match, disk space is read only for tracked disks
  and network counters only for tracked interfaces
- Smaller CSV files (about 12% on a busy machine, more on an idle one): per-core CPU is written in whole percent and
  compacted per core, machine memory and swap in whole MB; older files still load

### Ping - new

- New `ping-latency` (ms) and `ping-loss` (%) metrics, each with its own chart in the report and the live view
- Pings `gateway`, `1.1.1.1` and `8.8.8.8` by default, `--ping-target` replaces the list; `gateway` follows the
  current default route
- `--ping-interval` (0.5 s) and `--ping-timeout` (2.0 s)
- Uses an unprivileged ICMP socket when allowed, a raw one otherwise; when neither can be opened the ping columns are
  left out and the live view shows a dismissible warning
- The systemd unit and `session_caps` grant `CAP_NET_RAW`, and `ping-latency` is part of the default deploy metrics

### Web UI

- The Full Width button became a **Layout** select with a new **Gallery** mode - charts keep roughly their normal
  size and are packed side by side, as many per row as the screen fits; Gallery is the default
- New **Chart size** select (50%-200%) scaling the charts in the Normal and Gallery layouts, remembered across reloads

## Version 0.8.0 - 24.09.2026

### Process sessions - new

- New `session` command and **Record processes** button in the web UI - records every process for up to 5 minutes, to
  find out what is loading the machine right now
- Records per process: CPU, memory, lifetime, PID, read/write bytes, open files of writers, status and context switches
- New viewer (`/session`, or one self-contained HTML file) with linked charts, drag-to-select period, baseline
  comparison, process tree, lifetimes and a glossary of every column
- Byte totals are broken down per device, with duplicates (software RAID, NVMe multipath aliases) counted once
- Processes whose I/O counters cannot be read are marked unknown rather than zero, and the viewer says when a recording
  was made without the privileges to see everything
- The systemd unit and the `just` deploy recipes grant the capabilities needed to read every process's I/O
- Maximum sampling rate is 4 Hz - a faster tick divides per-process CPU by a stale denominator
- Kernel threads are hidden by default, with a checkbox to show them

### Live web view

- Disk space split into separate used and available charts
- Disks, GPUs and interfaces keep one colour across every chart
- The System Information panel lists every attached filesystem with its size, type and drive model, plus the OS, kernel
  version and hostname
- Added 30 second and 1 minute time ranges; the default is now 5 minutes
- Totals show the peak sample next to the average
- Live buffer and recordings can be downloaded as JSON

### Collecting

- Removed `--top-n-processes` and the `_top_cpu.csv` / `_top_ram.csv` files - sessions replace them
- `convert -d` now takes a single data file
- `--list-disks` reports the filesystem type of each mount
- The deployed service no longer collects `memory-free` by default - on Linux `memory-available` is the useful figure

### Fixes

- Fixed `session_viewer.html` never being committed, so the build worked only on the machine that wrote the file
- Fixed a session's "CPU total" chart reading 0% throughout
- Fixed CPU and memory figures of rows grouped by executable adding up values from moments that never overlapped
- Fixed the shape columns disagreeing with the numbers beside them
- A session recorded without CPU data says so instead of drawing an idle machine
- A finished recording is no longer lost when the record panel is closed
- Sorting the session table no longer scrolls it back to the first column

### Project

- Increased minimum rust version to 1.95 - `sysinfo` 0.39 has required it all along
- Updated all dependencies, `nvml-wrapper` across a major version (0.12 → 0.13)
- The `justfile` is split into groups (`just --groups`), with a description on every recipe
- Added a test that every `include_str!` target is tracked by git

## Version 0.7.0 - 18.03.2026
- Increased minimum rust version to 1.92
- Refactored code to use cargo workspace with two crates - `system_info_collector_core` and `system_info_collector`
- Added support for collecting network info (bytes sent/received)
- Added support for collecting GPU info (temperature, memory usage, gpu usage) - the 
- Added support for collecting top X processes by cpu/ram usage
- Added support for collecting disk space info
- Added enabled by default, compact mode, to minimize generated csv file size(missing value means that previous value should be used)

## Version 0.6.0 - 05.10.2025
- Increased minimum rust version to 1.85
- Updated sys-info - now less info is collected, to minimize disk requests
- New server mode - serve simple web server with plot and text info in realtime on given port
- Changed some options - making e.g. app mode mandatory

## Version 0.5.1 - 10.08.2023

- Fixes problem with start time, which was taken from current time instead collecting start time

## Version 0.5.0 - 23.07.2023

- Store time since app start instead full unix timestamp in each record to minimize generated csv size(usually 5-15%)

## Version 0.4.0 - 23.07.2023

- Fixed invalid per process cpu usage calculation(worked fine only on cpu with 8 cores)
- Do not save too much data unnecessary data into csv file(like timestamp in microseconds)
- Add support for collecting swap info

## Version 0.3.0 - 14.07.2023

- Create backup of data file if already exists
- Add instant flushing of data file
- Added instruction, how to create simple systemd service
- Maximum file limit can be set(default 100MB), to avoid out of space problems
- Collecting memory and cpu data from selected processes
- -1 value in plot to show that process was not found in system

## Version 0.2.0 - 09.07.2023

- Added CLI
- Ability to only produce, generate plot or both
- More modular code
- Using pseudo csv file format instead of real csv file - allows to generate smaller file sizes by using MEMORY_TOTAL
  only once instead in each row
- Fixed collecting data with non integers second intervals
- Generated html file should be now minimized (~30% smaller)
- Using local time offset instead of UTC time in plot

## Version 0.1.0 - 07.07.2023

- Initial release