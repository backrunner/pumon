use std::path::PathBuf;

use promon_core::ResolvedAppSpec;
use tokio::fs;

#[derive(Debug, Clone)]
pub struct LogPaths {
    pub out: PathBuf,
    pub err: PathBuf,
}

pub async fn ensure_log_paths(app: &ResolvedAppSpec, root: PathBuf) -> std::io::Result<LogPaths> {
    let dir = root.join(sanitize_name(&app.name));
    fs::create_dir_all(&dir).await?;
    Ok(LogPaths {
        out: app
            .log
            .out_file
            .clone()
            .unwrap_or_else(|| dir.join("out.log")),
        err: app
            .log
            .err_file
            .clone()
            .unwrap_or_else(|| dir.join("err.log")),
    })
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_name("api/web"), "api_web");
    }
}
