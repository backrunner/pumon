use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use promon_config::{find_config, load_config};
use promon_node_support::validate_runtime;
use promon_platform::{find_program, promon_home};
use promon_process::{list_apps, start_app, stop_app};

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
    },
    Stop {
        name: String,
    },
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init { output } => init(output, cli.json).await,
        Commands::Validate { config } => validate(config, cli.json).await,
        Commands::Doctor => doctor(cli.json).await,
        Commands::Start { target } => start(target, cli.json).await,
        Commands::Stop { name } => stop(name, cli.json).await,
        Commands::List => list(cli.json).await,
    }
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

async fn start(target: Option<PathBuf>, json: bool) -> Result<()> {
    let config = resolve_config(target)?;
    let apps = load_config(&config).await?;
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
    let stopped = stop_app(&name).await?;
    if json {
        print_json(serde_json::json!({ "stopped": stopped }))?;
    } else if let Some(process) = stopped {
        println!("Stopped {} pid={}", process.name, process.pid);
    } else {
        println!("No managed process named {name}");
    }
    Ok(())
}

async fn list(json: bool) -> Result<()> {
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

fn print_json(value: serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
