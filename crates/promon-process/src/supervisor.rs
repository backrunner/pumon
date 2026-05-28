use std::process::Stdio;
use std::time::Duration;

use chrono::Utc;
use promon_core::{ManagedProcess, ProcessStatus, PromonError, PromonResult, ResolvedAppSpec};
use promon_logging::ensure_log_paths;
use promon_node_support::resolve_runtime_command;
use promon_platform::{force_kill_process, is_process_alive, logs_dir, terminate_process};
use tokio::fs::OpenOptions;
use tokio::process::Command;
use tokio::time::sleep;

use crate::{load_processes, remove_process, upsert_process};

pub async fn start_app(app: &ResolvedAppSpec) -> PromonResult<ManagedProcess> {
    if let Some(existing) = load_processes()
        .await?
        .into_iter()
        .find(|process| process.name == app.name && is_process_alive(process.pid))
    {
        return Ok(existing);
    }

    let command = resolve_runtime_command(app)?;
    let log_paths = ensure_log_paths(app, logs_dir())
        .await
        .map_err(PromonError::Io)?;
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_paths.out)
        .await
        .map_err(PromonError::Io)?
        .into_std()
        .await;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_paths.err)
        .await
        .map_err(PromonError::Io)?
        .into_std()
        .await;

    let mut child = Command::new(&command.program);
    child
        .args(&command.args)
        .current_dir(&command.cwd)
        .envs(&command.env)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    let child = child.spawn().map_err(PromonError::Io)?;
    let pid = child
        .id()
        .ok_or_else(|| PromonError::Process(format!("failed to read pid for {}", app.name)))?;

    let process = ManagedProcess {
        name: app.name.clone(),
        pid,
        status: ProcessStatus::Running,
        cwd: command.cwd.clone(),
        command,
        started_at: Utc::now(),
        out_log: log_paths.out,
        err_log: log_paths.err,
    };
    upsert_process(process.clone()).await?;
    Ok(process)
}

pub async fn stop_app(name: &str) -> PromonResult<Option<ManagedProcess>> {
    let Some(process) = remove_process(name).await? else {
        return Ok(None);
    };

    if is_process_alive(process.pid) {
        terminate_process(process.pid)
            .await
            .map_err(PromonError::Io)?;
        sleep(Duration::from_millis(700)).await;
        if is_process_alive(process.pid) {
            force_kill_process(process.pid)
                .await
                .map_err(PromonError::Io)?;
        }
    }

    Ok(Some(process))
}

pub async fn list_apps() -> PromonResult<Vec<ManagedProcess>> {
    let mut processes = load_processes().await?;
    for process in &mut processes {
        process.status = if is_process_alive(process.pid) {
            ProcessStatus::Running
        } else {
            ProcessStatus::Unknown
        };
    }
    Ok(processes)
}
