## Version 0.8.0 - 24.09.2026

### Process sessions - new

- Added the `session` command and a **Record processes** panel in the web UI - a short recording of *every* process,
  for finding out what is loading the machine right now
- Recordings track per-process lifetimes, PIDs, syscall-level and device-level I/O, the open files of writers, process
  status and context switches
- Added a session viewer (`/session`, or a single self-contained HTML file): three linked charts with a labelled
  selection band, baseline comparison, per-metric shape columns, process tree and lifetime views, and a glossary of
  every column
- Per-process I/O is reported at three scopes - selected period, whole recording, and process lifetime - as separate
  table columns and as a breakdown in the expanded row
- Recordings carry whole-recording byte totals (device vs syscall) and per-process cumulative read/write counters, so
  writes the device did that no process claimed are called out explicitly
- Recordings carry a per-device byte breakdown, including the devices left out of the machine total and why -
  `/proc/diskstats` shows the same hardware under several names and a total that looks impossible is usually one write
  counted twice
- A process whose `/proc/<pid>/io` cannot be read is recorded as unknown rather than as zero, tagged in the table,
  counted in the metadata and announced at the top of the viewer - an unprivileged recording sees I/O for its own
  processes only, which previously looked like every daemon and kernel thread writing nothing
- Recordings carry each process's owning uid and the recorder's own uid, so incomplete coverage is self-evident
- The recording's own process is tagged in the table, and its share of the syscall read total is stated separately:
  those reads are the `/proc` files it read to measure everything else, not disk traffic
- Kernel threads are hidden from the table by default, with a checkbox to show them
- Recording duration is capped at 5 minutes, and the result is no longer downloaded unless asked for
- The systemd unit asks for `CAP_DAC_READ_SEARCH` and `CAP_SYS_PTRACE` via `AmbientCapabilities`, so recordings started
  from the web UI of a deployed service see every process; the `just` deploy recipes reapply the equivalent file
  capabilities, which `scp` strips
- Cut the recorder's own `/proc` read volume about fourfold by not refreshing threads it discards anyway
  (`ProcessRefreshKind::nothing()` leaves `tasks` enabled) and by re-reading `/proc/<pid>/status` once a second instead
  of every sample
- Lowered the maximum sampling rate from 5 Hz to 4 Hz: sampling exactly at the 200 ms floor `sysinfo` enforces cleared
  it only about half the time, which halved roughly a third of all per-process CPU samples (a busy core owing 4.17% per
  tick recorded as `4.17, 4.17, 2.09` repeating)
- Software RAID (`md*`) and NVMe multipath aliases (`nvme0c0n1`) are recognised as duplicates of the devices they sit
  on; previously their traffic was added on top
- The device write total is reconciled against per-process *device* bytes rather than syscall bytes: a writer that uses
  `mmap` (systemd-journald being the common one) has `wchar` of zero while its pages still reach the disk, which
  previously showed up as a large phantom shortfall

### Live web view and exports

- Charts now split disk space into separate used and available charts
- Disks, GPUs and network interfaces keep one colour across every chart; paired series (read/write, RX/TX) share a hue
  at two intensities
- The System Information panel and the startup log list every attached filesystem with its size, type and drive model
  (`/home - 915.8 GiB ext4, nvme1n1 SSDPR-PX600-1K0-80`), not only the disks whose usage is tracked - the model is what
  a drive is looked up or bought by, and `sysinfo` does not report it
- The separate "Tracked disks" tile is gone: tracked mounts are marked `DISK_N` in that one list instead of being
  spelled out twice
- The panel reports the operating system, kernel version and hostname
- Added 30 second and 1 minute ranges; the view defaults to 5 minutes
- The totals panel shows the peak sample next to the average, with the bytes that peak represents, since a tall spike
  and a small total are routinely mistaken for a contradiction
- Recordings and live buffer data can be downloaded as JSON (`/api/export/json`, and `?download=1` on a session file)

### Collecting

- Removed `--top-n-processes` and the `_top_cpu.csv` / `_top_ram.csv` files - process sessions cover the same ground
  with per-PID detail, lifetimes and I/O attribution, and without refreshing every process on every collection tick
- `convert -d` now takes a single data file, since there are no companion files left to pass
- `--list-disks` reports the filesystem type of each mount
- The deployed service no longer collects `memory-free` by default - on Linux it excludes the page cache and reads far
  below the memory actually obtainable, which `memory-available` already reports

### Fixes

- Fixed `session_viewer.html` never being committed: a blanket `*.html` in `.gitignore` excluded it while it is pulled
  in with `include_str!`, so the build worked only on the machine that wrote the file
- Fixed the "CPU total" chart of a session reading 0% throughout: refreshing processes updates only the aggregated CPU
  line of `/proc/stat`, so the per-CPU list the recorder summed stayed at the zero it was created with
- Fixed "CPU max" of a row grouped by executable being the largest single instance rather than the largest combined
  load of all of them, which put the max below the row's own average whenever the instances overlapped - 344
  short-lived `ffmpeg` processes peaking at 17% each showed avg 55%, max 17%
- Fixed "RSS max" and "RSS change" of a row grouped by executable adding up figures that belong to different moments:
  instances that took turns rather than running side by side reported the memory of all of them at once (37 respawning
  processes measured 120 MB where their combined peak was 33 MB)
- Fixed the RSS shape column holding a dead process's last value to the end of the window, which drew a row of
  respawning processes as a staircase that never comes down
- The table's numbers and its shape columns are built from one pair of series, so a shape can no longer disagree with
  the figure beside it
- A session whose CPU series was never recorded says so on the chart instead of drawing an idle machine
- A finished recording is no longer discarded when the record panel is closed
- Sorting the session table no longer scrolls it back to the first column

### Project

- Updated all dependencies, `nvml-wrapper` across a major version (0.12 → 0.13)
- `.gitignore` anchors the generated-file patterns to the repository root instead of matching by extension everywhere,
  so a page or fixture living next to its code cannot be excluded by accident
- Added a test that every `include_str!` target is tracked by git, so this class of breakage fails locally instead of
  on someone else's machine
- The `justfile` is split into groups (`just --groups`, `just --list <group>`) and every recipe has a one-line
  description
- Added a `verify-session` skill in `.claude/skills/` with a `verify_session.py` script that checks a recording's
  invariants, reconciles rates against totals, reports peaks in context and summarises attribution

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