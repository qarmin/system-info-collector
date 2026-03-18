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
    cargo run -p system_info_collector -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -s -l 10000 --all-networks; firefox system_data_plot.html

cross_arm_32:
    cargo zigbuild --target armv7-unknown-linux-gnueabihf -p system_info_collector

arm_send ip_address:
    # To avoid glibc version issues, using zigbuild
    cargo zigbuild --release --target armv7-unknown-linux-gnueabihf.2.28 -p system_info_collector
    ssh root@{{ ip_address }} 'mkdir -p /home/root/data_collector'
    scp -O target/armv7-unknown-linux-gnueabihf/release/system_info_collector root@{{ ip_address }}:/home/root/data_collector/system_info_collector

full_send ip_address service_file:
    just arm_send {{ip_address}}
    ssh root@{{ ip_address }} 'systemctl disable system-info-collector' || true
    ssh root@{{ ip_address }} 'systemctl stop system-info-collector' || true
    scp -O "{{ service_file }}" root@{{ ip_address }}:/etc/systemd/system/system-info-collector.service
    ssh root@{{ ip_address }} 'systemctl daemon-reload'
    ssh root@{{ ip_address }} 'systemctl enable system-info-collector'
    ssh root@{{ ip_address }} 'systemctl start system-info-collector'
    ssh root@{{ ip_address }} 'cat /home/root/data_collector/data.csv'
    ssh root@{{ ip_address }} 'systemctl status system-info-collector'

show_data ip_address:
    scp -O root@{{ ip_address }}:/home/root/data_collector/data.csv .
    cargo run --release -p system_info_collector -- convert -d data.csv -p plot.html -o

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
        --all-networks --all-disks \
        -c 0.5 -s -l 10000 --top-n-processes 5
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
        --all-networks --all-disks \
        -c 0.5 -s -l 10000

heavy:
    rm *.csv || true
    cargo run --release -p system_info_collector -- collect \
        -m cpu-usage-total cpu-usage-per-core --top-n-processes 10 -c 2.0 -s -l 10000
        
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
        --all-networks --all-disk \
        -c 0.5 -s -l 10000 --top-n-processes 5
        
cleancsv:
    rm *.csv
