## Unreleased
- Removed `--top-n-processes` and the `_top_cpu.csv` / `_top_ram.csv` files - process sessions cover the same ground with per-PID detail, lifetimes and I/O attribution, and without refreshing every process on every collection tick
- `convert -d` now takes a single data file, since there are no companion files left to pass
- Added `session` command and web UI panel - a short recording of every process, for finding out what is loading the machine right now
- Session recordings track per-process lifetimes, PIDs, syscall-level and device-level I/O, open files of writers, process status and context switches
- Added a session viewer (`/session`, or a single self-contained HTML file): three linked charts with a labelled selection band, baseline comparison, per-metric shape columns, process tree and lifetime views, and a glossary of every column
- Kernel threads are hidden from the session table by default, with a checkbox to show them
- Session recordings now carry whole-recording byte totals (device vs syscall) and per-process cumulative read/write counters, so writes the device did that no process claimed are called out explicitly
- Per-process I/O is reported at three scopes - selected period, whole recording, and process lifetime - as separate table columns and as a breakdown in the expanded row
- The recording's own process is tagged in the session table, and its share of the syscall read total is stated separately: those reads are the /proc files it read to measure everything else, not disk traffic
- Cut the recorder's own /proc read volume about fourfold by not refreshing threads it discards anyway (`ProcessRefreshKind::nothing()` leaves `tasks` enabled) and by re-reading `/proc/<pid>/status` once a second instead of every sample
- Live view gains 30 second and 1 minute ranges, and defaults to 5 minutes
- Recording duration is capped at 5 minutes, and the result is no longer downloaded unless asked for
- Sorting the session table no longer scrolls it back to the first column
- Recordings and live buffer data can be downloaded as JSON (`/api/export/json`, and `?download=1` on a session file)
- The totals panel shows the peak sample next to the average, with the bytes that peak represents, since a tall spike and a small total are routinely mistaken for a contradiction
- Recordings carry a per-device byte breakdown, including the devices left out of the machine total and why - `/proc/diskstats` shows the same hardware under several names and a total that looks impossible is usually one write counted twice
- Software RAID (`md*`) and NVMe multipath aliases (`nvme0c0n1`) are now recognised as duplicates of the devices they sit on; previously their traffic was added on top
- A process whose `/proc/<pid>/io` cannot be read is now recorded as unknown rather than as zero, tagged in the table, counted in the metadata and announced at the top of the viewer - an unprivileged recording sees I/O for its own processes only, which previously looked like every daemon and kernel thread writing nothing
- Recordings carry each process's owning uid and the recorder's own uid, so incomplete coverage is self-evident
- The device write total is now reconciled against per-process *device* bytes rather than syscall bytes: a writer that uses `mmap` (systemd-journald being the common one) has `wchar` of zero while its pages still reach the disk, which previously showed up as a large phantom shortfall
- The systemd unit asks for `CAP_DAC_READ_SEARCH` and `CAP_SYS_PTRACE` via `AmbientCapabilities`, so recordings started from the web UI of a deployed service see every process; the `just` deploy recipes also reapply the equivalent file capabilities, which `scp` strips
- A finished recording is no longer discarded when the record panel is closed
- Added a `verify-session` skill in `.claude/skills/` with a `verify_session.py` script that checks a recording's invariants, reconciles rates against totals, reports peaks in context and summarises attribution
- Split the Disk Space chart into separate used and available charts
- Disks, GPUs and network interfaces now keep one colour across every chart; paired series (read/write, RX/TX) share a hue at two intensities

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