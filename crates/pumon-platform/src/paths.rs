use std::path::PathBuf;

use directories::BaseDirs;

pub fn pumon_home() -> PathBuf {
    std::env::var_os("PUMON_HOME")
        .map(PathBuf::from)
        .or_else(|| BaseDirs::new().map(|dirs| dirs.home_dir().join(".pumon")))
        .unwrap_or_else(|| PathBuf::from(".pumon"))
}

pub fn state_dir() -> PathBuf {
    pumon_home().join("state")
}

pub fn logs_dir() -> PathBuf {
    pumon_home().join("logs")
}
