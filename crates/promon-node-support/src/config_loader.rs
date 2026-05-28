use std::path::{Path, PathBuf};

use promon_core::{PromonError, PromonResult};
use tokio::process::Command;

pub async fn load_js_config(path: &Path) -> PromonResult<serde_json::Value> {
    let loader = loader_path()?;
    let output = Command::new("node")
        .arg(loader)
        .arg(path)
        .output()
        .await
        .map_err(PromonError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(PromonError::Config(format!(
            "failed to load {}: {}",
            path.display(),
            stderr.trim()
        )));
    }

    serde_json::from_slice(&output.stdout).map_err(PromonError::Json)
}

fn loader_path() -> PromonResult<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| PromonError::Config("cannot resolve workspace root".to_string()))?;
    let loader = workspace
        .join("packages")
        .join("node-support")
        .join("dist")
        .join("config-loader.js");
    if loader.exists() {
        Ok(loader)
    } else {
        Err(PromonError::Config(format!(
            "node support config loader not found at {}",
            loader.display()
        )))
    }
}
