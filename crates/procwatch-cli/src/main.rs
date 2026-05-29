use std::{io, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use globset::{Glob, GlobSet, GlobSetBuilder};
use procwatch_config::{find_config, load_config};
use procwatch_core::{
    AppSpec, ExecMode, Instances, ManagedProcess, ProcessStatus, ProcwatchConfig, ResolvedAppSpec,
};
use procwatch_logging::tail_file;
use procwatch_node_support::{resolve_instances, resolve_runtime_command, validate_runtime};
use procwatch_platform::{find_program, logs_dir, procwatch_home, state_dir};
use procwatch_process::{
    list_apps, load_desired_apps, policy_restart_reason, prune_stale_processes, reload_app,
    reload_app_supervised, restart_app, restart_app_supervised, restart_backoff_remaining_now,
    run_app_foreground_until_shutdown, save_desired_apps, scale_app, scale_app_supervised,
    start_app, start_app_supervised, stop_all, stop_app, validate_restart_policy,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap},
    Frame, Terminal,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{watch, Mutex};
use tokio::task::JoinSet;

type DesiredApps = Arc<Mutex<Vec<ResolvedAppSpec>>>;
const IPC_VERSION: u16 = 1;
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const DEFAULT_WATCH_IGNORES: &[&str] = &[
    ".git",
    ".procwatch",
    "node_modules",
    "target",
    "**/.git/**",
    "**/.procwatch/**",
    "**/node_modules/**",
    "**/target/**",
];

#[derive(Debug, Parser)]
#[command(
    name = "procwatch",
    version,
    about = "Rust-first Node.js process manager"
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true)]
    env: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Init {
        #[arg(default_value = "ecosystem.config.json")]
        output: PathBuf,
    },
    Validate {
        config: Option<PathBuf>,
    },
    Doctor {
        config: Option<PathBuf>,
    },
    Start {
        target: Option<PathBuf>,
        #[arg(long)]
        wait: bool,
    },
    Stop {
        name: String,
    },
    Restart {
        target: Option<PathBuf>,
    },
    Reload {
        target: Option<PathBuf>,
    },
    Scale {
        target: PathBuf,
        instances: u16,
    },
    Status {
        name: Option<String>,
    },
    Prune,
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Logs {
        name: Option<String>,
        #[arg(short = 'n', long, default_value_t = 80)]
        lines: usize,
        #[arg(short, long)]
        follow: bool,
    },
    Watch {
        target: Option<PathBuf>,
        #[arg(long, default_value_t = 1000)]
        interval_ms: u64,
    },
    Tui {
        config: Option<PathBuf>,
    },
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(env) = cli.env.as_ref().filter(|value| !value.trim().is_empty()) {
        std::env::set_var("PROCWATCH_ENV", env);
    }
    match cli.command {
        Commands::Init { output } => init(output, cli.json).await,
        Commands::Validate { config } => validate(config, cli.json).await,
        Commands::Doctor { config } => doctor(config, cli.json).await,
        Commands::Start { target, wait } => start(target, wait, cli.json).await,
        Commands::Stop { name } => stop(name, cli.json).await,
        Commands::Restart { target } => restart(target, cli.json).await,
        Commands::Reload { target } => reload(target, cli.json).await,
        Commands::Scale { target, instances } => scale(target, instances, cli.json).await,
        Commands::Status { name } => status(name, cli.json).await,
        Commands::Prune => prune(cli.json).await,
        Commands::Service { command } => service(command, cli.json).await,
        Commands::Daemon { command } => daemon(command, cli.json).await,
        Commands::Logs {
            name,
            lines,
            follow,
        } => logs(name, lines, follow, cli.json).await,
        Commands::Watch {
            target,
            interval_ms,
        } => watch(target, interval_ms, cli.json).await,
        Commands::Tui { config } => tui(config).await,
        Commands::List => list(cli.json).await,
    }
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    Install { config: Option<PathBuf> },
    Start,
    Stop,
    Uninstall,
    Status,
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Start {
        config: Option<PathBuf>,
    },
    Stop,
    Status,
    Ping,
    List,
    #[command(hide = true)]
    Run {
        config: PathBuf,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct IpcEnvelope {
    version: u16,
    request_id: String,
    request: IpcRequest,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum IpcRequest {
    Ping,
    List,
    Shutdown,
    Stop { name: String },
    Start { config: PathBuf },
    StartApps { apps: Vec<ResolvedAppSpec> },
    Restart { config: PathBuf },
    RestartApps { apps: Vec<ResolvedAppSpec> },
    Reload { config: PathBuf },
    ReloadApps { apps: Vec<ResolvedAppSpec> },
    ScaleApps { apps: Vec<ResolvedAppSpec> },
    Prune,
}

#[derive(Debug, Serialize, Deserialize)]
struct IpcResponse {
    version: u16,
    request_id: String,
    ok: bool,
    payload: serde_json::Value,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    procwatch_home: PathBuf,
    platform: DoctorPlatformReport,
    programs: Vec<DoctorProgramReport>,
    directories: Vec<DoctorDirectoryReport>,
    service: DoctorServiceReport,
    config: DoctorConfigReport,
    issues: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DoctorPlatformReport {
    os: &'static str,
    arch: &'static str,
}

#[derive(Debug, Serialize)]
struct DoctorProgramReport {
    name: &'static str,
    path: Option<PathBuf>,
    found: bool,
}

#[derive(Debug, Serialize)]
struct DoctorDirectoryReport {
    name: &'static str,
    path: PathBuf,
    writable: bool,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct DoctorServiceReport {
    backend: &'static str,
    path: Option<PathBuf>,
    installed: bool,
    loaded: Option<bool>,
    active: Option<bool>,
    enabled: Option<bool>,
    detail: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct DoctorConfigReport {
    path: Option<PathBuf>,
    loaded: bool,
    apps: Vec<DoctorAppReport>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct DoctorAppReport {
    name: String,
    ok: bool,
    command: Option<String>,
    issues: Vec<String>,
}

async fn init(output: PathBuf, json: bool) -> Result<()> {
    let sample = r#"{
  "apps": [
    {
      "name": "procwatch-example",
      "script": "server.js",
      "cwd": ".",
      "env": {
        "NODE_ENV": "development"
      }
    }
  ]
}
"#;
    tokio::fs::write(&output, sample).await?;
    if json {
        print_json(serde_json::json!({ "created": output }))?;
    } else {
        println!("Created {}", output.display());
    }
    Ok(())
}

fn validate_app(app: &ResolvedAppSpec) -> Result<()> {
    validate_runtime(app)?;
    validate_restart_policy(app)?;
    Ok(())
}

async fn validate(config: Option<PathBuf>, json: bool) -> Result<()> {
    let config = resolve_config(config)?;
    let apps = load_and_validate_config(&config).await?;

    if json {
        print_json(serde_json::json!({ "config": config, "apps": apps }))?;
    } else {
        println!("Valid config: {}", config.display());
        for app in apps {
            println!("- {}", app.name);
        }
    }
    Ok(())
}

async fn load_and_validate_config(config: &std::path::Path) -> Result<Vec<ResolvedAppSpec>> {
    let apps = load_config_apps(config).await?;
    for app in &apps {
        validate_app(app)?;
    }
    Ok(apps)
}

async fn load_config_apps(path: &std::path::Path) -> Result<Vec<ResolvedAppSpec>> {
    Ok(load_config(path).await?.apps)
}

async fn doctor(config: Option<PathBuf>, json: bool) -> Result<()> {
    let report = doctor_report(config).await;
    if json {
        print_json(serde_json::to_value(&report)?)?;
    } else {
        print_doctor_report(&report);
    }
    Ok(())
}

async fn doctor_report(config: Option<PathBuf>) -> DoctorReport {
    let home = procwatch_home();
    let programs = doctor_programs();
    let directories = vec![
        doctor_directory_report("procwatch_home", home.clone()).await,
        doctor_directory_report("state_dir", state_dir()).await,
        doctor_directory_report("logs_dir", logs_dir()).await,
    ];
    let service = doctor_service_report().await;
    let config = doctor_config_report(config).await;

    let mut issues = Vec::new();
    issues.extend(
        programs
            .iter()
            .filter(|program| program.name == "node" && !program.found)
            .map(|program| format!("missing program: {}", program.name)),
    );
    issues.extend(
        directories
            .iter()
            .filter(|directory| !directory.writable)
            .map(|directory| {
                format!(
                    "{} is not writable: {}",
                    directory.name,
                    directory
                        .error
                        .clone()
                        .unwrap_or_else(|| directory.path.display().to_string())
                )
            }),
    );
    if let Some(error) = &config.error {
        issues.push(error.clone());
    }
    for app in &config.apps {
        for issue in &app.issues {
            issues.push(format!("{}: {issue}", app.name));
        }
    }

    DoctorReport {
        procwatch_home: home,
        platform: DoctorPlatformReport {
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        },
        programs,
        directories,
        service,
        config,
        issues,
    }
}

fn doctor_programs() -> Vec<DoctorProgramReport> {
    ["node", "npm", "pnpm", "yarn", "bun"]
        .into_iter()
        .map(|name| {
            let path = if name == "node" {
                std::env::var_os("PROCWATCH_NODE_PATH")
                    .map(PathBuf::from)
                    .or_else(|| find_program(name, None))
            } else {
                find_program(name, None)
            };
            DoctorProgramReport {
                name,
                found: path.as_ref().map(|path| path.exists()).unwrap_or(false),
                path,
            }
        })
        .collect()
}

async fn doctor_directory_report(name: &'static str, path: PathBuf) -> DoctorDirectoryReport {
    let writable = match ensure_directory_writable(&path).await {
        Ok(()) => true,
        Err(error) => {
            return DoctorDirectoryReport {
                name,
                path,
                writable: false,
                error: Some(error.to_string()),
            };
        }
    };
    DoctorDirectoryReport {
        name,
        path,
        writable,
        error: None,
    }
}

async fn ensure_directory_writable(path: &std::path::Path) -> Result<()> {
    tokio::fs::create_dir_all(path).await?;
    let probe = path.join(format!(
        ".procwatch-doctor-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    tokio::fs::write(&probe, b"ok").await?;
    tokio::fs::remove_file(&probe).await?;
    Ok(())
}

async fn doctor_service_report() -> DoctorServiceReport {
    match service_status_snapshot().await {
        Ok(snapshot) => DoctorServiceReport {
            backend: snapshot.backend,
            path: snapshot.path,
            installed: snapshot.installed,
            loaded: snapshot.loaded,
            active: snapshot.active,
            enabled: snapshot.enabled,
            detail: snapshot.detail,
            error: None,
        },
        Err(error) => DoctorServiceReport {
            backend: service_backend_name(),
            installed: false,
            path: None,
            loaded: None,
            active: None,
            enabled: None,
            detail: None,
            error: Some(error.to_string()),
        },
    }
}

async fn doctor_config_report(config: Option<PathBuf>) -> DoctorConfigReport {
    let explicit_config = config.is_some();
    let resolved = match resolve_config(config) {
        Ok(path) => path,
        Err(error) => {
            return DoctorConfigReport {
                path: None,
                loaded: false,
                apps: Vec::new(),
                error: explicit_config.then(|| format!("config not loaded: {error}")),
            };
        }
    };

    match load_config_apps(&resolved).await {
        Ok(apps) => {
            let app_reports = apps.iter().map(doctor_app_report).collect();
            DoctorConfigReport {
                path: Some(resolved),
                loaded: true,
                apps: app_reports,
                error: None,
            }
        }
        Err(error) => DoctorConfigReport {
            path: Some(resolved),
            loaded: false,
            apps: Vec::new(),
            error: Some(format!("config load failed: {error}")),
        },
    }
}

fn doctor_app_report(app: &ResolvedAppSpec) -> DoctorAppReport {
    let mut issues = Vec::new();
    let command = match resolve_runtime_command(app) {
        Ok(command) => Some(command.display_command()),
        Err(error) => {
            issues.push(error.to_string());
            None
        }
    };
    if let Err(error) = validate_app(app) {
        if !issues.iter().any(|issue| issue == &error.to_string()) {
            issues.push(error.to_string());
        }
    }
    DoctorAppReport {
        name: app.name.clone(),
        ok: issues.is_empty(),
        command,
        issues,
    }
}

fn print_doctor_report(report: &DoctorReport) {
    println!("Procwatch home: {}", report.procwatch_home.display());
    println!("Platform: {}/{}", report.platform.os, report.platform.arch);
    println!();

    println!("Programs:");
    for program in &report.programs {
        println!(
            "- {}: {}",
            program.name,
            program
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "missing".to_string())
        );
    }
    println!();

    println!("Directories:");
    for directory in &report.directories {
        println!(
            "- {}: {} ({})",
            directory.name,
            directory.path.display(),
            if directory.writable {
                "writable"
            } else {
                "not writable"
            }
        );
        if let Some(error) = &directory.error {
            println!("  {error}");
        }
    }
    println!();

    println!(
        "Service backend: {}{}",
        report.service.backend,
        report
            .service
            .path
            .as_ref()
            .map(|path| format!(" ({})", path.display()))
            .unwrap_or_default()
    );
    println!(
        "Service installed: {}",
        if report.service.installed {
            "yes"
        } else {
            "no"
        }
    );
    if let Some(loaded) = report.service.loaded {
        println!("Service loaded: {}", if loaded { "yes" } else { "no" });
    }
    if let Some(active) = report.service.active {
        println!("Service active: {}", if active { "yes" } else { "no" });
    }
    if let Some(enabled) = report.service.enabled {
        println!("Service enabled: {}", if enabled { "yes" } else { "no" });
    }
    if let Some(detail) = &report.service.detail {
        println!("Service detail: {detail}");
    }
    if let Some(error) = &report.service.error {
        println!("Service error: {error}");
    }
    println!();

    match (&report.config.path, report.config.loaded) {
        (Some(path), true) => println!("Config: loaded {}", path.display()),
        (Some(path), false) => println!("Config: failed {}", path.display()),
        (None, _) => println!("Config: not loaded"),
    }
    if let Some(error) = &report.config.error {
        println!("Config error: {error}");
    }
    for app in &report.config.apps {
        println!("- {}: {}", app.name, if app.ok { "ok" } else { "issue" });
        if let Some(command) = &app.command {
            println!("  command: {command}");
        }
        for issue in &app.issues {
            println!("  issue: {issue}");
        }
    }
    println!();

    if report.issues.is_empty() {
        println!("Doctor result: ok");
    } else {
        println!("Doctor result: {} issue(s)", report.issues.len());
        for issue in &report.issues {
            println!("- {issue}");
        }
    }
}

fn service_backend_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "launchd"
    }
    #[cfg(target_os = "linux")]
    {
        "systemd-user"
    }
    #[cfg(windows)]
    {
        "windows-service"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        "unsupported"
    }
}

async fn start(target: Option<PathBuf>, wait: bool, json: bool) -> Result<()> {
    let apps = resolve_apps(target).await?;
    if wait {
        for app in &apps {
            validate_app(app)?;
        }
        if json {
            print_json(serde_json::json!({ "supervising": apps }))?;
        } else {
            for app in &apps {
                println!("Supervising {}", app.name);
            }
        }
        return supervise_foreground_apps(apps).await;
    }

    if let Some(response) = try_daemon_request(IpcRequest::StartApps { apps: apps.clone() }).await?
    {
        return print_ipc_response(response, json);
    }

    let mut started = Vec::new();
    for app in apps {
        validate_app(&app)?;
        started.push(start_app(&app).await?);
    }

    if json {
        print_json(serde_json::json!({ "started": started }))?;
    } else {
        for process in started {
            println!("Started {} pid={}", process.name, process.pid);
        }
    }
    Ok(())
}

async fn supervise_foreground_apps(apps: Vec<ResolvedAppSpec>) -> Result<()> {
    let (shutdown_tx, _) = watch::channel(false);
    let mut tasks = JoinSet::new();
    for app in apps {
        let shutdown = shutdown_tx.subscribe();
        tasks.spawn(async move { run_app_foreground_until_shutdown(&app, shutdown).await });
    }

    let mut first_error: Option<anyhow::Error> = None;
    let shutdown_signal = shutdown_signal();
    tokio::pin!(shutdown_signal);
    loop {
        tokio::select! {
            _ = &mut shutdown_signal, if !tasks.is_empty() => {
                let _ = shutdown_tx.send(true);
                break;
            }
            result = tasks.join_next(), if !tasks.is_empty() => {
                let Some(result) = result else {
                    break;
                };
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        first_error = Some(error.into());
                        let _ = shutdown_tx.send(true);
                        break;
                    }
                    Err(error) => {
                        first_error = Some(anyhow::anyhow!("foreground supervisor task failed: {error}"));
                        let _ = shutdown_tx.send(true);
                        break;
                    }
                }
            }
        }

        if tasks.is_empty() {
            break;
        }
    }

    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) if first_error.is_none() => first_error = Some(error.into()),
            Err(error) if first_error.is_none() => {
                first_error = Some(anyhow::anyhow!(
                    "foreground supervisor task failed: {error}"
                ));
            }
            _ => {}
        }
    }

    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(())
}

async fn stop(name: String, json: bool) -> Result<()> {
    if let Some(response) = try_daemon_request(IpcRequest::Stop { name: name.clone() }).await? {
        return print_ipc_response(response, json);
    }

    let stopped = if name == "all" {
        let stopped = stop_all().await?;
        if json {
            print_json(serde_json::json!({ "stopped": stopped }))?;
        } else if stopped.is_empty() {
            println!("No managed processes");
        } else {
            for process in stopped {
                println!("Stopped {} pid={}", process.name, process.pid);
            }
        }
        return Ok(());
    } else {
        stop_app(&name).await?
    };

    if json {
        print_json(serde_json::json!({ "stopped": stopped }))?;
    } else if let Some(process) = stopped {
        println!("Stopped {} pid={}", process.name, process.pid);
    } else {
        println!("No managed process named {name}");
    }
    Ok(())
}

async fn restart(target: Option<PathBuf>, json: bool) -> Result<()> {
    let apps = resolve_apps(target).await?;
    if let Some(response) =
        try_daemon_request(IpcRequest::RestartApps { apps: apps.clone() }).await?
    {
        return print_ipc_response(response, json);
    }

    let mut restarted = Vec::new();
    for app in apps {
        validate_app(&app)?;
        restarted.push(restart_app(&app).await?);
    }

    if json {
        print_json(serde_json::json!({ "restarted": restarted }))?;
    } else {
        for process in restarted {
            println!("Restarted {} pid={}", process.name, process.pid);
        }
    }
    Ok(())
}

async fn reload(target: Option<PathBuf>, json: bool) -> Result<()> {
    let apps = resolve_apps(target).await?;
    if let Some(response) =
        try_daemon_request(IpcRequest::ReloadApps { apps: apps.clone() }).await?
    {
        return print_ipc_response(response, json);
    }

    let mut reloaded = Vec::new();
    for app in apps {
        validate_app(&app)?;
        reloaded.push(reload_app(&app).await?);
    }

    if json {
        print_json(serde_json::json!({ "reloaded": reloaded }))?;
    } else {
        for process in reloaded {
            println!("Reloaded {} pid={}", process.name, process.pid);
        }
    }
    Ok(())
}

async fn scale(target: PathBuf, instances: u16, json: bool) -> Result<()> {
    let mut apps = resolve_apps(Some(target)).await?;
    for app in &mut apps {
        app.instances = Instances::Count(instances.max(1));
    }

    if let Some(response) = try_daemon_request(IpcRequest::ScaleApps { apps: apps.clone() }).await?
    {
        return print_ipc_response(response, json);
    }

    let mut scaled = Vec::new();
    for app in apps {
        validate_app(&app)?;
        scaled.push(scale_app(&app).await?);
    }

    if json {
        print_json(serde_json::json!({ "scaled": scaled, "instances": instances.max(1) }))?;
    } else {
        for process in scaled {
            println!(
                "Scaled {} to {} instance(s), supervisor pid={}",
                process.name,
                instances.max(1),
                process.pid
            );
        }
    }
    Ok(())
}

async fn status(name: Option<String>, json: bool) -> Result<()> {
    let processes = current_processes().await?;
    let selected: Vec<_> = processes
        .into_iter()
        .filter(|process| {
            name.as_ref()
                .map(|name| name == &process.name)
                .unwrap_or(true)
        })
        .collect();

    if json {
        print_json(serde_json::json!({ "processes": selected, "count": selected.len() }))?;
    } else if selected.is_empty() {
        if let Some(name) = name {
            println!("No managed process named {name}");
        } else {
            println!("No managed processes");
        }
    } else {
        for (index, process) in selected.iter().enumerate() {
            if index > 0 {
                println!();
            }
            print_process_status(process);
        }
    }
    Ok(())
}

async fn prune(json: bool) -> Result<()> {
    if let Some(response) = try_daemon_request(IpcRequest::Prune).await? {
        return print_ipc_response(response, json);
    }

    let removed = prune_stale_processes().await?;
    if json {
        print_json(serde_json::json!({ "removed": removed, "count": removed.len() }))?;
    } else if removed.is_empty() {
        println!("No stale managed processes");
    } else {
        for process in removed {
            println!("Pruned {} pid={}", process.name, process.pid);
        }
    }
    Ok(())
}

async fn logs(name: Option<String>, lines: usize, follow: bool, json: bool) -> Result<()> {
    let processes = list_apps().await?;
    let selected: Vec<_> = processes
        .into_iter()
        .filter(|process| {
            name.as_ref()
                .map(|name| name == &process.name)
                .unwrap_or(true)
        })
        .collect();

    if json {
        let mut entries = Vec::new();
        for process in selected {
            entries.push(serde_json::json!({
                "name": process.name,
                "out": tail_file(&process.out_log, lines).await?,
                "err": tail_file(&process.err_log, lines).await?
            }));
        }
        print_json(serde_json::json!({ "logs": entries }))?;
        return Ok(());
    }

    if selected.is_empty() {
        println!("No matching managed processes");
        return Ok(());
    }

    for process in selected {
        println!(
            "==> {} stdout ({})",
            process.name,
            process.out_log.display()
        );
        for line in tail_file(&process.out_log, lines).await? {
            println!("{}", decorate_log_line(&process, "stdout", &line));
        }
        println!(
            "==> {} stderr ({})",
            process.name,
            process.err_log.display()
        );
        for line in tail_file(&process.err_log, lines).await? {
            println!("{}", decorate_log_line(&process, "stderr", &line));
        }
    }
    if follow {
        let mut offsets = std::collections::BTreeMap::new();
        for process in list_apps().await? {
            offsets.insert(
                process.out_log.clone(),
                log_follow_state(&process.out_log).await,
            );
            offsets.insert(
                process.err_log.clone(),
                log_follow_state(&process.err_log).await,
            );
        }
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let processes = list_apps().await?;
            for process in processes.into_iter().filter(|process| {
                name.as_ref()
                    .map(|name| name == &process.name)
                    .unwrap_or(true)
            }) {
                print_new_log_bytes(&process, "stdout", &process.out_log, &mut offsets).await?;
                print_new_log_bytes(&process, "stderr", &process.err_log, &mut offsets).await?;
            }
        }
    }
    Ok(())
}

async fn list(json: bool) -> Result<()> {
    if let Some(response) = try_daemon_request(IpcRequest::List).await? {
        return print_ipc_response(response, json);
    }

    let processes = list_apps().await?;
    if json {
        print_json(serde_json::json!({ "processes": processes }))?;
    } else if processes.is_empty() {
        println!("No managed processes");
    } else {
        println!(
            "{:<24} {:<8} {:<10} {:<9} Command",
            "Name", "PID", "Status", "Restarts"
        );
        for process in processes {
            println!(
                "{:<24} {:<8} {:<10} {:<9} {}",
                process.name,
                process.pid,
                format!("{:?}", process.status).to_lowercase(),
                process.restart_count,
                process.command.display_command()
            );
        }
    }
    Ok(())
}

async fn current_processes() -> Result<Vec<ManagedProcess>> {
    if let Some(response) = try_daemon_request(IpcRequest::List).await? {
        if !response.ok {
            anyhow::bail!(response
                .error
                .unwrap_or_else(|| "daemon list request failed".to_string()));
        }
        return processes_from_payload(response.payload);
    }
    list_apps().await.map_err(Into::into)
}

fn processes_from_payload(payload: serde_json::Value) -> Result<Vec<ManagedProcess>> {
    let processes = payload
        .get("processes")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    serde_json::from_value(processes).map_err(Into::into)
}

fn print_process_status(process: &ManagedProcess) {
    println!("Name: {}", process.name);
    println!("PID: {}", process.pid);
    println!("Status: {}", format!("{:?}", process.status).to_lowercase());
    println!("Restarts: {}", process.restart_count);
    println!("CWD: {}", process.cwd.display());
    println!("Started: {}", process.started_at);
    println!("Command: {}", process.command.display_command());
    println!("Stdout: {}", process.out_log.display());
    println!("Stderr: {}", process.err_log.display());
    if let Some(worker) = &process.worker_id {
        println!("Worker: {worker}");
    }
    if let Some(uptime_ms) = process.uptime_ms {
        println!("Uptime: {} ms", uptime_ms);
    }
    if let Some(memory_bytes) = process.memory_bytes {
        println!("Memory: {} bytes", memory_bytes);
    }
    if let Some(cpu_percent) = process.cpu_percent {
        println!("CPU: {:.1}%", cpu_percent);
    }
    if let Some(code) = process.last_exit_code {
        println!("Last exit code: {code}");
    }
    if let Some(signal) = process.last_exit_signal {
        println!("Last exit signal: {signal}");
    }
    if let Some(last_exit_at) = process.last_exit_at {
        println!("Last exit at: {last_exit_at}");
    }
}

async fn service(command: ServiceCommand, json: bool) -> Result<()> {
    match command {
        ServiceCommand::Install { config } => service_install(config, json).await,
        ServiceCommand::Start => service_start(json).await,
        ServiceCommand::Stop => service_stop(json).await,
        ServiceCommand::Uninstall => service_uninstall(json).await,
        ServiceCommand::Status => service_status(json).await,
    }
}

async fn daemon(command: DaemonCommand, json: bool) -> Result<()> {
    match command {
        DaemonCommand::Start { config } => daemon_start(config, json).await,
        DaemonCommand::Stop => daemon_stop(json).await,
        DaemonCommand::Status => daemon_status(json).await,
        DaemonCommand::Ping => daemon_ipc(IpcRequest::Ping, json).await,
        DaemonCommand::List => daemon_ipc(IpcRequest::List, json).await,
        DaemonCommand::Run { config } => daemon_run(config).await,
    }
}

async fn daemon_start(config: Option<PathBuf>, json: bool) -> Result<()> {
    let config = resolve_config(config)?;
    let apps = load_and_validate_config(&config).await?;
    if let Ok(response) = send_ipc(IpcRequest::Ping).await {
        if response.ok {
            let pid = daemon_pid().await;
            if json {
                print_json(serde_json::json!({ "already_running": true, "pid": pid }))?;
            } else {
                println!(
                    "procwatch daemon already running{}",
                    pid.map(|pid| format!(" pid={pid}")).unwrap_or_default()
                );
            }
            return Ok(());
        }
    }

    let exe = std::env::current_exe()?;
    let dir = procwatch_home().join("daemon");
    tokio::fs::create_dir_all(&dir).await?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("daemon.log"))?;
    let err = log.try_clone()?;
    let child = std::process::Command::new(exe)
        .arg("daemon")
        .arg("run")
        .arg(&config)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log))
        .stderr(std::process::Stdio::from(err))
        .spawn()?;
    let pid = child.id();
    tokio::fs::write(dir.join("daemon.pid"), pid.to_string()).await?;
    wait_for_daemon_ready().await?;
    if json {
        print_json(serde_json::json!({ "pid": pid, "config": config, "apps": apps }))?;
    } else {
        println!("Started procwatch daemon pid={pid}");
    }
    Ok(())
}

async fn daemon_stop(json: bool) -> Result<()> {
    let pid_path = procwatch_home().join("daemon").join("daemon.pid");
    let Some(pid) = daemon_pid().await else {
        let _ = tokio::fs::remove_file(&pid_path).await;
        cleanup_ipc_file().await;
        if json {
            print_json(serde_json::json!({ "already_stopped": true }))?;
        } else {
            println!("procwatch daemon not started");
        }
        return Ok(());
    };
    let _ = send_ipc(IpcRequest::Shutdown).await;
    procwatch_platform::terminate_process(pid).await?;
    wait_for_process_exit(pid, 30, std::time::Duration::from_millis(100)).await;
    if procwatch_platform::is_process_alive(pid) {
        procwatch_platform::force_kill_process(pid).await?;
        wait_for_process_exit(pid, 10, std::time::Duration::from_millis(100)).await;
    }
    let _ = tokio::fs::remove_file(&pid_path).await;
    cleanup_ipc_file().await;
    if json {
        print_json(serde_json::json!({ "stopped": pid }))?;
    } else {
        println!("Stopped procwatch daemon pid={pid}");
    }
    Ok(())
}

async fn daemon_status(json: bool) -> Result<()> {
    let pid = daemon_pid().await;
    let running = pid
        .map(procwatch_platform::is_process_alive)
        .unwrap_or(false);
    let ipc = send_ipc(IpcRequest::Ping)
        .await
        .map(|response| response.ok)
        .unwrap_or(false);
    if json {
        print_json(serde_json::json!({ "pid": pid, "running": running, "ipc": ipc }))?;
    } else if let Some(pid) = pid {
        println!(
            "procwatch daemon pid={pid} {} ipc={}",
            if running { "running" } else { "stale" },
            if ipc { "ready" } else { "unavailable" }
        );
    } else {
        println!("procwatch daemon not started");
    }
    Ok(())
}

async fn daemon_pid() -> Option<u32> {
    let pid_path = procwatch_home().join("daemon").join("daemon.pid");
    tokio::fs::read_to_string(&pid_path)
        .await
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
}

async fn wait_for_daemon_ready() -> Result<()> {
    for _ in 0..50 {
        if let Ok(response) = send_ipc(IpcRequest::Ping).await {
            if response.ok {
                return Ok(());
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!("daemon did not become ready within 5 seconds")
}

async fn wait_for_process_exit(pid: u32, attempts: usize, interval: std::time::Duration) {
    for _ in 0..attempts {
        if !procwatch_platform::is_process_alive(pid) {
            return;
        }
        tokio::time::sleep(interval).await;
    }
}

async fn cleanup_ipc_file() {
    let _ = tokio::fs::remove_file(ipc_path()).await;
}

async fn daemon_ipc(request: IpcRequest, json: bool) -> Result<()> {
    let response = send_ipc(request).await?;
    print_ipc_response(response, json)
}

async fn daemon_run(config: PathBuf) -> Result<()> {
    let config_apps = load_config_apps(&config).await?;
    let mut apps = load_desired_apps().await?;
    merge_app_specs(&mut apps, config_apps);
    for app in &apps {
        validate_app(app)?;
        start_app_supervised(app).await?;
    }
    save_desired_apps(&apps).await?;
    let desired = Arc::new(Mutex::new(apps));

    let listener = bind_ipc().await?;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let apps = desired.lock().await.clone();
                let processes = match list_apps().await {
                    Ok(processes) => processes,
                    Err(error) => {
                        eprintln!("failed to list managed apps during daemon reconciliation: {error}");
                        continue;
                    }
                };
                for app in &apps {
                    let process = processes.iter().find(|process| process.name == app.name);
                    match process {
                        Some(process) if matches!(process.status, procwatch_core::ProcessStatus::Running) => {
                            match policy_restart_reason(app, process) {
                                Ok(Some(reason)) => {
                                    eprintln!("restarting {}: {reason}", app.name);
                                    if let Err(error) = restart_app_supervised(app).await {
                                        eprintln!("failed to restart {}: {error}", app.name);
                                    }
                                }
                                Ok(None) => {}
                                Err(error) => {
                                    eprintln!("failed to evaluate restart policy for {}: {error}", app.name);
                                }
                            }
                        }
                        Some(process) if app.restart.autorestart => {
                            match restart_backoff_remaining_now(app, process) {
                                Ok(Some(_remaining)) => {}
                                Ok(None) => {
                                    if let Err(error) = start_app_supervised(app).await {
                                        eprintln!("failed to start {} during daemon reconciliation: {error}", app.name);
                                    }
                                }
                                Err(error) => {
                                    eprintln!("restart limit reached for {}: {error}", app.name);
                                }
                            }
                        }
                        None if app.restart.autorestart => {
                            if let Err(error) = start_app_supervised(app).await {
                                eprintln!("failed to start {} during daemon reconciliation: {error}", app.name);
                            }
                        }
                        _ => {}
                    }
                }
            }
            result = accept_ipc(&listener) => {
                if let Ok(stream) = result {
                    let desired = desired.clone();
                    tokio::spawn(async move {
                        let _ = handle_ipc(stream, desired).await;
                    });
                }
            }
            _ = &mut shutdown => {
                let _ = stop_all().await;
                break;
            },
        }
    }
    Ok(())
}

async fn tui(config: Option<PathBuf>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_tui(&mut terminal, config).await;
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiFocus {
    Processes,
    ConfigApps,
}

struct TuiModel {
    processes: Vec<ManagedProcess>,
    config_apps: Vec<ResolvedAppSpec>,
    focus: TuiFocus,
    selected_process: usize,
    selected_config_app: usize,
    logs: Vec<String>,
    message: String,
}

async fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    config: Option<PathBuf>,
) -> Result<()> {
    let (config_apps, message) = load_tui_config_apps(config).await?;
    let mut model = TuiModel {
        processes: Vec::new(),
        config_apps,
        focus: TuiFocus::Processes,
        selected_process: 0,
        selected_config_app: 0,
        logs: Vec::new(),
        message,
    };
    refresh_tui_model(&mut model).await?;
    let mut last_refresh = std::time::Instant::now();

    loop {
        terminal.draw(|frame| render_tui(frame, &model))?;
        if event::poll(std::time::Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Tab if !model.config_apps.is_empty() => {
                        model.focus = match model.focus {
                            TuiFocus::Processes => TuiFocus::ConfigApps,
                            TuiFocus::ConfigApps => TuiFocus::Processes,
                        };
                    }
                    KeyCode::Up => move_tui_selection(&mut model, -1),
                    KeyCode::Down => move_tui_selection(&mut model, 1),
                    KeyCode::Char('a') => {
                        model.message = tui_start_all(&model.config_apps).await;
                        refresh_tui_model(&mut model).await?;
                    }
                    KeyCode::Char('s') => {
                        model.message = tui_start_selected(&model).await;
                        refresh_tui_model(&mut model).await?;
                    }
                    KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
                        model.message = tui_stop_selected(&model).await;
                        refresh_tui_model(&mut model).await?;
                    }
                    KeyCode::Char('r') => {
                        model.message = tui_restart_selected(&model).await;
                        refresh_tui_model(&mut model).await?;
                    }
                    KeyCode::Char('l') => {
                        model.message = tui_reload_selected(&model).await;
                        refresh_tui_model(&mut model).await?;
                    }
                    KeyCode::Char('+') | KeyCode::Char('=') => {
                        model.message = tui_scale_selected(&mut model, 1).await;
                        refresh_tui_model(&mut model).await?;
                    }
                    KeyCode::Char('-') => {
                        model.message = tui_scale_selected(&mut model, -1).await;
                        refresh_tui_model(&mut model).await?;
                    }
                    KeyCode::Char('R') => {
                        model.message = "refreshed".to_string();
                        refresh_tui_model(&mut model).await?;
                    }
                    _ => {}
                }
                last_refresh = std::time::Instant::now();
            }
        }

        if last_refresh.elapsed() >= std::time::Duration::from_secs(1) {
            refresh_tui_model(&mut model).await?;
            last_refresh = std::time::Instant::now();
        }
    }
    Ok(())
}

async fn load_tui_config_apps(config: Option<PathBuf>) -> Result<(Vec<ResolvedAppSpec>, String)> {
    if let Some(config) = config {
        return Ok((
            resolve_apps(Some(config)).await?,
            "config loaded".to_string(),
        ));
    }

    match resolve_config(None) {
        Ok(config) => Ok((
            load_config_apps(&config).await?,
            format!("config loaded from {}", config.display()),
        )),
        Err(_) => Ok((
            Vec::new(),
            "no config loaded; pass procwatch tui <config> for start/restart/reload actions"
                .to_string(),
        )),
    }
}

async fn refresh_tui_model(model: &mut TuiModel) -> Result<()> {
    model.processes = current_processes().await.unwrap_or_default();
    if model.selected_process >= model.processes.len() {
        model.selected_process = model.processes.len().saturating_sub(1);
    }
    if model.selected_config_app >= model.config_apps.len() {
        model.selected_config_app = model.config_apps.len().saturating_sub(1);
    }
    model.logs = selected_tui_logs(model)
        .await
        .unwrap_or_else(|error| vec![format!("failed to read logs: {error}")]);
    Ok(())
}

fn move_tui_selection(model: &mut TuiModel, delta: isize) {
    let len = match model.focus {
        TuiFocus::Processes => model.processes.len(),
        TuiFocus::ConfigApps => model.config_apps.len(),
    };
    if len == 0 {
        return;
    }
    let current = match model.focus {
        TuiFocus::Processes => model.selected_process,
        TuiFocus::ConfigApps => model.selected_config_app,
    };
    let next = (current as isize + delta).clamp(0, len as isize - 1) as usize;
    match model.focus {
        TuiFocus::Processes => model.selected_process = next,
        TuiFocus::ConfigApps => model.selected_config_app = next,
    }
}

async fn selected_tui_logs(model: &TuiModel) -> Result<Vec<String>> {
    let Some(process) = model.processes.get(model.selected_process) else {
        return Ok(vec!["no selected process".to_string()]);
    };
    let mut lines = Vec::new();
    lines.push(format!("stdout {}", process.out_log.display()));
    lines.extend(
        tail_file(&process.out_log, 10)
            .await?
            .into_iter()
            .map(|line| decorate_log_line(process, "stdout", &line)),
    );
    if process.err_log != process.out_log {
        lines.push(format!("stderr {}", process.err_log.display()));
        lines.extend(
            tail_file(&process.err_log, 8)
                .await?
                .into_iter()
                .map(|line| decorate_log_line(process, "stderr", &line)),
        );
    }
    Ok(lines)
}

fn render_tui(frame: &mut Frame<'_>, model: &TuiModel) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(area);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(vertical[1]);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(body[0]);

    frame.render_widget(
        Paragraph::new(format!(
            "Procwatch TUI  |  {} process(es)  |  {} config app(s)",
            model.processes.len(),
            model.config_apps.len()
        ))
        .block(Block::default().borders(Borders::ALL)),
        vertical[0],
    );
    render_process_table(frame, left[0], model);
    render_config_table(frame, left[1], model);
    render_detail_panel(frame, body[1], model);
    frame.render_widget(
        Paragraph::new(format!(
            "Tab switch  Up/Down select  a start all  s start  x stop  r restart  l reload  +/- scale  R refresh  q quit  |  {}",
            model.message
        ))
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL)),
        vertical[2],
    );
}

fn render_process_table(frame: &mut Frame<'_>, area: Rect, model: &TuiModel) {
    let rows = model.processes.iter().map(|process| {
        Row::new([
            Cell::from(process.name.clone()),
            Cell::from(process.pid.to_string()),
            Cell::from(process_status_label(&process.status)),
            Cell::from(process.restart_count.to_string()),
            Cell::from(process.command.display_command()),
        ])
    });
    let title = if model.focus == TuiFocus::Processes {
        "Processes *"
    } else {
        "Processes"
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(22),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Min(18),
        ],
    )
    .header(
        Row::new(["Name", "PID", "Status", "Restarts", "Command"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(Block::default().title(title).borders(Borders::ALL))
    .row_highlight_style(Style::default().bg(Color::DarkGray));
    let mut state = TableState::default();
    if !model.processes.is_empty() {
        state.select(Some(model.selected_process));
    }
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_config_table(frame: &mut Frame<'_>, area: Rect, model: &TuiModel) {
    let rows = model.config_apps.iter().map(|app| {
        Row::new([
            Cell::from(app.name.clone()),
            Cell::from(exec_mode_label(app.exec_mode)),
            Cell::from(instances_label(&app.instances)),
            Cell::from(app.cwd.display().to_string()),
        ])
    });
    let title = if model.focus == TuiFocus::ConfigApps {
        "Config Apps *"
    } else {
        "Config Apps"
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(22),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Min(20),
        ],
    )
    .header(
        Row::new(["Name", "Mode", "Instances", "CWD"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(Block::default().title(title).borders(Borders::ALL))
    .row_highlight_style(Style::default().bg(Color::DarkGray));
    let mut state = TableState::default();
    if !model.config_apps.is_empty() {
        state.select(Some(model.selected_config_app));
    }
    frame.render_stateful_widget(table, area, &mut state);
}

fn render_detail_panel(frame: &mut Frame<'_>, area: Rect, model: &TuiModel) {
    let mut text = Vec::new();
    if let Some(process) = model.processes.get(model.selected_process) {
        text.push(format!("Name: {}", process.name));
        text.push(format!("PID: {}", process.pid));
        text.push(format!("Status: {}", process_status_label(&process.status)));
        text.push(format!("Restarts: {}", process.restart_count));
        text.push(format!("CWD: {}", process.cwd.display()));
        text.push(format!("Started: {}", process.started_at));
        if let Some(worker) = &process.worker_id {
            text.push(format!("Worker: {worker}"));
        }
        if let Some(uptime_ms) = process.uptime_ms {
            text.push(format!("Uptime: {uptime_ms} ms"));
        }
        if let Some(memory_bytes) = process.memory_bytes {
            text.push(format!("Memory: {memory_bytes} bytes"));
        }
        if let Some(cpu_percent) = process.cpu_percent {
            text.push(format!("CPU: {cpu_percent:.1}%"));
        }
        if let Some(code) = process.last_exit_code {
            text.push(format!("Last exit code: {code}"));
        }
        if let Some(signal) = process.last_exit_signal {
            text.push(format!("Last exit signal: {signal}"));
        }
        if let Some(last_exit_at) = process.last_exit_at {
            text.push(format!("Last exit at: {last_exit_at}"));
        }
        text.push(String::new());
    }
    text.extend(model.logs.iter().cloned());
    frame.render_widget(
        Paragraph::new(text.join("\n"))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title("Details / Logs")
                    .borders(Borders::ALL),
            ),
        area,
    );
}

fn process_status_label(status: &ProcessStatus) -> String {
    format!("{status:?}").to_lowercase()
}

fn exec_mode_label(mode: ExecMode) -> &'static str {
    match mode {
        ExecMode::Fork => "fork",
        ExecMode::Cluster => "cluster",
    }
}

fn instances_label(instances: &Instances) -> String {
    match instances {
        Instances::Count(value) => value.to_string(),
        Instances::Max(value) => value.clone(),
    }
}

async fn tui_start_all(apps: &[ResolvedAppSpec]) -> String {
    if apps.is_empty() {
        return "no config apps loaded".to_string();
    }
    match manage_start_apps(apps.to_vec()).await {
        Ok(count) => format!("started {count} app(s)"),
        Err(error) => format!("start failed: {error}"),
    }
}

async fn tui_start_selected(model: &TuiModel) -> String {
    let Some(app) = selected_tui_app(model) else {
        return "no config app selected".to_string();
    };
    match manage_start_apps(vec![app.clone()]).await {
        Ok(_) => format!("started {}", app.name),
        Err(error) => format!("start failed: {error}"),
    }
}

async fn tui_stop_selected(model: &TuiModel) -> String {
    let Some(process) = model.processes.get(model.selected_process) else {
        return "no process selected".to_string();
    };
    match manage_stop_process(&process.name).await {
        Ok(true) => format!("stopped {}", process.name),
        Ok(false) => format!("no managed process named {}", process.name),
        Err(error) => format!("stop failed: {error}"),
    }
}

async fn tui_restart_selected(model: &TuiModel) -> String {
    let Some(app) = selected_tui_app(model) else {
        return "restart requires a loaded config app".to_string();
    };
    match manage_restart_apps(vec![app.clone()]).await {
        Ok(_) => format!("restarted {}", app.name),
        Err(error) => format!("restart failed: {error}"),
    }
}

async fn tui_reload_selected(model: &TuiModel) -> String {
    let Some(app) = selected_tui_app(model) else {
        return "reload requires a loaded config app".to_string();
    };
    match manage_reload_apps(vec![app.clone()]).await {
        Ok(_) => format!("reloaded {}", app.name),
        Err(error) => format!("reload failed: {error}"),
    }
}

async fn tui_scale_selected(model: &mut TuiModel, delta: i32) -> String {
    let Some(index) = selected_tui_app_index(model) else {
        return "scale requires a loaded config app".to_string();
    };
    let mut app = model.config_apps[index].clone();
    if app.exec_mode != ExecMode::Cluster {
        return format!("{} is not a cluster app", app.name);
    }

    let current = resolve_instances(&app.instances) as i32;
    let next = (current + delta).clamp(1, u16::MAX as i32) as u16;
    if next as i32 == current {
        return format!("{} already has {current} worker(s)", app.name);
    }
    app.instances = Instances::Count(next);

    match manage_scale_apps(vec![app.clone()]).await {
        Ok(_) => {
            model.config_apps[index] = app.clone();
            format!("scaled {} to {next} worker(s)", app.name)
        }
        Err(error) => format!("scale failed: {error}"),
    }
}

fn selected_tui_app(model: &TuiModel) -> Option<&ResolvedAppSpec> {
    selected_tui_app_index(model).and_then(|index| model.config_apps.get(index))
}

fn selected_tui_app_index(model: &TuiModel) -> Option<usize> {
    match model.focus {
        TuiFocus::ConfigApps => (model.selected_config_app < model.config_apps.len())
            .then_some(model.selected_config_app),
        TuiFocus::Processes => {
            let process = model.processes.get(model.selected_process)?;
            model
                .config_apps
                .iter()
                .position(|app| app.name == process.name)
        }
    }
}

async fn manage_start_apps(apps: Vec<ResolvedAppSpec>) -> Result<usize> {
    for app in &apps {
        validate_app(app)?;
    }
    if let Some(response) = try_daemon_request(IpcRequest::StartApps { apps: apps.clone() }).await?
    {
        return ipc_payload_count(response, "started");
    }
    let mut count = 0;
    for app in &apps {
        start_app(app).await?;
        count += 1;
    }
    Ok(count)
}

async fn manage_restart_apps(apps: Vec<ResolvedAppSpec>) -> Result<usize> {
    for app in &apps {
        validate_app(app)?;
    }
    if let Some(response) =
        try_daemon_request(IpcRequest::RestartApps { apps: apps.clone() }).await?
    {
        return ipc_payload_count(response, "restarted");
    }
    let mut count = 0;
    for app in &apps {
        restart_app(app).await?;
        count += 1;
    }
    Ok(count)
}

async fn manage_reload_apps(apps: Vec<ResolvedAppSpec>) -> Result<usize> {
    for app in &apps {
        validate_app(app)?;
    }
    if let Some(response) =
        try_daemon_request(IpcRequest::ReloadApps { apps: apps.clone() }).await?
    {
        return ipc_payload_count(response, "reloaded");
    }
    let mut count = 0;
    for app in &apps {
        reload_app(app).await?;
        count += 1;
    }
    Ok(count)
}

async fn manage_scale_apps(apps: Vec<ResolvedAppSpec>) -> Result<usize> {
    for app in &apps {
        validate_app(app)?;
    }
    if let Some(response) = try_daemon_request(IpcRequest::ScaleApps { apps: apps.clone() }).await?
    {
        return ipc_payload_count(response, "scaled");
    }
    let mut count = 0;
    for app in &apps {
        scale_app(app).await?;
        count += 1;
    }
    Ok(count)
}

async fn manage_stop_process(name: &str) -> Result<bool> {
    if let Some(response) = try_daemon_request(IpcRequest::Stop {
        name: name.to_string(),
    })
    .await?
    {
        let payload = ensure_ipc_ok(response)?;
        return Ok(!payload
            .get("stopped")
            .map(serde_json::Value::is_null)
            .unwrap_or(false));
    }
    Ok(stop_app(name).await?.is_some())
}

fn ipc_payload_count(response: IpcResponse, key: &str) -> Result<usize> {
    let payload = ensure_ipc_ok(response)?;
    Ok(payload
        .get(key)
        .and_then(|value| value.as_array())
        .map(Vec::len)
        .unwrap_or(0))
}

fn ensure_ipc_ok(response: IpcResponse) -> Result<serde_json::Value> {
    if response.ok {
        Ok(response.payload)
    } else {
        anyhow::bail!(response
            .error
            .unwrap_or_else(|| "daemon request failed".to_string()));
    }
}

#[cfg(unix)]
type IpcListener = tokio::net::UnixListener;
#[cfg(unix)]
type IpcStream = tokio::net::UnixStream;

#[cfg(windows)]
type IpcListener = tokio::net::TcpListener;
#[cfg(windows)]
type IpcStream = tokio::net::TcpStream;

#[cfg(unix)]
async fn bind_ipc() -> Result<IpcListener> {
    let path = ipc_path();
    if path.exists() {
        let _ = tokio::fs::remove_file(&path).await;
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    Ok(tokio::net::UnixListener::bind(path)?)
}

#[cfg(windows)]
async fn bind_ipc() -> Result<IpcListener> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    if let Some(parent) = ipc_path().parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(ipc_path(), addr.to_string()).await?;
    Ok(listener)
}

async fn accept_ipc(listener: &IpcListener) -> Result<IpcStream> {
    #[cfg(unix)]
    {
        let (stream, _) = listener.accept().await?;
        Ok(stream)
    }
    #[cfg(windows)]
    {
        let (stream, _) = listener.accept().await?;
        Ok(stream)
    }
}

async fn connect_ipc() -> Result<IpcStream> {
    #[cfg(unix)]
    {
        Ok(tokio::net::UnixStream::connect(ipc_path()).await?)
    }
    #[cfg(windows)]
    {
        let addr = tokio::fs::read_to_string(ipc_path()).await?;
        Ok(tokio::net::TcpStream::connect(addr.trim()).await?)
    }
}

async fn send_ipc(request: IpcRequest) -> Result<IpcResponse> {
    let mut stream = connect_ipc().await?;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let request_id = format!("{}-{}", std::process::id(), timestamp);
    let envelope = IpcEnvelope {
        version: IPC_VERSION,
        request_id,
        request,
    };
    stream
        .write_all(format!("{}\n", serde_json::to_string(&envelope)?).as_bytes())
        .await?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        anyhow::bail!("daemon closed IPC connection without a response");
    }
    let response: IpcResponse = serde_json::from_str(&line)?;
    if response.version != IPC_VERSION {
        anyhow::bail!(
            "daemon IPC version mismatch: client={} daemon={}",
            IPC_VERSION,
            response.version
        );
    }
    Ok(response)
}

async fn handle_ipc(stream: IpcStream, desired: DesiredApps) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let fallback_request_id = serde_json::from_str::<serde_json::Value>(&line)
        .ok()
        .and_then(|value| {
            value
                .get("request_id")
                .and_then(|request_id| request_id.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string());
    let envelope: IpcEnvelope = match serde_json::from_str(&line) {
        Ok(envelope) => envelope,
        Err(error) => {
            let response =
                error_response(fallback_request_id, format!("invalid IPC request: {error}"));
            let mut stream = reader.into_inner();
            stream
                .write_all(format!("{}\n", serde_json::to_string(&response)?).as_bytes())
                .await?;
            return Ok(());
        }
    };
    let request_id = envelope.request_id;
    let response = if envelope.version != IPC_VERSION {
        error_response(
            request_id,
            format!(
                "unsupported IPC version {}; expected {}",
                envelope.version, IPC_VERSION
            ),
        )
    } else {
        match envelope.request {
            IpcRequest::Ping => ok_response(request_id, serde_json::json!({ "pong": true })),
            IpcRequest::List => match list_apps().await {
                Ok(processes) => {
                    ok_response(request_id, serde_json::json!({ "processes": processes }))
                }
                Err(error) => error_response(request_id, error.to_string()),
            },
            IpcRequest::Prune => match prune_stale_processes().await {
                Ok(processes) => ok_response(
                    request_id,
                    serde_json::json!({ "removed": processes, "count": processes.len() }),
                ),
                Err(error) => error_response(request_id, error.to_string()),
            },
            IpcRequest::Shutdown => match stop_all().await {
                Ok(processes) => {
                    ok_response(request_id, serde_json::json!({ "stopped": processes }))
                }
                Err(error) => error_response(request_id, error.to_string()),
            },
            IpcRequest::Stop { name } => {
                if name == "all" {
                    match stop_all().await {
                        Ok(processes) => {
                            {
                                desired.lock().await.clear();
                            }
                            match save_desired_apps(&Vec::<ResolvedAppSpec>::new()).await {
                                Ok(()) => ok_response(
                                    request_id,
                                    serde_json::json!({ "stopped": processes }),
                                ),
                                Err(error) => error_response(request_id, error.to_string()),
                            }
                        }
                        Err(error) => error_response(request_id, error.to_string()),
                    }
                } else {
                    match stop_app(&name).await {
                        Ok(process) => {
                            let next = {
                                let mut locked = desired.lock().await;
                                locked.retain(|app| app.name != name);
                                locked.clone()
                            };
                            match save_desired_apps(&next).await {
                                Ok(()) => ok_response(
                                    request_id,
                                    serde_json::json!({ "stopped": process }),
                                ),
                                Err(error) => error_response(request_id, error.to_string()),
                            }
                        }
                        Err(error) => error_response(request_id, error.to_string()),
                    }
                }
            }
            IpcRequest::Start { config } => match load_config_apps(&config).await {
                Ok(loaded) => start_desired_apps(request_id, loaded, desired.clone()).await,
                Err(error) => error_response(request_id, error.to_string()),
            },
            IpcRequest::StartApps { apps } => {
                start_desired_apps(request_id, apps, desired.clone()).await
            }
            IpcRequest::Restart { config } => match load_config_apps(&config).await {
                Ok(loaded) => restart_desired_apps(request_id, loaded, desired.clone()).await,
                Err(error) => error_response(request_id, error.to_string()),
            },
            IpcRequest::RestartApps { apps } => {
                restart_desired_apps(request_id, apps, desired.clone()).await
            }
            IpcRequest::Reload { config } => match load_config_apps(&config).await {
                Ok(loaded) => reload_desired_apps(request_id, loaded, desired.clone()).await,
                Err(error) => error_response(request_id, error.to_string()),
            },
            IpcRequest::ReloadApps { apps } => {
                reload_desired_apps(request_id, apps, desired.clone()).await
            }
            IpcRequest::ScaleApps { apps } => {
                scale_desired_apps(request_id, apps, desired.clone()).await
            }
        }
    };

    let mut stream = reader.into_inner();
    stream
        .write_all(format!("{}\n", serde_json::to_string(&response)?).as_bytes())
        .await?;
    Ok(())
}

async fn start_desired_apps(
    request_id: String,
    apps: Vec<ResolvedAppSpec>,
    desired: DesiredApps,
) -> IpcResponse {
    let mut started = Vec::new();
    for app in &apps {
        if let Err(error) = validate_app(app) {
            return error_response(request_id, error.to_string());
        }
        match start_app_supervised(app).await {
            Ok(process) => started.push(process),
            Err(error) => return error_response(request_id, error.to_string()),
        }
    }
    match merge_desired_apps(desired, apps).await {
        Ok(()) => ok_response(request_id, serde_json::json!({ "started": started })),
        Err(error) => error_response(request_id, error.to_string()),
    }
}

async fn restart_desired_apps(
    request_id: String,
    apps: Vec<ResolvedAppSpec>,
    desired: DesiredApps,
) -> IpcResponse {
    let mut restarted = Vec::new();
    for app in &apps {
        if let Err(error) = validate_app(app) {
            return error_response(request_id, error.to_string());
        }
        match restart_app_supervised(app).await {
            Ok(process) => restarted.push(process),
            Err(error) => return error_response(request_id, error.to_string()),
        }
    }
    match merge_desired_apps(desired, apps).await {
        Ok(()) => ok_response(request_id, serde_json::json!({ "restarted": restarted })),
        Err(error) => error_response(request_id, error.to_string()),
    }
}

async fn reload_desired_apps(
    request_id: String,
    apps: Vec<ResolvedAppSpec>,
    desired: DesiredApps,
) -> IpcResponse {
    let mut reloaded = Vec::new();
    for app in &apps {
        if let Err(error) = validate_app(app) {
            return error_response(request_id, error.to_string());
        }
        match reload_app_supervised(app).await {
            Ok(process) => reloaded.push(process),
            Err(error) => return error_response(request_id, error.to_string()),
        }
    }
    match merge_desired_apps(desired, apps).await {
        Ok(()) => ok_response(request_id, serde_json::json!({ "reloaded": reloaded })),
        Err(error) => error_response(request_id, error.to_string()),
    }
}

async fn scale_desired_apps(
    request_id: String,
    apps: Vec<ResolvedAppSpec>,
    desired: DesiredApps,
) -> IpcResponse {
    let mut scaled = Vec::new();
    for app in &apps {
        if let Err(error) = validate_app(app) {
            return error_response(request_id, error.to_string());
        }
        match scale_app_supervised(app).await {
            Ok(process) => scaled.push(process),
            Err(error) => return error_response(request_id, error.to_string()),
        }
    }
    match merge_desired_apps(desired, apps).await {
        Ok(()) => ok_response(request_id, serde_json::json!({ "scaled": scaled })),
        Err(error) => error_response(request_id, error.to_string()),
    }
}

fn ok_response(request_id: String, payload: serde_json::Value) -> IpcResponse {
    IpcResponse {
        version: IPC_VERSION,
        request_id,
        ok: true,
        payload,
        error: None,
    }
}

async fn merge_desired_apps(desired: DesiredApps, apps: Vec<ResolvedAppSpec>) -> Result<()> {
    let next = {
        let mut locked = desired.lock().await;
        merge_app_specs(&mut locked, apps);
        locked.clone()
    };
    save_desired_apps(&next).await?;
    Ok(())
}

fn merge_app_specs(existing: &mut Vec<ResolvedAppSpec>, apps: Vec<ResolvedAppSpec>) {
    for app in apps {
        existing.retain(|item| item.name != app.name);
        existing.push(app);
    }
}

fn error_response(request_id: String, error: String) -> IpcResponse {
    IpcResponse {
        version: IPC_VERSION,
        request_id,
        ok: false,
        payload: serde_json::Value::Null,
        error: Some(error),
    }
}

fn ipc_path() -> PathBuf {
    #[cfg(unix)]
    {
        procwatch_home().join("daemon").join("procwatch.sock")
    }
    #[cfg(windows)]
    {
        procwatch_home().join("daemon").join("procwatch.addr")
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {},
            _ = terminate.recv() => {},
        }
    }

    #[cfg(windows)]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn service_install(config: Option<PathBuf>, json: bool) -> Result<()> {
    let config = std::fs::canonicalize(resolve_config(config)?)?;
    let apps = load_and_validate_config(&config).await?;
    let exe = std::env::current_exe()?;
    let path = service_file_path()?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::create_dir_all(service_runtime_dir()).await?;
    let content = service_file_content(&exe, &config)?;
    tokio::fs::write(&path, content).await?;
    if json {
        print_json(serde_json::json!({ "installed": path, "config": config, "apps": apps }))?;
    } else {
        println!("Installed service definition at {}", path.display());
        println!("It runs: {} daemon run {}", exe.display(), config.display());
    }
    Ok(())
}

async fn service_start(json: bool) -> Result<()> {
    let path = service_file_path()?;
    if !path.exists() {
        anyhow::bail!("service is not installed at {}", path.display());
    }
    let result = service_start_command(&path).await?;
    if json {
        print_json(serde_json::json!({ "started": result }))?;
    } else {
        println!("{result}");
    }
    Ok(())
}

async fn service_stop(json: bool) -> Result<()> {
    let path = service_file_path()?;
    let result = service_stop_command(&path).await?;
    if json {
        print_json(serde_json::json!({ "stopped": result }))?;
    } else {
        println!("{result}");
    }
    Ok(())
}

async fn service_uninstall(json: bool) -> Result<()> {
    let path = service_file_path()?;
    if path.exists() {
        let _ = service_stop_command(&path).await;
        tokio::fs::remove_file(&path).await?;
        service_post_uninstall().await?;
    }
    if json {
        print_json(serde_json::json!({ "removed": path }))?;
    } else {
        println!("Removed service definition at {}", path.display());
    }
    Ok(())
}

async fn service_status(json: bool) -> Result<()> {
    let snapshot = service_status_snapshot().await?;
    if json {
        print_json(serde_json::json!({
            "backend": snapshot.backend,
            "path": snapshot.path,
            "installed": snapshot.installed,
            "loaded": snapshot.loaded,
            "active": snapshot.active,
            "enabled": snapshot.enabled,
            "detail": snapshot.detail,
        }))?;
    } else {
        println!("backend: {}", snapshot.backend);
        if let Some(path) = &snapshot.path {
            println!("path: {}", path.display());
        }
        println!(
            "installed: {}",
            if snapshot.installed { "yes" } else { "no" }
        );
        if let Some(loaded) = snapshot.loaded {
            println!("loaded: {}", if loaded { "yes" } else { "no" });
        }
        if let Some(active) = snapshot.active {
            println!("active: {}", if active { "yes" } else { "no" });
        }
        if let Some(enabled) = snapshot.enabled {
            println!("enabled: {}", if enabled { "yes" } else { "no" });
        }
        if let Some(detail) = &snapshot.detail {
            println!("detail: {detail}");
        }
    }
    Ok(())
}

fn service_runtime_dir() -> PathBuf {
    procwatch_home().join("daemon")
}

fn service_file_path() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME")?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("LaunchAgents")
            .join("top.backrunner.procwatch.plist"))
    }

    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME")?;
        Ok(PathBuf::from(home)
            .join(".config")
            .join("systemd")
            .join("user")
            .join("procwatch.service"))
    }

    #[cfg(windows)]
    {
        return Ok(procwatch_home()
            .join("service")
            .join("procwatch-service.txt"));
    }
}

struct ServiceStatusSnapshot {
    backend: &'static str,
    path: Option<PathBuf>,
    installed: bool,
    loaded: Option<bool>,
    active: Option<bool>,
    enabled: Option<bool>,
    detail: Option<String>,
}

async fn service_status_snapshot() -> Result<ServiceStatusSnapshot> {
    let path = service_file_path()?;
    let installed = path.exists();

    #[cfg(target_os = "macos")]
    {
        if !installed {
            return Ok(ServiceStatusSnapshot {
                backend: service_backend_name(),
                path: Some(path),
                installed: false,
                loaded: Some(false),
                active: Some(false),
                enabled: None,
                detail: None,
            });
        }

        let uid = command_output("id", &["-u"]).await?;
        let label = "top.backrunner.procwatch";
        let target = format!("gui/{}/{}", uid.trim(), label);
        let output = command_capture("launchctl", &["print", &target]).await?;
        let loaded = output.success;
        let active = output.success && output.stdout.contains("state = running");
        let detail = if output.success {
            if active {
                Some("launchd job is running".to_string())
            } else {
                Some("launchd job is loaded".to_string())
            }
        } else {
            Some(output.stderr.trim().to_string())
        }
        .filter(|value| !value.is_empty());

        return Ok(ServiceStatusSnapshot {
            backend: service_backend_name(),
            path: Some(path),
            installed: true,
            loaded: Some(loaded),
            active: Some(active),
            enabled: None,
            detail,
        });
    }

    #[cfg(target_os = "linux")]
    {
        if !installed {
            return Ok(ServiceStatusSnapshot {
                backend: service_backend_name(),
                path: Some(path),
                installed: false,
                loaded: Some(false),
                active: Some(false),
                enabled: Some(false),
                detail: None,
            });
        }

        let active =
            command_capture("systemctl", &["--user", "is-active", "procwatch.service"]).await?;
        let enabled =
            command_capture("systemctl", &["--user", "is-enabled", "procwatch.service"]).await?;
        let daemon_reload = command_capture(
            "systemctl",
            &[
                "--user",
                "show",
                "procwatch.service",
                "--property=LoadState",
            ],
        )
        .await?;

        let loaded = daemon_reload.stdout.contains("LoadState=loaded");
        let active_state = active.stdout.trim() == "active";
        let enabled_state = matches!(enabled.stdout.trim(), "enabled" | "linked");
        let detail = [
            daemon_reload.stdout.trim(),
            active.stdout.trim(),
            enabled.stdout.trim(),
        ]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(", ");

        return Ok(ServiceStatusSnapshot {
            backend: service_backend_name(),
            path: Some(path),
            installed: true,
            loaded: Some(loaded),
            active: Some(active_state),
            enabled: Some(enabled_state),
            detail: (!detail.is_empty()).then_some(detail),
        });
    }

    #[cfg(windows)]
    {
        return Ok(ServiceStatusSnapshot {
            backend: service_backend_name(),
            path: Some(path),
            installed,
            loaded: None,
            active: None,
            enabled: None,
            detail: Some(
                "Windows native service registration is not yet implemented in this MVP"
                    .to_string(),
            ),
        });
    }

    #[allow(unreachable_code)]
    Ok(ServiceStatusSnapshot {
        backend: service_backend_name(),
        path: Some(path),
        installed,
        loaded: None,
        active: None,
        enabled: None,
        detail: None,
    })
}

fn service_file_content(exe: &std::path::Path, config: &std::path::Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let runtime_dir = service_runtime_dir();
        let stdout_log = runtime_dir.join("service.out.log");
        let stderr_log = runtime_dir.join("service.err.log");
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>top.backrunner.procwatch</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>daemon</string>
    <string>run</string>
    <string>{}</string>
  </array>
  <key>WorkingDirectory</key><string>{}</string>
  <key>StandardOutPath</key><string>{}</string>
  <key>StandardErrorPath</key><string>{}</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
"#,
            plist_escape(exe),
            plist_escape(config),
            plist_escape(&runtime_dir),
            plist_escape(&stdout_log),
            plist_escape(&stderr_log),
        ))
    }

    #[cfg(target_os = "linux")]
    {
        let runtime_dir = service_runtime_dir();
        Ok(format!(
            "[Unit]\nDescription=Procwatch process supervisor\n\n[Service]\nWorkingDirectory={}\nEnvironment={}\nExecStart={} daemon run {}\nRestart=always\n\n[Install]\nWantedBy=default.target\n",
            systemd_quote(&runtime_dir),
            systemd_environment("PROCWATCH_HOME", &procwatch_home()),
            systemd_quote(exe),
            systemd_quote(config)
        ))
    }

    #[cfg(windows)]
    {
        return Ok(format!(
            "Procwatch service command:\n{} daemon run {}\nUse a Windows service wrapper to register this command for now.\n",
            exe.display(),
            config.display()
        ));
    }
}

#[cfg(target_os = "macos")]
fn plist_escape(path: &std::path::Path) -> String {
    path.display()
        .to_string()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(target_os = "linux")]
fn systemd_quote(path: &std::path::Path) -> String {
    let value = path
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{value}\"")
}

#[cfg(target_os = "linux")]
fn systemd_environment(name: &str, value: &std::path::Path) -> String {
    let quoted = value
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{name}={quoted}\"")
}

async fn service_start_command(path: &std::path::Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let uid = command_output("id", &["-u"]).await?;
        let target = format!("gui/{}", uid.trim());
        let status = command_capture(
            "launchctl",
            &["bootstrap", &target, &path.display().to_string()],
        )
        .await?;
        if !status.success {
            let kickstart = command_capture(
                "launchctl",
                &[
                    "kickstart",
                    "-k",
                    &format!("{target}/top.backrunner.procwatch"),
                ],
            )
            .await?;
            if !kickstart.success {
                anyhow::bail!(
                    "launchctl bootstrap failed: {}",
                    first_nonempty(status.stderr.trim(), status.stdout.trim())
                        .unwrap_or("unknown launchctl error")
                );
            }
        }
        Ok(format!("launchd service started via {}", path.display()))
    }

    #[cfg(target_os = "linux")]
    {
        let _ = path;
        run_status("systemctl", &["--user", "daemon-reload"]).await?;
        run_status(
            "systemctl",
            &["--user", "enable", "--now", "procwatch.service"],
        )
        .await?;
        Ok("systemd user service enabled and started".to_string())
    }

    #[cfg(windows)]
    {
        let _ = path;
        return Ok(
            "Windows native service registration is not yet implemented in this MVP".to_string(),
        );
    }
}

async fn service_post_uninstall() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        let _ = command_capture("systemctl", &["--user", "daemon-reload"]).await?;
    }

    Ok(())
}

async fn service_stop_command(path: &std::path::Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let uid = command_output("id", &["-u"]).await?;
        let target = format!("gui/{}", uid.trim());
        let status = command_capture(
            "launchctl",
            &["bootout", &target, &path.display().to_string()],
        )
        .await?;
        if !status.success {
            anyhow::bail!(
                "launchctl bootout failed for {}: {}",
                path.display(),
                first_nonempty(status.stderr.trim(), status.stdout.trim())
                    .unwrap_or("unknown launchctl error")
            );
        }
        Ok(format!("launchd service stopped via {}", path.display()))
    }

    #[cfg(target_os = "linux")]
    {
        let _ = path;
        run_status(
            "systemctl",
            &["--user", "disable", "--now", "procwatch.service"],
        )
        .await?;
        Ok("systemd user service stopped and disabled".to_string())
    }

    #[cfg(windows)]
    {
        let _ = path;
        return Ok(
            "Windows native service registration is not yet implemented in this MVP".to_string(),
        );
    }
}

#[cfg(target_os = "macos")]
async fn command_output(program: &str, args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!("{program} failed with status {}", output.status);
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

struct CommandCapture {
    #[allow(dead_code)]
    success: bool,
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
}

#[cfg(target_os = "macos")]
fn first_nonempty<'a>(primary: &'a str, fallback: &'a str) -> Option<&'a str> {
    if !primary.is_empty() {
        Some(primary)
    } else if !fallback.is_empty() {
        Some(fallback)
    } else {
        None
    }
}

async fn command_capture(program: &str, args: &[&str]) -> Result<CommandCapture> {
    let output = match tokio::time::timeout(
        COMMAND_TIMEOUT,
        tokio::process::Command::new(program).args(args).output(),
    )
    .await
    {
        Ok(output) => output?,
        Err(_) => {
            return Ok(CommandCapture {
                success: false,
                stdout: String::new(),
                stderr: format!(
                    "{program} timed out after {} seconds",
                    COMMAND_TIMEOUT.as_secs()
                ),
            });
        }
    };
    Ok(CommandCapture {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

#[cfg(target_os = "linux")]
async fn run_status(program: &str, args: &[&str]) -> Result<()> {
    let status = tokio::time::timeout(
        COMMAND_TIMEOUT,
        tokio::process::Command::new(program).args(args).status(),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "{program} timed out after {} seconds",
            COMMAND_TIMEOUT.as_secs()
        )
    })??;
    if !status.success() {
        anyhow::bail!("{program} failed with status {status}");
    }
    Ok(())
}

async fn watch(target: Option<PathBuf>, interval_ms: u64, json: bool) -> Result<()> {
    let apps = resolve_apps(target).await?;
    let configured_apps: Vec<_> = apps
        .iter()
        .filter(|app| app.watch.enabled)
        .cloned()
        .collect();
    let apps = if configured_apps.is_empty() {
        apps
    } else {
        configured_apps
    };
    for app in &apps {
        validate_app(app)?;
        start_app_supervised(app).await?;
    }
    if json {
        print_json(serde_json::json!({ "watching": apps }))?;
    } else {
        for app in &apps {
            println!("Watching {}", app.name);
        }
    }

    let mut snapshots = Vec::new();
    for app in &apps {
        snapshots.push(WatchedApp {
            app: app.clone(),
            snapshot: snapshot_app(app)?,
            pending: None,
        });
    }

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(interval_ms.max(100))).await;
        let now = std::time::Instant::now();
        for watched in &mut snapshots {
            let next = snapshot_app(&watched.app)?;
            if let Some((pending_snapshot, changed_at)) = &watched.pending {
                if &next != pending_snapshot {
                    watched.pending = Some((next, now));
                    continue;
                }
                if now.duration_since(*changed_at)
                    < std::time::Duration::from_millis(watched.app.watch.debounce_ms.max(100))
                {
                    continue;
                }

                watched.snapshot = pending_snapshot.clone();
                watched.pending = None;
                if watched.app.watch.reload {
                    let reloaded = reload_app_supervised(&watched.app).await?;
                    if json {
                        print_json(serde_json::json!({ "reloaded": reloaded }))?;
                    } else {
                        println!("Reloaded {} after file change", watched.app.name);
                    }
                } else {
                    let restarted = restart_app_supervised(&watched.app).await?;
                    if json {
                        print_json(serde_json::json!({ "restarted": restarted }))?;
                    } else {
                        println!("Restarted {} after file change", watched.app.name);
                    }
                }
                continue;
            }

            if watched.snapshot != next {
                watched.pending = Some((next, now));
            }
        }
    }
}

fn resolve_config(path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = path {
        if path.is_file() {
            return Ok(path);
        }
        if path.is_dir() {
            return find_config(&path)
                .with_context(|| format!("no ecosystem config found in {}", path.display()));
        }
    }

    find_config(&std::env::current_dir()?).context("no ecosystem config found in current directory")
}

async fn resolve_apps(target: Option<PathBuf>) -> Result<Vec<procwatch_core::ResolvedAppSpec>> {
    if let Some(target) = target {
        if target.is_file() && !looks_like_config(&target) {
            let cwd = target
                .parent()
                .map(PathBuf::from)
                .unwrap_or(std::env::current_dir()?);
            let name = target
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("app")
                .to_string();
            let app = AppSpec {
                name,
                script: Some(
                    target
                        .file_name()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| target.clone()),
                ),
                cwd: Some(cwd),
                ..AppSpec::default()
            };
            let temp_path = std::env::current_dir()?.join("inline-script.json");
            return Ok(procwatch_config::normalize_config(
                ProcwatchConfig {
                    apps: vec![app],
                    ..ProcwatchConfig::default()
                },
                &temp_path,
            )?
            .apps);
        }

        let config = resolve_config(Some(target))?;
        return load_config_apps(&config).await;
    }

    let config = resolve_config(None)?;
    load_config_apps(&config).await
}

fn looks_like_config(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(|name| name.starts_with("ecosystem.config."))
        .unwrap_or(false)
}

fn print_json(value: serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

async fn try_daemon_request(request: IpcRequest) -> Result<Option<IpcResponse>> {
    if !ipc_path().exists() {
        return Ok(None);
    }

    match send_ipc(request).await {
        Ok(response) => Ok(Some(response)),
        Err(error) => {
            let daemon_running = daemon_pid()
                .await
                .map(procwatch_platform::is_process_alive)
                .unwrap_or(false);
            if daemon_running {
                Err(error).context("daemon IPC request failed while daemon pid is still alive")
            } else {
                Ok(None)
            }
        }
    }
}

fn print_ipc_response(response: IpcResponse, json: bool) -> Result<()> {
    if json {
        print_json(serde_json::to_value(response)?)?;
    } else if response.ok {
        println!("{}", serde_json::to_string_pretty(&response.payload)?);
    } else {
        anyhow::bail!(response
            .error
            .unwrap_or_else(|| "daemon request failed".to_string()));
    }
    Ok(())
}

async fn print_new_log_bytes(
    process: &ManagedProcess,
    stream: &str,
    path: &PathBuf,
    offsets: &mut std::collections::BTreeMap<PathBuf, LogFollowState>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let bytes = tokio::fs::read(path).await?;
    let previous = follow_start_offset(offsets.get(path), &bytes);
    if bytes.len() <= previous {
        offsets.insert(path.clone(), LogFollowState::from_bytes(&bytes));
        return Ok(());
    }
    let text = String::from_utf8_lossy(&bytes[previous..]);
    for line in text.lines() {
        println!("{}", decorate_log_line(process, stream, line));
    }
    offsets.insert(path.clone(), LogFollowState::from_bytes(&bytes));
    Ok(())
}

fn decorate_log_line(process: &ManagedProcess, stream: &str, line: &str) -> String {
    format!(
        "{} [{}] [worker:{}] {} | {}",
        log_timestamp(),
        process.name,
        process.worker_id.as_deref().unwrap_or("-"),
        stream,
        line
    )
}

fn log_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}Z", now.as_secs(), now.subsec_millis())
}

#[derive(Debug, Clone)]
struct LogFollowState {
    offset: usize,
    prefix: Vec<u8>,
}

impl LogFollowState {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            offset: bytes.len(),
            prefix: log_prefix(bytes),
        }
    }
}

async fn log_follow_state(path: &PathBuf) -> LogFollowState {
    let bytes = tokio::fs::read(path).await.unwrap_or_default();
    LogFollowState::from_bytes(&bytes)
}

fn follow_start_offset(previous: Option<&LogFollowState>, bytes: &[u8]) -> usize {
    let Some(previous) = previous else {
        return 0;
    };
    if bytes.len() < previous.offset || !bytes.starts_with(&previous.prefix) {
        return 0;
    }
    previous.offset
}

fn log_prefix(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().take(64).copied().collect()
}

struct WatchedApp {
    app: ResolvedAppSpec,
    snapshot: std::collections::BTreeMap<PathBuf, u64>,
    pending: Option<(std::collections::BTreeMap<PathBuf, u64>, std::time::Instant)>,
}

struct WatchMatcher {
    include: Option<GlobSet>,
    ignore: GlobSet,
}

fn snapshot_app(app: &ResolvedAppSpec) -> Result<std::collections::BTreeMap<PathBuf, u64>> {
    let mut snapshot = std::collections::BTreeMap::new();
    let matcher = watch_matcher(app)?;
    for root in watch_roots(app) {
        if !root.exists() {
            continue;
        }
        collect_snapshot(&app.cwd, &root, &matcher, &mut snapshot)?;
    }
    Ok(snapshot)
}

fn collect_snapshot(
    base: &std::path::Path,
    path: &std::path::Path,
    matcher: &WatchMatcher,
    snapshot: &mut std::collections::BTreeMap<PathBuf, u64>,
) -> Result<()> {
    let metadata = std::fs::metadata(path)?;
    if metadata.is_file() {
        collect_snapshot_file(base, path, &metadata, matcher, snapshot)?;
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }

    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            ".git" | ".procwatch" | "node_modules" | "target"
        ) {
            continue;
        }
        let metadata = entry.metadata()?;
        if watch_ignored(base, &path, matcher) {
            continue;
        }
        if metadata.is_dir() {
            collect_snapshot(base, &path, matcher, snapshot)?;
        } else if metadata.is_file() {
            collect_snapshot_file(base, &path, &metadata, matcher, snapshot)?;
        }
    }
    Ok(())
}

fn collect_snapshot_file(
    base: &std::path::Path,
    path: &std::path::Path,
    metadata: &std::fs::Metadata,
    matcher: &WatchMatcher,
    snapshot: &mut std::collections::BTreeMap<PathBuf, u64>,
) -> Result<()> {
    if watch_ignored(base, path, matcher) || !watch_included(base, path, matcher) {
        return Ok(());
    }
    let relative = path.strip_prefix(base).unwrap_or(path).to_path_buf();
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    snapshot.insert(relative, modified ^ metadata.len());
    Ok(())
}

fn watch_roots(app: &ResolvedAppSpec) -> Vec<PathBuf> {
    if app.watch.paths.is_empty() {
        return vec![app.cwd.clone()];
    }
    app.watch
        .paths
        .iter()
        .map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                app.cwd.join(path)
            }
        })
        .collect()
}

fn watch_matcher(app: &ResolvedAppSpec) -> Result<WatchMatcher> {
    let mut ignore = GlobSetBuilder::new();
    for pattern in DEFAULT_WATCH_IGNORES
        .iter()
        .copied()
        .chain(app.watch.ignore.iter().map(String::as_str))
    {
        add_watch_glob(&mut ignore, pattern)?;
    }
    let ignore = ignore.build()?;

    let include = if app.watch.include.is_empty() {
        None
    } else {
        let mut include = GlobSetBuilder::new();
        for pattern in &app.watch.include {
            add_watch_glob(&mut include, pattern)?;
        }
        Some(include.build()?)
    };

    Ok(WatchMatcher { include, ignore })
}

fn add_watch_glob(builder: &mut GlobSetBuilder, pattern: &str) -> Result<()> {
    builder.add(Glob::new(pattern).with_context(|| format!("invalid watch glob: {pattern}"))?);
    if !pattern.chars().any(|ch| matches!(ch, '*' | '?' | '[')) && !pattern.contains('/') {
        builder.add(
            Glob::new(&format!("**/{pattern}"))
                .with_context(|| format!("invalid watch glob: {pattern}"))?,
        );
        builder.add(
            Glob::new(&format!("**/{pattern}/**"))
                .with_context(|| format!("invalid watch glob: {pattern}"))?,
        );
    }
    Ok(())
}

fn watch_ignored(base: &std::path::Path, path: &std::path::Path, matcher: &WatchMatcher) -> bool {
    let relative = path.strip_prefix(base).unwrap_or(path);
    matcher.ignore.is_match(relative)
}

fn watch_included(base: &std::path::Path, path: &std::path::Path, matcher: &WatchMatcher) -> bool {
    let Some(include) = &matcher.include else {
        return true;
    };
    let relative = path.strip_prefix(base).unwrap_or(path);
    include.is_match(relative)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use procwatch_core::{ExecMode, Instances, LogPolicy, RestartPolicy, WatchSpec};

    use super::*;

    #[test]
    fn watch_snapshot_changes_for_tracked_files() {
        let dir = temp_dir("snapshot-change");
        std::fs::write(dir.join("server.js"), "console.log(1);\n").unwrap();
        let mut app = test_app(&dir);
        app.watch.paths = vec![PathBuf::from(".")];

        let first = snapshot_app(&app).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(dir.join("server.js"), "console.log(2);\n").unwrap();
        let second = snapshot_app(&app).unwrap();

        std::fs::remove_dir_all(&dir).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn watch_snapshot_ignores_configured_paths() {
        let dir = temp_dir("snapshot-ignore");
        std::fs::create_dir_all(dir.join("tmp")).unwrap();
        std::fs::write(dir.join("tmp").join("ignored.js"), "console.log(1);\n").unwrap();
        let mut app = test_app(&dir);
        app.watch.ignore = vec!["tmp".to_string()];

        let first = snapshot_app(&app).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        std::fs::write(dir.join("tmp").join("ignored.js"), "console.log(2);\n").unwrap();
        let second = snapshot_app(&app).unwrap();

        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn log_follow_continues_from_previous_offset_for_append() {
        let previous = LogFollowState::from_bytes(b"prefix\n");
        assert_eq!(follow_start_offset(Some(&previous), b"prefix\nnext\n"), 7);
    }

    #[test]
    fn log_follow_restarts_after_truncation() {
        let previous = LogFollowState::from_bytes(b"prefix\nlonger\n");
        assert_eq!(follow_start_offset(Some(&previous), b"new\n"), 0);
    }

    #[test]
    fn log_follow_restarts_after_rotation_replacement() {
        let previous = LogFollowState::from_bytes(b"old-prefix\n");
        assert_eq!(
            follow_start_offset(Some(&previous), b"new-prefix\nwith enough bytes\n"),
            0
        );
    }

    fn temp_dir(name: &str) -> PathBuf {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("procwatch-{name}-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_app(cwd: &std::path::Path) -> ResolvedAppSpec {
        ResolvedAppSpec {
            name: "watch-test".to_string(),
            script: Some(PathBuf::from("server.js")),
            command: None,
            cwd: cwd.to_path_buf(),
            args: Vec::new(),
            node_args: Vec::new(),
            interpreter: "node".to_string(),
            interpreter_args: Vec::new(),
            package_manager: None,
            package_script: None,
            env: BTreeMap::new(),
            exec_mode: ExecMode::Fork,
            instances: Instances::Count(1),
            watch: WatchSpec {
                enabled: true,
                ..WatchSpec::default()
            },
            restart: RestartPolicy::default(),
            max_memory_restart: None,
            cron_restart: None,
            log: LogPolicy::default(),
        }
    }
}
