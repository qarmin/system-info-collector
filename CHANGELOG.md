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
- Using pseudo csv file format instead of real csv file - allows to generate smaller file sizes by using MEMORY_TOTAL only once instead in each row
- Fixed collecting data with non integers second intervals 
- Generated html file should be now minimized (~30% smaller)
- Using local time offset instead of UTC time in plot

## Version 0.1.0 - 07.07.2023
- Initial release