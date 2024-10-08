use goldberg::{goldberg_stmts, goldberg_string as s};
use std::fs;
use std::path::Path;
use std::process::{exit, Command};
use std::thread;
use std::time::Duration;
use sysinfo::{ProcessExt, SystemExt};
use zip::ZipArchive;

use winapi::um::wincon::{ENABLE_VIRTUAL_TERMINAL_PROCESSING};
use winapi::um::consoleapi::{GetConsoleMode, SetConsoleMode};
use winapi::um::processenv::GetStdHandle;
use winapi::um::winbase::STD_OUTPUT_HANDLE;
use winapi::um::handleapi::INVALID_HANDLE_VALUE;

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
    
        let mut processes = sysinfo::System::new_all();
        processes.refresh_all();
    
        let mut process_to_kill = None;
        for (pid, process) in processes.processes() {
            if process.name() == "Dreamio.exe" {
                println!("{}", s!("Shutting down the game..."));
    
                if !process.kill() {
                    eprintln!("{}", s!("\x1b[31mError shutting down the game!\x1b[37m"));
                    exit(1);
                }
                process_to_kill = Some(*pid);
                break;
            }
        }
    
        if let Some(pid_to_kill) = process_to_kill {
            let mut is_process_running = true;
            while is_process_running {
                thread::sleep(Duration::from_millis(100));
                processes.refresh_processes();
                is_process_running = processes.processes().iter().any(|(p, _)| *p == pid_to_kill);
            }
        }    
    
        let string = String::from(s!("update.zip"));
        let update_zip_path = Path::new(&string);    
    
        if !update_zip_path.exists() {
            eprintln!("{}", s!("\x1b[31m\nNo update found!\x1b[37m"));
            exit(1);
        }

        println!("{}", s!("Extracting update..."));

        let update_zip_data = fs::read(update_zip_path).expect("Failed to read update.zip");
        let reader = std::io::Cursor::new(update_zip_data);
        let mut archive = ZipArchive::new(reader).expect("Failed to open update.zip");
    
        for i in 0..archive.len() {
            let mut file = archive.by_index(i).expect("Failed to access file in archive");
            let outpath = file.mangled_name();
    
            if file.name().ends_with('/') {
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
    
        println!("{}", s!("Cleaning up..."));
    
        fs::remove_file(update_zip_path).expect("Failed to remove update.zip");
    
        println!("{}", s!("Restarting the game..."));
    
        Command::new("Dreamio.exe")
            .spawn()
            .expect("Failed to restart the game");

        println!("{}", s!("\x1b[32m\nDone!\x1b[37m"));
    }
}
