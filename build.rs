use std::process::Command;
use std::env;

fn main() {
    if cfg!(target_os = "windows") {
        let out_dir = env::var("OUT_DIR").unwrap();
        let status = Command::new("windres")
            .args(&["src/DreamioUpdater.rc", "-o"])
            .arg(&format!("{}/DreamioUpdater.o", out_dir))
            .status()
            .unwrap();
        assert!(status.success(), "Failed to run windres");
        println!("cargo:rustc-link-search=native={}", out_dir);
        println!("cargo:rustc-link-arg={}/DreamioUpdater.o", out_dir);
    }
}