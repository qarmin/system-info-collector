# System info collector

This is simple app to collect data about system cpu and memory usage over time.

After collecting results into csv file, html file can be created with plot.

![Screenshot](https://github.com/qarmin/czkawka/assets/41945903/58371709-996a-41cf-a352-d28addf24ad9)

## Why?

I needed a simple and fast application to collect basic information about the amount of RAM used and CPU consumption on
a slow(4x1Ghz) 32 bit ARM computer which uses custom OS.

I looked at a few applications, i.e. grafana, but they are usually too heavy or work in a client server architecture
which in this case I would prefer to avoid.

## How to use it?

Just run app, without any arguments and close app after while with ctrl+c, results will be collected inside
readable `data.csv` file and then `out.html` file will be produced and opened automatically in web browser.

## Performance and memory usage

During testing on i7-4770, app used stable 15-20MB Ram and most of the time, cpu usage was lesser than 0.1%.

In collect mode, app only needs to read cpu/ram usage and then save it to file, so that is why it uses so little
resources.

Converting csv file to html file is more resource intensive, so should be done on more powerful computer.

Results from testing on i7-4770 250000 samples for memory, cpu total and per core usage - with 1s interval, collecting
such number of samples should take ~3 days(I used smaller interval to mimic real usage):

Example of first 4 lines of csv file:

```
INTERVAL_SECONDS=1,CPU_CORE_COUNT=8,MEMORY_TOTAL=23943.921875
UNIX_TIMESTAMP,CPU_USAGE_TOTAL,CPU_USAGE_PER_CORE,MEMORY_USED
1688908461.4185224,0.00,0.00;0.00;0.00;0.00;0.00;0.00;0.00;0.00,10472.640625
1688908462.4186845,5.78,4.42;6.14;5.31;5.36;7.21;8.04;4.46;5.26,10473.49609375
```

- CSV file size: 19.55 MiB
- Loading and parsing csv file: 407 ms
- HTML file size: 129 MiB (new versions use simple regex minimizer, so size should be ~30% smaller)
- Creating html file: 1.68 s

## Plans

- Rotating files
- Allow to track certain process memory/cpu usage
- Creating backups of data if file already exists

## Example commands

Collect used memory and cpu usage in interval of 1 second and save it to system_data.csv file

```
./system_info_collector
```

Collect and convert csv data and automatically open html file in browser, additionally will show more detailed logs

```
./system_info_collector -l debug -a collect-and-convert -o
```

Convert csv data file into html document with plot and open it in browser

```
./system_info_collector -a convert -d /home/user/data.csv -p /home/user/plot.html -o
```

Collect all possible data(at this moment) with interval of 0.2 seconds

```
./system_info_collector -l debug -a collect-and-convert -o -m memory-used -m memory-free -m memory-available -m cpu-usage-total -m cpu-usage-per-core -c 0.2

```

Shows help about available arguments

```
./system_info_collector --help
```

## License

MIT License

Copyright (c) 2023 Rafał Mikrut and contributors