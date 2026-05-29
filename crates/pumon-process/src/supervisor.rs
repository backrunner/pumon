use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Datelike, Local, Timelike, Utc};
use pumon_core::{
    ExecMode, ManagedProcess, ProcessStatus, PumonError, PumonResult, ResolvedAppSpec,
};
use pumon_logging::{ensure_log_paths, spawn_rotating_log_writer};
use pumon_node_support::{cluster_control_path, resolve_instances, resolve_runtime_command};
use pumon_platform::{
    force_kill_process_tree, is_process_alive, logs_dir, process_command, terminate_process_tree,
};
use serde::{Deserialize, Serialize};
use tokio::fs::OpenOptions;
use tokio::io::AsyncRead;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

use crate::{load_processes, remove_process, save_processes, upsert_process};

type LogWriterHandle = JoinHandle<std::io::Result<()>>;
type LogWriterHandles = (LogWriterHandle, LogWriterHandle);

#[derive(Debug, Clone, Copy)]
enum LogCaptureMode {
    DirectFile,
    PipeRotating,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyRestartReason {
    MemoryLimit { used_bytes: u64, limit_bytes: u64 },
    Scheduled { rule: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClusterControlRequest {
    Reload,
    Scale { instances: usize },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClusterControlWireRequest {
    Reload {
        token: Option<String>,
    },
    Scale {
        instances: usize,
        token: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct ClusterControlAddress {
    host: String,
    port: u16,
    pid: Option<u32>,
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClusterControlResponse {
    ok: bool,
    error: Option<String>,
}

impl std::fmt::Display for PolicyRestartReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MemoryLimit {
                used_bytes,
                limit_bytes,
            } => write!(
                formatter,
                "memory {used_bytes} exceeded limit {limit_bytes}"
            ),
            Self::Scheduled { rule } => write!(formatter, "scheduled restart matched {rule}"),
        }
    }
}

pub async fn start_app(app: &ResolvedAppSpec) -> PumonResult<ManagedProcess> {
    start_app_with_log_mode(app, LogCaptureMode::DirectFile).await
}

pub async fn start_app_supervised(app: &ResolvedAppSpec) -> PumonResult<ManagedProcess> {
    start_app_with_log_mode(app, LogCaptureMode::PipeRotating).await
}

async fn start_app_with_log_mode(
    app: &ResolvedAppSpec,
    log_mode: LogCaptureMode,
) -> PumonResult<ManagedProcess> {
    start_app_with_log_mode_and_restart_count(app, log_mode, None).await
}

async fn start_app_with_log_mode_and_restart_count(
    app: &ResolvedAppSpec,
    log_mode: LogCaptureMode,
    restart_count_override: Option<u32>,
) -> PumonResult<ManagedProcess> {
    let existing = load_processes()
        .await?
        .into_iter()
        .find(|process| process.name == app.name);
    if let Some(existing) = existing
        .as_ref()
        .filter(|process| is_managed_process_alive(process))
    {
        return Ok(existing.clone());
    }

    let restart_count = restart_count_override.unwrap_or_else(|| {
        existing
            .as_ref()
            .map(|process| process.restart_count.saturating_add(1))
            .unwrap_or(0)
    });
    if matches!(log_mode, LogCaptureMode::PipeRotating) {
        validate_restart_count(app, restart_count)?;
    }

    let command = resolve_runtime_command(app)?;
    let log_paths = ensure_log_paths(app, logs_dir())
        .await
        .map_err(PumonError::Io)?;
    let direct_logs = match log_mode {
        LogCaptureMode::DirectFile => Some(open_direct_log_stdio(&log_paths).await?),
        LogCaptureMode::PipeRotating => None,
    };

    let mut child = Command::new(&command.program);
    child.env_clear();
    child
        .args(&command.args)
        .current_dir(&command.cwd)
        .stdin(Stdio::null());
    child.envs(&command.env);
    child.envs(std::env::vars_os());
    match direct_logs {
        Some((stdout, stderr)) => {
            child.stdout(stdout).stderr(stderr);
        }
        None => {
            child.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
    }
    configure_process_group(&mut child);

    let mut child = child.spawn().map_err(PumonError::Io)?;
    let pid = child
        .id()
        .ok_or_else(|| PumonError::Process(format!("failed to read pid for {}", app.name)))?;
    if matches!(log_mode, LogCaptureMode::PipeRotating) {
        start_background_log_writers(app, &mut child, &log_paths)?;
    }
    let record_exit = matches!(log_mode, LogCaptureMode::PipeRotating);

    let process = ManagedProcess {
        name: app.name.clone(),
        pid,
        status: ProcessStatus::Running,
        cwd: command.cwd.clone(),
        command,
        started_at: Utc::now(),
        out_log: log_paths.out,
        err_log: log_paths.err,
        restart_count,
        ..ManagedProcess::default()
    };
    upsert_process(process.clone()).await?;
    if record_exit {
        spawn_exit_recorder(app.name.clone(), pid, child);
    }
    Ok(process)
}

pub fn validate_restart_policy(app: &ResolvedAppSpec) -> PumonResult<()> {
    if let Some(limit) = app.max_memory_restart.as_deref() {
        parse_memory_limit(limit)?;
    }
    if let Some(rule) = app.cron_restart.as_deref() {
        parse_restart_delay(rule)?;
    }
    Ok(())
}

pub fn policy_restart_reason(
    app: &ResolvedAppSpec,
    process: &ManagedProcess,
) -> PumonResult<Option<PolicyRestartReason>> {
    policy_restart_reason_at(app, process, Utc::now(), process_memory_bytes(process.pid))
}

pub fn restart_backoff_remaining(
    app: &ResolvedAppSpec,
    process: &ManagedProcess,
    now: DateTime<Utc>,
) -> PumonResult<Option<Duration>> {
    validate_restart_count(app, process.restart_count.saturating_add(1))?;

    let Some(last_exit_at) = process.last_exit_at else {
        return Ok(None);
    };

    let unstable = process_is_within_unstable_window(app, process);
    let delay = restart_delay_for_attempt(app, process.restart_count.saturating_add(1), unstable);
    if delay.is_zero() {
        return Ok(None);
    }
    let elapsed = now
        .signed_duration_since(last_exit_at)
        .to_std()
        .unwrap_or_default();
    if elapsed >= delay {
        Ok(None)
    } else {
        Ok(Some(delay - elapsed))
    }
}

pub fn restart_backoff_remaining_now(
    app: &ResolvedAppSpec,
    process: &ManagedProcess,
) -> PumonResult<Option<Duration>> {
    restart_backoff_remaining(app, process, Utc::now())
}

fn validate_restart_count(app: &ResolvedAppSpec, next_restart_count: u32) -> PumonResult<()> {
    if let Some(max) = app.restart.max_restarts {
        if next_restart_count > max {
            return Err(PumonError::Process(format!(
                "app {} exceeded max_restarts={max}",
                app.name
            )));
        }
    }
    Ok(())
}

fn process_is_within_unstable_window(app: &ResolvedAppSpec, process: &ManagedProcess) -> bool {
    let Some(window) = app.restart.unstable_startup_window_ms else {
        return false;
    };
    process
        .uptime_ms
        .map(|uptime| uptime <= window)
        .unwrap_or(true)
}

fn restart_delay_for_attempt(app: &ResolvedAppSpec, attempt: u32, unstable: bool) -> Duration {
    let base = app.restart.restart_delay_ms.unwrap_or(1000);
    if base == 0 {
        return Duration::ZERO;
    }

    let multiplier = if unstable {
        2_u64.saturating_pow(attempt.min(6))
    } else {
        1
    };
    Duration::from_millis(base.saturating_mul(multiplier))
}

fn policy_restart_reason_at(
    app: &ResolvedAppSpec,
    process: &ManagedProcess,
    now: DateTime<Utc>,
    memory_bytes: u64,
) -> PumonResult<Option<PolicyRestartReason>> {
    if let Some(limit) = app
        .max_memory_restart
        .as_deref()
        .map(parse_memory_limit)
        .transpose()?
    {
        if memory_bytes > limit {
            return Ok(Some(PolicyRestartReason::MemoryLimit {
                used_bytes: memory_bytes,
                limit_bytes: limit,
            }));
        }
    }

    let Some(rule) = app.cron_restart.as_deref() else {
        return Ok(None);
    };

    if rule.split_whitespace().count() >= 5 {
        if cron_restart_due(rule, process.started_at, now)? {
            return Ok(Some(PolicyRestartReason::Scheduled {
                rule: rule.to_string(),
            }));
        }
    } else if now
        .signed_duration_since(process.started_at)
        .to_std()
        .unwrap_or_default()
        >= parse_duration_ms(rule)?
    {
        return Ok(Some(PolicyRestartReason::Scheduled {
            rule: rule.to_string(),
        }));
    }

    Ok(None)
}

pub async fn run_app_foreground(app: &ResolvedAppSpec) -> PumonResult<()> {
    run_app_foreground_inner(app, None).await
}

pub async fn run_app_foreground_until_shutdown(
    app: &ResolvedAppSpec,
    shutdown: watch::Receiver<bool>,
) -> PumonResult<()> {
    run_app_foreground_inner(app, Some(shutdown)).await
}

async fn run_app_foreground_inner(
    app: &ResolvedAppSpec,
    mut shutdown: Option<watch::Receiver<bool>>,
) -> PumonResult<()> {
    let mut restarts = 0_u32;
    loop {
        if shutdown_requested(shutdown.as_ref()) {
            return Ok(());
        }

        let command = resolve_runtime_command(app)?;
        let log_paths = ensure_log_paths(app, logs_dir())
            .await
            .map_err(PumonError::Io)?;

        let mut command_builder = Command::new(&command.program);
        command_builder.env_clear();
        command_builder
            .args(&command.args)
            .current_dir(&command.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command_builder.envs(&command.env);
        command_builder.envs(std::env::vars_os());
        configure_process_group(&mut command_builder);
        let mut child = command_builder.spawn().map_err(PumonError::Io)?;
        let pid = child
            .id()
            .ok_or_else(|| PumonError::Process(format!("failed to read pid for {}", app.name)))?;
        let (stdout_log, stderr_log) = foreground_log_writers(app, &mut child, &log_paths)?;
        let process = ManagedProcess {
            name: app.name.clone(),
            pid,
            status: ProcessStatus::Running,
            cwd: command.cwd.clone(),
            command: command.clone(),
            started_at: Utc::now(),
            out_log: log_paths.out.clone(),
            err_log: log_paths.err.clone(),
            restart_count: restarts,
            ..ManagedProcess::default()
        };
        upsert_process(process.clone()).await?;
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
            if let Some(status) = child.try_wait().map_err(PumonError::Io)? {
                break status;
            }

            if let Some(shutdown) = shutdown.as_mut() {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_ok() && *shutdown.borrow() {
                            terminate_process_tree(pid).await.map_err(PumonError::Io)?;
                            sleep(Duration::from_millis(500)).await;
                            if is_process_alive(pid) {
                                force_kill_process_tree(pid)
                                    .await
                                    .map_err(PumonError::Io)?;
                            }
                            break child.wait().await.map_err(PumonError::Io)?;
                        }
                    }
                    _ = sleep(Duration::from_millis(500)) => {}
                }
            } else {
                sleep(Duration::from_millis(500)).await;
            }

            if let Some(limit) = memory_limit {
                if process_memory_bytes(pid) > limit {
                    terminate_process_tree(pid).await.map_err(PumonError::Io)?;
                    sleep(Duration::from_millis(500)).await;
                    if is_process_alive(pid) {
                        force_kill_process_tree(pid).await.map_err(PumonError::Io)?;
                    }
                    break child.wait().await.map_err(PumonError::Io)?;
                }
            }

            if let Some(interval) = interval_restart {
                if started.elapsed() >= interval {
                    terminate_process_tree(pid).await.map_err(PumonError::Io)?;
                    sleep(Duration::from_millis(500)).await;
                    if is_process_alive(pid) {
                        force_kill_process_tree(pid).await.map_err(PumonError::Io)?;
                    }
                    break child.wait().await.map_err(PumonError::Io)?;
                }
            }
        };
        wait_log_writer(stdout_log).await?;
        wait_log_writer(stderr_log).await?;
        remove_process(&app.name).await?;

        if shutdown_requested(shutdown.as_ref()) {
            return Ok(());
        }

        if status.success() || !app.restart.autorestart {
            return Ok(());
        }

        restarts += 1;
        if let Some(max) = app.restart.max_restarts {
            if restarts > max {
                return Err(PumonError::Process(format!(
                    "app {} exceeded max_restarts={max}",
                    app.name
                )));
            }
        }

        let mut delay = app.restart.restart_delay_ms.unwrap_or(1000);
        let unstable_window = app.restart.unstable_startup_window_ms.unwrap_or(0);
        if unstable_window > 0
            && started.elapsed() <= Duration::from_millis(unstable_window)
            && restarts > 0
        {
            delay = delay.saturating_mul(2_u64.pow(restarts.min(6)));
        }
        if let Some(shutdown) = shutdown.as_mut() {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_ok() && *shutdown.borrow() {
                        return Ok(());
                    }
                }
                _ = sleep(Duration::from_millis(delay)) => {}
            }
        } else {
            sleep(Duration::from_millis(delay)).await;
        }
    }
}

fn shutdown_requested(shutdown: Option<&watch::Receiver<bool>>) -> bool {
    shutdown.map(|receiver| *receiver.borrow()).unwrap_or(false)
}

fn spawn_exit_recorder(name: String, pid: u32, mut child: tokio::process::Child) {
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => {
                if let Err(error) = record_process_exit(&name, pid, status).await {
                    eprintln!("failed to record exit for {name} pid={pid}: {error}");
                }
            }
            Err(error) => {
                eprintln!("failed to wait for {name} pid={pid}: {error}");
            }
        }
    });
}

async fn record_process_exit(
    name: &str,
    pid: u32,
    status: std::process::ExitStatus,
) -> PumonResult<()> {
    let mut processes = load_processes().await?;
    let Some(process) = processes
        .iter_mut()
        .find(|process| process.name == name && process.pid == pid)
    else {
        return Ok(());
    };

    process.status = if status.success() {
        ProcessStatus::Stopped
    } else {
        ProcessStatus::Unknown
    };
    process.last_exit_code = status.code();
    process.last_exit_signal = exit_signal(&status);
    process.last_exit_at = Some(Utc::now());
    process.uptime_ms = Some(
        Utc::now()
            .signed_duration_since(process.started_at)
            .to_std()
            .unwrap_or_default()
            .as_millis() as u64,
    );
    save_processes(&processes).await?;
    Ok(())
}

fn exit_signal(status: &std::process::ExitStatus) -> Option<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status.signal()
    }

    #[cfg(not(unix))]
    {
        let _ = status;
        None
    }
}

fn start_background_log_writers(
    app: &ResolvedAppSpec,
    child: &mut tokio::process::Child,
    log_paths: &pumon_logging::LogPaths,
) -> PumonResult<()> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PumonError::Process(format!("failed to capture stdout for {}", app.name)))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| PumonError::Process(format!("failed to capture stderr for {}", app.name)))?;
    spawn_log_writer(app, stdout, log_paths.out.clone());
    spawn_log_writer(app, stderr, log_paths.err.clone());
    Ok(())
}

fn foreground_log_writers(
    app: &ResolvedAppSpec,
    child: &mut tokio::process::Child,
    log_paths: &pumon_logging::LogPaths,
) -> PumonResult<LogWriterHandles> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PumonError::Process(format!("failed to capture stdout for {}", app.name)))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| PumonError::Process(format!("failed to capture stderr for {}", app.name)))?;
    Ok((
        spawn_log_writer(app, stdout, log_paths.out.clone()),
        spawn_log_writer(app, stderr, log_paths.err.clone()),
    ))
}

fn spawn_log_writer<R>(
    app: &ResolvedAppSpec,
    reader: R,
    path: std::path::PathBuf,
) -> LogWriterHandle
where
    R: AsyncRead + Send + Unpin + 'static,
{
    spawn_rotating_log_writer(reader, path, app.log.max_size_bytes, app.log.retain)
}

async fn open_direct_log_stdio(log_paths: &pumon_logging::LogPaths) -> PumonResult<(Stdio, Stdio)> {
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_paths.out)
        .await
        .map_err(PumonError::Io)?
        .into_std()
        .await;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_paths.err)
        .await
        .map_err(PumonError::Io)?
        .into_std()
        .await;
    Ok((Stdio::from(stdout), Stdio::from(stderr)))
}

async fn wait_log_writer(handle: LogWriterHandle) -> PumonResult<()> {
    match handle.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(PumonError::Io(error)),
        Err(error) => Err(PumonError::Process(format!(
            "log writer task failed: {error}"
        ))),
    }
}

fn parse_memory_limit(value: &str) -> PumonResult<u64> {
    let trimmed = value.trim();
    let split = trimmed
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let number: u64 = number
        .parse()
        .map_err(|_| PumonError::Config(format!("invalid memory limit: {value}")))?;
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        _ => return Err(PumonError::Config(format!("invalid memory unit: {value}"))),
    };
    Ok(number.saturating_mul(multiplier))
}

fn parse_restart_delay(value: &str) -> PumonResult<Duration> {
    if value.split_whitespace().count() >= 5 {
        return next_cron_delay(value);
    }
    parse_duration_ms(value)
}

fn parse_duration_ms(value: &str) -> PumonResult<Duration> {
    let trimmed = value.trim();
    let split = trimmed
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let number: u64 = number
        .parse()
        .map_err(|_| PumonError::Config(format!("invalid restart interval: {value}")))?;
    let millis = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "ms" => number,
        "s" | "sec" | "secs" => number * 1000,
        "m" | "min" | "mins" => number * 60 * 1000,
        "h" | "hr" | "hrs" => number * 60 * 60 * 1000,
        _ => {
            return Err(PumonError::Config(format!(
                "cron_restart currently accepts intervals such as 30s, 5m, or 1h: {value}"
            )))
        }
    };
    Ok(Duration::from_millis(millis))
}

fn next_cron_delay(value: &str) -> PumonResult<Duration> {
    let spec = parse_cron_spec(value)?;
    let now = chrono::Local::now();

    for offset in 1..=(366 * 24 * 60) {
        let candidate = now + chrono::Duration::minutes(offset);
        if cron_matches(&spec, candidate) {
            let delay = candidate
                .signed_duration_since(now)
                .to_std()
                .map_err(|_| PumonError::Config(format!("invalid cron_restart: {value}")))?;
            return Ok(delay);
        }
    }

    Err(PumonError::Config(format!(
        "cron_restart has no matching time in the next year: {value}"
    )))
}

struct CronSpec {
    minutes: Vec<u32>,
    hours: Vec<u32>,
    days: Vec<u32>,
    months: Vec<u32>,
    weekdays: Vec<u32>,
    day_is_wildcard: bool,
    weekday_is_wildcard: bool,
}

fn parse_cron_spec(value: &str) -> PumonResult<CronSpec> {
    let fields: Vec<_> = value.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(PumonError::Config(format!(
            "cron_restart cron syntax expects 5 fields: {value}"
        )));
    }

    Ok(CronSpec {
        minutes: parse_cron_field(fields[0], 0, 59)?,
        hours: parse_cron_field(fields[1], 0, 23)?,
        days: parse_cron_field(fields[2], 1, 31)?,
        months: parse_cron_field(fields[3], 1, 12)?,
        weekdays: parse_cron_field(fields[4], 0, 7)?,
        day_is_wildcard: fields[2] == "*",
        weekday_is_wildcard: fields[4] == "*",
    })
}

fn cron_restart_due(
    rule: &str,
    started_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> PumonResult<bool> {
    let spec = parse_cron_spec(rule)?;
    let started = started_at.with_timezone(&Local);
    let now = now.with_timezone(&Local);
    if same_minute(started, now) {
        return Ok(false);
    }
    Ok(cron_matches(&spec, now))
}

fn cron_matches(spec: &CronSpec, candidate: DateTime<Local>) -> bool {
    let weekday = candidate.weekday().num_days_from_sunday();
    let day_matches = spec.days.contains(&candidate.day());
    let weekday_matches =
        spec.weekdays.contains(&weekday) || (weekday == 0 && spec.weekdays.contains(&7));
    let calendar_day_matches = if spec.day_is_wildcard && spec.weekday_is_wildcard {
        true
    } else if spec.day_is_wildcard {
        weekday_matches
    } else if spec.weekday_is_wildcard {
        day_matches
    } else {
        day_matches || weekday_matches
    };

    spec.minutes.contains(&candidate.minute())
        && spec.hours.contains(&candidate.hour())
        && spec.months.contains(&candidate.month())
        && calendar_day_matches
}

fn same_minute(left: DateTime<Local>, right: DateTime<Local>) -> bool {
    left.year() == right.year()
        && left.month() == right.month()
        && left.day() == right.day()
        && left.hour() == right.hour()
        && left.minute() == right.minute()
}

fn parse_cron_field(value: &str, min: u32, max: u32) -> PumonResult<Vec<u32>> {
    let mut values = Vec::new();
    for part in value.split(',') {
        if part == "*" {
            values.extend(min..=max);
            continue;
        }
        if let Some(step) = part.strip_prefix("*/") {
            let step: usize = step
                .parse()
                .map_err(|_| PumonError::Config(format!("invalid cron step: {value}")))?;
            if step == 0 {
                return Err(PumonError::Config(format!("invalid cron step: {value}")));
            }
            values.extend((min..=max).step_by(step));
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            let start: u32 = start
                .parse()
                .map_err(|_| PumonError::Config(format!("invalid cron range: {value}")))?;
            let end: u32 = end
                .parse()
                .map_err(|_| PumonError::Config(format!("invalid cron range: {value}")))?;
            if start < min || end > max || start > end {
                return Err(PumonError::Config(format!(
                    "cron range out of bounds: {value}"
                )));
            }
            values.extend(start..=end);
            continue;
        }
        let item: u32 = part
            .parse()
            .map_err(|_| PumonError::Config(format!("invalid cron field: {value}")))?;
        if item < min || item > max {
            return Err(PumonError::Config(format!(
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

pub async fn stop_app(name: &str) -> PumonResult<Option<ManagedProcess>> {
    let Some(process) = remove_process(name).await? else {
        return Ok(None);
    };

    if is_managed_process_alive(&process) {
        terminate_process_tree(process.pid)
            .await
            .map_err(PumonError::Io)?;
        sleep(Duration::from_millis(700)).await;
        if is_managed_process_alive(&process) {
            force_kill_process_tree(process.pid)
                .await
                .map_err(PumonError::Io)?;
        }
    }
    Ok(Some(process))
}

pub async fn stop_all() -> PumonResult<Vec<ManagedProcess>> {
    let processes = load_processes().await?;
    save_processes(&[]).await?;
    for process in &processes {
        if is_managed_process_alive(process) {
            terminate_process_tree(process.pid)
                .await
                .map_err(PumonError::Io)?;
            sleep(Duration::from_millis(700)).await;
            if is_managed_process_alive(process) {
                force_kill_process_tree(process.pid)
                    .await
                    .map_err(PumonError::Io)?;
            }
        }
    }
    Ok(processes)
}

fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
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

pub async fn restart_app(app: &ResolvedAppSpec) -> PumonResult<ManagedProcess> {
    let _ = stop_app(&app.name).await?;
    start_app(app).await
}

pub async fn restart_app_supervised(app: &ResolvedAppSpec) -> PumonResult<ManagedProcess> {
    let _ = stop_app(&app.name).await?;
    start_app_supervised(app).await
}

pub async fn reload_app(app: &ResolvedAppSpec) -> PumonResult<ManagedProcess> {
    reload_app_with_log_mode(app, LogCaptureMode::DirectFile).await
}

pub async fn reload_app_supervised(app: &ResolvedAppSpec) -> PumonResult<ManagedProcess> {
    reload_app_with_log_mode(app, LogCaptureMode::PipeRotating).await
}

async fn reload_app_with_log_mode(
    app: &ResolvedAppSpec,
    log_mode: LogCaptureMode,
) -> PumonResult<ManagedProcess> {
    if cluster_control(app, ClusterControlRequest::Reload)
        .await?
        .is_some()
    {
        return managed_process_for_app(app).await;
    }
    restart_app_with_log_mode(app, log_mode).await
}

pub async fn scale_app(app: &ResolvedAppSpec) -> PumonResult<ManagedProcess> {
    scale_app_with_log_mode(app, LogCaptureMode::DirectFile).await
}

pub async fn scale_app_supervised(app: &ResolvedAppSpec) -> PumonResult<ManagedProcess> {
    scale_app_with_log_mode(app, LogCaptureMode::PipeRotating).await
}

async fn scale_app_with_log_mode(
    app: &ResolvedAppSpec,
    log_mode: LogCaptureMode,
) -> PumonResult<ManagedProcess> {
    let instances = resolve_instances(&app.instances);
    if cluster_control(app, ClusterControlRequest::Scale { instances })
        .await?
        .is_some()
    {
        return managed_process_for_app(app).await;
    }
    restart_app_with_log_mode(app, log_mode).await
}

async fn restart_app_with_log_mode(
    app: &ResolvedAppSpec,
    log_mode: LogCaptureMode,
) -> PumonResult<ManagedProcess> {
    let previous = stop_app(&app.name).await?;
    let restart_count = previous
        .as_ref()
        .map(|process| process.restart_count.saturating_add(1));
    start_app_with_log_mode_and_restart_count(app, log_mode, restart_count).await
}

async fn managed_process_for_app(app: &ResolvedAppSpec) -> PumonResult<ManagedProcess> {
    let mut process = load_processes()
        .await?
        .into_iter()
        .find(|process| process.name == app.name)
        .ok_or_else(|| PumonError::Process(format!("managed process not found: {}", app.name)))?;
    let command = resolve_runtime_command(app)?;
    process.cwd = command.cwd.clone();
    process.command = command;
    upsert_process(process.clone()).await?;
    Ok(process)
}

async fn cluster_control(
    app: &ResolvedAppSpec,
    request: ClusterControlRequest,
) -> PumonResult<Option<ClusterControlResponse>> {
    if app.exec_mode != ExecMode::Cluster {
        return Ok(None);
    }

    let path = cluster_control_path(&app.name);
    if !path.exists() {
        return Ok(None);
    }

    let raw = tokio::fs::read_to_string(&path)
        .await
        .map_err(PumonError::Io)?;
    let address: ClusterControlAddress = match serde_json::from_str(&raw) {
        Ok(address) => address,
        Err(_) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Ok(None);
        }
    };
    if !matches!(address.host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        return Err(PumonError::Process(format!(
            "cluster control address for {} is not loopback: {}",
            app.name, address.host
        )));
    }
    if let Some(pid) = address.pid {
        let matches_managed_process = load_processes()
            .await?
            .into_iter()
            .any(|process| process.name == app.name && process.pid == pid);
        if !matches_managed_process || !is_process_alive(pid) {
            let _ = tokio::fs::remove_file(&path).await;
            return Ok(None);
        }
    }

    let stream = match timeout(
        Duration::from_secs(3),
        TcpStream::connect((address.host.as_str(), address.port)),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        _ => {
            let _ = tokio::fs::remove_file(&path).await;
            return Ok(None);
        }
    };

    let mut reader = BufReader::new(stream);
    let wire_request = cluster_control_wire_request(request, address.token.clone());
    let request = format!(
        "{}\n",
        serde_json::to_string(&wire_request).map_err(PumonError::Json)?
    );
    match timeout(
        Duration::from_secs(3),
        reader.get_mut().write_all(request.as_bytes()),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(error)) => return Err(PumonError::Io(error)),
        Err(_) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Ok(None);
        }
    }

    let mut line = String::new();
    let bytes_read = match timeout(Duration::from_secs(3), reader.read_line(&mut line)).await {
        Ok(Ok(bytes_read)) => bytes_read,
        Ok(Err(error)) => return Err(PumonError::Io(error)),
        Err(_) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Ok(None);
        }
    };
    if bytes_read == 0 {
        return Err(PumonError::Process(format!(
            "cluster control closed without response for {}",
            app.name
        )));
    }
    let response: ClusterControlResponse = serde_json::from_str(&line).map_err(PumonError::Json)?;
    if response.ok {
        Ok(Some(response))
    } else {
        Err(PumonError::Process(response.error.unwrap_or_else(|| {
            format!("cluster control request failed for {}", app.name)
        })))
    }
}

fn cluster_control_wire_request(
    request: ClusterControlRequest,
    token: Option<String>,
) -> ClusterControlWireRequest {
    match request {
        ClusterControlRequest::Reload => ClusterControlWireRequest::Reload { token },
        ClusterControlRequest::Scale { instances } => {
            ClusterControlWireRequest::Scale { instances, token }
        }
    }
}

pub async fn list_apps() -> PumonResult<Vec<ManagedProcess>> {
    let mut processes = load_processes().await?;
    let mut system = sysinfo::System::new();
    let pids: Vec<_> = processes
        .iter()
        .map(|process| sysinfo::Pid::from_u32(process.pid))
        .collect();
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&pids), true);
    for process in &mut processes {
        if is_managed_process_alive(process) {
            process.status = ProcessStatus::Running;
            if let Some(snapshot) = system.process(sysinfo::Pid::from_u32(process.pid)) {
                process.uptime_ms = Some(snapshot.run_time().saturating_mul(1000));
                process.memory_bytes = Some(snapshot.memory());
                process.cpu_percent = Some(snapshot.cpu_usage());
            }
        } else if !matches!(process.status, ProcessStatus::Stopped) {
            process.status = ProcessStatus::Unknown;
        }
    }
    Ok(processes)
}

pub async fn prune_stale_processes() -> PumonResult<Vec<ManagedProcess>> {
    let mut active = Vec::new();
    let mut stale = Vec::new();
    for mut process in load_processes().await? {
        if is_managed_process_alive(&process) {
            process.status = ProcessStatus::Running;
            active.push(process);
        } else {
            process.status = ProcessStatus::Unknown;
            stale.push(process);
        }
    }
    save_processes(&active).await?;
    Ok(stale)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use pumon_core::{ExecMode, Instances, LogPolicy, RestartPolicy, RuntimeCommand, WatchSpec};

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
    fn enforces_restart_count_limit() {
        let mut app = test_app();
        app.restart = RestartPolicy {
            max_restarts: Some(1),
            ..RestartPolicy::default()
        };

        assert!(validate_restart_count(&app, 1).is_ok());
        assert!(validate_restart_count(&app, 2).is_err());
    }

    #[test]
    fn computes_exponential_backoff_when_unstable_window_is_enabled() {
        let mut app = test_app();
        app.restart = RestartPolicy {
            restart_delay_ms: Some(1000),
            unstable_startup_window_ms: Some(10_000),
            ..RestartPolicy::default()
        };

        assert_eq!(
            restart_delay_for_attempt(&app, 0, true),
            Duration::from_secs(1)
        );
        assert_eq!(
            restart_delay_for_attempt(&app, 2, true),
            Duration::from_secs(4)
        );
        assert_eq!(
            restart_delay_for_attempt(&app, 2, false),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn parses_cron_fields() {
        assert_eq!(
            parse_cron_field("*/15", 0, 59).unwrap(),
            vec![0, 15, 30, 45]
        );
        assert_eq!(parse_cron_field("1,3-5", 0, 7).unwrap(), vec![1, 3, 4, 5]);
    }

    #[test]
    fn interval_policy_restarts_after_elapsed_duration() {
        let mut app = test_app();
        app.cron_restart = Some("2s".to_string());
        let mut process = test_process();
        process.started_at = Utc::now() - chrono::Duration::seconds(3);

        let reason = policy_restart_reason_at(&app, &process, Utc::now(), 0)
            .unwrap()
            .unwrap();
        assert_eq!(
            reason,
            PolicyRestartReason::Scheduled {
                rule: "2s".to_string()
            }
        );
    }

    #[test]
    fn memory_policy_restarts_when_limit_is_exceeded() {
        let mut app = test_app();
        app.max_memory_restart = Some("64M".to_string());
        let process = test_process();

        let reason = policy_restart_reason_at(&app, &process, Utc::now(), 65 * 1024 * 1024)
            .unwrap()
            .unwrap();
        assert_eq!(
            reason,
            PolicyRestartReason::MemoryLimit {
                used_bytes: 65 * 1024 * 1024,
                limit_bytes: 64 * 1024 * 1024
            }
        );
    }

    #[test]
    fn cron_policy_matches_current_minute_only_for_older_processes() {
        let now = Utc::now();
        let local_now = now.with_timezone(&Local);
        let mut app = test_app();
        app.cron_restart = Some(format!("{} {} * * *", local_now.minute(), local_now.hour()));

        let mut old_process = test_process();
        old_process.started_at = now - chrono::Duration::minutes(2);
        assert!(policy_restart_reason_at(&app, &old_process, now, 0)
            .unwrap()
            .is_some());

        let mut new_process = test_process();
        new_process.started_at = now;
        assert!(policy_restart_reason_at(&app, &new_process, now, 0)
            .unwrap()
            .is_none());
    }

    fn test_app() -> ResolvedAppSpec {
        ResolvedAppSpec {
            name: "api".to_string(),
            script: Some(PathBuf::from("server.js")),
            command: None,
            cwd: PathBuf::from("/tmp/api"),
            args: vec![],
            node_args: vec![],
            interpreter: "node".to_string(),
            interpreter_args: vec![],
            package_manager: None,
            package_script: None,
            env: BTreeMap::new(),
            exec_mode: ExecMode::Fork,
            instances: Instances::Count(1),
            watch: WatchSpec::default(),
            restart: RestartPolicy::default(),
            max_memory_restart: None,
            cron_restart: None,
            log: LogPolicy::default(),
        }
    }

    fn test_process() -> ManagedProcess {
        ManagedProcess {
            name: "api".to_string(),
            pid: 123,
            status: ProcessStatus::Running,
            cwd: PathBuf::from("/tmp/api"),
            command: RuntimeCommand {
                program: PathBuf::from("node"),
                args: vec!["server.js".to_string()],
                cwd: PathBuf::from("/tmp/api"),
                env: BTreeMap::new(),
            },
            started_at: Utc::now(),
            out_log: PathBuf::from("/tmp/api/out.log"),
            err_log: PathBuf::from("/tmp/api/err.log"),
            ..ManagedProcess::default()
        }
    }
}
