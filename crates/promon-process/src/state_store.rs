use std::path::PathBuf;

use promon_core::{ManagedProcess, PromonError, PromonResult};
use promon_platform::state_dir;
use tokio::fs;

fn state_file() -> PathBuf {
    state_dir().join("processes.json")
}

pub async fn load_processes() -> PromonResult<Vec<ManagedProcess>> {
    let path = state_file();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path).await.map_err(PromonError::Io)?;
    serde_json::from_str(&raw).map_err(PromonError::Json)
}

pub async fn save_processes(processes: &[ManagedProcess]) -> PromonResult<()> {
    fs::create_dir_all(state_dir())
        .await
        .map_err(PromonError::Io)?;
    let raw = serde_json::to_string_pretty(processes).map_err(PromonError::Json)?;
    fs::write(state_file(), raw).await.map_err(PromonError::Io)
}

pub async fn upsert_process(process: ManagedProcess) -> PromonResult<()> {
    let mut processes = load_processes().await?;
    processes.retain(|item| item.name != process.name);
    processes.push(process);
    save_processes(&processes).await
}

pub async fn remove_process(name: &str) -> PromonResult<Option<ManagedProcess>> {
    let mut processes = load_processes().await?;
    let removed = processes
        .iter()
        .position(|item| item.name == name)
        .map(|index| processes.remove(index));
    save_processes(&processes).await?;
    Ok(removed)
}
