use std::path::{Path, PathBuf};
use std::process::Stdio;

pub fn find_program(program: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    if let Some(cwd) = cwd {
        let local = cwd.join("node_modules").join(".bin").join(program);
        if local.exists() {
            return Some(local);
        }

        #[cfg(windows)]
        {
            let local_cmd = cwd
                .join("node_modules")
                .join(".bin")
                .join(format!("{program}.cmd"));
            if local_cmd.exists() {
                return Some(local_cmd);
            }
        }
    }

    which::which(program).ok()
}

pub fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}")])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

pub fn process_command(pid: u32) -> Option<String> {
    #[cfg(unix)]
    {
        std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "command="])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty())
    }

    #[cfg(windows)]
    {
        std::process::Command::new("wmic")
            .args([
                "process",
                "where",
                &format!("ProcessId={pid}"),
                "get",
                "CommandLine",
                "/value",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty())
    }
}

pub async fn terminate_process(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        tokio::process::Command::new("kill")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
    }

    #[cfg(windows)]
    {
        tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
    }

    Ok(())
}

pub async fn force_kill_process(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        tokio::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
    }

    #[cfg(windows)]
    {
        tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
    }

    Ok(())
}

pub async fn terminate_process_tree(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let group = format!("-{pid}");
        let status = tokio::process::Command::new("kill")
            .args(["-TERM", &group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
        if !status.success() {
            terminate_process(pid).await?;
        }
    }

    #[cfg(windows)]
    {
        terminate_process(pid).await?;
    }

    Ok(())
}

pub async fn force_kill_process_tree(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let group = format!("-{pid}");
        let status = tokio::process::Command::new("kill")
            .args(["-9", &group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
        if !status.success() {
            force_kill_process(pid).await?;
        }
    }

    #[cfg(windows)]
    {
        force_kill_process(pid).await?;
    }

    Ok(())
}
