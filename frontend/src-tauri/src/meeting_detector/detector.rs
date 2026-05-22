use crate::meeting_detector::meeting_apps::{
    BROWSER_PROCESSES, TEAMS_PROCESSES, ZOOM_ACTIVE_PROCESSES, ZOOM_PROCESSES,
};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedMeeting {
    pub app_name: String,
    pub process_name: String,
    pub detected_at: String,
    pub is_active_meeting: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingDetectionSettings {
    pub enabled: bool,
    pub auto_start_recording: bool,
    pub auto_stop_recording: bool,
    pub detect_zoom: bool,
    pub detect_teams: bool,
    pub detect_google_meet: bool,
    pub notify_on_detection: bool,
    pub poll_interval_secs: u64,
}

impl Default for MeetingDetectionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_start_recording: false,
            auto_stop_recording: true,
            detect_zoom: true,
            detect_teams: true,
            detect_google_meet: true,
            notify_on_detection: true,
            poll_interval_secs: 5,
        }
    }
}

impl MeetingDetectionSettings {
    fn settings_path() -> Option<PathBuf> {
        dirs::data_dir().map(|path| {
            path.join("com.meetily.ai")
                .join("meeting_detection_settings.json")
        })
    }

    pub fn load() -> Self {
        let Some(path) = Self::settings_path() else {
            return Self::default();
        };

        if !path.exists() {
            return Self::default();
        }

        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(settings) => {
                    info!("Loaded meeting detection settings from {:?}", path);
                    settings
                }
                Err(err) => {
                    error!("Failed to parse meeting detection settings: {}", err);
                    Self::default()
                }
            },
            Err(err) => {
                error!("Failed to read meeting detection settings: {}", err);
                Self::default()
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path =
            Self::settings_path().ok_or_else(|| "Could not determine settings path".to_string())?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("Failed to create settings directory: {}", err))?;
        }

        let contents = serde_json::to_string_pretty(self)
            .map_err(|err| format!("Failed to serialize settings: {}", err))?;

        std::fs::write(&path, contents)
            .map_err(|err| format!("Failed to write settings: {}", err))?;

        info!("Saved meeting detection settings to {:?}", path);
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingDetectionStatus {
    pub is_monitoring: bool,
    pub current_meeting: Option<DetectedMeeting>,
    pub settings: MeetingDetectionSettings,
    pub auto_recording_active: bool,
}

pub struct MeetingDetector {
    system: System,
    settings: Arc<RwLock<MeetingDetectionSettings>>,
    is_monitoring: Arc<AtomicBool>,
    current_meeting: Arc<RwLock<Option<DetectedMeeting>>>,
    auto_recording_active: Arc<AtomicBool>,
}

impl MeetingDetector {
    pub fn new() -> Self {
        let loaded_settings = MeetingDetectionSettings::load();
        info!(
            "MeetingDetector initialized with settings: enabled={}, auto_start={}",
            loaded_settings.enabled, loaded_settings.auto_start_recording
        );

        Self {
            system: System::new_with_specifics(
                RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
            ),
            settings: Arc::new(RwLock::new(loaded_settings)),
            is_monitoring: Arc::new(AtomicBool::new(false)),
            current_meeting: Arc::new(RwLock::new(None)),
            auto_recording_active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn get_settings(&self) -> MeetingDetectionSettings {
        self.settings.read().await.clone()
    }

    pub async fn set_settings(&self, settings: MeetingDetectionSettings) {
        if let Err(err) = settings.save() {
            error!("Failed to save meeting detection settings: {}", err);
        }

        let mut current = self.settings.write().await;
        *current = settings;
    }

    pub fn is_monitoring(&self) -> bool {
        self.is_monitoring.load(Ordering::SeqCst)
    }

    pub async fn get_status(&self) -> MeetingDetectionStatus {
        MeetingDetectionStatus {
            is_monitoring: self.is_monitoring(),
            current_meeting: self.current_meeting.read().await.clone(),
            settings: self.get_settings().await,
            auto_recording_active: self.auto_recording_active.load(Ordering::SeqCst),
        }
    }

    pub fn detect_meeting(
        &mut self,
        settings: &MeetingDetectionSettings,
    ) -> Option<DetectedMeeting> {
        self.system.refresh_processes(ProcessesToUpdate::All, true);
        detect_meeting_from_system(&self.system, settings)
    }

    pub async fn start_monitoring<R: Runtime>(&self, app: AppHandle<R>) {
        if self.is_monitoring.load(Ordering::SeqCst) {
            warn!("Meeting detection is already running");
            return;
        }

        self.is_monitoring.store(true, Ordering::SeqCst);
        info!("Starting meeting detection monitor");

        let is_monitoring = self.is_monitoring.clone();
        let settings = self.settings.clone();
        let current_meeting = self.current_meeting.clone();
        let auto_recording_active = self.auto_recording_active.clone();

        tokio::spawn(async move {
            let mut system = System::new_with_specifics(
                RefreshKind::new().with_processes(ProcessRefreshKind::everything()),
            );
            let mut previous_meeting: Option<DetectedMeeting> = None;

            while is_monitoring.load(Ordering::SeqCst) {
                let current_settings = settings.read().await.clone();
                let poll_interval = Duration::from_secs(current_settings.poll_interval_secs.max(1));

                if !current_settings.enabled {
                    tokio::time::sleep(poll_interval).await;
                    continue;
                }

                system.refresh_processes(ProcessesToUpdate::All, true);
                let detected_meeting = detect_meeting_from_system(&system, &current_settings);

                match (previous_meeting.is_some(), detected_meeting.clone()) {
                    (false, Some(meeting_info)) => {
                        info!(
                            "Meeting detected: {} ({})",
                            meeting_info.app_name, meeting_info.process_name
                        );

                        {
                            let mut current = current_meeting.write().await;
                            *current = Some(meeting_info.clone());
                        }

                        let _ = app.emit("meeting-detected", &meeting_info);

                        if current_settings.notify_on_detection {
                            let _ = app.emit(
                                "meeting-detection-notification",
                                serde_json::json!({
                                    "title": format!("{} Meeting Detected", meeting_info.app_name),
                                    "body": "Auto-recording will start if enabled."
                                }),
                            );
                        }

                        if current_settings.auto_start_recording {
                            let meeting_name = format!("{} Meeting", meeting_info.app_name);
                            info!("Auto-starting recording for: {}", meeting_name);

                            let _ = app.emit(
                                "auto-start-recording",
                                serde_json::json!({
                                    "meeting_name": meeting_name,
                                    "app_name": meeting_info.app_name
                                }),
                            );

                            auto_recording_active.store(true, Ordering::SeqCst);
                        }

                        previous_meeting = Some(meeting_info);
                    }
                    (true, None) => {
                        info!("Meeting ended");

                        {
                            let mut current = current_meeting.write().await;
                            *current = None;
                        }

                        let _ = app.emit("meeting-ended", ());

                        if current_settings.auto_stop_recording
                            && auto_recording_active.load(Ordering::SeqCst)
                        {
                            info!("Auto-stopping recording");
                            let _ = app.emit("auto-stop-recording", ());
                            auto_recording_active.store(false, Ordering::SeqCst);
                        }

                        previous_meeting = None;
                    }
                    (true, Some(meeting_info)) => {
                        previous_meeting = Some(meeting_info);
                    }
                    (false, None) => {}
                }

                tokio::time::sleep(poll_interval).await;
            }

            info!("Meeting detection monitor stopped");
        });
    }

    pub fn stop_monitoring(&self) {
        info!("Stopping meeting detection monitor");
        self.is_monitoring.store(false, Ordering::SeqCst);
    }
}

impl Default for MeetingDetector {
    fn default() -> Self {
        Self::new()
    }
}

pub fn detect_meeting_from_system(
    system: &System,
    settings: &MeetingDetectionSettings,
) -> Option<DetectedMeeting> {
    if let Some(meeting) = detect_by_platform_window_title(settings) {
        return Some(meeting);
    }

    for process in system.processes().values() {
        let process_name = process.name().to_string_lossy().to_string();
        let name = process_name.to_lowercase();

        if settings.detect_zoom {
            if matches_process_name(&name, ZOOM_ACTIVE_PROCESSES) {
                return Some(detected("Zoom", process_name, true));
            }

            if matches_process_name(&name, ZOOM_PROCESSES) {
                debug!("Zoom process is running; waiting for active meeting signal");
            }
        }

        if settings.detect_teams && matches_process_name(&name, TEAMS_PROCESSES) {
            return Some(detected("Microsoft Teams", process_name, true));
        }

        if settings.detect_google_meet && matches_process_name(&name, BROWSER_PROCESSES) {
            debug!("Browser process is running; Google Meet requires title detection");
        }
    }

    None
}

fn detected(app_name: &str, process_name: String, is_active_meeting: bool) -> DetectedMeeting {
    DetectedMeeting {
        app_name: app_name.to_string(),
        process_name,
        detected_at: chrono::Local::now().to_rfc3339(),
        is_active_meeting,
    }
}

fn normalize_process_name(value: &str) -> String {
    let lower = value.trim().to_lowercase();
    lower
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(lower.as_str())
        .trim()
        .to_string()
}

fn process_name_candidates(value: &str) -> [String; 2] {
    let normalized = normalize_process_name(value);
    let without_exe = normalized
        .strip_suffix(".exe")
        .unwrap_or(normalized.as_str())
        .to_string();
    [normalized, without_exe]
}

fn matches_process_name(value: &str, needles: &[&str]) -> bool {
    let candidates = process_name_candidates(value);
    needles.iter().any(|needle| {
        let needle_candidates = process_name_candidates(needle);
        candidates
            .iter()
            .any(|candidate| needle_candidates.iter().any(|needle| candidate == needle))
    })
}

#[cfg(target_os = "windows")]
fn detect_by_platform_window_title(settings: &MeetingDetectionSettings) -> Option<DetectedMeeting> {
    for title in windows_visible_window_titles() {
        let normalized = title.to_lowercase();

        if settings.detect_google_meet
            && (normalized.contains("google meet")
                || normalized.contains("meet.google.com")
                || normalized.contains("meet -"))
        {
            return Some(detected("Google Meet", title, true));
        }

        if settings.detect_zoom
            && normalized.contains("zoom")
            && (normalized.contains("meeting") || normalized.contains("call"))
        {
            return Some(detected("Zoom", title, true));
        }

        if settings.detect_teams
            && (normalized.contains("microsoft teams") || normalized.contains("teams"))
            && (normalized.contains("meeting")
                || normalized.contains("call")
                || normalized.contains("participants"))
        {
            return Some(detected("Microsoft Teams", title, true));
        }
    }

    None
}

#[cfg(not(target_os = "windows"))]
fn detect_by_platform_window_title(
    _settings: &MeetingDetectionSettings,
) -> Option<DetectedMeeting> {
    None
}

#[cfg(target_os = "windows")]
fn windows_visible_window_titles() -> Vec<String> {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, IsWindowVisible,
    };

    unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> i32 {
        let titles = &mut *(lparam as *mut Vec<String>);

        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }

        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return 1;
        }

        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        if copied > 0 {
            let title = String::from_utf16_lossy(&buffer[..copied as usize]);
            if !title.trim().is_empty() {
                titles.push(title);
            }
        }

        1
    }

    let mut titles = Vec::new();
    unsafe {
        EnumWindows(Some(enum_window), &mut titles as *mut _ as LPARAM);
    }

    titles
}

#[cfg(test)]
mod tests {
    use super::matches_process_name;
    use crate::meeting_detector::meeting_apps::{BROWSER_PROCESSES, TEAMS_PROCESSES};

    #[test]
    fn teams_process_matching_does_not_match_steam_service() {
        assert!(!matches_process_name("steamservice.exe", TEAMS_PROCESSES));
        assert!(!matches_process_name("C:\\Program Files (x86)\\Steam\\steamservice.exe", TEAMS_PROCESSES));
    }

    #[test]
    fn teams_process_matching_accepts_known_teams_names() {
        assert!(matches_process_name("teams.exe", TEAMS_PROCESSES));
        assert!(matches_process_name("msteams.exe", TEAMS_PROCESSES));
        assert!(matches_process_name("ms-teams", TEAMS_PROCESSES));
    }

    #[test]
    fn browser_process_matching_is_exact() {
        assert!(matches_process_name("chrome.exe", BROWSER_PROCESSES));
        assert!(!matches_process_name("chromedriver.exe", BROWSER_PROCESSES));
    }
}
