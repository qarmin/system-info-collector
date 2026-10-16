# Examples:
#   just full_install
#   just full_install "cpu-usage-total memory-used memory-free memory-available network-rx network-tx network-total gpu-utilization gpu-memory-used gpu-temperature disk-used disk-available disk-busy" "/ /home"
#   just full_send 192.168.1.50
#   just full_send 192.168.1.50 "cpu-usage-total memory-used" "/"
#   just full_send_arm 192.168.1.60 "cpu-usage-total memory-used" "" system-info-collector-arm.service
#
# Recipes are grouped - `just --groups` lists the groups, `just --list` shows them section by
# section, `just --list --group <name>` shows one group on its own:
#
#   setup    checking and installing the tools the other recipes need
#   build    compiling for this machine and for other targets
#   dev      formatting, lints, dependency updates
#   release  cargo install here, cargo publish to crates.io
#   run      collecting on this machine
#   session  recording every process for a short period
#   plots    turning collected CSVs into HTML reports
#   deploy   installing the systemd service here or on another machine
#
# Notes:
#   - `metrics` is space-separated (matches the CLI's `-m` flag), not comma-separated.
#   - `disks` is space-separated mount points/devices for --disk, e.g. "/ /home" - only
#     takes effect if `metrics` includes a disk-* metric. The special value "all"
#     means --all-disks (every real, non-virtual disk, discovered at service start).
#   - See `default_metrics`/`default_disks` below for the full list of valid metrics values.
#   - `processes` is the repeated `-e NAME|SEARCH_TEXT` flags; see `arm_processes` below.
#   - "all" skips virtual filesystems and boot partitions; add `--exclude-disk <mount>` to the
#     service ExecStart to drop anything else.
#   - The send/install recipes grant the binary the capabilities the `session` command needs to
#     read every process's I/O. See `session_caps` below; `just grant_caps <ip>` reapplies them
#     on their own if a binary was copied over some other way.

user := `whoami`

zig_version := "0.16.0"

# cargo-zigbuild writes missing glibc headers (arm-features.h for the .2.28 ARM target) into
# zig's own lib dir, so a read-only zig such as the snap fails there. `just setup` puts a
# writable one in ~/.cargo/bin, and it wins here even when another zig comes first in PATH.
export CARGO_ZIGBUILD_ZIG_PATH := env("CARGO_ZIGBUILD_ZIG_PATH", if path_exists(home_directory() / ".cargo/bin/zig") == "true" { home_directory() / ".cargo/bin/zig" } else { "zig" })

# ─── setup: checking and installing the tools the other recipes need ─────────

# Read-only; exits non-zero when a required tool is missing. Never calls sudo - system
# packages are printed as an install command instead.
[doc('Checks that the tools the build and deploy recipes need are installed')]
[group('setup')]
doctor:
    #!/usr/bin/env bash
    set +e
    fail=0
    check() {
        local name="$1"; shift
        local out
        if out="$("$@" 2>&1)"; then
            printf "  [OK]   %-36s %s\n" "$name" "$(head -n1 <<<"$out")"
        else
            printf "  [BRAK] %s\n" "$name"
            fail=$((fail + 1))
        fi
    }
    info() {
        local name="$1"; shift
        local out
        if out="$("$@" 2>&1)"; then
            printf "  [OK]   %-36s %s\n" "$name" "$(head -n1 <<<"$out")"
        else
            printf "  [-]    %s (opcjonalne)\n" "$name"
        fi
    }
    zig_lib_writable() {
        local lib
        lib="$("$CARGO_ZIGBUILD_ZIG_PATH" env | grep -oE 'lib_dir"?[ :=]+"[^"]+"' | sed -E 's/.*"([^"]+)"$/\1/')"
        [ -n "$lib" ] && [ -w "$lib" ] && echo "$lib"
    }

    echo "Toolchain Rust:"
    check "rustc"                                rustc --version
    check "cargo"                                cargo --version
    check "clippy"                               cargo clippy --version
    check "rustfmt (nightly, do 'just fix')"     cargo +nightly fmt --version

    echo ""
    echo "Cross-kompilacja (cross_*, *_send, full_*):"
    check "cargo-zigbuild"                       bash -c "command -v cargo-zigbuild"
    check "zig ($CARGO_ZIGBUILD_ZIG_PATH)"       "$CARGO_ZIGBUILD_ZIG_PATH" version
    check "zig lib zapisywalny"                  zig_lib_writable
    check "target armv7-unknown-linux-gnueabihf" bash -c "rustup target list --installed | grep -qx armv7-unknown-linux-gnueabihf"
    check "target aarch64-unknown-linux-gnu"     bash -c "rustup target list --installed | grep -qx aarch64-unknown-linux-gnu"
    check "target x86_64-unknown-linux-gnu"      bash -c "rustup target list --installed | grep -qx x86_64-unknown-linux-gnu"

    echo ""
    echo "Narzędzia opcjonalne:"
    info "cargo-edit (do 'just upgrade')"        cargo upgrade --version
    info "samply (do 'just samplyrd')"           samply --version
    info "setcap (do 'session_root', full_install)" bash -c "command -v setcap"

    echo ""
    if [ "$fail" -eq 0 ]; then
        echo "Wszystko OK."
    else
        echo "$fail wymaganych narzędzi brakuje - uruchom 'just setup', żeby je zainstalować."
        exit 1
    fi

# Only Rust-toolchain-level and user-local things; no sudo.
[doc('Installs what `doctor` reports missing - rustup parts, cargo-zigbuild, zig into ~/.cargo')]
[group('setup')]
setup:
    #!/usr/bin/env bash
    set -e
    rustup component add clippy
    rustup toolchain install nightly --profile minimal --component rustfmt
    rustup target add armv7-unknown-linux-gnueabihf aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu
    command -v cargo-zigbuild >/dev/null 2>&1 || cargo install cargo-zigbuild --locked

    # Installed even when another zig exists, since that one may be read-only (snap);
    # CARGO_ZIGBUILD_ZIG_PATH above picks this one. Pinned to x86_64-linux hosts.
    zig_home="$HOME/.cargo/zig-x86_64-linux-{{ zig_version }}"
    if [ ! -x "$zig_home/zig" ]; then
        curl -fsSL "https://ziglang.org/download/{{ zig_version }}/zig-x86_64-linux-{{ zig_version }}.tar.xz" \
            | tar -C "$HOME/.cargo" -xJf -
    fi
    mkdir -p "$HOME/.cargo/bin"
    ln -sf "$zig_home/zig" "$HOME/.cargo/bin/zig"

    echo "Gotowe - uruchom 'just doctor', żeby zweryfikować."

# ─── build: compiling for this machine and for other targets ─────────────────

[doc('Release + debug build, clippy and the test suite')]
[group('build')]
build_all:
    cargo build --release
    cargo build
    cargo clippy
    cargo test

[doc('Cross-compiles for 32-bit ARM (armv7 hard-float)')]
[group('build')]
cross_arm_32:
    RUSTFLAGS="" cargo zigbuild --target armv7-unknown-linux-gnueabihf -p system_info_collector

[doc('Cross-compiles for 64-bit ARM (aarch64)')]
[group('build')]
cross_arm_64:
    RUSTFLAGS="" cargo zigbuild --target aarch64-unknown-linux-gnu -p system_info_collector

[doc('Cross-compiles for x86_64 against an older glibc')]
[group('build')]
cross_x86_64:
    # To avoid glibc version issues on older target distros, using zigbuild against an older glibc version
    RUSTFLAGS="" cargo zigbuild --target x86_64-unknown-linux-gnu.2.28 -p system_info_collector

# ─── dev: formatting, lints and dependencies ─────────────────────────────────

[doc('Formats, applies clippy fixes, formats again')]
[group('dev')]
fix:
    cargo +nightly fmt
    cargo clippy --fix --allow-dirty --allow-staged --all-targets --all-features
    cargo +nightly fmt

# `-i` also raises the requirements written in Cargo.toml, not just the lockfile,
# so a semver-breaking bump is actually picked up. Needs cargo-edit.
[doc('Updates dependencies, including semver-breaking ones')]
[group('dev')]
upgrade:
    cargo upgrade -i
    cargo update

# Verifies that every file pulled in with include_str! is actually in the repository.
# Embedding a file that .gitignore excludes compiles here and nowhere else, which is
# how session_viewer.html once shipped broken. Part of `cargo test`, so `build_all`
# already covers it; this is for running it on its own.
[doc('Checks that every include_str! target is tracked by git')]
[group('dev')]
check_embedded:
    cargo test --test embedded_files_are_committed -- --nocapture

# ─── release: putting the binary on this machine and on crates.io ────────────

[doc('cargo install from this checkout, into ~/.cargo/bin')]
[group('release')]
install:
    cargo install --path crates/system_info_collector

[doc('Publishes both crates to crates.io (core first)')]
[group('release')]
publish:
    cd crates/system_info_collector_core && cargo publish --dry-run
    cd crates/system_info_collector_core && cargo publish
    cd crates/system_info_collector && cargo publish --dry-run
    cd crates/system_info_collector && cargo publish

# ─── run: collecting on this machine ─────────────────────────────────────────

[doc('Collects with tracked processes, then converts and opens the plot')]
[group('run')]
run:
    # Well, do not run this to test things, because just runs this in a shell command, that is captured as normal program
    cargo run -p system_info_collector -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -o --all-networks; firefox system_data_plot.html

[doc('Same as `run`, with the live web server on as well')]
[group('run')]
runs:
    # Well, do not run this to test things, because just runs this in a shell command, that is captured as normal program
    cargo run -p system_info_collector -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -s --all-networks; firefox system_data_plot.html

[doc('Same as `run`, release build')]
[group('run')]
runr:
    # Well, do not run this to test things, because just runs this in a shell command, that is captured as normal program
    cargo run --release -p system_info_collector -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -o --all-networks; firefox system_data_plot.html

[doc('Same as `runs`, release build')]
[group('run')]
runrs:
    # Well, do not run this to test things, because just runs this in a shell command, that is captured as normal program
    cargo run --release -p system_info_collector -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -s --all-networks; firefox system_data_plot.html

[doc('Every metric at once, with the live server - deletes local CSVs first')]
[group('run')]
all:
    rm *.csv || true
    # Run with all collection modes enabled + HTTP live data server
    cargo run --release -p system_info_collector -- collect \
        -m cpu-usage-total  \
           memory-used memory-free memory-available \
           swap-used swap-free \
           network-rx network-tx network-total \
           gpu-utilization gpu-memory-used gpu-temperature \
           disk-used disk-available disk-busy disk-read disk-write \
           cpu-usage-per-core \
        --all-networks --all-disks -e "NEMO|nemo" \
        -c 0.5 -s
    just show

[doc('The metrics worth having day to day, with the live server')]
[group('run')]
normal:
    rm *.csv || true
    # All without cpu-usage-per-core and swap, because it is not needed for normal usage and takes more resources to collect and plot
    cargo run --release -p system_info_collector -- collect \
        -m cpu-usage-total  \
           memory-used memory-free memory-available \
           network-rx network-tx network-total \
           gpu-utilization gpu-memory-used gpu-temperature \
           disk-used disk-available disk-busy disk-read disk-write \
        --all-networks --all-disks -e "GNOME SHELL|gnome-shell" \
        -c 0.5 -s

[doc('Per-core CPU only - the most expensive metric, for measuring overhead')]
[group('run')]
heavy:
    rm *.csv || true
    cargo run --release -p system_info_collector -- collect \
        -m cpu-usage-total cpu-usage-per-core -c 2.0 -s

[doc('Profiles the collector under samply (rdebug profile)')]
[group('run')]
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
        -c 0.5 -s

# Prints detected CPU, memory, disks (with a live busy%/throughput sample), network interfaces and
# GPUs (with one live sample per GPU), reusing the same discovery/monitoring code the collector uses.
# Handy for verifying GPU detection (e.g. AMD) on a machine without re-running the full collector.
[doc('Prints the CPU, memory, disks, interfaces and GPUs the collector detects')]
[group('run')]
system-check:
    cargo run -p system_info_collector_core --example system_check

# ─── session: recording every process for a short period ─────────────────────

[doc('Records every process for N seconds and opens the viewer')]
[group('session')]
session duration="60":
    cargo run --release -p system_info_collector -- session --duration {{ duration }} --open

# A recording made without the capabilities below sees /proc/<pid>/io for this
# user's own processes only, so every daemon and kernel thread reads as having
# done no I/O. `cargo run` builds a fresh binary each time, which drops the
# xattrs, hence granting them here rather than once.
[doc('Records every process as root, so every process has I/O counters')]
[group('session')]
session_root duration="60":
    cargo build --release -p system_info_collector
    sudo setcap {{ session_caps }} target/release/system_info_collector
    target/release/system_info_collector session --duration {{ duration }} --open

# ─── plots: turning collected CSVs into reports ──────────────────────────────

[doc('Converts system_data.csv into plot.html and opens it')]
[group('plots')]
show:
    rm *.html || true
    cargo run --release -p system_info_collector -- convert -d system_data.csv -p plot.html -o
    firefox plot.html

[doc('Fetches data.csv from an ARM device and plots it')]
[group('plots')]
show_data ip_address:
    scp -O root@{{ ip_address }}:/home/root/data_collector/data.csv .
    cargo run --release -p system_info_collector -- convert -d data.csv -p plot.html -o

[doc('Plots a local data.csv, one report per calendar day')]
[group('plots')]
show_data_custom ip_address path:
    #scp -O {{ ip_address }}:{{ path }} .
    cargo run --release -p system_info_collector -- convert -d data.csv -p plot.html -o --split-mode per-day

[doc('Deletes the CSVs in the project root')]
[group('plots')]
cleancsv:
    rm *.csv

# ─── deploy: shipping the binary and the systemd service ─────────────────────

# Capabilities the session recorder needs to read every process's /proc/<pid>/io.
#
# Without them a recording made as an ordinary user gets I/O counters for that
# user's own processes only - no system daemons, no kernel threads, so none of the
# writeback that buffered writes are charged to. Those rows come back as zero,
# which is indistinguishable from "wrote nothing", so attribution silently covers
# a couple of percent of the machine.
#
# cap_dac_read_search opens the 0400 file, cap_sys_ptrace satisfies the ptrace
# access check the kernel additionally applies to it - both are required, either
# alone still fails.
#
# These are xattrs on the binary, so they do not survive scp and the send recipes
# reapply them after every deploy. They are also ignored on a filesystem mounted
# nosuid and cannot be stored at all on one without xattr support.
#
# cap_net_raw is for the ping-latency/ping-loss metrics, which need a raw ICMP socket
# wherever net.ipv4.ping_group_range does not allow unprivileged ones.
session_caps := "cap_dac_read_search,cap_sys_ptrace,cap_net_raw+ep"

# Defaults for the `metrics`/`disks` arguments on full_send_arm/full_send/full_install below.
# memory-free is deliberately absent: on Linux it counts out page cache, so it reads far
# lower than the memory actually obtainable - memory-available is the useful figure.
default_metrics := "cpu-usage-total memory-used memory-available network-rx network-tx gpu-utilization gpu-memory-used disk-used disk-available disk-busy disk-read disk-write ping-latency"
# Space-separated mount points/device names to pass as repeated --disk flags, e.g. "/ /home".
# "all" means --all-disks (every real disk, discovered at service start); empty means no disk tracked.
default_disks := "all"
# Repeated `-e NAME|SEARCH_TEXT` flags tracking named processes. Only the perimeter ARM devices
# run these binaries, so full_send/full_install leave it empty and full_send_arm defaults to it.
arm_processes := '-e "GUI|./ap600_gui -platform wayland" -e "CORE|5000 --timeout" -e "HAL|5001 --timeout" -e "DBCORE|./dbcore"'

# Reapply the session capabilities on a remote machine, e.g. after a hand-copied binary.
[group('deploy')]
grant_caps ip_address:
    ssh -t {{ user }}@{{ ip_address }} 'sudo setcap {{ session_caps }} /home/{{ user }}/data_collector/system_info_collector && getcap /home/{{ user }}/data_collector/system_info_collector'

# Same, for a binary installed by full_install on this machine.
[group('deploy')]
grant_caps_local:
    sudo setcap {{ session_caps }} /home/{{ user }}/data_collector/system_info_collector
    getcap /home/{{ user }}/data_collector/system_info_collector

[doc('Stops the service on an ARM perimeter device (root)')]
[group('deploy')]
stop_remote ip_address:
    ssh root@{{ ip_address }} 'systemctl stop system-info-collector' || true
    ssh root@{{ ip_address }} 'pkill -f /home/root/data_collector/system_info_collector' || true

# The x86_64 target is a regular user's machine, not the root-only ARM perimeter device, so it connects as $USER and needs sudo for service management
[doc('Stops the service on an x86_64 machine reachable as $USER')]
[group('deploy')]
stop_remote_x86_64 ip_address:
    ssh -t {{ user }}@{{ ip_address }} 'sudo systemctl stop system-info-collector' || true
    ssh {{ user }}@{{ ip_address }} 'pkill -f /home/{{ user }}/data_collector/system_info_collector' || true

[doc('Builds for the ARM device (32/64-bit detected over ssh) and copies the binary over, without touching the service')]
[group('deploy')]
arm_send ip_address:
    #!/usr/bin/env bash
    set -euo pipefail
    target=$(just _arm_target {{ ip_address }})
    echo "Detected $target on {{ ip_address }}"
    # To avoid glibc version issues, using zigbuild
    RUSTFLAGS="" cargo zigbuild --release --target "$target.2.28" -p system_info_collector
    ssh root@{{ ip_address }} 'mkdir -p /home/root/data_collector'
    just stop_remote {{ ip_address }}
    # Removing the old binary, because scp into a still running one fails with "Text file busy"
    ssh root@{{ ip_address }} 'rm -f /home/root/data_collector/system_info_collector' || true
    scp -O "target/$target/release/system_info_collector" root@{{ ip_address }}:/home/root/data_collector/system_info_collector

# Checks the glibc loader rather than `uname -m`, because a 64-bit kernel can run a 32-bit userland
_arm_target ip_address:
    #!/usr/bin/env bash
    set -euo pipefail
    arch=$(ssh root@{{ ip_address }} 'if [ -e /lib/ld-linux-aarch64.so.1 ]; then echo aarch64; elif [ -e /lib/ld-linux-armhf.so.3 ]; then echo armhf; else uname -m; fi')
    case "$arch" in
        aarch64) echo aarch64-unknown-linux-gnu ;;
        armhf|armv7l) echo armv7-unknown-linux-gnueabihf ;;
        *) echo "Unsupported architecture on {{ ip_address }}: $arch" >&2; exit 1 ;;
    esac

[doc('Builds for x86_64 and copies the binary over, without touching the service')]
[group('deploy')]
x86_64_send ip_address:
    # To avoid glibc version issues on older target distros, using zigbuild against an older glibc version
    RUSTFLAGS="" cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.28 -p system_info_collector
    ssh {{ user }}@{{ ip_address }} 'mkdir -p /home/{{ user }}/data_collector'
    # Uploading under a temp name and renaming into place atomically, instead of stopping the service first, so scp never hits "Text file busy" and no sudo/password is needed here
    scp -O target/x86_64-unknown-linux-gnu/release/system_info_collector {{ user }}@{{ ip_address }}:/home/{{ user }}/data_collector/system_info_collector.new
    ssh {{ user }}@{{ ip_address }} 'mv -f /home/{{ user }}/data_collector/system_info_collector.new /home/{{ user }}/data_collector/system_info_collector'

# metrics values: cpu-usage-total cpu-usage-per-core swap-free swap-used memory-used memory-free memory-available network-rx network-tx network-total gpu-utilization gpu-memory-used gpu-temperature disk-used disk-available disk-busy disk-read disk-write ping-latency ping-loss
# disks: space-separated mount points/devices for --disk, e.g. disks="/ /home" - only matters if metrics includes a disk-* metric
[doc('Builds, ships and starts the service on an ARM perimeter device (root, 32/64-bit detected over ssh)')]
[group('deploy')]
full_send_arm ip_address metrics=default_metrics disks=default_disks service_file="system-info-collector.service" processes=arm_processes:
    ssh root@{{ ip_address }} 'systemctl disable system-info-collector' || true
    just stop_remote {{ ip_address }}
    just arm_send {{ ip_address }}
    if [ "{{ disks }}" = "all" ]; then disk_flags="--all-disks"; \
    else disk_flags=""; for d in {{ disks }}; do disk_flags="$disk_flags --disk $d"; done; fi; \
    processes='{{ processes }}'; \
    sed -e 's/__USER__/root/g' -e 's/__METRICS__/{{ metrics }}/g' -e "s#__DISKS__#$disk_flags#g" -e "s#__PROCESSES__#$processes#g" "{{ service_file }}" > /tmp/system-info-collector.service
    scp -O /tmp/system-info-collector.service root@{{ ip_address }}:/etc/systemd/system/system-info-collector.service
    # That service runs as root, so it can already read every process; this is only
    # for running `session` by hand as a non-root user there, and squashfs/jffs2 roots
    # have no xattrs to store capabilities in, hence the tolerated failure.
    ssh root@{{ ip_address }} 'setcap {{ session_caps }} /home/root/data_collector/system_info_collector && getcap /home/root/data_collector/system_info_collector' || echo "setcap unavailable on this target - the service runs as root anyway"
    ssh root@{{ ip_address }} 'systemctl daemon-reload && systemctl enable system-info-collector && systemctl restart system-info-collector || exit 1; cat /home/root/data_collector/data.csv; systemctl status --no-pager system-info-collector; true'

# Unlike full_send_arm (root, perimeter device), this targets a normal x86_64 machine reachable as the local $USER account,
# so the service file's __USER__ placeholder is filled in with that username instead of root before it's shipped over.
# All sudo steps are bundled into a single `ssh -t` call so the remote password is only asked once.
# metrics values: cpu-usage-total cpu-usage-per-core swap-free swap-used memory-used memory-free memory-available network-rx network-tx network-total gpu-utilization gpu-memory-used gpu-temperature disk-used disk-available disk-busy disk-read disk-write ping-latency ping-loss
# disks: space-separated mount points/devices for --disk, e.g. disks="/ /home" - only matters if metrics includes a disk-* metric
[doc('Builds, ships and starts the service on an x86_64 machine ($USER + sudo)')]
[group('deploy')]
full_send ip_address metrics=default_metrics disks=default_disks service_file="system-info-collector.service" processes="":
    if [ "{{ disks }}" = "all" ]; then disk_flags="--all-disks"; \
    else disk_flags=""; for d in {{ disks }}; do disk_flags="$disk_flags --disk $d"; done; fi; \
    processes='{{ processes }}'; \
    sed -e 's/__USER__/{{ user }}/g' -e 's/__METRICS__/{{ metrics }}/g' -e "s#__DISKS__#$disk_flags#g" -e "s#__PROCESSES__#$processes#g" "{{ service_file }}" > /tmp/system-info-collector.service
    just x86_64_send {{ ip_address }}
    scp -O /tmp/system-info-collector.service {{ user }}@{{ ip_address }}:/tmp/system-info-collector.service
    ssh -t {{ user }}@{{ ip_address }} 'sudo mv /tmp/system-info-collector.service /etc/systemd/system/system-info-collector.service && sudo setcap {{ session_caps }} /home/{{ user }}/data_collector/system_info_collector && getcap /home/{{ user }}/data_collector/system_info_collector && sudo systemctl daemon-reload && sudo systemctl enable system-info-collector && sudo systemctl restart system-info-collector && sudo systemctl status --no-pager system-info-collector'
    ssh {{ user }}@{{ ip_address }} 'cat /home/{{ user }}/data_collector/data.csv' || true

# Local equivalent of full_send: builds, installs, and starts the systemd service on this machine, no ssh involved.
# Uses cargo zigbuild against the same older glibc target as full_send/x86_64_send, so it's the exact same binary either way ships.
# metrics values: cpu-usage-total cpu-usage-per-core swap-free swap-used memory-used memory-free memory-available network-rx network-tx network-total gpu-utilization gpu-memory-used gpu-temperature disk-used disk-available disk-busy disk-read disk-write ping-latency ping-loss
# disks: defaults to "all" (--all-disks, every real disk discovered at service start); pass
# space-separated mount points/devices instead, e.g. disks="/ /home", to pick them by hand
[doc('Builds, installs and starts the service on this machine, no ssh involved')]
[group('deploy')]
full_install metrics=default_metrics disks=default_disks service_file="system-info-collector.service" processes="":
    RUSTFLAGS="" cargo zigbuild --release --target x86_64-unknown-linux-gnu.2.28 -p system_info_collector
    mkdir -p /home/{{ user }}/data_collector
    # Copying under a temp name and renaming into place atomically (like x86_64_send), since the
    # service may currently be running the old binary and a direct overwrite fails with "Text file busy"
    cp target/x86_64-unknown-linux-gnu/release/system_info_collector /home/{{ user }}/data_collector/system_info_collector.new
    mv -f /home/{{ user }}/data_collector/system_info_collector.new /home/{{ user }}/data_collector/system_info_collector
    /home/{{ user }}/data_collector/system_info_collector collect --list-disks
    if [ "{{ disks }}" = "all" ]; then disk_flags="--all-disks"; \
    else disk_flags=""; for d in {{ disks }}; do disk_flags="$disk_flags --disk $d"; done; fi; \
    processes='{{ processes }}'; \
    sed -e 's/__USER__/{{ user }}/g' -e 's/__METRICS__/{{ metrics }}/g' -e "s#__DISKS__#$disk_flags#g" -e "s#__PROCESSES__#$processes#g" "{{ service_file }}" > /tmp/system-info-collector.service
    sudo cp /tmp/system-info-collector.service /etc/systemd/system/system-info-collector.service
    sudo setcap {{ session_caps }} /home/{{ user }}/data_collector/system_info_collector
    getcap /home/{{ user }}/data_collector/system_info_collector
    sudo systemctl daemon-reload
    sudo systemctl enable system-info-collector
    sudo systemctl restart system-info-collector
    sudo systemctl status --no-pager system-info-collector

# ─── access: ssh key for reaching a device without a password ────────────────

claude_key := "tmp/claude_tmp_key"

[doc('Creates an ed25519 ssh key in tmp/ unless one is already there')]
[group('access')]
generate_key:
    mkdir -p tmp
    [ -f {{ claude_key }} ] || ssh-keygen -t ed25519 -N "" -C "claude-tmp" -f {{ claude_key }}
    cat {{ claude_key }}.pub

# sshd on older devices rejects the `expiry-time` option, so expiry is a root crontab entry on the device that
# checks every minute and deletes the key (and itself) once the deadline passes. Needs a running crond and `crontab`
# (busybox has both). Re-running replaces the previous key line and deadline.
[doc('Installs the tmp/ public key on a device (root by default), auto-removed after `lifetime` (a `date -d` span, e.g. "30 days" or "1 minute")')]
[group('access')]
send_key ip_address remote_user="root" lifetime="30 days": generate_key
    #!/usr/bin/env bash
    set -e
    deadline="$(date -d '+{{ lifetime }}' +%s)"
    blob="$(cut -d' ' -f2 {{ claude_key }}.pub)"
    key="$(cat {{ claude_key }}.pub)"
    cron_line="* * * * * [ \$(date +\%s) -ge $deadline ] && sed -i \"\\\\,$blob,d\" \$HOME/.ssh/authorized_keys && (crontab -l | grep -v claude-tmp-expiry | crontab -) # claude-tmp-expiry"
    ssh {{ remote_user }}@{{ ip_address }} 'sh -s' <<EOF
    mkdir -p ~/.ssh && chmod 700 ~/.ssh && touch ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys
    sed -i '\\|$blob|d' ~/.ssh/authorized_keys
    echo '$key' >> ~/.ssh/authorized_keys
    (crontab -l 2>/dev/null | grep -v claude-tmp-expiry; echo '$cron_line') | crontab -
    crontab -l | grep claude-tmp-expiry
    EOF
    echo "key valid until $(date -d @$deadline)"
    ssh -i {{ claude_key }} -o IdentitiesOnly=yes -o BatchMode=yes {{ remote_user }}@{{ ip_address }} true \
        && echo "key login works: ssh -i {{ claude_key }} {{ remote_user }}@{{ ip_address }}" \
        || echo "key login failed"

[doc('Removes the tmp/ public key and its expiry job from a device')]
[group('access')]
revoke_key ip_address remote_user="root":
    #!/usr/bin/env bash
    set -e
    blob="$(cut -d' ' -f2 {{ claude_key }}.pub)"
    ssh {{ remote_user }}@{{ ip_address }} 'sh -s' <<EOF
    sed -i '\\|$blob|d' ~/.ssh/authorized_keys
    (crontab -l 2>/dev/null | grep -v claude-tmp-expiry) | crontab -
    echo key removed
    EOF
