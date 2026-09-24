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