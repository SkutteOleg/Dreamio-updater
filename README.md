# DREAMIO: AI-Powered Adventures - Updater

### This is the updater for [DREAMIO: AI-Powered Adventures](https://dreamio.xyz)

## About

This repository contains the source code for the DREAMIO: AI-Powered Adventures updater. The updater is automatically built and released whenever changes are pushed to the `master` branch.

## Latest Release

You can find the latest release of the DREAMIO updater [here](https://github.com/SkutteOleg/Dreamio-updater/releases/latest).

## SHA256 Verification

Each release includes a SHA256 hash in the release notes. You can use this to verify the integrity of the downloaded updater.

## Manual Build Instructions

To manually build the DREAMIO updater, follow these steps:

1. Install Rust nightly toolchain and the `just` command runner on your Windows machine.

2. Clone this repository and navigate to the project directory.

3. Run the following command to build the updater:
   ```
   just build
   ```

4. After successful compilation, you should find `DreamioUpdater.exe` in the `target/x86_64-pc-windows-msvc/release/` directory.

5. (Optional) Calculate the SHA256 hash of the updater:
   ```
   certutil -hashfile target/x86_64-pc-windows-msvc/release/DreamioUpdater.exe SHA256
   ```

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.