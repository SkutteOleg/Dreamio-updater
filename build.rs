fn main() {
    if cfg!(target_os = "windows") {
        println!("cargo:rustc-link-arg=.\\DreamioUpdater.res");
    }
}