use std::process::Stdio;
use std::time::Duration;

use chrono::{Datelike, Timelike, Utc};
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
            .map(parse_restart_delay)
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

fn parse_restart_delay(value: &str) -> PromonResult<Duration> {
    if value.split_whitespace().count() >= 5 {
        return next_cron_delay(value);
    }
    parse_duration_ms(value)
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

fn next_cron_delay(value: &str) -> PromonResult<Duration> {
    let fields: Vec<_> = value.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(PromonError::Config(format!(
            "cron_restart cron syntax expects 5 fields: {value}"
        )));
    }

    let minutes = parse_cron_field(fields[0], 0, 59)?;
    let hours = parse_cron_field(fields[1], 0, 23)?;
    let days = parse_cron_field(fields[2], 1, 31)?;
    let months = parse_cron_field(fields[3], 1, 12)?;
    let weekdays = parse_cron_field(fields[4], 0, 7)?;
    let day_is_wildcard = fields[2] == "*";
    let weekday_is_wildcard = fields[4] == "*";
    let now = chrono::Local::now();

    for offset in 1..=(366 * 24 * 60) {
        let candidate = now + chrono::Duration::minutes(offset);
        let weekday = candidate.weekday().num_days_from_sunday();
        let day_matches = days.contains(&candidate.day());
        let weekday_matches =
            weekdays.contains(&weekday) || (weekday == 0 && weekdays.contains(&7));
        let calendar_day_matches = if day_is_wildcard && weekday_is_wildcard {
            true
        } else if day_is_wildcard {
            weekday_matches
        } else if weekday_is_wildcard {
            day_matches
        } else {
            day_matches || weekday_matches
        };
        if minutes.contains(&candidate.minute())
            && hours.contains(&candidate.hour())
            && months.contains(&candidate.month())
            && calendar_day_matches
        {
            let delay = candidate
                .signed_duration_since(now)
                .to_std()
                .map_err(|_| PromonError::Config(format!("invalid cron_restart: {value}")))?;
            return Ok(delay);
        }
    }

    Err(PromonError::Config(format!(
        "cron_restart has no matching time in the next year: {value}"
    )))
}

fn parse_cron_field(value: &str, min: u32, max: u32) -> PromonResult<Vec<u32>> {
    let mut values = Vec::new();
    for part in value.split(',') {
        if part == "*" {
            values.extend(min..=max);
            continue;
        }
        if let Some(step) = part.strip_prefix("*/") {
            let step: usize = step
                .parse()
                .map_err(|_| PromonError::Config(format!("invalid cron step: {value}")))?;
            if step == 0 {
                return Err(PromonError::Config(format!("invalid cron step: {value}")));
            }
            values.extend((min..=max).step_by(step));
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            let start: u32 = start
                .parse()
                .map_err(|_| PromonError::Config(format!("invalid cron range: {value}")))?;
            let end: u32 = end
                .parse()
                .map_err(|_| PromonError::Config(format!("invalid cron range: {value}")))?;
            if start < min || end > max || start > end {
                return Err(PromonError::Config(format!(
                    "cron range out of bounds: {value}"
                )));
            }
            values.extend(start..=end);
            continue;
        }
        let item: u32 = part
            .parse()
            .map_err(|_| PromonError::Config(format!("invalid cron field: {value}")))?;
        if item < min || item > max {
            return Err(PromonError::Config(format!(
                "cron field out of bounds: {value}"
            )));
        }
        values.push(item);
    }
    values.sort_unstable();
    values.dedup();
    Ok(values)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_memory_units() {
        assert_eq!(parse_memory_limit("64M").unwrap(), 64 * 1024 * 1024);
        assert_eq!(parse_memory_limit("1G").unwrap(), 1024 * 1024 * 1024);
    }

    #[test]
    fn parses_interval_restart() {
        assert_eq!(parse_restart_delay("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_restart_delay("5m").unwrap(), Duration::from_secs(300));
    }

    #[test]
    fn parses_cron_fields() {
        assert_eq!(
            parse_cron_field("*/15", 0, 59).unwrap(),
            vec![0, 15, 30, 45]
        );
        assert_eq!(parse_cron_field("1,3-5", 0, 7).unwrap(), vec![1, 3, 4, 5]);
    }
}
