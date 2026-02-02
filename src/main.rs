#![windows_subsystem = "windows"]

use crossbeam_channel::{Receiver, Sender};
use eframe::{egui, App, Frame};
use egui::ColorImage;
use qbsdiff::Bspatch;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use reqwest;
use serde_json::Value;
use std::env;
use std::fs::{self, File};
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{ProcessExt, System, SystemExt};
use windows::{
    Win32::Foundation::HWND,
    Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    },
    Win32::UI::Shell::{ITaskbarList3, TaskbarList, TBPFLAG},
    Win32::UI::WindowsAndMessaging::{
        FlashWindowEx, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG,
    },
};
use zip::ZipArchive;

pub struct SharedState {
    pub update_complete: bool,
}

#[derive(Clone, Copy)]
pub enum ProgressState {
    NoProgress,
    Indeterminate,
    Normal,
    Error,
}

struct Taskbar {
    taskbar_list: ITaskbarList3,
}

impl Taskbar {
    fn new() -> Option<Self> {
        unsafe {
            if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() {
                let taskbar_list: ITaskbarList3 =
                    CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok()?;
                Some(Self { taskbar_list })
            } else {
                None
            }
        }
    }

    fn set_progress_state(&self, hwnd: HWND, state: ProgressState) {
        let tbp_flags = match state {
            ProgressState::NoProgress => TBPFLAG(0),
            ProgressState::Indeterminate => TBPFLAG(0x1),
            ProgressState::Normal => TBPFLAG(0x2),
            ProgressState::Error => TBPFLAG(0x4),
        };
        unsafe {
            self.taskbar_list.SetProgressState(hwnd, tbp_flags).ok();
        }
    }

    fn set_progress_value(&self, hwnd: HWND, completed: u64, total: u64) {
        unsafe {
            self.taskbar_list
                .SetProgressValue(hwnd, completed, total)
                .ok();
        }
    }
}

pub struct ProgressUpdate {
    pub downloaded: u64,
    pub total: u64,
    pub bytes_per_sec: f64,
    pub eta: Duration,
    pub elapsed: Duration,
}

pub enum UpdateMessage {
    Log(String),
    Error(String, Option<String>),
    Status(String),
    Progress(f32),
    ProgressUpdate(ProgressUpdate),
    ApplyingProgress(String),
    UpdateComplete,
    UpdateFailed,
}

pub struct LogEntry {
    message: String,
    is_error: bool,
}

pub struct UpdateGUI {
    logs: Vec<LogEntry>,
    progress: f32,
    status: String,
    update_receiver: Receiver<UpdateMessage>,
    update_sender: Sender<UpdateMessage>,
    update_complete: bool,
    bytes_downloaded: u64,
    total_bytes: u64,
    bytes_per_sec: f64,
    eta: Duration,
    elapsed: Duration,
    applying_progress: String,
    exit_code: i32,
    update_failed: bool,
    last_error_response: Option<String>,
    shared_state: Arc<Mutex<SharedState>>,
    image: Option<ColorImage>,
    texture: Option<egui::TextureHandle>,
    taskbar: Option<Taskbar>,
    window_handle: Option<HWND>,
    flashing: bool,
}

impl UpdateGUI {
    pub fn new(shared_state: Arc<Mutex<SharedState>>) -> Self {
        let (sender, receiver) = crossbeam_channel::unbounded();

        let image = image::load_from_memory(include_bytes!("../assets/logo.png")).unwrap();
        let size = [image.width() as _, image.height() as _];
        let image_buffer = image.to_rgba8();
        let pixels = image_buffer.as_flat_samples();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

        let app = Self {
            logs: vec![],
            progress: 0.0,
            status: "Ready to update.".to_string(),
            update_receiver: receiver,
            update_sender: sender,
            update_complete: false,
            bytes_downloaded: 0,
            total_bytes: 0,
            bytes_per_sec: 0.0,
            eta: Duration::from_secs(0),
            elapsed: Duration::from_secs(0),
            applying_progress: "".to_string(),
            exit_code: 1,
            update_failed: false,
            last_error_response: None,
            shared_state,
            image: Some(color_image),
            texture: None,
            taskbar: Taskbar::new(),
            window_handle: None,
            flashing: false,
        };
        app.start_update_thread();
        app
    }

    #[cfg(not(test))]
    fn start_update_thread(&self) {
        let sender = self.update_sender.clone();
        thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                update_task(sender).await;
            });
        });
    }

    #[cfg(test)]
    fn start_update_thread(&self) {
        // Do not start the thread in tests
    }

    fn retry(&mut self) {
        self.flash_window(false);
        self.flashing = false;
        if let (Some(taskbar), Some(hwnd)) = (&self.taskbar, self.window_handle) {
            taskbar.set_progress_state(hwnd, ProgressState::NoProgress);
        }
        let shared_state = Arc::clone(&self.shared_state);
        *self = Self::new(shared_state);
    }

    fn flash_window(&self, start: bool) {
        if let Some(hwnd) = self.window_handle {
            let mut info = FLASHWINFO {
                cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
                hwnd,
                dwFlags: if start {
                    FLASHW_ALL | FLASHW_TIMERNOFG
                } else {
                    windows::Win32::UI::WindowsAndMessaging::FLASHWINFO_FLAGS(0)
                },
                uCount: if start { 0 } else { 0 },
                dwTimeout: 0,
            };
            unsafe {
                FlashWindowEx(&mut info);
            }
        }
    }
}

impl App for UpdateGUI {
    fn update(&mut self, ctx: &egui::Context, frame: &mut Frame) {
        if self.window_handle.is_none() {
            if let Ok(window_handle) = frame.window_handle() {
                let raw_handle: RawWindowHandle = window_handle.into();
                if let RawWindowHandle::Win32(handle) = raw_handle {
                    self.window_handle = Some(HWND(handle.hwnd.get() as isize));
                }
            }
        }

        while let Ok(msg) = self.update_receiver.try_recv() {
            match msg {
                UpdateMessage::Log(log) => self.logs.push(LogEntry {
                    message: log,
                    is_error: false,
                }),
                UpdateMessage::Error(log, response) => {
                    self.logs.push(LogEntry {
                        message: log,
                        is_error: true,
                    });
                    self.last_error_response = response;
                    if let (Some(taskbar), Some(hwnd)) = (&self.taskbar, self.window_handle) {
                        taskbar.set_progress_state(hwnd, ProgressState::Error);
                    }
                    self.flashing = true;
                    self.flash_window(true);
                }
                UpdateMessage::Status(status) => self.status = status,
                UpdateMessage::Progress(progress) => {
                    self.progress = progress;
                    if let (Some(taskbar), Some(hwnd)) = (&self.taskbar, self.window_handle) {
                        taskbar.set_progress_state(hwnd, ProgressState::Normal);
                        taskbar.set_progress_value(hwnd, (progress * 100.0) as u64, 100);
                    }
                }
                UpdateMessage::ProgressUpdate(update) => {
                    self.bytes_downloaded = update.downloaded;
                    self.total_bytes = update.total;
                    self.bytes_per_sec = update.bytes_per_sec;
                    self.eta = update.eta;
                    self.elapsed = update.elapsed;
                    self.progress = if update.total > 0 {
                        update.downloaded as f32 / update.total as f32
                    } else {
                        0.0
                    };
                    if let (Some(taskbar), Some(hwnd)) = (&self.taskbar, self.window_handle) {
                        taskbar.set_progress_state(hwnd, ProgressState::Normal);
                        taskbar.set_progress_value(hwnd, update.downloaded, update.total);
                    }
                }
                UpdateMessage::ApplyingProgress(progress_text) => {
                    self.applying_progress = progress_text;
                    if let (Some(taskbar), Some(hwnd)) = (&self.taskbar, self.window_handle) {
                        taskbar.set_progress_state(hwnd, ProgressState::Indeterminate);
                    }
                }
                UpdateMessage::UpdateComplete => {
                    if let (Some(taskbar), Some(hwnd)) = (&self.taskbar, self.window_handle) {
                        taskbar.set_progress_state(hwnd, ProgressState::NoProgress);
                    }
                    self.update_complete = true;
                    self.logs.push(LogEntry {
                        message: "Launching game...".to_string(),
                        is_error: false,
                    });
                    match Command::new("Dreamio.exe").spawn() {
                        Ok(_) => {
                            self.logs.push(LogEntry {
                                message: "Game launched successfully.".to_string(),
                                is_error: false,
                            });
                            let mut state = self.shared_state.lock().unwrap();
                            state.update_complete = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Err(e) => {
                            self.logs.push(LogEntry {
                                message: format!("Failed to launch game: {}", e),
                                is_error: true,
                            });
                            let mut state = self.shared_state.lock().unwrap();
                            state.update_complete = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
                UpdateMessage::UpdateFailed => {
                    if let (Some(taskbar), Some(hwnd)) = (&self.taskbar, self.window_handle) {
                        taskbar.set_progress_state(hwnd, ProgressState::Error);
                    }
                    self.update_complete = true;
                    self.exit_code = 1;
                    self.update_failed = true;
                    self.flashing = true;
                    self.flash_window(true);
                }
            }
        }

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            let texture: &egui::TextureHandle = self.texture.get_or_insert_with(|| {
                ui.ctx()
                    .load_texture("icon", self.image.take().unwrap(), Default::default())
            });

            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                ui.image(texture);
            });

            ui.separator();

            ui.label(&self.status);

            let progress_text = if self.status == "Applying update..." {
                self.applying_progress.clone()
            } else {
                format!(
                    "[{}] {}/{} ({}/s, ETA: {})",
                    format_duration(self.elapsed),
                    format_bytes(self.bytes_downloaded),
                    format_bytes(self.total_bytes),
                    format_bytes(self.bytes_per_sec as u64),
                    format_duration(self.eta)
                )
            };
            ui.add(
                egui::ProgressBar::new(self.progress)
                    .fill(egui::Color32::from_hex("#ddb99b").unwrap()),
            );
            ui.label(progress_text);

            if self.update_failed {
                ui.separator();

                if self.last_error_response.is_some() {
                    ui.heading("A security appliance or firewall might be blocking the request. Please check your firewall software, for instance Xfinity Advanced Security.");
                    ui.horizontal(|ui| {
                        if ui.button("Retry").clicked() {
                            self.retry();
                        }
                        if ui.button("More details").clicked() {
                            if let Some(response) = &self.last_error_response {
                                let path = std::env::temp_dir().join("dreamio-updater-error.html");
                                if let Ok(mut file) = std::fs::File::create(&path) {
                                    if file.write_all(response.as_bytes()).is_ok() {
                                        opener::open(path).ok();
                                    }
                                }
                            }
                        }
                    });
                } else {
                    ui.heading("Please try again. If the issue persists, you can download the latest version of the game manually:");
                    ui.hyperlink("https://storage.googleapis.com/dreamio/downloads/Builds/Windows/latest.zip");
                    if ui.button("Retry").clicked() {
                        self.retry();
                    }
                }
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                for log in &self.logs {
                    let text = if log.is_error {
                        egui::RichText::new(&log.message)
                            .color(egui::Color32::RED)
                            .monospace()
                    } else {
                        egui::RichText::new(&log.message).monospace()
                    };
                    ui.label(text);
                }
            });
        });

        if !self.update_complete || self.flashing {
            ctx.request_repaint();
        }
    }
}

async fn update_task(sender: Sender<UpdateMessage>) {
    sender
        .send(UpdateMessage::Status("Checking for updates...".to_string()))
        .unwrap();

    let update_zip_path = PathBuf::from("update.zip");
    let version_file_path = Path::new("version.json");

    let mut system = System::new();
    system.refresh_processes();

    let pids_to_kill: Vec<_> = system
        .processes_by_name("Dreamio.exe")
        .map(|p| p.pid())
        .collect();

    if !pids_to_kill.is_empty() {
        sender
            .send(UpdateMessage::Log(
                "Game process found. Shutting down...".to_string(),
            ))
            .unwrap();
        for pid in &pids_to_kill {
            if let Some(process) = system.process(*pid) {
                if !process.kill() {
                    sender
                        .send(UpdateMessage::Error(
                            format!("Failed to send kill signal to process {}", pid),
                            None,
                        ))
                        .unwrap();
                }
            }
        }

        sender
            .send(UpdateMessage::Log(
                "Waiting for game process to fully terminate...".to_string(),
            ))
            .unwrap();
        loop {
            system.refresh_processes();
            let any_process_alive = pids_to_kill
                .iter()
                .any(|pid| system.process(*pid).is_some());
            if !any_process_alive {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        sender
            .send(UpdateMessage::Log(
                "Game process terminated successfully.".to_string(),
            ))
            .unwrap();
    } else {
        sender
            .send(UpdateMessage::Log(
                "No running game process found.".to_string(),
            ))
            .unwrap();
    }

    if update_zip_path.exists() {
        if let Err(e) = apply_update(&update_zip_path, &sender) {
            sender
                .send(UpdateMessage::Error(
                    format!("Failed to apply update: {}", e),
                    None,
                ))
                .unwrap();
            sender.send(UpdateMessage::UpdateFailed).unwrap();
            cleanup();
            return;
        }
        cleanup();
    }

    if !version_file_path.exists() {
        sender
            .send(UpdateMessage::Log("Downloading the game...".to_string()))
            .unwrap();
        match get_latest_update_url() {
            Ok(latest_url) => {
                if let Err(e) = download_and_apply_update(&latest_url, &update_zip_path, &sender) {
                    sender
                        .send(UpdateMessage::Error(
                            format!("Failed to download or apply update: {}", e),
                            None,
                        ))
                        .unwrap();
                    sender.send(UpdateMessage::UpdateFailed).unwrap();
                    cleanup();
                    return;
                }
            }
            Err(e) => {
                let error_string = e.to_string();
                if error_string.contains("Received an HTML response") {
                    let response_body = error_string
                        .splitn(2, "Response:")
                        .nth(1)
                        .map(|s| s.trim().to_string());
                    sender
                        .send(UpdateMessage::Error(
                            "Failed to get latest update URL: Received an HTML response instead of JSON. A security appliance or firewall might be blocking the request. Please check your firewall software, for instance Xfinity Advanced Security.".to_string(),
                            response_body,
                        ))
                        .unwrap();
                } else {
                    sender
                        .send(UpdateMessage::Error(
                            format!("Failed to get latest update URL: {}", e),
                            None,
                        ))
                        .unwrap();
                }
                sender.send(UpdateMessage::UpdateFailed).unwrap();
                cleanup();
                return;
            }
        }
    }

    loop {
        match get_version_info() {
            Ok((version_code, update_url)) => {
                sender
                    .send(UpdateMessage::Log(format!(
                        "Downloading update for version {}...",
                        version_code
                    )))
                    .unwrap();
                match download_and_apply_update(&update_url, &update_zip_path, &sender) {
                    Ok(_) => {
                        match get_version_info() {
                            Ok((new_version_code, _)) => {
                                if new_version_code == version_code {
                                    sender
                                        .send(UpdateMessage::Log(
                                            "Update complete. No more updates available."
                                                .to_string(),
                                        ))
                                        .unwrap();
                                    break;
                                }
                            }
                            Err(e) => {
                                sender
                                    .send(UpdateMessage::Error(
                                        format!("Failed to read updated version info: {}", e),
                                        None,
                                    ))
                                    .unwrap();
                                sender.send(UpdateMessage::UpdateFailed).unwrap();
                                cleanup();
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        if e.to_string().contains("404") {
                            sender
                                .send(UpdateMessage::Log(
                                    "No more updates available.".to_string(),
                                ))
                                .unwrap();
                            break;
                        } else {
                            sender
                                .send(UpdateMessage::Error(
                                    format!("Error downloading update: {}", e),
                                    None,
                                ))
                                .unwrap();
                            sender.send(UpdateMessage::UpdateFailed).unwrap();
                            cleanup();
                            return;
                        }
                    }
                }
            }
            Err(e) => {
                sender
                    .send(UpdateMessage::Error(
                        format!("Failed to read version info: {}", e),
                        None,
                    ))
                    .unwrap();
                sender.send(UpdateMessage::UpdateFailed).unwrap();
                cleanup();
                return;
            }
        }
    }

    sender
        .send(UpdateMessage::Status("Update complete.".to_string()))
        .unwrap();
    sender
        .send(UpdateMessage::Log("Update process finished.".to_string()))
        .unwrap();
    sender.send(UpdateMessage::UpdateComplete).unwrap();
}

fn apply_patch(old_file: &Path, patch_data: &[u8], new_file: &Path) -> io::Result<()> {
    let old_contents = fs::read(old_file)?;
    let mut new_contents = Vec::new();

    let patcher = Bspatch::new(patch_data)?;
    patcher.apply(&old_contents, Cursor::new(&mut new_contents))?;

    fs::write(new_file, &new_contents)?;

    Ok(())
}

fn download_file(
    url: &str,
    path: &Path,
    sender: &Sender<UpdateMessage>,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("DreamioUpdater/1.0")
        .timeout(Duration::from_secs(30))
        .build()?;
    let mut response = client.get(url).send()?;

    if !response.status().is_success() {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::Other,
            format!("HTTP error: {}", response.status()),
        )));
    }

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut file = File::create(path)?;
    let mut buffer = [0; 8192];
    let start_time = Instant::now();

    loop {
        let n = match response.read(&mut buffer) {
            Ok(n) => n,
            Err(e) => return Err(Box::new(e)),
        };

        if n == 0 {
            break;
        }

        file.write_all(&buffer[..n])?;
        downloaded += n as u64;

        let elapsed = start_time.elapsed();
        let bytes_per_sec = if elapsed.as_secs() > 0 {
            downloaded as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        let eta_secs = if bytes_per_sec > 0.0 {
            (total_size - downloaded) as f64 / bytes_per_sec
        } else {
            0.0
        };
        let eta = Duration::from_secs(eta_secs as u64);

        sender
            .send(UpdateMessage::ProgressUpdate(ProgressUpdate {
                downloaded,
                total: total_size,
                bytes_per_sec,
                eta,
                elapsed,
            }))
            .unwrap();
    }

    Ok(())
}

fn download_and_apply_update(
    url: &str,
    update_zip_path: &Path,
    sender: &Sender<UpdateMessage>,
) -> Result<(), Box<dyn std::error::Error>> {
    sender
        .send(UpdateMessage::Status("Downloading update...".to_string()))
        .unwrap();
    if let Err(e) = download_file(url, update_zip_path, sender) {
        if url.starts_with("https://") {
            let http_url = url.replace("https", "http");
            sender
                .send(UpdateMessage::Log(
                    "HTTPS download failed, trying HTTP...".to_string(),
                ))
                .unwrap();
            download_file(&http_url, update_zip_path, sender)?;
        } else {
            return Err(e);
        }
    }
    apply_update(update_zip_path, sender)?;
    cleanup();
    Ok(())
}

fn get_latest_update_url() -> Result<String, Box<dyn std::error::Error>> {
    let url = "https://storage.googleapis.com/dreamio/downloads/Builds/Windows/version.json";
    let client = reqwest::blocking::Client::builder()
        .user_agent("DreamioUpdater/1.0")
        .timeout(Duration::from_secs(30))
        .build()?;

    let response = match client.get(url).send() {
        Ok(res) => res,
        Err(_) => {
            let http_url = url.replace("https", "http");
            client.get(&http_url).send()?
        }
    };

    let content_type = response.headers().get(reqwest::header::CONTENT_TYPE).cloned();
    let response_text = response.text()?;

    if let Some(content_type) = content_type {
        if let Ok(content_type) = content_type.to_str() {
            if content_type.contains("text/html") {
                return Err(format!("Received an HTML response instead of JSON. A security appliance or firewall might be blocking the request. Please check your firewall software, for instance Xfinity Advanced Security. Response: {}", response_text).into());
            }
        }
    }

    let json: Value = serde_json::from_str(&response_text)?;
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
        "https://storage.googleapis.com/dreamio/downloads/Builds/Windows/patches/{}.zip",
        version_code
    );

    Ok((version_code, update_url))
}

fn apply_update(update_zip_path: &Path, sender: &Sender<UpdateMessage>) -> io::Result<()> {
    sender
        .send(UpdateMessage::Status("Applying update...".to_string()))
        .unwrap();
    let update_zip_data = fs::read(update_zip_path)?;
    let reader = Cursor::new(update_zip_data);
    let mut archive = ZipArchive::new(reader)?;
    let archive_len = archive.len();

    let current_exe = env::current_exe()?;
    let current_exe_name = current_exe.file_name().unwrap().to_str().unwrap();

    for i in 0..archive_len {
        let mut file = match archive.by_index(i) {
            Ok(file) => file,
            Err(e) => {
                sender
                    .send(UpdateMessage::Error(
                        format!("Error accessing file in archive: {}. Skipping.", e),
                        None,
                    ))
                    .unwrap();
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
                sender
                    .send(UpdateMessage::Error(
                        format!(
                            "Error creating directory {}: {}. Skipping.",
                            out_path.display(),
                            e
                        ),
                        None,
                    ))
                    .unwrap();
                continue;
            }
        } else if file.name().ends_with(".patch") {
            let original_file = out_path.with_extension("");
            let mut patch_data = Vec::new();
            if let Err(e) = file.read_to_end(&mut patch_data) {
                sender
                    .send(UpdateMessage::Error(
                        format!(
                            "Error reading patch data for {}: {}. Skipping.",
                            original_file.display(),
                            e
                        ),
                        None,
                    ))
                    .unwrap();
                continue;
            }
            if let Err(e) = apply_patch(&original_file, &patch_data, &original_file) {
                sender
                    .send(UpdateMessage::Error(
                        format!(
                            "Error applying patch to {}: {}. Skipping.",
                            original_file.display(),
                            e
                        ),
                        None,
                    ))
                    .unwrap();
                continue;
            }
        } else if file.name().ends_with(".delete") {
            let file_to_delete = out_path.with_extension("");
            if file_to_delete.exists() {
                if let Err(e) = fs::remove_file(&file_to_delete) {
                    sender
                        .send(UpdateMessage::Error(
                            format!(
                                "Error deleting file {}: {}. Skipping.",
                                file_to_delete.display(),
                                e
                            ),
                            None,
                        ))
                        .unwrap();
                }
            }
        } else {
            if let Some(parent) = out_path.parent() {
                if !parent.exists() {
                    if let Err(e) = fs::create_dir_all(parent) {
                        sender
                            .send(UpdateMessage::Error(
                                format!(
                                    "Error creating directory {}: {}. Skipping.",
                                    parent.display(),
                                    e
                                ),
                                None,
                            ))
                            .unwrap();
                        continue;
                    }
                }
            }
            let mut outfile = match File::create(&out_path) {
                Ok(file) => file,
                Err(e) => {
                    sender
                        .send(UpdateMessage::Error(
                            format!("Error creating file {}: {}. Skipping.", out_path.display(), e),
                            None,
                        ))
                        .unwrap();
                    continue;
                }
            };
            if let Err(e) = io::copy(&mut file, &mut outfile) {
                sender
                    .send(UpdateMessage::Error(
                        format!("Error writing to file {}: {}. Skipping.", out_path.display(), e),
                        None,
                    ))
                    .unwrap();
                continue;
            }
        }
        let progress_text = format!(
            "Applying file {}/{}: {}",
            i + 1,
            archive_len,
            file.name()
        );
        sender
            .send(UpdateMessage::ApplyingProgress(progress_text))
            .unwrap();
        sender
            .send(UpdateMessage::Progress(
                (i + 1) as f32 / archive_len as f32
            ))
            .unwrap();
    }

    Ok(())
}

fn cleanup() {
    let update_zip_path = PathBuf::from("update.zip");
    if update_zip_path.exists() {
        fs::remove_file(&update_zip_path).ok();
    }
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;

    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn main() {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([272.0, 500.0])
            .with_min_inner_size([272.0, 294.0])
            .with_icon(load_icon()),
        run_and_return: true,
        ..Default::default()
    };

    let shared_state = Arc::new(Mutex::new(SharedState {
        update_complete: false,
    }));
    let shared_state_clone = Arc::clone(&shared_state);

    eframe::run_native(
        "DREAMIO: AI-Powered Adventures - Updater",
        native_options,
        Box::new(|_cc| Ok(Box::new(UpdateGUI::new(shared_state_clone)))),
    )
    .unwrap();

    let state = shared_state.lock().unwrap();
    if state.update_complete {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}

fn load_icon() -> egui::IconData {
    let (icon_rgba, icon_width, icon_height) = {
        let icon_bytes = include_bytes!("../assets/icon.ico");
        let image = image::load_from_memory(icon_bytes)
            .expect("Failed to load icon from memory")
            .into_rgba8();
        let (width, height) = image.dimensions();
        let rgba = image.into_raw();
        (rgba, width, height)
    };

    egui::IconData {
        rgba: icon_rgba,
        width: icon_width,
        height: icon_height,
    }
}
