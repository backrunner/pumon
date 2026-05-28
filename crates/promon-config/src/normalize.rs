use std::path::{Path, PathBuf};

use promon_core::{PromonConfig, PromonError, PromonResult, ResolvedAppSpec};

pub fn normalize_config(config: PromonConfig, path: &Path) -> PromonResult<Vec<ResolvedAppSpec>> {
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    if config.apps.is_empty() {
        return Err(PromonError::Config(
            "config does not contain apps".to_string(),
        ));
    }

    config
        .apps
        .into_iter()
        .map(|app| {
            if app.name.trim().is_empty() {
                return Err(PromonError::Config("app name is required".to_string()));
            }
            if app.script.is_none() && app.command.is_none() && app.package_script.is_none() {
                return Err(PromonError::Config(format!(
                    "app {} requires script, command, or package_script",
                    app.name
                )));
            }

            let cwd = app
                .cwd
                .map(|cwd| absolutize(base, &cwd))
                .unwrap_or_else(|| {
                    std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf())
                });

            Ok(ResolvedAppSpec {
                name: app.name,
                script: app.script,
                command: app.command,
                cwd,
                args: app.args,
                node_args: app.node_args,
                interpreter: app.interpreter.unwrap_or_else(|| "node".to_string()),
                interpreter_args: app.interpreter_args,
                package_manager: app.package_manager,
                package_script: app.package_script,
                env: app.env,
                exec_mode: app.exec_mode,
                instances: app.instances,
                watch: app.watch,
                restart: app.restart,
                max_memory_restart: app.max_memory_restart,
                cron_restart: app.cron_restart,
                log: app.log,
            })
        })
        .collect()
}

fn absolutize(base: &Path, value: &Path) -> PathBuf {
    if value.is_absolute() {
        value.to_path_buf()
    } else {
        base.join(value)
    }
}

#[cfg(test)]
mod tests {
    use promon_core::AppSpec;

    use super::*;

    #[test]
    fn requires_an_app() {
        let err = normalize_config(PromonConfig { apps: vec![] }, Path::new("x.json")).unwrap_err();
        assert!(err.to_string().contains("does not contain apps"));
    }

    #[test]
    fn normalizes_basic_app() {
        let app = AppSpec {
            name: "api".to_string(),
            script: Some(PathBuf::from("server.js")),
            ..AppSpec::default()
        };
        let result = normalize_config(PromonConfig { apps: vec![app] }, Path::new(".")).unwrap();
        assert_eq!(result[0].name, "api");
        assert_eq!(result[0].interpreter, "node");
    }
}
