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
    cargo run -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -o; firefox system_data_plot.html

runs:
    # Well, do not run this to test things, because just runs this in a shell command, that is captured as normal program
    cargo run -- collect -e "FIREFOX|firefox" -e "NEMO|nemo" -c 0.2 -C -s -l 10000; firefox system_data_plot.html

cross_arm_32:
    cargo build --target armv7-unknown-linux-gnueabihf