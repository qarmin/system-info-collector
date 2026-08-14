build_all:
    cargo build --release
    cargo build
    cargo clippy
    cargo test

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

stop_remote ip_address:
    ssh root@{{ ip_address }} 'systemctl stop system-info-collector' || true
    ssh root@{{ ip_address }} 'pkill -f /home/root/data_collector/system_info_collector' || true

arm_send ip_address:
    # To avoid glibc version issues, using zigbuild
    cargo zigbuild --release --target armv7-unknown-linux-gnueabihf.2.28 -p system_info_collector
    ssh root@{{ ip_address }} 'mkdir -p /home/root/data_collector'
    just stop_remote {{ ip_address }}
    # Removing the old binary, because scp into a still running one fails with "Text file busy"
    ssh root@{{ ip_address }} 'rm -f /home/root/data_collector/system_info_collector' || true
    scp -O target/armv7-unknown-linux-gnueabihf/release/system_info_collector root@{{ ip_address }}:/home/root/data_collector/system_info_collector

full_send ip_address service_file:
    ssh root@{{ ip_address }} 'systemctl disable system-info-collector' || true
    just stop_remote {{ ip_address }}
    just arm_send {{ ip_address }}
    scp -O "{{ service_file }}" root@{{ ip_address }}:/etc/systemd/system/system-info-collector.service
    ssh root@{{ ip_address }} 'systemctl daemon-reload'
    ssh root@{{ ip_address }} 'systemctl enable system-info-collector'
    ssh root@{{ ip_address }} 'systemctl restart system-info-collector'
    ssh root@{{ ip_address }} 'cat /home/root/data_collector/data.csv' || true
    ssh root@{{ ip_address }} 'systemctl status system-info-collector' || true

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
           network-rx network-tx \
           gpu-utilization gpu-memory-used gpu-temperature \
           disk-used disk-available \
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
           network-rx network-tx \
           gpu-utilization gpu-memory-used gpu-temperature \
           disk-used disk-available \
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
             network-rx network-tx \
             gpu-utilization gpu-memory-used gpu-temperature \
             disk-used disk-available \
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