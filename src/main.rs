use goldberg::{goldberg_stmts, goldberg_string as s};
use qbsdiff::Bspatch;
use reqwest;
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
use winapi::um::wincon::ENABLE_VIRTUAL_TERMINAL_PROCESSING;

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
    let mut response = reqwest::blocking::get(url)?;
    let mut file = File::create(path)?;
    io::copy(&mut response, &mut file)?;
    Ok(())
}

fn apply_update(update_zip_path: &Path) -> io::Result<()> {
    println!("{}", s!("Update file found. Preparing to extract..."));

    let update_zip_data = fs::read(update_zip_path)?;
    let reader = Cursor::new(update_zip_data);
    let mut archive = ZipArchive::new(reader)?;

    println!("{}", s!("Applying update..."));
    let total_files = archive.len();

    let current_exe = env::current_exe()?;
    let current_exe_name = current_exe.file_name().unwrap().to_str().unwrap();

    for i in 0..total_files {
        print!("\r\x1B[2K");
        let progress_percentage = (i + 1) as f32 / total_files as f32 * 100.0;
        print!("Progress: {:.1}%", progress_percentage);
        io::stdout().flush()?;

        let mut file = archive.by_index(i)?;
        let outpath = PathBuf::from(file.name());

        if outpath
            .file_name()
            .map(|f| f == current_exe_name)
            .unwrap_or(false)
        {
            continue;
        }

        if file.name().ends_with('/') {
            fs::create_dir_all(&outpath)?;
        } else if file.name().ends_with(".patch") {
            let original_file = outpath.with_extension("");
            let mut patch_data = Vec::new();
            file.read_to_end(&mut patch_data)?;
            apply_patch(&original_file, &patch_data, &original_file)?;
        } else if file.name().ends_with(".delete") {
            let file_to_delete = outpath.with_extension("");
            if file_to_delete.exists() {
                fs::remove_file(&file_to_delete)?;
            }
        } else {
            if let Some(parent) = outpath.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let mut outfile = File::create(&outpath)?;
            io::copy(&mut file, &mut outfile)?;
        }
    }
    println!("\n{}", s!("Update applied successfully."));
    Ok(())
}

fn main() {
    enable_ansi_support();

    let update_zip_path = PathBuf::from("update.zip");
    let version_file_path = Path::new("version");
    let latest_url = "https://games.skutteoleg.com/dreamio/downloads/Builds/Windows/latest.zip";

    goldberg_stmts! {
        println!("{}", s!("\x1b[34mDREAMIO: AI-Powered Adventures - Updater\n\x1b[37m"));

        println!("{}", s!("Checking for running game process..."));
        let mut processes = sysinfo::System::new_all();
        processes.refresh_all();

        let mut process_to_kill = None;
        for (pid, process) in processes.processes() {
            if process.name() == "Dreamio.exe" {
                println!("{}", s!("Game process found. Shutting down..."));

                if !process.kill() {
                    eprintln!("{}", s!("\x1b[31mError shutting down the game!\x1b[37m"));
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
            println!("{}", s!("No update.zip or version file found. Downloading latest update."));

            match download_file(latest_url, &update_zip_path) {
                Ok(_) => {
                    println!("Successfully downloaded latest update.");
                    if let Err(e) = apply_update(&update_zip_path) {
                        eprintln!("Failed to apply update: {}", e);
                        exit(1);
                    }
                    println!("{}", s!("Cleaning up temporary files..."));
                    if let Err(e) = fs::remove_file(&update_zip_path) {
                        eprintln!("\x1b[31mFailed to remove update.zip: {}\x1b[37m", e);
                    }
                    println!("{}", s!("Cleanup completed."));
                },
                Err(e) => {
                    eprintln!("Error downloading latest update: {}", e);
                    exit(1);
                }
            }
        } else if !update_zip_path.exists() && version_file_path.exists() {
            println!("{}", s!("No update.zip found. Checking for updates based on version file."));

            loop {
                let version = fs::read_to_string(version_file_path)
                    .expect("Failed to read version file");
                let version = version.trim();

                let download_url = format!("https://games.skutteoleg.com/dreamio/downloads/Builds/Windows/{}.zip", version);

                println!("Attempting to download update: {}", download_url);

                match download_file(&download_url, &update_zip_path) {
                    Ok(_) => {
                        println!("Successfully downloaded update.");
                        if let Err(e) = apply_update(&update_zip_path) {
                            eprintln!("Failed to apply update: {}", e);
                            exit(1);
                        }
                        println!("{}", s!("Cleaning up temporary files..."));
                        if let Err(e) = fs::remove_file(&update_zip_path) {
                            eprintln!("\x1b[31mFailed to remove update.zip: {}\x1b[37m", e);
                        }
                        println!("{}", s!("Cleanup completed."));

                        let new_version = fs::read_to_string(version_file_path)
                            .expect("Failed to read updated version file");
                        if new_version.trim() == version {
                            println!("Update complete. No more updates available.");
                            break;
                        }
                    },
                    Err(e) => {
                        if e.to_string().contains("404") {
                            println!("No more updates available.");
                            break;
                        } else {
                            eprintln!("Error downloading update: {}", e);
                            exit(1);
                        }
                    }
                }
            }
        } else if !update_zip_path.exists() {
            eprintln!("{}", s!("\x1b[31m\nNo update file (update.zip) found!\x1b[37m"));
            exit(1);
        }

        if update_zip_path.exists() {
            if let Err(e) = apply_update(&update_zip_path) {
                eprintln!("Failed to apply update: {}", e);
                exit(1);
            }
            println!("{}", s!("Cleaning up temporary files..."));
            if let Err(e) = fs::remove_file(&update_zip_path) {
                eprintln!("\x1b[31mFailed to remove update.zip: {}\x1b[37m", e);
            }
            println!("{}", s!("Cleanup completed."));
        }

        println!("{}", s!("Restarting the game..."));
        match Command::new("Dreamio.exe").spawn() {
            Ok(_) => println!("{}", s!("Game restarted successfully.")),
            Err(e) => eprintln!("{}", format!("\x1b[31mFailed to restart the game: {}\x1b[37m", e)),
        }

        println!("{}", s!("\x1b[32m\nUpdate process completed!\x1b[37m"));
    }
}
