build:
    RUST_BACKTRACE=full RUSTFLAGS="-Zlocation-detail=none" cargo +nightly build -Z build-std=std,panic_abort -Z build-std-features="optimize_for_size" -Z build-std-features=panic_immediate_abort --target x86_64-pc-windows-gnu --release
    upx --best --lzma target/x86_64-pc-windows-gnu/release/DreamioUpdater.exe
