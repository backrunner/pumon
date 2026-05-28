use std::path::PathBuf;

use directories::BaseDirs;

pub fn promon_home() -> PathBuf {
    std::env::var_os("PROMON_HOME")
        .map(PathBuf::from)
        .or_else(|| BaseDirs::new().map(|dirs| dirs.home_dir().join(".promon")))
        .unwrap_or_else(|| PathBuf::from(".promon"))
}

pub fn state_dir() -> PathBuf {
    promon_home().join("state")
}

pub fn logs_dir() -> PathBuf {
    promon_home().join("logs")
}
