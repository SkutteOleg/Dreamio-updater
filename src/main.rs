use goldberg::{goldberg_stmts, goldberg_string as s};
use std::fs;
use std::io::Write;
use std::path::Path;
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

        println!("{}", s!("Extracting update files..."));
        let total_files = archive.len();
        let file_names: Vec<String> = archive.file_names().map(|f| f.to_string()).collect();

        for (i, file_name) in file_names.iter().enumerate() {
            print!("\r\x1B[2K");
            let progress_percentage = (i + 1) as f32 / total_files as f32 * 100.0;
            print!("Progress: {:.1}%", progress_percentage);
            std::io::stdout().flush().unwrap();

            let mut file = archive.by_name(file_name).expect("Failed to access file in archive");
            let outpath = file.mangled_name();

            if file_name.ends_with('/') {
                fs::create_dir_all(&outpath).expect("Failed to create directory");
            } else {
                if let Some(parent) = outpath.parent() {
                    if !parent.exists() {
                        fs::create_dir_all(&parent).expect("Failed to create directory");
                    }
                }

                let mut outfile = match fs::File::create(&outpath) {
                    Ok(file) => file,
                    Err(_e) => {
                        continue;
                    }
                };
                std::io::copy(&mut file, &mut outfile).expect("Failed to copy file");
            }
        }
        println!("\n{}", s!("All files extracted successfully."));

        println!("{}", s!("Cleaning up temporary files..."));
        fs::remove_file(update_zip_path).expect("Failed to remove update.zip");
        println!("{}", s!("Cleanup completed."));

        println!("{}", s!("Restarting the game..."));
        match Command::new("Dreamio.exe").spawn() {
            Ok(_) => println!("{}", s!("Game restarted successfully.")),
            Err(e) => eprintln!("{}", format!("\x1b[31mFailed to restart the game: {}\x1b[37m", e)),
        }

        println!("{}", s!("\x1b[32m\nUpdate process completed!\x1b[37m"));
    }
}
