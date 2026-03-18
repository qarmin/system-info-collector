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