use std::path::{Path, PathBuf};

use procwatch_core::ResolvedAppSpec;
use tokio::fs::{self, OpenOptions};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

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
        err: if app.log.merge.unwrap_or(false) {
            app.log
                .out_file
                .clone()
                .unwrap_or_else(|| dir.join("out.log"))
        } else {
            app.log
                .err_file
                .clone()
                .unwrap_or_else(|| dir.join("err.log"))
        },
    };
    if let Some(max_size) = app.log.max_size_bytes {
        rotate_if_needed(&paths.out, max_size, app.log.retain).await?;
        rotate_if_needed(&paths.err, max_size, app.log.retain).await?;
    }
    ensure_parent_dir(&paths.out).await?;
    ensure_parent_dir(&paths.err).await?;
    Ok(paths)
}

async fn ensure_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    Ok(())
}

pub fn spawn_rotating_log_writer<R>(
    reader: R,
    path: PathBuf,
    max_size_bytes: Option<u64>,
    retain: usize,
) -> JoinHandle<std::io::Result<()>>
where
    R: AsyncRead + Send + Unpin + 'static,
{
    tokio::spawn(async move {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await?;
        }
        if let Some(max_size) = max_size_bytes {
            rotate_if_needed(&path, max_size, retain).await?;
        }

        let mut reader = reader;
        let mut file = open_append(&path).await?;
        let mut current_size = fs::metadata(&path)
            .await
            .map(|meta| meta.len())
            .unwrap_or(0);
        let mut buffer = vec![0_u8; 8192];

        loop {
            let read = reader.read(&mut buffer).await?;
            if read == 0 {
                file.flush().await?;
                return Ok(());
            }

            if let Some(max_size) = max_size_bytes {
                if max_size > 0
                    && retain > 0
                    && current_size > 0
                    && current_size.saturating_add(read as u64) > max_size
                {
                    file.flush().await?;
                    drop(file);
                    rotate_existing(&path, retain).await?;
                    file = open_append(&path).await?;
                    current_size = fs::metadata(&path)
                        .await
                        .map(|meta| meta.len())
                        .unwrap_or(0);
                }
            }

            file.write_all(&buffer[..read]).await?;
            current_size = current_size.saturating_add(read as u64);
        }
    })
}

async fn open_append(path: &Path) -> std::io::Result<fs::File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
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

pub async fn rotate_if_needed(path: &PathBuf, max_size: u64, retain: usize) -> std::io::Result<()> {
    if max_size == 0 || retain == 0 || !path.exists() {
        return Ok(());
    }

    let metadata = fs::metadata(path).await?;
    if metadata.len() < max_size {
        return Ok(());
    }

    rotate_existing(path, retain).await
}

async fn rotate_existing(path: &PathBuf, retain: usize) -> std::io::Result<()> {
    if retain == 0 || !path.exists() {
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
    use std::collections::VecDeque;
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::ReadBuf;

    use super::*;

    #[test]
    fn sanitizes_names() {
        assert_eq!(sanitize_name("api/web"), "api_web");
    }

    #[tokio::test]
    async fn writer_rotates_before_exceeding_limit() {
        let dir = temp_dir("runtime-rotate");
        let path = dir.join("out.log");
        let reader = ChunkReader::new(vec![b"123456".as_slice(), b"abcdef".as_slice()]);

        spawn_rotating_log_writer(reader, path.clone(), Some(10), 2)
            .await
            .expect("writer task should finish")
            .expect("writer should succeed");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abcdef");
        assert_eq!(
            std::fs::read_to_string(rotated_path(&path, 1)).unwrap(),
            "123456"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn ensure_log_paths_merges_streams_when_requested() {
        let dir = temp_dir("merge");
        let mut app = test_app();
        app.log.merge = Some(true);

        let paths = ensure_log_paths(&app, dir.clone())
            .await
            .expect("log paths should be created");

        assert_eq!(paths.out, paths.err);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn ensure_log_paths_creates_custom_parent_dirs() {
        let dir = temp_dir("custom-parent");
        let mut app = test_app();
        app.log.out_file = Some(dir.join("nested").join("out.log"));
        app.log.err_file = Some(dir.join("nested").join("err.log"));

        let paths = ensure_log_paths(&app, dir.clone())
            .await
            .expect("custom log parents should be created");

        assert!(paths.out.parent().unwrap().exists());
        assert!(paths.err.parent().unwrap().exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    struct ChunkReader {
        chunks: VecDeque<&'static [u8]>,
    }

    impl ChunkReader {
        fn new(chunks: Vec<&'static [u8]>) -> Self {
            Self {
                chunks: chunks.into(),
            }
        }
    }

    impl AsyncRead for ChunkReader {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if let Some(chunk) = self.chunks.pop_front() {
                buffer.put_slice(chunk);
            }
            Poll::Ready(Ok(()))
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("procwatch-log-{name}-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn test_app() -> ResolvedAppSpec {
        use std::collections::BTreeMap;

        use procwatch_core::{ExecMode, Instances, LogPolicy, RestartPolicy, WatchSpec};

        ResolvedAppSpec {
            name: "api".to_string(),
            script: Some(PathBuf::from("server.js")),
            command: None,
            cwd: PathBuf::from("/tmp/api"),
            args: Vec::new(),
            node_args: Vec::new(),
            interpreter: "node".to_string(),
            interpreter_args: Vec::new(),
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
}
