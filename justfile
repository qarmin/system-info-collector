# Examples:
#   just full_install
#   just full_install "cpu-usage-total memory-used memory-free memory-available network-rx network-tx network-total gpu-utilization gpu-memory-used gpu-temperature disk-used disk-available disk-busy" "/ /home"
#   just full_send 192.168.1.50
#   just full_send 192.168.1.50 "cpu-usage-total memory-used" "/"
#   just full_send_arm 192.168.1.60 "cpu-usage-total memory-used" "" system-info-collector-arm.service
#
# Notes:
#   - `metrics` is space-separated (matches the CLI's `-m` flag), not comma-separated.
#   - `disks` is space-separated mount points/devices for --disk, e.g. "/ /home" - only
#     takes effect if `metrics` includes a disk-* metric. The special value "all"
#     means --all-disks (every real, non-virtual disk, discovered at service start).
#   - See `default_metrics`/`default_disks` below for the full list of valid metrics values.
#   - "all" skips virtual filesystems and boot partitions; add `--exclude-disk <mount>` to the
#     service ExecStart to drop anything else.

user := `whoami`

build_all:
    cargo build --release
    cargo build
    cargo clippy
    cargo test

# Prints detected CPU, memory, disks (with a live busy%/throughput sample), network interfaces and
# GPUs (with one live sample per GPU), reusing the same discovery/monitoring code the collector uses.
# Handy for verifying GPU detection (e.g. AMD) on a machine without re-running the full collector.
system-check:
    cargo run -p system_info_collector_core --example system_check

upgrade:
    cargo +nightly -Z unstable-options update --breaking
    cargo update

fix:
    cargo +nightly fmt
    cargo clippy --fix --allow-dirty --allow-staged --all-targets --all-features
    cargo +nightly fmt

run:
    # Well, do not run this to test things, because just runs this in a shell command, that is captured as normal program
    cargo run -p system_info_collector -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -o --all-networks; firefox system_data_plot.html

runs:
    # Well, do not run this to test things, because just runs this in a shell command, that is captured as normal program
    cargo run -p system_info_collector -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -s --all-networks; firefox system_data_plot.html

runr:
    # Well, do not run this to test things, because just runs this in a shell command, that is captured as normal program
    cargo run --release -p system_info_collector -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -o --all-networks; firefox system_data_plot.html

runrs:
    # Well, do not run this to test things, because just runs this in a shell command, that is captured as normal program
    cargo run --release -p system_info_collector -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -s --all-networks; firefox system_data_plot.html

cross_arm_32:
    cargo zigbuild --target armv7-unknown-linux-gnueabihf -p system_info_collector

cross_x86_64:
    # To avoid glibc version issues on older target distros, using zigbuild against an older glibc version
    cargo zigbuild --target x86_64-unknown-linux-gnu.2.28 -p system_info_collector

stop_remote ip_address:
    ssh root@{{ ip_address }} 'systemctl stop system-info-collector' || true
    ssh root@{{ ip_address }} 'pkill -f /home/root/data_collector/system_info_collector' || true

# The x86_64 target is a regular user's machine, not the root-only ARM perimeter device, so it connects as $USER and needs sudo for service management
stop_remote_x86_64 ip_address:
    ssh -t {{ user }}@{{ ip_address }} 'sudo systemctl stop system-info-collector' || true
    ssh {{ user }}@{{ ip_address }} 'pkill -f /home/{{ user }}/data_collector/system_info_collector' || true

arm_send ip_address:
    # To avoid glibc version issues, using zigbuild
    cargo zigbuild --release --target armv7-unknown-linux-gnueabihf.2.28 -p system_info_collector
    ssh root@{{ ip_address }} 'mkdir -p /home/root/data_collector'
    just stop_remote {{ ip_address }}
    # Removing the old binary, because scp into a still running one fails with "Text file busy"
    ssh root@{{ ip_address }} 'rm -f /home/root/data_collector/system_info_collector' || true
    scp -O target/armv7-unknown-linux-gnueabihf/release/system_info_collector root@{{ ip_address }}:/home/root/data_collector/system_info_collector

x86_64_send ip_address:
    # To avoid glibc version issues on older target distros, using zigbuild against an older glibc version
    cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.28 -p system_info_collector
    ssh {{ user }}@{{ ip_address }} 'mkdir -p /home/{{ user }}/data_collector'
    # Uploading under a temp name and renaming into place atomically, instead of stopping the service first, so scp never hits "Text file busy" and no sudo/password is needed here
    scp -O target/x86_64-unknown-linux-gnu/release/system_info_collector {{ user }}@{{ ip_address }}:/home/{{ user }}/data_collector/system_info_collector.new
    ssh {{ user }}@{{ ip_address }} 'mv -f /home/{{ user }}/data_collector/system_info_collector.new /home/{{ user }}/data_collector/system_info_collector'

# Defaults for the `metrics`/`disks` arguments on full_send_arm/full_send/full_install below.
default_metrics := "cpu-usage-total memory-used memory-free memory-available network-rx network-tx disk-used disk-available disk-busy disk-read disk-write"
# Space-separated mount points/device names to pass as repeated --disk flags, e.g. "/ /home".
# "all" means --all-disks (every real disk, discovered at service start); empty means no disk tracked.
default_disks := "all"

# metrics values: cpu-usage-total cpu-usage-per-core swap-free swap-used memory-used memory-free memory-available network-rx network-tx network-total gpu-utilization gpu-memory-used gpu-temperature disk-used disk-available disk-busy disk-read disk-write
# disks: space-separated mount points/devices for --disk, e.g. disks="/ /home" - only matters if metrics includes a disk-* metric
full_send_arm ip_address metrics=default_metrics disks=default_disks service_file="system-info-collector.service":
    ssh root@{{ ip_address }} 'systemctl disable system-info-collector' || true
    just stop_remote {{ ip_address }}
    just arm_send {{ ip_address }}
    if [ "{{ disks }}" = "all" ]; then disk_flags="--all-disks"; \
    else disk_flags=""; for d in {{ disks }}; do disk_flags="$disk_flags --disk $d"; done; fi; \
    sed -e 's/__METRICS__/{{ metrics }}/g' -e "s#__DISKS__#$disk_flags#g" "{{ service_file }}" > /tmp/system-info-collector.service
    scp -O /tmp/system-info-collector.service root@{{ ip_address }}:/etc/systemd/system/system-info-collector.service
    ssh root@{{ ip_address }} 'systemctl daemon-reload'
    ssh root@{{ ip_address }} 'systemctl enable system-info-collector'
    ssh root@{{ ip_address }} 'systemctl restart system-info-collector'
    ssh root@{{ ip_address }} 'cat /home/root/data_collector/data.csv' || true
    ssh root@{{ ip_address }} 'systemctl status system-info-collector' || true

# Unlike full_send_arm (root, perimeter device), this targets a normal x86_64 machine reachable as the local $USER account,
# so the service file's __USER__ placeholder is filled in with that username instead of root before it's shipped over.
# All sudo steps are bundled into a single `ssh -t` call so the remote password is only asked once.
# metrics values: cpu-usage-total cpu-usage-per-core swap-free swap-used memory-used memory-free memory-available network-rx network-tx network-total gpu-utilization gpu-memory-used gpu-temperature disk-used disk-available disk-busy disk-read disk-write
# disks: space-separated mount points/devices for --disk, e.g. disks="/ /home" - only matters if metrics includes a disk-* metric
full_send ip_address metrics=default_metrics disks=default_disks service_file="system-info-collector.service":
    if [ "{{ disks }}" = "all" ]; then disk_flags="--all-disks"; \
    else disk_flags=""; for d in {{ disks }}; do disk_flags="$disk_flags --disk $d"; done; fi; \
    sed -e 's/__USER__/{{ user }}/g' -e 's/__METRICS__/{{ metrics }}/g' -e "s#__DISKS__#$disk_flags#g" "{{ service_file }}" > /tmp/system-info-collector.service
    just x86_64_send {{ ip_address }}
    scp -O /tmp/system-info-collector.service {{ user }}@{{ ip_address }}:/tmp/system-info-collector.service
    ssh -t {{ user }}@{{ ip_address }} 'sudo mv /tmp/system-info-collector.service /etc/systemd/system/system-info-collector.service && sudo systemctl daemon-reload && sudo systemctl enable system-info-collector && sudo systemctl restart system-info-collector && sudo systemctl status --no-pager system-info-collector'
    ssh {{ user }}@{{ ip_address }} 'cat /home/{{ user }}/data_collector/data.csv' || true

show_data ip_address:
    scp -O root@{{ ip_address }}:/home/root/data_collector/data.csv .
    cargo run --release -p system_info_collector -- convert -d data.csv -p plot.html -o

show_data_custom ip_address path:
    #scp -O {{ ip_address }}:{{ path }} .
    cargo run --release -p system_info_collector -- convert -d data.csv -p plot.html -o --split-mode per-day

show:
    rm *.html || true
    # If all data files are present, show them together, otherwise show only the main one
    if [ -f system_data_top_cpu.csv ] && [ -f system_data_top_ram.csv ]; then \
    cargo run --release -p system_info_collector -- convert -d system_data.csv -d system_data_top_cpu.csv -d system_data_top_ram.csv -p plot.html -o; firefox plot.html; firefox plot_top_cpu.html; firefox plot_top_ram.html \
    ; else \
    cargo run --release -p system_info_collector -- convert -d system_data.csv -p plot.html -o; firefox plot.html \
    ; fi


all:
    rm *.csv || true
    # Run with all collection modes enabled + HTTP live data server + top-N processes
    cargo run --release -p system_info_collector -- collect \
        -m cpu-usage-total  \
           memory-used memory-free memory-available \
           swap-used swap-free \
           network-rx network-tx network-total \
           gpu-utilization gpu-memory-used gpu-temperature \
           disk-used disk-available disk-busy disk-read disk-write \
           cpu-usage-per-core \
        --all-networks --all-disks -e "NEMO|nemo" \
        -c 0.5 -s --top-n-processes 5
    just show

normal:
    rm *.csv || true
    # All without top-N processes and cpu-usage-per-core and swap, because it is not needed for normal usage and takes more resources to collect and plot
    cargo run --release -p system_info_collector -- collect \
        -m cpu-usage-total  \
           memory-used memory-free memory-available \
           network-rx network-tx network-total \
           gpu-utilization gpu-memory-used gpu-temperature \
           disk-used disk-available disk-busy disk-read disk-write \
        --all-networks --all-disks -e "GNOME SHELL|gnome-shell" \
        -c 0.5 -s

heavy:
    rm *.csv || true
    cargo run --release -p system_info_collector -- collect \
        -m cpu-usage-total cpu-usage-per-core --top-n-processes 10 -c 2.0 -s
        
samplyrd:
    rm *.csv || true
    cargo build --profile rdebug
    samply record target/rdebug/system_info_collector collect \
        -m cpu-usage-total  \
             memory-used memory-free memory-available \
             swap-used swap-free \
             network-rx network-tx network-total \
             gpu-utilization gpu-memory-used gpu-temperature \
             disk-used disk-available disk-busy disk-read disk-write \
        --all-networks --all-disks \
        -c 0.5 -s --top-n-processes 5
        
cleancsv:
    rm *.csv

publish:
    cd crates/system_info_collector_core && cargo publish --dry-run
    cd crates/system_info_collector_core && cargo publish
    cd crates/system_info_collector && cargo publish --dry-run
    cd crates/system_info_collector && cargo publish

install:
    cargo install --path crates/system_info_collector

# Local equivalent of full_send: builds, installs, and starts the systemd service on this machine, no ssh involved.
# Uses cargo zigbuild against the same older glibc target as full_send/x86_64_send, so it's the exact same binary either way ships.
# metrics values: cpu-usage-total cpu-usage-per-core swap-free swap-used memory-used memory-free memory-available network-rx network-tx network-total gpu-utilization gpu-memory-used gpu-temperature disk-used disk-available disk-busy disk-read disk-write
# disks: defaults to "all" (--all-disks, every real disk discovered at service start); pass
# space-separated mount points/devices instead, e.g. disks="/ /home", to pick them by hand
full_install metrics=default_metrics disks=default_disks service_file="system-info-collector.service":
    cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.28 -p system_info_collector
    mkdir -p /home/{{ user }}/data_collector
    # Copying under a temp name and renaming into place atomically (like x86_64_send), since the
    # service may currently be running the old binary and a direct overwrite fails with "Text file busy"
    cp target/x86_64-unknown-linux-gnu/release/system_info_collector /home/{{ user }}/data_collector/system_info_collector.new
    mv -f /home/{{ user }}/data_collector/system_info_collector.new /home/{{ user }}/data_collector/system_info_collector
    /home/{{ user }}/data_collector/system_info_collector collect --list-disks
    if [ "{{ disks }}" = "all" ]; then disk_flags="--all-disks"; \
    else disk_flags=""; for d in {{ disks }}; do disk_flags="$disk_flags --disk $d"; done; fi; \
    sed -e 's/__USER__/{{ user }}/g' -e 's/__METRICS__/{{ metrics }}/g' -e "s#__DISKS__#$disk_flags#g" "{{ service_file }}" > /tmp/system-info-collector.service
    sudo cp /tmp/system-info-collector.service /etc/systemd/system/system-info-collector.service
    sudo systemctl daemon-reload
    sudo systemctl enable system-info-collector
    sudo systemctl restart system-info-collector
    sudo systemctl status --no-pager system-info-collector