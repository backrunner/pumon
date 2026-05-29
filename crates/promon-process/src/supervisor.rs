use std::process::Stdio;
use std::time::Duration;

use chrono::Utc;
use promon_core::{ManagedProcess, ProcessStatus, PromonError, PromonResult, ResolvedAppSpec};
use promon_logging::ensure_log_paths;
use promon_node_support::resolve_runtime_command;
use promon_platform::{
    force_kill_process, is_process_alive, logs_dir, process_command, terminate_process,
};
use tokio::fs::OpenOptions;
use tokio::process::Command;
use tokio::time::sleep;

use crate::{load_processes, remove_process, save_processes, upsert_process};

pub async fn start_app(app: &ResolvedAppSpec) -> PromonResult<ManagedProcess> {
    if let Some(existing) = load_processes()
        .await?
        .into_iter()
        .find(|process| process.name == app.name && is_managed_process_alive(process))
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

pub async fn run_app_foreground(app: &ResolvedAppSpec) -> PromonResult<()> {
    let mut restarts = 0_u32;
    loop {
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

        let mut child = Command::new(&command.program)
            .args(&command.args)
            .current_dir(&command.cwd)
            .envs(&command.env)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(PromonError::Io)?;
        let pid = child
            .id()
            .ok_or_else(|| PromonError::Process(format!("failed to read pid for {}", app.name)))?;
        let started = std::time::Instant::now();
        let memory_limit = app
            .max_memory_restart
            .as_deref()
            .map(parse_memory_limit)
            .transpose()?;
        let interval_restart = app
            .cron_restart
            .as_deref()
            .map(parse_duration_ms)
            .transpose()?;

        let status = loop {
            if let Some(status) = child.try_wait().map_err(PromonError::Io)? {
                break status;
            }

            if let Some(limit) = memory_limit {
                if process_memory_bytes(pid) > limit {
                    terminate_process(pid).await.map_err(PromonError::Io)?;
                    sleep(Duration::from_millis(500)).await;
                    if is_process_alive(pid) {
                        force_kill_process(pid).await.map_err(PromonError::Io)?;
                    }
                    break child.wait().await.map_err(PromonError::Io)?;
                }
            }

            if let Some(interval) = interval_restart {
                if started.elapsed() >= interval {
                    terminate_process(pid).await.map_err(PromonError::Io)?;
                    sleep(Duration::from_millis(500)).await;
                    if is_process_alive(pid) {
                        force_kill_process(pid).await.map_err(PromonError::Io)?;
                    }
                    break child.wait().await.map_err(PromonError::Io)?;
                }
            }

            sleep(Duration::from_millis(500)).await;
        };

        if status.success() || !app.restart.autorestart {
            return Ok(());
        }

        restarts += 1;
        if let Some(max) = app.restart.max_restarts {
            if restarts > max {
                return Err(PromonError::Process(format!(
                    "app {} exceeded max_restarts={max}",
                    app.name
                )));
            }
        }

        let delay = app.restart.restart_delay_ms.unwrap_or(1000);
        sleep(Duration::from_millis(delay)).await;
    }
}

fn parse_memory_limit(value: &str) -> PromonResult<u64> {
    let trimmed = value.trim();
    let split = trimmed
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let number: u64 = number
        .parse()
        .map_err(|_| PromonError::Config(format!("invalid memory limit: {value}")))?;
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        _ => return Err(PromonError::Config(format!("invalid memory unit: {value}"))),
    };
    Ok(number.saturating_mul(multiplier))
}

fn parse_duration_ms(value: &str) -> PromonResult<Duration> {
    let trimmed = value.trim();
    let split = trimmed
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let number: u64 = number
        .parse()
        .map_err(|_| PromonError::Config(format!("invalid restart interval: {value}")))?;
    let millis = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "ms" => number,
        "s" | "sec" | "secs" => number * 1000,
        "m" | "min" | "mins" => number * 60 * 1000,
        "h" | "hr" | "hrs" => number * 60 * 60 * 1000,
        _ => {
            return Err(PromonError::Config(format!(
                "cron_restart currently accepts intervals such as 30s, 5m, or 1h: {value}"
            )))
        }
    };
    Ok(Duration::from_millis(millis))
}

fn process_memory_bytes(pid: u32) -> u64 {
    let mut system = sysinfo::System::new();
    system.refresh_processes(
        sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
        true,
    );
    system
        .process(sysinfo::Pid::from_u32(pid))
        .map(|process| process.memory())
        .unwrap_or(0)
}

pub async fn stop_app(name: &str) -> PromonResult<Option<ManagedProcess>> {
    let Some(process) = remove_process(name).await? else {
        return Ok(None);
    };

    if is_managed_process_alive(&process) {
        terminate_process(process.pid)
            .await
            .map_err(PromonError::Io)?;
        sleep(Duration::from_millis(700)).await;
        if is_managed_process_alive(&process) {
            force_kill_process(process.pid)
                .await
                .map_err(PromonError::Io)?;
        }
    }

    Ok(Some(process))
}

pub async fn stop_all() -> PromonResult<Vec<ManagedProcess>> {
    let processes = load_processes().await?;
    save_processes(&[]).await?;
    for process in &processes {
        if is_managed_process_alive(process) {
            terminate_process(process.pid)
                .await
                .map_err(PromonError::Io)?;
            sleep(Duration::from_millis(700)).await;
            if is_managed_process_alive(process) {
                force_kill_process(process.pid)
                    .await
                    .map_err(PromonError::Io)?;
            }
        }
    }
    Ok(processes)
}

fn is_managed_process_alive(process: &ManagedProcess) -> bool {
    if !is_process_alive(process.pid) {
        return false;
    }
    let Some(command) = process_command(process.pid) else {
        return true;
    };
    let program = process
        .command
        .program
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let matches_program = command.contains(program);
    let matches_arg = process
        .command
        .args
        .first()
        .map(|arg| command.contains(arg))
        .unwrap_or(true);
    matches_program && matches_arg
}

pub async fn restart_app(app: &ResolvedAppSpec) -> PromonResult<ManagedProcess> {
    let _ = stop_app(&app.name).await?;
    start_app(app).await
}

pub async fn list_apps() -> PromonResult<Vec<ManagedProcess>> {
    let mut processes = load_processes().await?;
    for process in &mut processes {
        process.status = if is_managed_process_alive(process) {
            ProcessStatus::Running
        } else {
            ProcessStatus::Unknown
        };
    }
    Ok(processes)
}
