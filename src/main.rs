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
use zip::ZipArchive;

use winapi::um::consoleapi::{GetConsoleMode, SetConsoleMode};
use winapi::um::handleapi::INVALID_HANDLE_VALUE;
use winapi::um::processenv::GetStdHandle;
use winapi::um::winbase::STD_OUTPUT_HANDLE;
use winapi::um::wincon::SetConsoleTitleW;
use winapi::um::wincon::ENABLE_VIRTUAL_TERMINAL_PROCESSING;

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
        .template("[{elapsed_precise}] [{bar:30.cyan/blue}] {bytes}/{total_bytes} (ETA: {eta_precise})")
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

fn get_latest_update_url() -> Result<String, Box<dyn std::error::Error>> {
    let url = "https://games.skutteoleg.com/dreamio/downloads/Builds/Windows/version.json";
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

    let update_url = format!("https://games.skutteoleg.com/dreamio/downloads/Builds/Windows/{}.zip", version_code);

    Ok((version_code, update_url))
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

        let outpath = PathBuf::from(file.name());

        if outpath
            .file_name()
            .map(|f| f == current_exe_name)
            .unwrap_or(false)
        {
            continue;
        }

        if file.name().ends_with('/') {
            if let Err(e) = fs::create_dir_all(&outpath) {
                eprintln!(
                    "\x1b[31mError creating directory {}: {}. Skipping.\x1b[37m",
                    outpath.display(),
                    e
                );
                continue;
            }
        } else if file.name().ends_with(".patch") {
            let original_file = outpath.with_extension("");
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
            let file_to_delete = outpath.with_extension("");
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
            if let Some(parent) = outpath.parent() {
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
            let mut outfile = match File::create(&outpath) {
                Ok(file) => file,
                Err(e) => {
                    eprintln!(
                        "\x1b[31mError creating file {}: {}. Skipping.\x1b[37m",
                        outpath.display(),
                        e
                    );
                    continue;
                }
            };
            if let Err(e) = io::copy(&mut file, &mut outfile) {
                eprintln!(
                    "\x1b[31mError writing to file {}: {}. Skipping.\x1b[37m",
                    outpath.display(),
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
    println!("{}", s!("Cleaning up temporary files..."));
    if let Err(e) = fs::remove_file(&update_zip_path) {
        eprintln!("\x1b[31mFailed to remove update.zip: {}\x1b[37m", e);
    }
    println!("{}", s!("Cleanup completed."));
}

fn wait_for_key_press() {
    println!("\nPress any key to exit...");
    let mut buffer = [0u8; 1];
    std::io::stdin().read_exact(&mut buffer).unwrap();
}

fn main() {
    enable_ansi_support();
    set_window_title("DREAMIO: AI-Powered Adventures - Updater");

    let update_zip_path = PathBuf::from("update.zip");
    let version_file_path = Path::new("version.json");

    goldberg_stmts! {
        println!("{}", s!("\x1b[36m  ___  ___ ___   _   __  __ ___ ___  "));
        println!("{}", s!(" |   \\| _ \\ __| /_\\ |  \\/  |_ _/ _ \\ "));
        println!("{}", s!(" | |) |   / _| / _ \\| |\\/| || | (_) |"));
        println!("{}", s!(" |___/|_|_\\___/_/ \\_\\_|  |_|___\\___/ "));
        println!("{}", s!("                                     "));
        println!("{}", s!(" DREAMIO: AI-Powered Adventures - Updater\n\x1b[37m"));

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

        if !update_zip_path.exists() && !version_file_path.exists() {
            println!("{}", s!("Downloading latest update."));

            match get_latest_update_url() {
                Ok(latest_url) => {
                    match download_file(&latest_url, &update_zip_path) {
                        Ok(_) => {
                            println!("Successfully downloaded latest update.");
                            if let Err(e) = apply_update(&update_zip_path) {
                                eprintln!("\x1b[31mFailed to apply update: {}\x1b[37m", e);
                                cleanup();
                                wait_for_key_press();
                                exit(1);
                            }
                            cleanup();
                        },
                        Err(e) => {
                            eprintln!("\x1b[31mError downloading latest update: {}\x1b[37m", e);
                            wait_for_key_press();
                            exit(1);
                        }
                    }
                },
                Err(e) => {
                    eprintln!("\x1b[31mFailed to get latest update URL: {}\x1b[37m", e);
                    wait_for_key_press();
                    exit(1);
                }
            }
        } else if !update_zip_path.exists() && version_file_path.exists() {
            loop {
                match get_version_info() {
                    Ok((version_code, update_url)) => {
                        println!("Attempting to download update for version {}", version_code);

                        match download_file(&update_url, &update_zip_path) {
                            Ok(_) => {
                                println!("Successfully downloaded update.");
                                if let Err(e) = apply_update(&update_zip_path) {
                                    eprintln!("\x1b[31mFailed to apply update: {}\x1b[37m", e);
                                    cleanup();
                                    wait_for_key_press();
                                    exit(1);
                                }
                                cleanup();

                                match get_version_info() {
                                    Ok((new_version_code, _)) => {
                                        if new_version_code == version_code {
                                            println!("Update complete. No more updates available.");
                                            break;
                                        }
                                    },
                                    Err(e) => {
                                        eprintln!("\x1b[31mFailed to read updated version info: {}\x1b[37m", e);
                                        wait_for_key_press();
                                        exit(1);
                                    }
                                }
                            },
                            Err(e) => {
                                if e.to_string().contains("404") {
                                    println!("No more updates available.");
                                    break;
                                } else {
                                    eprintln!("\x1b[31mError downloading update: {}\x1b[37m", e);
                                    wait_for_key_press();
                                    exit(1);
                                }
                            }
                        }
                    },
                    Err(e) => {
                        eprintln!("\x1b[31mFailed to read version info: {}\x1b[37m", e);
                        wait_for_key_press();
                        exit(1);
                    }
                }
            }
        } else if !update_zip_path.exists() {
            eprintln!("{}", s!("\x1b[31mNo update file (update.zip) found!\x1b[37m"));
            wait_for_key_press();
            exit(1);
        }

        if update_zip_path.exists() {
            if let Err(e) = apply_update(&update_zip_path) {
                eprintln!("\x1b[31mFailed to apply update: {}\x1b[37m", e);
                cleanup();
                wait_for_key_press();
                exit(1);
            }
            cleanup();
        }

        println!("{}", s!("Launching the game..."));
        match Command::new("Dreamio.exe").spawn() {
            Ok(_) => println!("{}", s!("Game launched successfully.")),
            Err(e) => eprintln!("{}", format!("\x1b[31mFailed to restart the game: {}\x1b[37m", e)),
        }

        println!("{}", s!("\x1b[32m\nUpdate process completed!\x1b[37m"));
    }
}
