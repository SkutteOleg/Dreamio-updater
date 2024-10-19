use chrono::Local;
use eframe::egui;
use log::{error, info, LevelFilter};
use qbsdiff::Bspatch;
use reqwest;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::{error, panic, thread};
use sysinfo::{ProcessExt, SystemExt};
use zip::ZipArchive;

enum UpdateMessage {
    Progress(f32, String),
    Error(String),
    Complete,
}

struct UpdaterApp {
    progress: f32,
    status: String,
    rx: Receiver<UpdateMessage>,
    _tx: Sender<UpdateMessage>,
    update_complete: bool,
    error_occurred: bool,
}

impl Default for UpdaterApp {
    fn default() -> Self {
        let (tx, rx) = channel();
        let _tx = tx.clone();

        thread::spawn(move || {
            if let Err(e) = run_update_process(tx.clone()) {
                tx.send(UpdateMessage::Error(e.to_string())).unwrap();
            }
        });

        Self {
            progress: 0.0,
            status: "Starting...".to_string(),
            rx,
            _tx,
            update_complete: false,
            error_occurred: false,
        }
    }
}

impl eframe::App for UpdaterApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                UpdateMessage::Progress(progress, status) => {
                    self.progress = progress;
                    self.status = status.clone();
                    info!("Progress: {}%, Status: {}", progress * 100.0, status);
                }
                UpdateMessage::Error(error) => {
                    self.status = format!("Error: {}", error);
                    self.error_occurred = true;
                    error!("Error: {}", error);
                }
                UpdateMessage::Complete => {
                    self.progress = 1.0;
                    self.status = "Update complete! Ready to launch game.".to_string();
                    self.update_complete = true;
                    info!("Update complete");
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("DREAMIO: AI-Powered Adventures - Updater");
                ui.add_space(20.0);
                ui.add(egui::ProgressBar::new(self.progress).show_percentage());
                ui.add_space(10.0);
                ui.label(&self.status);

                if self.update_complete || self.error_occurred {
                    ui.add_space(10.0);
                    if ui.button("Close").clicked() {
                        std::process::exit(0);
                    }
                }
            });
        });
    }
}

fn run_update_process(tx: Sender<UpdateMessage>) -> Result<(), Box<dyn error::Error>> {
    info!("Starting update process");
    thread::sleep(std::time::Duration::from_secs(2)); // Add a 2-second delay

    match panic::catch_unwind(|| -> Result<(), Box<dyn error::Error>> {
        // Check if the current directory is accessible
        let current_dir = std::env::current_dir()?;
        info!("Current directory: {:?}", current_dir);

        // Check if we have write permissions
        let test_file_path = current_dir.join("test_write.tmp");
        match File::create(&test_file_path) {
            Ok(_) => {
                info!("Write permission confirmed");
                fs::remove_file(test_file_path)?;
            }
            Err(e) => {
                error!("No write permission in current directory: {}", e);
                return Err(Box::new(e));
            }
        }

        tx.send(UpdateMessage::Progress(
            0.0,
            "Checking for running game process...".to_string(),
        ))
        .map_err(|e| Box::new(e) as Box<dyn error::Error>)?;

        let mut processes = sysinfo::System::new_all();
        info!("Refreshing process list");
        processes.refresh_all();

        info!("Checking for Dreamio.exe process");
        let mut found_process = false;
        for (pid, process) in processes.processes() {
            if process.name() == "Dreamio.exe" {
                info!(
                    "Found Dreamio.exe process (PID: {}). Attempting to shut down.",
                    pid
                );
                found_process = true;
                tx.send(UpdateMessage::Progress(
                    0.1,
                    "Game process found. Shutting down...".to_string(),
                ))
                .map_err(|e| Box::new(e) as Box<dyn error::Error>)?;
                process.kill();
                break;
            }
        }
        if !found_process {
            info!("Dreamio.exe process not found");
        }

        tx.send(UpdateMessage::Progress(
            0.2,
            "Checking for updates...".to_string(),
        ))
        .map_err(|e| Box::new(e) as Box<dyn error::Error>)?;

        let update_zip_path = PathBuf::from("update.zip");
        let version_file_path = Path::new("version.json");

        info!("Checking for update.zip and version.json");
        if !update_zip_path.exists() && !version_file_path.exists() {
            info!("Neither update.zip nor version.json found. Downloading latest update.");
            tx.send(UpdateMessage::Progress(
                0.3,
                "Downloading latest update...".to_string(),
            ))
            .map_err(|e| Box::new(e) as Box<dyn error::Error>)?;
            let latest_url = get_latest_update_url().map_err(|e| {
                error!("Failed to get latest update URL: {}", e);
                e
            })?;
            info!("Got latest update URL: {}", latest_url);
            download_and_apply_update(&latest_url, &update_zip_path, tx.clone())?;
        } else if !update_zip_path.exists() && version_file_path.exists() {
            info!("version.json found but update.zip missing. Getting version info.");
            let (version_code, update_url) = get_version_info().map_err(|e| {
                error!("Failed to get version info: {}", e);
                e
            })?;
            info!(
                "Got version info. Version: {}, URL: {}",
                version_code, update_url
            );
            tx.send(UpdateMessage::Progress(
                0.3,
                format!("Downloading update for version {}...", version_code),
            ))
            .map_err(|e| Box::new(e) as Box<dyn error::Error>)?;
            download_and_apply_update(&update_url, &update_zip_path, tx.clone())?;
        } else if update_zip_path.exists() {
            info!("update.zip found. Proceeding with update application.");
        } else {
            error!("Unexpected file state: update.zip missing but version.json exists");
            return Err("No update file (update.zip) found!".into());
        }

        if update_zip_path.exists() {
            info!("Applying update from update.zip");
            tx.send(UpdateMessage::Progress(
                0.7,
                "Applying update...".to_string(),
            ))
            .map_err(|e| Box::new(e) as Box<dyn error::Error>)?;
            apply_update(&update_zip_path, tx.clone())?;
            info!("Update applied successfully. Cleaning up.");
            cleanup();
        }

        info!("Launching the game");
        tx.send(UpdateMessage::Progress(
            0.9,
            "Launching the game...".to_string(),
        ))
        .map_err(|e| Box::new(e) as Box<dyn error::Error>)?;
        match Command::new("Dreamio.exe").spawn() {
            Ok(_) => info!("Game launched successfully"),
            Err(e) => {
                error!("Failed to launch game: {}", e);
                return Err(Box::new(e));
            }
        }

        info!("Update process completed successfully");
        tx.send(UpdateMessage::Complete)
            .map_err(|e| Box::new(e) as Box<dyn error::Error>)?;
        Ok(())
    }) {
        Ok(result) => result,
        Err(panic_error) => {
            let error_msg = if let Some(s) = panic_error.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = panic_error.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "Unknown panic occurred".to_string()
            };
            error!("Update process panicked: {}", error_msg);
            tx.send(UpdateMessage::Error(error_msg.clone()))
                .unwrap_or_default();
            Err(error_msg.into())
        }
    }
}

fn get_latest_update_url() -> Result<String, Box<dyn error::Error>> {
    let url = "https://dreamio.xyz/downloads/Builds/Windows/version.json";
    let response = reqwest::blocking::get(url)?;
    let json: Value = response.json()?;
    Ok(json["latestUrl"]
        .as_str()
        .ok_or("Invalid latestUrl in JSON")?
        .to_string())
}

fn get_version_info() -> Result<(String, String), Box<dyn error::Error>> {
    let version_file_path = Path::new("version.json");
    let version_content = fs::read_to_string(version_file_path)?;
    let json: Value = serde_json::from_str(&version_content)?;
    let version_code = json["versionCode"]
        .as_str()
        .ok_or("Invalid versionCode in JSON")?
        .to_string();
    let update_url = format!(
        "https://dreamio.xyz/downloads/Builds/Windows/patches/{}.zip",
        version_code
    );
    Ok((version_code, update_url))
}

fn download_and_apply_update(
    url: &str,
    update_zip_path: &Path,
    tx: Sender<UpdateMessage>,
) -> Result<(), Box<dyn error::Error>> {
    let (download_tx, download_rx) = channel();
    let download_url = url.to_string();
    let download_path = update_zip_path.to_path_buf();

    thread::spawn(move || {
        if let Err(e) = download_file(&download_url, &download_path, download_tx.clone()) {
            error!("Download failed: {}", e);
            download_tx
                .send(UpdateMessage::Error(e.to_string()))
                .unwrap();
        }
    });

    while let Ok(message) = download_rx.recv() {
        tx.send(message)?;
    }

    apply_update(update_zip_path, tx)?;
    Ok(())
}

fn download_file(
    url: &str,
    path: &Path,
    tx: Sender<UpdateMessage>,
) -> Result<(), Box<dyn error::Error>> {
    let client = reqwest::blocking::Client::new();
    let mut response = match client.get(url).send() {
        Ok(resp) => resp,
        Err(e) => {
            error!("Failed to send request: {}", e);
            return Err(Box::new(e));
        }
    };
    let total_size = response.content_length().unwrap_or(0);

    let mut file = File::create(path)?;
    let mut downloaded: u64 = 0;
    let mut buffer = [0; 8192];

    while let Ok(n) = response.read(&mut buffer) {
        if n == 0 {
            break;
        }
        match file.write_all(&buffer[..n]) {
            Ok(_) => {}
            Err(e) => {
                error!("Failed to write to file: {}", e);
                return Err(Box::new(e));
            }
        }
        downloaded += n as u64;
        let progress = downloaded as f32 / total_size as f32;
        if let Err(e) = tx.send(UpdateMessage::Progress(
            progress * 0.5,
            format!("Downloading... {:.1}%", progress * 100.0),
        )) {
            error!("Failed to send progress update: {}", e);
            return Err(Box::new(e));
        }
    }

    Ok(())
}

fn apply_update(update_zip_path: &Path, tx: Sender<UpdateMessage>) -> io::Result<()> {
    let update_zip_data = fs::read(update_zip_path)?;
    let reader = Cursor::new(update_zip_data);
    let mut archive = ZipArchive::new(reader)?;
    let total_files = archive.len();

    for i in 0..total_files {
        let progress = (i as f32 / total_files as f32) * 0.5 + 0.5;
        tx.send(UpdateMessage::Progress(
            progress,
            format!("Applying update... {:.1}%", progress * 100.0),
        ))
        .unwrap();

        let mut file = archive.by_index(i)?;
        let out_path = PathBuf::from(file.name());

        if file.name().ends_with('/') {
            fs::create_dir_all(&out_path)?;
        } else if file.name().ends_with(".patch") {
            let original_file = out_path.with_extension("");
            let mut patch_data = Vec::new();
            file.read_to_end(&mut patch_data)?;
            apply_patch(&original_file, &patch_data, &original_file)?;
        } else if file.name().ends_with(".delete") {
            let file_to_delete = out_path.with_extension("");
            if file_to_delete.exists() {
                fs::remove_file(&file_to_delete)?;
            }
        } else {
            if let Some(parent) = out_path.parent() {
                if !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            let mut outfile = File::create(&out_path)?;
            io::copy(&mut file, &mut outfile)?;
        }
    }

    Ok(())
}

fn apply_patch(old_file: &Path, patch_data: &[u8], new_file: &Path) -> io::Result<()> {
    let old_contents = fs::read(old_file)?;
    let mut new_contents = Vec::new();
    let patcher = Bspatch::new(patch_data)?;
    patcher.apply(&old_contents, Cursor::new(&mut new_contents))?;
    fs::write(new_file, &new_contents)?;
    Ok(())
}

fn cleanup() {
    let update_zip_path = PathBuf::from("update.zip");
    if update_zip_path.exists() {
        let _ = fs::remove_file(&update_zip_path);
    }
}

fn setup_logger() -> Result<(), fern::InitError> {
    let log_file = OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open("dreamio_updater.log")?;

    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{}[{}][{}] {}",
                Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                record.target(),
                record.level(),
                message
            ))
        })
        .level(LevelFilter::Debug) // or LevelFilter::Trace
        .chain(io::stdout()) // Add this line to also log to console
        .chain(log_file)
        .apply()?;

    Ok(())
}

#[cfg(windows)]
fn hide_console_window() {
    use std::ptr;
    use winapi::um::wincon::GetConsoleWindow;
    use winapi::um::winuser::{ShowWindow, SW_HIDE};

    let window = unsafe { GetConsoleWindow() };
    if window != ptr::null_mut() {
        unsafe {
            ShowWindow(window, SW_HIDE);
        }
    }
}

#[cfg(not(windows))]
fn hide_console_window() {}

fn main() -> eframe::Result<()> {
    // Comment out this line for debugging
    hide_console_window();

    if let Err(e) = setup_logger() {
        eprintln!("Failed to set up logger: {}", e);
    }

    info!("Starting DREAMIO Updater");

    // Set up panic hook
    panic::set_hook(Box::new(|panic_info| {
        if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            error!("Updater panicked: {}", s);
        } else if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            error!("Updater panicked: {}", s);
        } else {
            error!("Updater panicked: Unknown panic");
        }
        if let Some(location) = panic_info.location() {
            error!("Panic occurred in file '{}' at line {}", location.file(), location.line());
        } else {
            error!("Panic occurred but can't get location information...");
        }
    }));

    let app = UpdaterApp::default();
    let options = eframe::NativeOptions {
        initial_window_size: Some(egui::vec2(400.0, 200.0)),
        ..Default::default()
    };

    info!("Initializing GUI");
    eframe::run_native("DREAMIO Updater", options, Box::new(|_cc| Box::new(app)))?;

    info!("DREAMIO Updater finished successfully");
    Ok(())
}
