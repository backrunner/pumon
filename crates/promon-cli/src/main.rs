use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use promon_config::{find_config, load_config};
use promon_core::{AppSpec, Instances, PromonConfig, ResolvedAppSpec};
use promon_logging::tail_file;
use promon_node_support::validate_runtime;
use promon_platform::{find_program, promon_home};
use promon_process::{list_apps, restart_app, run_app_foreground, start_app, stop_all, stop_app};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

type DesiredApps = Arc<Mutex<Vec<ResolvedAppSpec>>>;

#[derive(Debug, Parser)]
#[command(name = "promon", version, about = "Rust-first Node.js process manager")]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
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
    Doctor,
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
    Scale {
        target: PathBuf,
        instances: u16,
    },
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
    Tui,
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init { output } => init(output, cli.json).await,
        Commands::Validate { config } => validate(config, cli.json).await,
        Commands::Doctor => doctor(cli.json).await,
        Commands::Start { target, wait } => start(target, wait, cli.json).await,
        Commands::Stop { name } => stop(name, cli.json).await,
        Commands::Restart { target } => restart(target, cli.json).await,
        Commands::Scale { target, instances } => scale(target, instances, cli.json).await,
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
        Commands::Tui => tui().await,
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
#[serde(tag = "type", rename_all = "snake_case")]
enum IpcRequest {
    Ping,
    List,
    Stop { name: String },
    Start { config: PathBuf },
    StartApps { apps: Vec<ResolvedAppSpec> },
    Restart { config: PathBuf },
    RestartApps { apps: Vec<ResolvedAppSpec> },
}

#[derive(Debug, Serialize, Deserialize)]
struct IpcResponse {
    ok: bool,
    payload: serde_json::Value,
    error: Option<String>,
}

async fn init(output: PathBuf, json: bool) -> Result<()> {
    let sample = r#"{
  "apps": [
    {
      "name": "promon-example",
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

async fn validate(config: Option<PathBuf>, json: bool) -> Result<()> {
    let config = resolve_config(config)?;
    let apps = load_config(&config).await?;
    for app in &apps {
        validate_runtime(app)?;
    }

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

async fn doctor(json: bool) -> Result<()> {
    let node = find_program("node", None).map(|path| path.display().to_string());
    let npm = find_program("npm", None).map(|path| path.display().to_string());
    let pnpm = find_program("pnpm", None).map(|path| path.display().to_string());
    let yarn = find_program("yarn", None).map(|path| path.display().to_string());
    let bun = find_program("bun", None).map(|path| path.display().to_string());
    let report = serde_json::json!({
        "promon_home": promon_home(),
        "node": node,
        "npm": npm,
        "pnpm": pnpm,
        "yarn": yarn,
        "bun": bun
    });

    if json {
        print_json(report)?;
    } else {
        println!("Promon home: {}", promon_home().display());
        println!("node: {}", report["node"].as_str().unwrap_or("missing"));
        println!("npm: {}", report["npm"].as_str().unwrap_or("missing"));
        println!("pnpm: {}", report["pnpm"].as_str().unwrap_or("missing"));
        println!("yarn: {}", report["yarn"].as_str().unwrap_or("missing"));
        println!("bun: {}", report["bun"].as_str().unwrap_or("missing"));
    }
    Ok(())
}

async fn start(target: Option<PathBuf>, wait: bool, json: bool) -> Result<()> {
    let apps = resolve_apps(target).await?;
    if wait {
        if json {
            print_json(serde_json::json!({ "supervising": apps }))?;
        } else {
            for app in &apps {
                println!("Supervising {}", app.name);
            }
        }
        for app in apps {
            validate_runtime(&app)?;
            run_app_foreground(&app).await?;
        }
        return Ok(());
    }

    if let Some(response) = try_daemon_request(IpcRequest::StartApps { apps: apps.clone() }).await?
    {
        return print_ipc_response(response, json);
    }

    let mut started = Vec::new();
    for app in apps {
        validate_runtime(&app)?;
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
        validate_runtime(&app)?;
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

async fn scale(target: PathBuf, instances: u16, json: bool) -> Result<()> {
    let mut apps = resolve_apps(Some(target)).await?;
    for app in &mut apps {
        app.instances = Instances::Count(instances.max(1));
    }

    if let Some(response) =
        try_daemon_request(IpcRequest::RestartApps { apps: apps.clone() }).await?
    {
        return print_ipc_response(response, json);
    }

    let mut scaled = Vec::new();
    for app in apps {
        validate_runtime(&app)?;
        scaled.push(restart_app(&app).await?);
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
            println!("{line}");
        }
        println!(
            "==> {} stderr ({})",
            process.name,
            process.err_log.display()
        );
        for line in tail_file(&process.err_log, lines).await? {
            println!("{line}");
        }
    }
    if follow {
        let mut offsets = std::collections::BTreeMap::new();
        for process in list_apps().await? {
            offsets.insert(
                process.out_log.clone(),
                tokio::fs::metadata(&process.out_log)
                    .await
                    .map(|meta| meta.len())
                    .unwrap_or(0),
            );
            offsets.insert(
                process.err_log.clone(),
                tokio::fs::metadata(&process.err_log)
                    .await
                    .map(|meta| meta.len())
                    .unwrap_or(0),
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
                print_new_log_bytes(&process.name, "stdout", &process.out_log, &mut offsets)
                    .await?;
                print_new_log_bytes(&process.name, "stderr", &process.err_log, &mut offsets)
                    .await?;
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
        println!("{:<24} {:<8} {:<10} Command", "Name", "PID", "Status");
        for process in processes {
            println!(
                "{:<24} {:<8} {:<10} {}",
                process.name,
                process.pid,
                format!("{:?}", process.status).to_lowercase(),
                process.command.display_command()
            );
        }
    }
    Ok(())
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
    let exe = std::env::current_exe()?;
    let dir = promon_home().join("daemon");
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
    if json {
        print_json(serde_json::json!({ "pid": pid, "config": config }))?;
    } else {
        println!("Started promon daemon pid={pid}");
    }
    Ok(())
}

async fn daemon_stop(json: bool) -> Result<()> {
    let pid_path = promon_home().join("daemon").join("daemon.pid");
    let pid = tokio::fs::read_to_string(&pid_path)
        .await?
        .trim()
        .parse::<u32>()?;
    let _ = send_ipc(IpcRequest::Stop {
        name: "all".to_string(),
    })
    .await;
    promon_platform::terminate_process(pid).await?;
    let _ = tokio::fs::remove_file(&pid_path).await;
    if json {
        print_json(serde_json::json!({ "stopped": pid }))?;
    } else {
        println!("Stopped promon daemon pid={pid}");
    }
    Ok(())
}

async fn daemon_status(json: bool) -> Result<()> {
    let pid_path = promon_home().join("daemon").join("daemon.pid");
    let pid = tokio::fs::read_to_string(&pid_path)
        .await
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok());
    let running = pid.map(promon_platform::is_process_alive).unwrap_or(false);
    if json {
        print_json(serde_json::json!({ "pid": pid, "running": running }))?;
    } else if let Some(pid) = pid {
        println!(
            "promon daemon pid={pid} {}",
            if running { "running" } else { "stale" }
        );
    } else {
        println!("promon daemon not started");
    }
    Ok(())
}

async fn daemon_ipc(request: IpcRequest, json: bool) -> Result<()> {
    let response = send_ipc(request).await?;
    print_ipc_response(response, json)
}

async fn daemon_run(config: PathBuf) -> Result<()> {
    let apps = load_config(&config).await?;
    for app in &apps {
        validate_runtime(app)?;
        start_app(app).await?;
    }
    let desired = Arc::new(Mutex::new(apps));

    let listener = bind_ipc().await?;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let apps = desired.lock().await.clone();
                for app in &apps {
                    let processes = list_apps().await?;
                    let alive = processes.iter().any(|process| {
                        process.name == app.name && matches!(process.status, promon_core::ProcessStatus::Running)
                    });
                    if !alive && app.restart.autorestart {
                        let _ = start_app(app).await;
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

async fn tui() -> Result<()> {
    loop {
        print!("\x1b[2J\x1b[H");
        println!("Promon TUI");
        println!("Press Ctrl+C to exit\n");
        let processes = list_apps().await?;
        if processes.is_empty() {
            println!("No managed processes");
        } else {
            println!("{:<24} {:<8} {:<10} Command", "Name", "PID", "Status");
            for process in processes {
                println!(
                    "{:<24} {:<8} {:<10} {}",
                    process.name,
                    process.pid,
                    format!("{:?}", process.status).to_lowercase(),
                    process.command.display_command()
                );
            }
        }

        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {}
        }
    }
    Ok(())
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
    stream
        .write_all(format!("{}\n", serde_json::to_string(&request)?).as_bytes())
        .await?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    Ok(serde_json::from_str(&line)?)
}

async fn handle_ipc(stream: IpcStream, desired: DesiredApps) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let request: IpcRequest = serde_json::from_str(&line)?;
    let response = match request {
        IpcRequest::Ping => IpcResponse {
            ok: true,
            payload: serde_json::json!({ "pong": true }),
            error: None,
        },
        IpcRequest::List => match list_apps().await {
            Ok(processes) => IpcResponse {
                ok: true,
                payload: serde_json::json!({ "processes": processes }),
                error: None,
            },
            Err(error) => error_response(error.to_string()),
        },
        IpcRequest::Stop { name } => {
            if name == "all" {
                match stop_all().await {
                    Ok(processes) => {
                        desired.lock().await.clear();
                        IpcResponse {
                            ok: true,
                            payload: serde_json::json!({ "stopped": processes }),
                            error: None,
                        }
                    }
                    Err(error) => error_response(error.to_string()),
                }
            } else {
                match stop_app(&name).await {
                    Ok(process) => {
                        desired.lock().await.retain(|app| app.name != name);
                        IpcResponse {
                            ok: true,
                            payload: serde_json::json!({ "stopped": process }),
                            error: None,
                        }
                    }
                    Err(error) => error_response(error.to_string()),
                }
            }
        }
        IpcRequest::Start { config } => match load_config(&config).await {
            Ok(loaded) => start_desired_apps(loaded, desired.clone()).await,
            Err(error) => error_response(error.to_string()),
        },
        IpcRequest::StartApps { apps } => start_desired_apps(apps, desired.clone()).await,
        IpcRequest::Restart { config } => match load_config(&config).await {
            Ok(loaded) => restart_desired_apps(loaded, desired.clone()).await,
            Err(error) => error_response(error.to_string()),
        },
        IpcRequest::RestartApps { apps } => restart_desired_apps(apps, desired.clone()).await,
    };

    let mut stream = reader.into_inner();
    stream
        .write_all(format!("{}\n", serde_json::to_string(&response)?).as_bytes())
        .await?;
    Ok(())
}

async fn start_desired_apps(apps: Vec<ResolvedAppSpec>, desired: DesiredApps) -> IpcResponse {
    let mut started = Vec::new();
    for app in &apps {
        if let Err(error) = validate_runtime(app) {
            return error_response(error.to_string());
        }
        match start_app(app).await {
            Ok(process) => started.push(process),
            Err(error) => return error_response(error.to_string()),
        }
    }
    merge_desired_apps(desired, apps).await;
    IpcResponse {
        ok: true,
        payload: serde_json::json!({ "started": started }),
        error: None,
    }
}

async fn restart_desired_apps(apps: Vec<ResolvedAppSpec>, desired: DesiredApps) -> IpcResponse {
    let mut restarted = Vec::new();
    for app in &apps {
        if let Err(error) = validate_runtime(app) {
            return error_response(error.to_string());
        }
        match restart_app(app).await {
            Ok(process) => restarted.push(process),
            Err(error) => return error_response(error.to_string()),
        }
    }
    merge_desired_apps(desired, apps).await;
    IpcResponse {
        ok: true,
        payload: serde_json::json!({ "restarted": restarted }),
        error: None,
    }
}

async fn merge_desired_apps(desired: DesiredApps, apps: Vec<ResolvedAppSpec>) {
    let mut locked = desired.lock().await;
    for app in apps {
        locked.retain(|existing| existing.name != app.name);
        locked.push(app);
    }
}

fn error_response(error: String) -> IpcResponse {
    IpcResponse {
        ok: false,
        payload: serde_json::Value::Null,
        error: Some(error),
    }
}

fn ipc_path() -> PathBuf {
    #[cfg(unix)]
    {
        promon_home().join("daemon").join("promon.sock")
    }
    #[cfg(windows)]
    {
        promon_home().join("daemon").join("promon.addr")
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
    let config = resolve_config(config)?;
    let exe = std::env::current_exe()?;
    let path = service_file_path()?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let content = service_file_content(&exe, &config)?;
    tokio::fs::write(&path, content).await?;
    if json {
        print_json(serde_json::json!({ "installed": path, "config": config }))?;
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
        tokio::fs::remove_file(&path).await?;
    }
    if json {
        print_json(serde_json::json!({ "removed": path }))?;
    } else {
        println!("Removed service definition at {}", path.display());
    }
    Ok(())
}

async fn service_status(json: bool) -> Result<()> {
    let path = service_file_path()?;
    if json {
        print_json(serde_json::json!({ "path": path, "installed": path.exists() }))?;
    } else {
        println!(
            "{}: {}",
            path.display(),
            if path.exists() {
                "installed"
            } else {
                "not installed"
            }
        );
    }
    Ok(())
}

fn service_file_path() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME")?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("LaunchAgents")
            .join("top.backrunner.promon.plist"))
    }

    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME")?;
        return Ok(PathBuf::from(home)
            .join(".config")
            .join("systemd")
            .join("user")
            .join("promon.service"));
    }

    #[cfg(windows)]
    {
        return Ok(promon_home().join("service").join("promon-service.txt"));
    }
}

fn service_file_content(exe: &std::path::Path, config: &std::path::Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>top.backrunner.promon</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>daemon</string>
    <string>run</string>
    <string>{}</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
"#,
            exe.display(),
            config.display()
        ))
    }

    #[cfg(target_os = "linux")]
    {
        return Ok(format!(
            "[Unit]\nDescription=Promon process supervisor\n\n[Service]\nExecStart={} daemon run {}\nRestart=always\n\n[Install]\nWantedBy=default.target\n",
            exe.display(),
            config.display()
        ));
    }

    #[cfg(windows)]
    {
        return Ok(format!(
            "Promon service command:\n{} daemon run {}\nUse a Windows service wrapper or the future native daemon service backend to register this command.\n",
            exe.display(),
            config.display()
        ));
    }
}

async fn service_start_command(path: &std::path::Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let uid = command_output("id", &["-u"]).await?;
        let target = format!("gui/{}", uid.trim());
        let status = tokio::process::Command::new("launchctl")
            .args(["bootstrap", &target, &path.display().to_string()])
            .status()
            .await?;
        if !status.success() {
            let _ = tokio::process::Command::new("launchctl")
                .args([
                    "kickstart",
                    "-k",
                    &format!("{target}/top.backrunner.promon"),
                ])
                .status()
                .await?;
        }
        Ok(format!("launchd service started via {}", path.display()))
    }

    #[cfg(target_os = "linux")]
    {
        run_status("systemctl", &["--user", "daemon-reload"]).await?;
        run_status(
            "systemctl",
            &["--user", "enable", "--now", "promon.service"],
        )
        .await?;
        return Ok("systemd user service enabled and started".to_string());
    }

    #[cfg(windows)]
    {
        let _ = path;
        return Ok(
            "Windows native service registration is not available in this MVP backend".to_string(),
        );
    }
}

async fn service_stop_command(path: &std::path::Path) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        let uid = command_output("id", &["-u"]).await?;
        let target = format!("gui/{}", uid.trim());
        let status = tokio::process::Command::new("launchctl")
            .args(["bootout", &target, &path.display().to_string()])
            .status()
            .await?;
        if !status.success() {
            anyhow::bail!("launchctl bootout failed for {}", path.display());
        }
        Ok(format!("launchd service stopped via {}", path.display()))
    }

    #[cfg(target_os = "linux")]
    {
        let _ = path;
        run_status(
            "systemctl",
            &["--user", "disable", "--now", "promon.service"],
        )
        .await?;
        return Ok("systemd user service stopped and disabled".to_string());
    }

    #[cfg(windows)]
    {
        let _ = path;
        return Ok(
            "Windows native service registration is not available in this MVP backend".to_string(),
        );
    }
}

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

#[cfg(target_os = "linux")]
async fn run_status(program: &str, args: &[&str]) -> Result<()> {
    let status = tokio::process::Command::new(program)
        .args(args)
        .status()
        .await?;
    if !status.success() {
        anyhow::bail!("{program} failed with status {status}");
    }
    Ok(())
}

async fn watch(target: Option<PathBuf>, interval_ms: u64, json: bool) -> Result<()> {
    let apps = resolve_apps(target).await?;
    for app in &apps {
        validate_runtime(app)?;
        start_app(app).await?;
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
        snapshots.push((app.clone(), snapshot_dir(&app.cwd)?));
    }

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(interval_ms.max(100))).await;
        for (app, snapshot) in &mut snapshots {
            let next = snapshot_dir(&app.cwd)?;
            if *snapshot != next {
                *snapshot = next;
                let restarted = restart_app(app).await?;
                if json {
                    print_json(serde_json::json!({ "restarted": restarted }))?;
                } else {
                    println!("Restarted {} after file change", app.name);
                }
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

async fn resolve_apps(target: Option<PathBuf>) -> Result<Vec<promon_core::ResolvedAppSpec>> {
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
            return Ok(promon_config::normalize_config(
                PromonConfig { apps: vec![app] },
                &temp_path,
            )?);
        }

        let config = resolve_config(Some(target))?;
        return load_config(&config).await.map_err(Into::into);
    }

    let config = resolve_config(None)?;
    load_config(&config).await.map_err(Into::into)
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
    match send_ipc(request).await {
        Ok(response) => Ok(Some(response)),
        Err(_) => Ok(None),
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
    name: &str,
    stream: &str,
    path: &PathBuf,
    offsets: &mut std::collections::BTreeMap<PathBuf, u64>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let bytes = tokio::fs::read(path).await?;
    let previous = *offsets.get(path).unwrap_or(&0) as usize;
    if bytes.len() <= previous {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&bytes[previous..]);
    for line in text.lines() {
        println!("{name} {stream} | {line}");
    }
    offsets.insert(path.clone(), bytes.len() as u64);
    Ok(())
}

fn snapshot_dir(root: &std::path::Path) -> Result<std::collections::BTreeMap<PathBuf, u64>> {
    let mut snapshot = std::collections::BTreeMap::new();
    collect_snapshot(root, root, &mut snapshot)?;
    Ok(snapshot)
}

fn collect_snapshot(
    root: &std::path::Path,
    dir: &std::path::Path,
    snapshot: &mut std::collections::BTreeMap<PathBuf, u64>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(
            name.as_ref(),
            ".git" | ".promon" | "node_modules" | "target"
        ) {
            continue;
        }
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            collect_snapshot(root, &path, snapshot)?;
        } else if metadata.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            let modified = metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            snapshot.insert(relative, modified ^ metadata.len());
        }
    }
    Ok(())
}
