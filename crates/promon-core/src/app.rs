use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExecMode {
    #[default]
    Fork,
    Cluster,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Instances {
    Count(u16),
    Max(String),
}

impl Default for Instances {
    fn default() -> Self {
        Self::Count(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RestartPolicy {
    #[serde(default = "default_autorestart")]
    pub autorestart: bool,
    pub max_restarts: Option<u32>,
    pub restart_delay_ms: Option<u64>,
}

fn default_autorestart() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LogPolicy {
    pub out_file: Option<PathBuf>,
    pub err_file: Option<PathBuf>,
    pub merge: Option<bool>,
    pub max_size_bytes: Option<u64>,
    #[serde(default = "default_log_retain")]
    pub retain: usize,
}

fn default_log_retain() -> usize {
    5
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WatchSpec {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AppSpec {
    pub name: String,
    pub script: Option<PathBuf>,
    pub command: Option<String>,
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub node_args: Vec<String>,
    pub interpreter: Option<String>,
    #[serde(default)]
    pub interpreter_args: Vec<String>,
    pub package_manager: Option<String>,
    pub package_script: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub exec_mode: ExecMode,
    #[serde(default)]
    pub instances: Instances,
    #[serde(default)]
    pub watch: WatchSpec,
    #[serde(default)]
    pub restart: RestartPolicy,
    pub max_memory_restart: Option<String>,
    pub cron_restart: Option<String>,
    #[serde(default)]
    pub log: LogPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PromonConfig {
    #[serde(default)]
    pub apps: Vec<AppSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedAppSpec {
    pub name: String,
    pub script: Option<PathBuf>,
    pub command: Option<String>,
    pub cwd: PathBuf,
    pub args: Vec<String>,
    pub node_args: Vec<String>,
    pub interpreter: String,
    pub interpreter_args: Vec<String>,
    pub package_manager: Option<String>,
    pub package_script: Option<String>,
    pub env: BTreeMap<String, String>,
    pub exec_mode: ExecMode,
    pub instances: Instances,
    pub watch: WatchSpec,
    pub restart: RestartPolicy,
    pub max_memory_restart: Option<String>,
    pub cron_restart: Option<String>,
    pub log: LogPolicy,
}
