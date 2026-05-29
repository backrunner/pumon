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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManagedProcess {
    pub name: String,
    pub pid: u32,
    pub status: ProcessStatus,
    pub cwd: PathBuf,
    pub command: RuntimeCommand,
    pub started_at: DateTime<Utc>,
    pub out_log: PathBuf,
    pub err_log: PathBuf,
    #[serde(default)]
    pub restart_count: u32,
    pub last_exit_code: Option<i32>,
    pub last_exit_signal: Option<i32>,
    pub last_exit_at: Option<DateTime<Utc>>,
    pub worker_id: Option<String>,
    pub uptime_ms: Option<u64>,
    pub memory_bytes: Option<u64>,
    pub cpu_percent: Option<f32>,
}

impl Default for ManagedProcess {
    fn default() -> Self {
        Self {
            name: String::new(),
            pid: 0,
            status: ProcessStatus::Unknown,
            cwd: PathBuf::new(),
            command: RuntimeCommand {
                program: PathBuf::new(),
                args: Vec::new(),
                cwd: PathBuf::new(),
                env: Default::default(),
            },
            started_at: Utc::now(),
            out_log: PathBuf::new(),
            err_log: PathBuf::new(),
            restart_count: 0,
            last_exit_code: None,
            last_exit_signal: None,
            last_exit_at: None,
            worker_id: None,
            uptime_ms: None,
            memory_bytes: None,
            cpu_percent: None,
        }
    }
}
