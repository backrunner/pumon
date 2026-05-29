use std::path::PathBuf;

use directories::BaseDirs;

pub fn procwatch_home() -> PathBuf {
    std::env::var_os("PROCWATCH_HOME")
        .map(PathBuf::from)
        .or_else(|| BaseDirs::new().map(|dirs| dirs.home_dir().join(".procwatch")))
        .unwrap_or_else(|| PathBuf::from(".procwatch"))
}

pub fn state_dir() -> PathBuf {
    procwatch_home().join("state")
}

pub fn logs_dir() -> PathBuf {
    procwatch_home().join("logs")
}
