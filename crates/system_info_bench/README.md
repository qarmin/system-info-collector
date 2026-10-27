# system_info_bench

Measures programs and compares them: CPU and memory of the whole process tree (children started
later included, short-lived ones too), plus numbers the program prints. A small static binary,
fine to copy to a device.

```sh
# One program, 10 s after a 2 s warm-up, 3 runs, medians with min/max
system_info_bench run --warmup 2s --duration 10s --repeat 3 --spread -- ./app --flag

# Until it exits, with a number read from its output
system_info_bench run --until-exit --metric 'fps:mean=fps=([\d.]+)' -- ./render scene.json

# Something already running
system_info_bench attach --duration 30s 4242

# Variants crossed with a matrix, see examples/compression.toml
system_info_bench matrix bench.toml --format markdown --svg charts/ --csv samples/
```

CPU is 100% per fully busy core. `peak rss` sums each process's own peak (`VmHWM`), `rss max` is
the largest total seen in one sample. Tables go to stdout as `terminal`, `markdown`, `tsv` or
`csv`; `--svg`, `--csv` and `--json` write charts, every sample, and everything.

## Benchmark file

```toml
repeats = 3
warmup = "2s"
duration = "10s"        # or: until_exit = true, timeout = "60s"
interval = "250ms"
scope = "tree"          # or "process"
logs = "logs"           # keep every run's output; otherwise only captured for metrics

[matrix]
size = ["1024x768", "3840x2160"]
mode = ["idle", "busy"]

[[variant]]
name = "app"
program = "./target/release/app"
args = ["--size", "{size}", "--mode", "{mode}"]
env = { RUST_LOG = "warn" }
matrix = ["size", "mode"]   # dimensions to expand over; all when left out

[[metric]]
name = "frames/s"
pattern = 'frames=(\d+) t=([\d.]+)'
extract = "rate"            # last, first, min, max, mean, sum, count, rate

[[service]]                 # started before every run, killed with its children after it
command = "Xvfb :99 -screen 0 {size}x24"
settle = "1s"
```

`{name}`, `{repeat}` and every matrix dimension can be used in programs, arguments, environment
values and service commands; `{{` and `}}` are literal braces.  Exit code 1 means some run failed
or exited during the warm-up, 2 a usage or file error.
