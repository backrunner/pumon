use std::path::{Path, PathBuf};

use pumon_core::{PumonError, PumonResult};
use tokio::process::Command;
use which::which;

pub async fn load_js_config(path: &Path) -> PumonResult<serde_json::Value> {
    let loader = loader_path()?;
    let node = node_executable()?;
    let selected_env = std::env::var("PUMON_ENV")
        .ok()
        .or_else(|| std::env::var("NODE_ENV").ok())
        .filter(|value| !value.trim().is_empty());

    let candidates = config_loader_candidates(path);
    let mut last_error = None;
    for args in candidates {
        let mut command = Command::new(&node);
        command.args(args).arg(&loader).arg(path);
        if let Some(cwd) = path.parent() {
            command.current_dir(cwd);
        }
        if let Some(value) = &selected_env {
            command.env("NODE_ENV", value);
        }

        let output = command.output().await.map_err(PumonError::Io)?;
        if output.status.success() {
            return serde_json::from_slice(&output.stdout).map_err(PumonError::Json);
        }

        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        last_error = Some(stderr);
    }

    Err(PumonError::Config(format!(
        "failed to load {}: {}",
        path.display(),
        last_error
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "node config loader exited unsuccessfully".to_string())
    )))
}

fn node_executable() -> PumonResult<PathBuf> {
    if let Some(path) = std::env::var_os("PUMON_NODE_PATH").map(PathBuf::from) {
        if path.exists() {
            return Ok(path);
        }
        return Err(PumonError::Config(format!(
            "PUMON_NODE_PATH points to missing file: {}",
            path.display()
        )));
    }

    which("node").map_err(|_| PumonError::Config("node executable not found".to_string()))
}

fn is_typescript_config(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("ts" | "mts" | "cts")
    )
}

fn config_loader_candidates(path: &Path) -> Vec<Vec<&'static str>> {
    if !is_typescript_config(path) {
        return vec![Vec::new()];
    }

    let ext = path.extension().and_then(|value| value.to_str());
    let mut candidates = vec![vec!["--experimental-strip-types"], vec!["--import", "tsx"]];

    if matches!(ext, Some("cts")) {
        candidates.push(vec!["--require", "ts-node/register"]);
    } else {
        candidates.push(vec!["--loader", "ts-node/esm"]);
    }

    candidates
}

fn loader_path() -> PumonResult<PathBuf> {
    if let Some(path) = std::env::var_os("PUMON_NODE_SUPPORT_LOADER").map(PathBuf::from) {
        if path.exists() {
            return Ok(path);
        }
        return Err(PumonError::Config(format!(
            "PUMON_NODE_SUPPORT_LOADER points to missing file: {}",
            path.display()
        )));
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| PumonError::Config("cannot resolve workspace root".to_string()))?;
    let loader = workspace
        .join("packages")
        .join("node-support")
        .join("dist")
        .join("config-loader.js");
    if loader.exists() {
        Ok(loader)
    } else {
        Err(PumonError::Config(format!(
            "node support config loader not found at {}",
            loader.display()
        )))
    }
}
