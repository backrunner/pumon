use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};

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
    pub unstable_startup_window_ms: Option<u64>,
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
pub struct PumonDaemonSpec {
    pub enabled: Option<bool>,
    pub scope: Option<String>,
    pub ipc: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PumonSettings {
    pub home: Option<PathBuf>,
    pub node_path: Option<PathBuf>,
    #[serde(default)]
    pub daemon: PumonDaemonSpec,
    #[serde(default)]
    pub log_rotate: LogPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchSpec {
    pub enabled: bool,
    pub paths: Vec<PathBuf>,
    pub include: Vec<String>,
    pub ignore: Vec<String>,
    pub debounce_ms: u64,
    pub reload: bool,
}

impl Default for WatchSpec {
    fn default() -> Self {
        Self {
            enabled: false,
            paths: Vec::new(),
            include: Vec::new(),
            ignore: Vec::new(),
            debounce_ms: 1000,
            reload: false,
        }
    }
}

impl<'de> Deserialize<'de> for WatchSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let input = WatchSpecInput::deserialize(deserializer)?;
        Ok(match input {
            WatchSpecInput::Enabled(enabled) => Self {
                enabled,
                ..Self::default()
            },
            WatchSpecInput::Object(object) => Self {
                enabled: object.enabled.unwrap_or(true),
                paths: object.paths,
                include: object.include,
                ignore: object.ignore,
                debounce_ms: object.debounce_ms.unwrap_or(1000),
                reload: object.reload.unwrap_or(false),
            },
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WatchSpecInput {
    Enabled(bool),
    Object(WatchSpecObject),
}

#[derive(Debug, Deserialize)]
struct WatchSpecObject {
    enabled: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_path_list")]
    paths: Vec<PathBuf>,
    #[serde(default, deserialize_with = "deserialize_string_list")]
    include: Vec<String>,
    #[serde(
        default,
        alias = "ignore_watch",
        deserialize_with = "deserialize_string_list"
    )]
    ignore: Vec<String>,
    debounce_ms: Option<u64>,
    reload: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

fn deserialize_path_list<'de, D>(deserializer: D) -> Result<Vec<PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match OneOrMany::<PathBuf>::deserialize(deserializer)? {
        OneOrMany::One(value) => vec![value],
        OneOrMany::Many(values) => values,
    })
}

fn deserialize_string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match OneOrMany::<String>::deserialize(deserializer)? {
        OneOrMany::One(value) => vec![value],
        OneOrMany::Many(values) => values,
    })
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
    #[serde(default, deserialize_with = "deserialize_string_list")]
    pub ignore_watch: Vec<String>,
    #[serde(default)]
    pub restart: RestartPolicy,
    pub max_memory_restart: Option<String>,
    pub cron_restart: Option<String>,
    #[serde(default)]
    pub log: LogPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PumonConfig {
    #[serde(default)]
    pub pumon: PumonSettings,
    #[serde(default)]
    pub apps: Vec<AppSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedConfig {
    pub pumon: PumonSettings,
    pub apps: Vec<ResolvedAppSpec>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_boolean_watch_spec() {
        let watch: WatchSpec = serde_json::from_value(serde_json::json!(true)).unwrap();
        assert!(watch.enabled);
        assert_eq!(watch.debounce_ms, 1000);
        assert!(watch.paths.is_empty());
    }

    #[test]
    fn deserializes_object_watch_spec() {
        let watch: WatchSpec = serde_json::from_value(serde_json::json!({
            "paths": "src",
            "include": ["**/*.js"],
            "ignore": "dist",
            "debounce_ms": 250,
            "reload": true
        }))
        .unwrap();
        assert!(watch.enabled);
        assert_eq!(watch.paths, vec![PathBuf::from("src")]);
        assert_eq!(watch.include, vec!["**/*.js"]);
        assert_eq!(watch.ignore, vec!["dist"]);
        assert_eq!(watch.debounce_ms, 250);
        assert!(watch.reload);
    }

    #[test]
    fn deserializes_top_level_ignore_watch() {
        let app: AppSpec = serde_json::from_value(serde_json::json!({
            "name": "api",
            "script": "server.js",
            "ignore_watch": "tmp"
        }))
        .unwrap();
        assert_eq!(app.ignore_watch, vec!["tmp"]);
    }
}
