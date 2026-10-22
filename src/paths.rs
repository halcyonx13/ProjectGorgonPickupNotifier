//! Well-known file locations.

use std::path::PathBuf;

pub const APP_NAME: &str = "PG Pickup Notifier";

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn home() -> PathBuf {
    env_dir("USERPROFILE").or_else(|| env_dir("HOME")).unwrap_or_else(|| PathBuf::from("."))
}

/// `%USERPROFILE%\AppData\LocalLow\Elder Game\Project Gorgon\ChatLogs`
pub fn default_chatlog_dir() -> PathBuf {
    home().join("AppData").join("LocalLow").join("Elder Game").join("Project Gorgon").join("ChatLogs")
}

/// `%APPDATA%\PG Pickup Notifier` – settings.
pub fn config_dir() -> PathBuf {
    env_dir("APPDATA").unwrap_or_else(|| home().join(".config")).join(APP_NAME)
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// `%LOCALAPPDATA%\PG Pickup Notifier` – cache and log.
pub fn data_dir() -> PathBuf {
    env_dir("LOCALAPPDATA").unwrap_or_else(|| home().join(".cache")).join(APP_NAME)
}

pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

pub fn icon_cache_dir() -> PathBuf {
    cache_dir().join("icons")
}

pub fn log_file() -> PathBuf {
    data_dir().join("log.txt")
}
