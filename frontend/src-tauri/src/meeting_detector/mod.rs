//! Meeting auto-detection for video conferencing applications.

pub mod commands;
pub mod detector;

pub use commands::*;

mod meeting_apps {
    pub const ZOOM_PROCESSES: &[&str] = &[
        "zoom",
        "zoom.exe",
        "zoom.us",
        "cpthost",
        "cpthost.exe",
        "zoom meeting",
    ];

    pub const ZOOM_ACTIVE_PROCESSES: &[&str] = &["cpthost", "cpthost.exe", "zoom meeting"];

    pub const TEAMS_PROCESSES: &[&str] = &[
        "teams",
        "teams.exe",
        "ms-teams",
        "msteams",
        "msteams.exe",
        "microsoft teams",
    ];

    pub const BROWSER_PROCESSES: &[&str] = &[
        "chrome",
        "chrome.exe",
        "msedge",
        "msedge.exe",
        "firefox",
        "firefox.exe",
        "brave",
        "brave.exe",
        "browser",
        "arc",
        "safari",
    ];
}
