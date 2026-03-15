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
    cargo run -p system_info_collector -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -o; firefox system_data_plot.html

runs:
    # Well, do not run this to test things, because just runs this in a shell command, that is captured as normal program
    cargo run -p system_info_collector -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -s -l 10000; firefox system_data_plot.html

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
    cargo run --release -p system_info_collector -- convert -d system_data.csv -d system_data_top_cpu.csv -d system_data_top_ram.csv -p plot.html -o; firefox plot.html

all:
    # Run with all collection modes enabled + HTTP live data server + top-N processes
    cargo run --release -p system_info_collector -- collect \
        -m cpu-usage-total  \
           memory-used memory-free memory-available \
           swap-used swap-free \
           network-rx-bytes-per-sec network-tx-bytes-per-sec \
           gpu-utilization gpu-memory-used gpu-temperature \
        -c 0.5 -s -l 10000 --top-n-processes 5
    # Without cpu-usage-per-core, because it is too much data for
    just show
