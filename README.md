## 1. Build

```
cargo +nightly build --target x86_64-pc-windows-msvc --release
```

## 2. Minify
```
upx --best --lzma DreamioUpdater.exe
```