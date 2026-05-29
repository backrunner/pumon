use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use pumon_core::{ManagedProcess, PumonError, PumonResult, ResolvedAppSpec};
use pumon_platform::state_dir;
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;

fn db_path() -> PathBuf {
    state_dir().join("state.sqlite3")
}

fn legacy_process_path() -> PathBuf {
    state_dir().join("processes.json")
}

fn legacy_desired_path() -> PathBuf {
    state_dir().join("desired-apps.json")
}

pub async fn load_processes() -> PumonResult<Vec<ManagedProcess>> {
    tokio::task::spawn_blocking(load_processes_blocking)
        .await
        .map_err(|error| PumonError::Process(format!("state store task failed: {error}")))?
}

pub async fn save_processes(processes: &[ManagedProcess]) -> PumonResult<()> {
    let processes = processes.to_vec();
    tokio::task::spawn_blocking(move || save_processes_blocking(&processes))
        .await
        .map_err(|error| PumonError::Process(format!("state store task failed: {error}")))?
}

pub async fn load_desired_apps() -> PumonResult<Vec<ResolvedAppSpec>> {
    tokio::task::spawn_blocking(load_desired_apps_blocking)
        .await
        .map_err(|error| PumonError::Process(format!("state store task failed: {error}")))?
}

pub async fn save_desired_apps(apps: &[ResolvedAppSpec]) -> PumonResult<()> {
    let apps = apps.to_vec();
    tokio::task::spawn_blocking(move || save_desired_apps_blocking(&apps))
        .await
        .map_err(|error| PumonError::Process(format!("state store task failed: {error}")))?
}

pub async fn upsert_process(process: ManagedProcess) -> PumonResult<()> {
    tokio::task::spawn_blocking(move || upsert_process_blocking(process))
        .await
        .map_err(|error| PumonError::Process(format!("state store task failed: {error}")))?
}

pub async fn remove_process(name: &str) -> PumonResult<Option<ManagedProcess>> {
    let name = name.to_string();
    tokio::task::spawn_blocking(move || remove_process_blocking(&name))
        .await
        .map_err(|error| PumonError::Process(format!("state store task failed: {error}")))?
}

fn load_processes_blocking() -> PumonResult<Vec<ManagedProcess>> {
    let conn = open_connection()?;
    let mut stmt = conn
        .prepare("SELECT json FROM processes ORDER BY name ASC")
        .map_err(db_error)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error)?;
    let mut processes = Vec::new();
    for row in rows {
        let raw = row.map_err(db_error)?;
        processes.push(serde_json::from_str(&raw).map_err(PumonError::Json)?);
    }
    Ok(processes)
}

fn save_processes_blocking(processes: &[ManagedProcess]) -> PumonResult<()> {
    let mut conn = open_connection()?;
    let tx = conn.transaction().map_err(db_error)?;
    tx.execute("DELETE FROM processes", []).map_err(db_error)?;
    {
        let mut stmt = tx
            .prepare("INSERT OR REPLACE INTO processes(name, json) VALUES(?1, ?2)")
            .map_err(db_error)?;
        for process in processes {
            persist_process(&mut stmt, process)?;
        }
    }
    set_schema_version(&tx)?;
    tx.commit().map_err(db_error)?;
    Ok(())
}

fn load_desired_apps_blocking() -> PumonResult<Vec<ResolvedAppSpec>> {
    let conn = open_connection()?;
    let mut stmt = conn
        .prepare("SELECT json FROM desired_apps ORDER BY name ASC")
        .map_err(db_error)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error)?;
    let mut apps = Vec::new();
    for row in rows {
        let raw = row.map_err(db_error)?;
        apps.push(serde_json::from_str(&raw).map_err(PumonError::Json)?);
    }
    Ok(apps)
}

fn save_desired_apps_blocking(apps: &[ResolvedAppSpec]) -> PumonResult<()> {
    let mut conn = open_connection()?;
    let tx = conn.transaction().map_err(db_error)?;
    tx.execute("DELETE FROM desired_apps", [])
        .map_err(db_error)?;
    {
        let mut stmt = tx
            .prepare("INSERT OR REPLACE INTO desired_apps(name, json) VALUES(?1, ?2)")
            .map_err(db_error)?;
        for app in apps {
            persist_desired_app(&mut stmt, app)?;
        }
    }
    set_schema_version(&tx)?;
    tx.commit().map_err(db_error)?;
    Ok(())
}

fn upsert_process_blocking(process: ManagedProcess) -> PumonResult<()> {
    let mut conn = open_connection()?;
    let tx = conn.transaction().map_err(db_error)?;
    {
        let mut stmt = tx
            .prepare("INSERT OR REPLACE INTO processes(name, json) VALUES(?1, ?2)")
            .map_err(db_error)?;
        persist_process(&mut stmt, &process)?;
    }
    set_schema_version(&tx)?;
    tx.commit().map_err(db_error)?;
    Ok(())
}

fn remove_process_blocking(name: &str) -> PumonResult<Option<ManagedProcess>> {
    let mut conn = open_connection()?;
    let tx = conn.transaction().map_err(db_error)?;
    let removed = tx
        .query_row(
            "SELECT json FROM processes WHERE name = ?1",
            [name],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?
        .map(|raw| serde_json::from_str(&raw).map_err(PumonError::Json))
        .transpose()?;
    tx.execute("DELETE FROM processes WHERE name = ?1", [name])
        .map_err(db_error)?;
    set_schema_version(&tx)?;
    tx.commit().map_err(db_error)?;
    Ok(removed)
}

fn persist_process(
    stmt: &mut rusqlite::Statement<'_>,
    process: &ManagedProcess,
) -> PumonResult<()> {
    let json = serde_json::to_string(process).map_err(PumonError::Json)?;
    stmt.execute(params![process.name, json])
        .map_err(db_error)?;
    Ok(())
}

fn persist_desired_app(
    stmt: &mut rusqlite::Statement<'_>,
    app: &ResolvedAppSpec,
) -> PumonResult<()> {
    let json = serde_json::to_string(app).map_err(PumonError::Json)?;
    stmt.execute(params![app.name, json]).map_err(db_error)?;
    Ok(())
}

fn open_connection() -> PumonResult<Connection> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir).map_err(PumonError::Io)?;
    let mut conn = Connection::open(db_path()).map_err(db_error)?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(db_error)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(db_error)?;
    ensure_schema(&conn)?;
    migrate_legacy_state(&mut conn)?;
    Ok(conn)
}

fn ensure_schema(conn: &Connection) -> PumonResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS processes (
            name TEXT PRIMARY KEY,
            json TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS desired_apps (
            name TEXT PRIMARY KEY,
            json TEXT NOT NULL
        );
        "#,
    )
    .map_err(db_error)?;
    Ok(())
}

fn migrate_legacy_state(conn: &mut Connection) -> PumonResult<()> {
    let version = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(db_error)?;
    if version.as_deref() == Some("1") {
        return Ok(());
    }

    if legacy_process_path().exists() {
        let processes: Vec<ManagedProcess> =
            load_legacy_json_vec(&legacy_process_path(), "processes")?;
        if !processes.is_empty() {
            let tx = conn.transaction().map_err(db_error)?;
            tx.execute("DELETE FROM processes", []).map_err(db_error)?;
            {
                let mut stmt = tx
                    .prepare("INSERT OR REPLACE INTO processes(name, json) VALUES(?1, ?2)")
                    .map_err(db_error)?;
                for process in &processes {
                    persist_process(&mut stmt, process)?;
                }
            }
            tx.commit().map_err(db_error)?;
        }
        backup_legacy_file(&legacy_process_path());
    }

    if legacy_desired_path().exists() {
        let apps: Vec<ResolvedAppSpec> =
            load_legacy_json_vec(&legacy_desired_path(), "desired-apps")?;
        if !apps.is_empty() {
            let tx = conn.transaction().map_err(db_error)?;
            tx.execute("DELETE FROM desired_apps", [])
                .map_err(db_error)?;
            {
                let mut stmt = tx
                    .prepare("INSERT OR REPLACE INTO desired_apps(name, json) VALUES(?1, ?2)")
                    .map_err(db_error)?;
                for app in &apps {
                    persist_desired_app(&mut stmt, app)?;
                }
            }
            tx.commit().map_err(db_error)?;
        }
        backup_legacy_file(&legacy_desired_path());
    }

    set_schema_version(conn)?;
    Ok(())
}

fn set_schema_version(conn: &Connection) -> PumonResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES('schema_version', '1')",
        [],
    )
    .map_err(db_error)?;
    Ok(())
}

fn load_legacy_json_vec<T>(path: &Path, backup_prefix: &str) -> PumonResult<Vec<T>>
where
    T: DeserializeOwned,
{
    let raw = std::fs::read_to_string(path).map_err(PumonError::Io)?;
    match serde_json::from_str(&raw) {
        Ok(value) => Ok(value),
        Err(error) => {
            backup_corrupt_legacy_state(path, backup_prefix, &raw)?;
            let _ = error;
            Ok(Vec::new())
        }
    }
}

fn backup_corrupt_legacy_state(path: &Path, prefix: &str, raw: &str) -> PumonResult<()> {
    let backup = state_dir().join(format!(
        "{prefix}.corrupt.{}.json",
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    match std::fs::rename(path, &backup) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::write(&backup, raw).map_err(PumonError::Io)?;
            let _ = std::fs::remove_file(path);
            Ok(())
        }
    }
}

fn backup_legacy_file(path: &Path) {
    if !path.exists() {
        return;
    }
    let backup = path.with_extension("json.migrated");
    let _ = std::fs::rename(path, backup);
}

fn db_error(error: impl std::fmt::Display) -> PumonError {
    PumonError::Process(format!("state store error: {error}"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::sync::OnceLock;

    use pumon_core::{
        ExecMode, Instances, LogPolicy, ProcessStatus, RestartPolicy, RuntimeCommand, WatchSpec,
    };

    use super::*;

    static ENV_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        ENV_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    struct PumonHomeGuard {
        previous: Option<OsString>,
        home: PathBuf,
    }

    impl PumonHomeGuard {
        fn install(name: &str) -> Self {
            let previous = std::env::var_os("PUMON_HOME");
            let home = std::env::temp_dir().join(format!(
                "pumon-state-store-{name}-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ));
            std::env::set_var("PUMON_HOME", &home);
            Self { previous, home }
        }
    }

    impl Drop for PumonHomeGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var("PUMON_HOME", previous);
            } else {
                std::env::remove_var("PUMON_HOME");
            }
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    #[tokio::test]
    async fn saves_and_loads_processes() {
        let _lock = env_lock().lock().await;
        let _guard = PumonHomeGuard::install("roundtrip");
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
            ..ManagedProcess::default()
        };

        save_processes(std::slice::from_ref(&process))
            .await
            .expect("state save should succeed");
        assert_eq!(
            load_processes().await.expect("state load should succeed"),
            vec![process]
        );
        assert!(db_path().exists(), "sqlite database should be created");
    }

    #[tokio::test]
    async fn migrates_legacy_json_state() {
        let _lock = env_lock().lock().await;
        let _guard = PumonHomeGuard::install("legacy-migration");
        std::fs::create_dir_all(state_dir()).unwrap();
        let process = ManagedProcess {
            name: "legacy".to_string(),
            pid: 321,
            status: ProcessStatus::Running,
            cwd: PathBuf::from("/tmp/legacy"),
            command: RuntimeCommand {
                program: PathBuf::from("node"),
                args: vec!["server.js".to_string()],
                cwd: PathBuf::from("/tmp/legacy"),
                env: BTreeMap::new(),
            },
            started_at: Utc::now(),
            out_log: PathBuf::from("/tmp/legacy/out.log"),
            err_log: PathBuf::from("/tmp/legacy/err.log"),
            ..ManagedProcess::default()
        };
        std::fs::write(
            legacy_process_path(),
            serde_json::to_string_pretty(&vec![process.clone()]).unwrap(),
        )
        .unwrap();

        assert_eq!(
            load_processes()
                .await
                .expect("legacy migration should work"),
            vec![process]
        );
        assert!(db_path().exists(), "sqlite database should be created");
    }

    #[tokio::test]
    async fn saves_and_loads_desired_apps() {
        let _lock = env_lock().lock().await;
        let _guard = PumonHomeGuard::install("desired-apps");
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
