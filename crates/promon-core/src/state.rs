use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::RuntimeCommand;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessStatus {
    Running,
    Stopped,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProcess {
    pub name: String,
    pub pid: u32,
    pub status: ProcessStatus,
    pub cwd: PathBuf,
    pub command: RuntimeCommand,
    pub started_at: DateTime<Utc>,
    pub out_log: PathBuf,
    pub err_log: PathBuf,
}
