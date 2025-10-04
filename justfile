build:
    RUSTFLAGS="-Zlocation-detail=none -Zunstable-options -Cpanic=immediate-abort" cargo +nightly build -Z build-std=std,panic_abort -Z build-std-features="optimize_for_size" --target x86_64-pc-windows-msvc --release
