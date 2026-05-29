use std::path::{Path, PathBuf};

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
    let paths = LogPaths {
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
    };
    if let Some(max_size) = app.log.max_size_bytes {
        rotate_if_needed(&paths.out, max_size, app.log.retain).await?;
        rotate_if_needed(&paths.err, max_size, app.log.retain).await?;
    }
    Ok(paths)
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

pub async fn tail_file(path: &PathBuf, lines: usize) -> std::io::Result<Vec<String>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw = fs::read_to_string(path).await?;
    let mut values: Vec<String> = raw.lines().map(ToOwned::to_owned).collect();
    if values.len() > lines {
        values = values.split_off(values.len() - lines);
    }
    Ok(values)
}

async fn rotate_if_needed(path: &PathBuf, max_size: u64, retain: usize) -> std::io::Result<()> {
    if max_size == 0 || retain == 0 || !path.exists() {
        return Ok(());
    }

    let metadata = fs::metadata(path).await?;
    if metadata.len() < max_size {
        return Ok(());
    }

    for index in (1..=retain).rev() {
        let source = rotated_path(path, index);
        let dest = rotated_path(path, index + 1);
        if source.exists() {
            if index == retain {
                let _ = fs::remove_file(&source).await;
            } else {
                let _ = fs::rename(&source, &dest).await;
            }
        }
    }
    fs::rename(path, rotated_path(path, 1)).await?;
    Ok(())
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".{index}"));
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_name("api/web"), "api_web");
    }
}
