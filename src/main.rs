use goldberg::{goldberg_stmts, goldberg_string as s};
use indicatif::{ProgressBar, ProgressStyle};
use qbsdiff::Bspatch;
use reqwest;
use serde_json::Value;
use std::env;
use std::fs::{self, File};
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use std::thread;
use std::time::Duration;
use sysinfo::{ProcessExt, SystemExt};
use terminal_link::Link;
use zip::ZipArchive;

use winapi::um::consoleapi::{GetConsoleMode, SetConsoleMode};
use winapi::um::handleapi::INVALID_HANDLE_VALUE;
use winapi::um::processenv::GetStdHandle;
use winapi::um::winbase::STD_OUTPUT_HANDLE;
use winapi::um::wincon::{SetConsoleTitleW, ENABLE_VIRTUAL_TERMINAL_PROCESSING};

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

fn set_window_title(title: &str) {
    let wide: Vec<u16> = OsStr::new(title)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        SetConsoleTitleW(wide.as_ptr());
    }
}

fn enable_ansi_support() {
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle == INVALID_HANDLE_VALUE {
            return;
        }
        let mut original_mode: u32 = 0;
        if GetConsoleMode(handle, &mut original_mode) == 0 {
            return;
        }
        let mode = original_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING;
        SetConsoleMode(handle, mode);
    }
}

fn apply_patch(old_file: &Path, patch_data: &[u8], new_file: &Path) -> io::Result<()> {
    let old_contents = fs::read(old_file)?;
    let mut new_contents = Vec::new();

    let patcher = Bspatch::new(patch_data)?;
    patcher.apply(&old_contents, Cursor::new(&mut new_contents))?;

    fs::write(new_file, &new_contents)?;

    Ok(())
}

fn download_file(url: &str, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::new();
    let mut response = client.get(url).send()?;

    if !response.status().is_success() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::Other,
            format!("HTTP error: {}", response.status()),
        )));
    }

    let total_size = response.content_length().unwrap_or(0);

    let pb = ProgressBar::new(total_size);
    pb.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}] [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA: {eta_precise})")
        .progress_chars("=>-"));

    let mut file = File::create(path)?;
    let mut downloaded: u64 = 0;
    let mut buffer = [0; 8192]; // 8KB buffer

    while let Ok(n) = response.read(&mut buffer) {
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n])?;
        downloaded += n as u64;
        pb.set_position(downloaded);
    }

    pb.finish_with_message("Download completed");
    Ok(())
}

fn handle_error(message: &str, error: &dyn std::error::Error) -> ! {
    eprintln!("\x1b[31m{}: {}\x1b[37m", message, error);
    let url = "https://dreamio.xyz/downloads/Builds/Windows/latest.zip";
    let link = Link::new(url, url);
    eprintln!(
        "\x1b[33mPlease download the latest game archive manually: {}\x1b[37m",
        link
    );
    eprintln!("\x1b[33m(Ctrl+Click the link to open)\x1b[37m");
    cleanup();
    wait_for_key_press();
    exit(1);
}

fn download_and_apply_update(
    url: &str,
    update_zip_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    download_file(url, update_zip_path)?;
    println!("Successfully downloaded update.");
    apply_update(update_zip_path)?;
    cleanup();
    Ok(())
}

fn get_latest_update_url() -> Result<String, Box<dyn std::error::Error>> {
    let url = "https://dreamio.xyz/downloads/Builds/Windows/version.json";
    let response = reqwest::blocking::get(url)?;
    let json: Value = response.json()?;

    let update_url = json["latestUrl"]
        .as_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid latestUrl in JSON"))?
        .to_string();

    Ok(update_url)
}

fn get_version_info() -> Result<(String, String), Box<dyn std::error::Error>> {
    let version_file_path = Path::new("version.json");
    let version_content = fs::read_to_string(version_file_path)?;
    let json: Value = serde_json::from_str(&version_content)?;

    let version_code = json["versionCode"]
        .as_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid versionCode in JSON"))?
        .to_string();

    let update_url = format!(
        "https://dreamio.xyz/downloads/Builds/Windows/patches/{}.zip",
        version_code
    );

    Ok((version_code, update_url))
}

fn print_header() {
    println!("{}", s!("\x1b[36m  ___  ___ ___   _   __  __ ___ ___  "));
    println!("{}", s!(" |   \\| _ \\ __| /_\\ |  \\/  |_ _/ _ \\ "));
    println!("{}", s!(" | |) |   / _| / _ \\| |\\/| || | (_) |"));
    println!("{}", s!(" |___/|_|_\\___/_/ \\_\\_|  |_|___\\___/ "));
    println!("{}", s!("                                     "));
    println!(
        "{}",
        s!(" DREAMIO: AI-Powered Adventures - Updater\n\x1b[37m")
    );
}

fn apply_update(update_zip_path: &Path) -> io::Result<()> {
    println!("{}", s!("Update file found. Preparing to extract..."));

    let update_zip_data = fs::read(update_zip_path)?;
    let reader = Cursor::new(update_zip_data);
    let mut archive = ZipArchive::new(reader)?;

    println!("{}", s!("Applying update..."));
    let total_files = archive.len();

    let pb = ProgressBar::new(total_files as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template(
                "[{elapsed_precise}] [{bar:30.cyan/blue}] {pos}/{len} files (ETA: {eta_precise})",
            )
            .progress_chars("=>-"),
    );

    let current_exe = env::current_exe()?;
    let current_exe_name = current_exe.file_name().unwrap().to_str().unwrap();

    for i in 0..total_files {
        pb.set_position(i as u64 + 1);

        let mut file = match archive.by_index(i) {
            Ok(file) => file,
            Err(e) => {
                eprintln!(
                    "\x1b[31mError accessing file in archive: {}. Skipping.\x1b[37m",
                    e
                );
                continue;
            }
        };

        let out_path = PathBuf::from(file.name());

        if out_path
            .file_name()
            .map(|f| f == current_exe_name)
            .unwrap_or(false)
        {
            continue;
        }

        if file.name().ends_with('/') {
            if let Err(e) = fs::create_dir_all(&out_path) {
                eprintln!(
                    "\x1b[31mError creating directory {}: {}. Skipping.\x1b[37m",
                    out_path.display(),
                    e
                );
                continue;
            }
        } else if file.name().ends_with(".patch") {
            let original_file = out_path.with_extension("");
            let mut patch_data = Vec::new();
            if let Err(e) = file.read_to_end(&mut patch_data) {
                eprintln!(
                    "\x1b[31mError reading patch data for {}: {}. Skipping.\x1b[37m",
                    original_file.display(),
                    e
                );
                continue;
            }
            if let Err(e) = apply_patch(&original_file, &patch_data, &original_file) {
                eprintln!(
                    "\x1b[31mError applying patch to {}: {}. Skipping.\x1b[37m",
                    original_file.display(),
                    e
                );
                continue;
            }
        } else if file.name().ends_with(".delete") {
            let file_to_delete = out_path.with_extension("");
            if file_to_delete.exists() {
                if let Err(e) = fs::remove_file(&file_to_delete) {
                    eprintln!(
                        "\x1b[31mError deleting file {}: {}. Skipping.\x1b[37m",
                        file_to_delete.display(),
                        e
                    );
                }
            }
        } else {
            if let Some(parent) = out_path.parent() {
                if !parent.exists() {
                    if let Err(e) = fs::create_dir_all(parent) {
                        eprintln!(
                            "\x1b[31mError creating directory {}: {}. Skipping.\x1b[37m",
                            parent.display(),
                            e
                        );
                        continue;
                    }
                }
            }
            let mut outfile = match File::create(&out_path) {
                Ok(file) => file,
                Err(e) => {
                    eprintln!(
                        "\x1b[31mError creating file {}: {}. Skipping.\x1b[37m",
                        out_path.display(),
                        e
                    );
                    continue;
                }
            };
            if let Err(e) = io::copy(&mut file, &mut outfile) {
                eprintln!(
                    "\x1b[31mError writing to file {}: {}. Skipping.\x1b[37m",
                    out_path.display(),
                    e
                );
                continue;
            }
        }
    }

    pb.finish_with_message("Update applied successfully.");
    Ok(())
}

fn cleanup() {
    let update_zip_path = PathBuf::from("update.zip");
    if update_zip_path.exists() {
        println!("{}", s!("Cleaning up temporary files..."));
        if let Err(e) = fs::remove_file(&update_zip_path) {
            eprintln!("\x1b[31mFailed to remove update.zip: {}\x1b[37m", e);
        }
        println!("{}", s!("Cleanup completed."));
    }
}

fn wait_for_key_press() {
    println!("\nPress any key to exit...");
    let mut buffer = [0u8; 1];
    io::stdin().read_exact(&mut buffer).unwrap();
}

fn main() {
    enable_ansi_support();
    set_window_title("DREAMIO: AI-Powered Adventures - Updater");

    let update_zip_path = PathBuf::from("update.zip");
    let version_file_path = Path::new("version.json");

    goldberg_stmts! {
        print_header();

        println!("{}", s!("Checking for running game process..."));
        let mut processes = sysinfo::System::new_all();
        processes.refresh_all();

        let mut process_to_kill = None;
        for (pid, process) in processes.processes() {
            if process.name() == "Dreamio.exe" {
                println!("{}", s!("Game process found. Shutting down..."));

                if !process.kill() {
                    eprintln!("{}", s!("\x1b[31mError shutting down the game!\x1b[37m"));
                    wait_for_key_press();
                    exit(1);
                }
                process_to_kill = Some(*pid);
                break;
            }
        }

        if let Some(pid_to_kill) = process_to_kill {
            println!("{}", s!("Waiting for game process to fully terminate..."));
            let mut is_process_running = true;
            while is_process_running {
                thread::sleep(Duration::from_millis(100));
                processes.refresh_processes();
                is_process_running = processes.processes().iter().any(|(p, _)| *p == pid_to_kill);
            }
            println!("{}", s!("Game process terminated successfully."));
        } else {
            println!("{}", s!("No running game process found."));
        }

        if update_zip_path.exists() {
            if let Err(e) = apply_update(&update_zip_path) {
                handle_error("Failed to apply update", &e);
            }
            cleanup();
        }

        if !version_file_path.exists() {
            println!("{}", s!("Downloading latest update."));
            match get_latest_update_url() {
                Ok(latest_url) => {
                    if let Err(e) = download_and_apply_update(&latest_url, &update_zip_path) {
                        handle_error("Failed to download or apply update", &*e);
                    }
                }
                Err(e) => handle_error("Failed to get latest update URL", &*e),
            }
        }

        loop {
            match get_version_info() {
                Ok((version_code, update_url)) => {
                    println!("Attempting to download update for version {}", version_code);
                    match download_and_apply_update(&update_url, &update_zip_path) {
                        Ok(_) => {
                            match get_version_info() {
                                Ok((new_version_code, _)) => {
                                    if new_version_code == version_code {
                                        println!("Update complete. No more updates available.");
                                        break;
                                    }
                                }
                                Err(e) => {
                                    handle_error("Failed to read updated version info", &*e)
                                }
                            }
                        }
                        Err(e) => {
                            if e.to_string().contains("404") {
                                println!("No more updates available.");
                                break;
                            } else {
                                handle_error("Error downloading update", &*e);
                            }
                        }
                    }
                }
                Err(e) => handle_error("Failed to read version info", &*e),
            }
        }

        println!("{}", s!("Launching the game..."));
        match Command::new("Dreamio.exe").spawn() {
            Ok(_) => println!("{}", s!("Game launched successfully.")),
            Err(e) => eprintln!("{}", format!("\x1b[31mFailed to restart the game: {}\x1b[37m", e)),
        }

        println!("{}", s!("\x1b[32m\nUpdate process completed!\x1b[37m"));
    }
}
