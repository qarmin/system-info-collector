# System info collector
This is simple app to collect data about system cpu and memory usage over time.

After collecting results into csv file, html file can be created with plot.

![Screenshot](https://github.com/qarmin/czkawka/assets/41945903/58371709-996a-41cf-a352-d28addf24ad9)

## Why?
I needed a simple and fast application to collect basic information about the amount of RAM used and CPU consumption on a slow(4x1Ghz) 32 bit ARM computer which uses custom OS.

I looked at a few applications, i.e. grafana, but they are usually too heavy or work in a client server architecture which in this case I would prefer to avoid.

## How to use it?
Just run app, without any arguments and close app after while with ctrl+c, results will be collected inside readable `data.csv` file and then `out.html` file will be produced and opened automatically in web browser.



## Performance and memory usage
During testing on i7-4770, app used stable 15-20MB Ram and most of the time, cpu usage was lesser than 0.1%.

In collect mode, app only needs to read cpu/ram usage and then save it to file, so that is why it uses so little resources.

Converting csv file to html file is more resource intensive, so should be done on more powerful computer. 


```
12:44:56 [INFO] system_info_collector::csv_file_loader: Data csv file is 19.16 MiB in size
12:44:57 [INFO] system_info_collector::ploty_creator: Loading data took 1.458324683s
12:44:57 [INFO] system_info_collector::ploty_creator: Trying to create html file...
12:45:09 [INFO] system_info_collector::ploty_creator: Creating plot took 11.877464213s
12:45:09 [INFO] system_info_collector::ploty_creator: Opening file system_data_plot.html
12:45:09 [INFO] system_info_collector: Closing app successfully
```











## Plans
- Rotating files
- Allow to track certain process memory/cpu usage