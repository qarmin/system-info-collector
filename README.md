# System info collector
This is simple app to collect data about system cpu and memory usage over time.

After collecting results into csv file, html file can be created with plot.

![Screenshot](https://github.com/qarmin/czkawka/assets/41945903/58371709-996a-41cf-a352-d28addf24ad9)

## Why?
I needed a simple and fast application to collect basic information about the amount of RAM used and CPU consumption on a slow(4x1Ghz) 32 bit ARM computer which uses custom OS.

I looked at a few applications, i.e. grafana, but they are usually too heavy or work in a client server architecture which in this case I would prefer to avoid.

## How to use it?
Just run app, without any arguments and close app after while with ctrl+c, results will be collected inside `data.svg` file and then `out.html` file will be produced and opened automatically in web browser.

## Plans
- CLI - with multiple options like choosing output file, time of collecting data, etc.
- Ability to only produce csv file or to only generate html file
- Rotating files
- Choosing which parameters to collect
- Allow to track certain process memory/cpu usage