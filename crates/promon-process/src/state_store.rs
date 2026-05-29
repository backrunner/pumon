use std::path::{Path, PathBuf};

use chrono::Utc;
use fs2::FileExt;
use promon_core::{ManagedProcess, PromonError, PromonResult, ResolvedAppSpec};
use promon_platform::state_dir;
use serde::{de::DeserializeOwned, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

fn state_file() -> PathBuf {
    state_dir().join("processes.json")
}

fn desired_state_file() -> PathBuf {
    state_dir().join("desired-apps.json")
}

pub async fn load_processes() -> PromonResult<Vec<ManagedProcess>> {
    let path = state_file();
    let _lock = StateFileLock::acquire(&path).await?;
    load_json_vec_unlocked(&path, "processes").await
}

pub async fn save_processes(processes: &[ManagedProcess]) -> PromonResult<()> {
    let path = state_file();
    let _lock = StateFileLock::acquire(&path).await?;
    save_json_unlocked(&path, processes).await
}

pub async fn load_desired_apps() -> PromonResult<Vec<ResolvedAppSpec>> {
    let path = desired_state_file();
    let _lock = StateFileLock::acquire(&path).await?;
    load_json_vec_unlocked(&path, "desired-apps").await
}

pub async fn save_desired_apps(apps: &[ResolvedAppSpec]) -> PromonResult<()> {
    let path = desired_state_file();
    let _lock = StateFileLock::acquire(&path).await?;
    save_json_unlocked(&path, apps).await
}

pub async fn upsert_process(process: ManagedProcess) -> PromonResult<()> {
    let path = state_file();
    let _lock = StateFileLock::acquire(&path).await?;
    let mut processes: Vec<ManagedProcess> = load_json_vec_unlocked(&path, "processes").await?;
    processes.retain(|item| item.name != process.name);
    processes.push(process);
    save_json_unlocked(&path, &processes).await
}

pub async fn remove_process(name: &str) -> PromonResult<Option<ManagedProcess>> {
    let path = state_file();
    let _lock = StateFileLock::acquire(&path).await?;
    let mut processes: Vec<ManagedProcess> = load_json_vec_unlocked(&path, "processes").await?;
    let removed = processes
        .iter()
        .position(|item| item.name == name)
        .map(|index| processes.remove(index));
    save_json_unlocked(&path, &processes).await?;
    Ok(removed)
}

struct StateFileLock(std::fs::File);

impl StateFileLock {
    async fn acquire(path: &Path) -> PromonResult<Self> {
        let dir = state_dir();
        fs::create_dir_all(&dir).await.map_err(PromonError::Io)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(lock_path(path))
            .map_err(PromonError::Io)?;
        file.lock_exclusive().map_err(PromonError::Io)?;
        Ok(Self(file))
    }
}

impl Drop for StateFileLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn lock_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".lock");
    PathBuf::from(value)
}

async fn load_json_vec_unlocked<T>(path: &Path, backup_prefix: &str) -> PromonResult<Vec<T>>
where
    T: DeserializeOwned,
{
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path).await.map_err(PromonError::Io)?;
    match serde_json::from_str(&raw) {
        Ok(processes) => Ok(processes),
        Err(_) => {
            backup_corrupt_state(path, backup_prefix, &raw).await?;
            Ok(Vec::new())
        }
    }
}

async fn save_json_unlocked<T>(path: &Path, value: &T) -> PromonResult<()>
where
    T: Serialize + ?Sized,
{
    let dir = state_dir();
    fs::create_dir_all(&dir).await.map_err(PromonError::Io)?;
    let raw = serde_json::to_string_pretty(value).map_err(PromonError::Json)?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("state.json");
    let tmp = dir.join(format!(
        "{file_name}.tmp.{}.{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let mut file = fs::File::create(&tmp).await.map_err(PromonError::Io)?;
    file.write_all(raw.as_bytes())
        .await
        .map_err(PromonError::Io)?;
    file.sync_all().await.map_err(PromonError::Io)?;
    drop(file);
    if let Err(error) = fs::rename(&tmp, path).await {
        let _ = fs::remove_file(&tmp).await;
        return Err(PromonError::Io(error));
    }
    sync_parent_dir(&dir);
    Ok(())
}

async fn backup_corrupt_state(path: &Path, prefix: &str, raw: &str) -> PromonResult<()> {
    let backup = state_dir().join(format!(
        "{prefix}.corrupt.{}.json",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    match fs::rename(path, &backup).await {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::write(&backup, raw).await.map_err(PromonError::Io)?;
            let _ = fs::remove_file(path).await;
            Ok(())
        }
    }
}

fn sync_parent_dir(dir: &Path) {
    let _ = std::fs::File::open(dir).and_then(|file| file.sync_all());
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::sync::OnceLock;

    use promon_core::{
        ExecMode, Instances, LogPolicy, ProcessStatus, RestartPolicy, RuntimeCommand, WatchSpec,
    };
    use promon_platform::state_dir;

    use super::*;

    static ENV_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        ENV_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    struct PromonHomeGuard {
        previous: Option<OsString>,
        home: PathBuf,
    }

    impl PromonHomeGuard {
        fn install(name: &str) -> Self {
            let previous = std::env::var_os("PROMON_HOME");
            let home = std::env::temp_dir().join(format!(
                "promon-state-store-{name}-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ));
            std::env::set_var("PROMON_HOME", &home);
            Self { previous, home }
        }
    }

    impl Drop for PromonHomeGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var("PROMON_HOME", previous);
            } else {
                std::env::remove_var("PROMON_HOME");
            }
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    #[tokio::test]
    async fn saves_atomically_and_recovers_corrupt_state() {
        let _lock = env_lock().lock().await;
        let _guard = PromonHomeGuard::install("roundtrip-corrupt");
        let process = ManagedProcess {
            name: "app".to_string(),
            pid: 123,
            status: ProcessStatus::Running,
            cwd: PathBuf::from("/tmp/app"),
            command: RuntimeCommand {
                program: PathBuf::from("node"),
                args: vec!["server.js".to_string()],
                cwd: PathBuf::from("/tmp/app"),
                env: BTreeMap::new(),
            },
            started_at: Utc::now(),
            out_log: PathBuf::from("/tmp/app/out.log"),
            err_log: PathBuf::from("/tmp/app/err.log"),
        };

        save_processes(std::slice::from_ref(&process))
            .await
            .expect("state save should succeed");
        assert_eq!(
            load_processes().await.expect("state load should succeed"),
            vec![process]
        );
        let has_temp_files = std::fs::read_dir(state_dir())
            .expect("state dir should exist")
            .any(|entry| {
                entry
                    .expect("state dir entry should be readable")
                    .file_name()
                    .to_string_lossy()
                    .starts_with("processes.json.tmp")
            });
        assert!(!has_temp_files, "atomic save should not leave temp files");

        let state_path = state_file();
        tokio::fs::write(&state_path, "{bad-json")
            .await
            .expect("corrupt state write should succeed");
        assert_eq!(
            load_processes()
                .await
                .expect("corrupt state should recover"),
            Vec::<ManagedProcess>::new()
        );
        assert!(
            !state_path.exists(),
            "corrupt primary state file should be moved away"
        );

        let backups: Vec<_> = std::fs::read_dir(state_dir())
            .expect("state dir should exist")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("processes.corrupt.")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read_to_string(backups[0].path()).expect("backup should be readable"),
            "{bad-json"
        );
    }

    #[tokio::test]
    async fn saves_and_loads_desired_apps() {
        let _lock = env_lock().lock().await;
        let _guard = PromonHomeGuard::install("desired-apps");
        let app = ResolvedAppSpec {
            name: "api".to_string(),
            script: Some(PathBuf::from("server.js")),
            command: None,
            cwd: PathBuf::from("/tmp/api"),
            args: vec!["--port".to_string(), "3000".to_string()],
            node_args: vec![],
            interpreter: "node".to_string(),
            interpreter_args: vec![],
            package_manager: None,
            package_script: None,
            env: BTreeMap::from([("NODE_ENV".to_string(), "test".to_string())]),
            exec_mode: ExecMode::Fork,
            instances: Instances::Count(1),
            watch: WatchSpec::default(),
            restart: RestartPolicy::default(),
            max_memory_restart: None,
            cron_restart: None,
            log: LogPolicy::default(),
        };

        save_desired_apps(std::slice::from_ref(&app))
            .await
            .expect("desired app save should succeed");

        assert_eq!(
            load_desired_apps()
                .await
                .expect("desired app load should succeed"),
            vec![app]
        );
    }
}
