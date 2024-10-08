use goldberg::{goldberg_stmts, goldberg_string as s};
use std::fs::{self, File};
use std::io::{self, Write, Read};
use std::path::{Path, PathBuf};
use std::process::{exit, Command};
use std::thread;
use std::time::Duration;
use sysinfo::{ProcessExt, SystemExt};
use zip::ZipArchive;
use bsdiff;
use std::env;

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

fn apply_patch(old_file: &Path, patch_file: &Path, new_file: &Path) -> io::Result<()> {
    let old_contents = fs::read(old_file)?;
    let patch_contents = fs::read(patch_file)?;
    let mut new_contents = Vec::new();

    bsdiff::patch(&old_contents, &mut patch_contents.as_slice(), &mut new_contents)?;

    fs::write(new_file, &new_contents)?;

    Ok(())
}

fn main() {
    enable_ansi_support();

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

        let string = String::from(s!("update.zip"));
        let update_zip_path = Path::new(&string);

        if !update_zip_path.exists() {
            eprintln!("{}", s!("\x1b[31m\nNo update file (update.zip) found!\x1b[37m"));
            exit(1);
        }

        println!("{}", s!("Update file found. Preparing to extract..."));

        let update_zip_data = fs::read(update_zip_path).expect("Failed to read update.zip");
        let reader = std::io::Cursor::new(update_zip_data);
        let mut archive = ZipArchive::new(reader).expect("Failed to open update.zip");

        println!("{}", s!("Applying update..."));
        let total_files = archive.len();

        let current_exe = env::current_exe().expect("Failed to get current executable path");
        let current_exe_name = current_exe.file_name().expect("Failed to get executable name").to_str().expect("Failed to convert OsStr to str");

        for i in 0..total_files {
            print!("\r\x1B[2K");
            let progress_percentage = (i + 1) as f32 / total_files as f32 * 100.0;
            print!("Progress: {:.1}%", progress_percentage);
            std::io::stdout().flush().unwrap();

            let mut file = archive.by_index(i).expect("Failed to access file in archive");
            let outpath = PathBuf::from(file.name());

            // Skip if the file is the current executable
            if outpath.file_name().map(|f| f == current_exe_name).unwrap_or(false) {
                continue;
            }

            if file.name().ends_with('/') {
                if let Err(e) = fs::create_dir_all(&outpath) {
                    eprintln!("\x1b[31mFailed to create directory {}: {}\x1b[37m", outpath.display(), e);
                    continue;
                }
            } else if file.name().ends_with(".patch") {
                // Handle patch files
                let original_file = outpath.with_extension("");
                let temp_patch_file = outpath.with_extension("tmp");

                let mut patch_data = Vec::new();
                if let Err(e) = file.read_to_end(&mut patch_data) {
                    eprintln!("\x1b[31mFailed to read patch data for {}: {}\x1b[37m", file.name(), e);
                    continue;
                }

                if let Err(e) = fs::write(&temp_patch_file, &patch_data) {
                    eprintln!("\x1b[31mFailed to create temporary patch file {}: {}\x1b[37m", temp_patch_file.display(), e);
                    continue;
                }

                if let Err(e) = apply_patch(&original_file, &temp_patch_file, &original_file) {
                    eprintln!("\n\x1b[31mFailed to apply patch to {}: {}\x1b[37m", original_file.display(), e);
                }

                if let Err(e) = fs::remove_file(&temp_patch_file) {
                    eprintln!("\x1b[31mFailed to remove temporary patch file {}: {}\x1b[37m", temp_patch_file.display(), e);
                }
            } else if file.name().ends_with(".delete") {
                // Handle delete files
                let file_to_delete = outpath.with_extension("");
                if file_to_delete.exists() {
                    if let Err(e) = fs::remove_file(&file_to_delete) {
                        eprintln!("\x1b[31mFailed to delete file {}: {}\x1b[37m", file_to_delete.display(), e);
                    }
                }
            } else {
                // Handle new files
                if let Some(parent) = outpath.parent() {
                    if !parent.exists() {
                        if let Err(e) = fs::create_dir_all(&parent) {
                            eprintln!("\x1b[31mFailed to create directory {}: {}\x1b[37m", parent.display(), e);
                            continue;
                        }
                    }
                }

                match File::create(&outpath) {
                    Ok(mut outfile) => {
                        if let Err(e) = std::io::copy(&mut file, &mut outfile) {
                            eprintln!("\x1b[31mFailed to copy file {}: {}\x1b[37m", outpath.display(), e);
                        }
                    },
                    Err(e) => eprintln!("\x1b[31mFailed to create file {}: {}\x1b[37m", outpath.display(), e),
                }
            }
        }
        println!("\n{}", s!("Update applied successfully."));

        println!("{}", s!("Cleaning up temporary files..."));
        if let Err(e) = fs::remove_file(update_zip_path) {
            eprintln!("\x1b[31mFailed to remove update.zip: {}\x1b[37m", e);
        }
        println!("{}", s!("Cleanup completed."));

        println!("{}", s!("Restarting the game..."));
        match Command::new("Dreamio.exe").spawn() {
            Ok(_) => println!("{}", s!("Game restarted successfully.")),
            Err(e) => eprintln!("{}", format!("\x1b[31mFailed to restart the game: {}\x1b[37m", e)),
        }

        println!("{}", s!("\x1b[32m\nUpdate process completed!\x1b[37m"));
    }
}
