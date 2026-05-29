use std::path::{Path, PathBuf};

use pumon_core::{
    AppSpec, LogPolicy, PumonConfig, PumonDaemonSpec, PumonError, PumonResult, PumonSettings,
    ResolvedAppSpec, ResolvedConfig,
};

pub fn normalize_config(config: PumonConfig, path: &Path) -> PumonResult<ResolvedConfig> {
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    if config.apps.is_empty() {
        return Err(PumonError::Config(
            "config does not contain apps".to_string(),
        ));
    }

    let pumon = resolve_pumon_settings(&config.pumon, base);
    let default_log = pumon.log_rotate.clone();
    let apps = config
        .apps
        .into_iter()
        .map(|app| normalize_app(app, base, &default_log))
        .collect::<PumonResult<Vec<_>>>()?;

    Ok(ResolvedConfig { pumon, apps })
}

fn normalize_app(
    app: AppSpec,
    base: &Path,
    default_log: &LogPolicy,
) -> PumonResult<ResolvedAppSpec> {
    if app.name.trim().is_empty() {
        return Err(PumonError::Config("app name is required".to_string()));
    }
    if app.script.is_none() && app.command.is_none() && app.package_script.is_none() {
        return Err(PumonError::Config(format!(
            "app {} requires script, command, or package_script",
            app.name
        )));
    }

    let cwd = app
        .cwd
        .map(|cwd| absolutize(base, &cwd))
        .unwrap_or_else(|| std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf()));
    let mut watch = app.watch;
    watch.ignore.extend(app.ignore_watch);

    let log = normalize_log_policy(merge_log_policy(default_log, app.log), &cwd);

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
        watch,
        restart: app.restart,
        max_memory_restart: app.max_memory_restart,
        cron_restart: app.cron_restart,
        log,
    })
}

fn resolve_pumon_settings(settings: &PumonSettings, base: &Path) -> PumonSettings {
    let home = settings.home.as_ref().map(|path| absolutize(base, path));
    let node_path = settings
        .node_path
        .as_ref()
        .map(|path| absolutize(base, path));
    let daemon = PumonDaemonSpec {
        enabled: settings.daemon.enabled,
        scope: settings.daemon.scope.clone(),
        ipc: settings
            .daemon
            .ipc
            .as_ref()
            .map(|path| absolutize(base, path)),
    };

    PumonSettings {
        home,
        node_path,
        daemon,
        log_rotate: settings.log_rotate.clone(),
    }
}

fn merge_log_policy(defaults: &LogPolicy, app: LogPolicy) -> LogPolicy {
    let default_retain = LogPolicy::default().retain;
    LogPolicy {
        out_file: app.out_file,
        err_file: app.err_file,
        merge: app.merge.or(defaults.merge),
        max_size_bytes: app.max_size_bytes.or(defaults.max_size_bytes),
        retain: if app.retain == default_retain {
            defaults.retain
        } else {
            app.retain
        },
    }
}

fn normalize_log_policy(mut log: LogPolicy, base: &Path) -> LogPolicy {
    log.out_file = log.out_file.map(|path| absolutize(base, &path));
    log.err_file = log.err_file.map(|path| absolutize(base, &path));
    log
}

fn absolutize(base: &Path, value: &Path) -> PathBuf {
    if value.is_absolute() {
        value.to_path_buf()
    } else {
        base.join(value)
    }
}

pub fn selected_env() -> Option<String> {
    std::env::var("PUMON_ENV")
        .ok()
        .or_else(|| std::env::var("NODE_ENV").ok())
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_an_app() {
        let err = normalize_config(
            PumonConfig {
                apps: vec![],
                ..PumonConfig::default()
            },
            Path::new("x.json"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not contain apps"));
    }

    #[test]
    fn normalizes_basic_app() {
        let app = AppSpec {
            name: "api".to_string(),
            script: Some(PathBuf::from("server.js")),
            ..AppSpec::default()
        };
        let result = normalize_config(
            PumonConfig {
                apps: vec![app],
                ..PumonConfig::default()
            },
            Path::new("."),
        )
        .unwrap();
        assert_eq!(result.apps[0].name, "api");
        assert_eq!(result.apps[0].interpreter, "node");
    }

    #[test]
    fn merges_default_log_rotation() {
        let app = AppSpec {
            name: "api".to_string(),
            script: Some(PathBuf::from("server.js")),
            ..AppSpec::default()
        };
        let result = normalize_config(
            PumonConfig {
                pumon: PumonSettings {
                    log_rotate: LogPolicy {
                        out_file: None,
                        err_file: None,
                        merge: Some(true),
                        max_size_bytes: Some(128),
                        retain: 2,
                    },
                    ..PumonSettings::default()
                },
                apps: vec![app],
            },
            Path::new("."),
        )
        .unwrap();
        assert_eq!(result.apps[0].log.max_size_bytes, Some(128));
        assert_eq!(result.apps[0].log.retain, 2);
        assert_eq!(result.apps[0].log.merge, Some(true));
    }

    #[test]
    fn normalizes_relative_log_paths_against_app_cwd() {
        let app = AppSpec {
            name: "api".to_string(),
            script: Some(PathBuf::from("server.js")),
            cwd: Some(PathBuf::from("services/api")),
            log: LogPolicy {
                out_file: Some(PathBuf::from("logs/out.log")),
                err_file: Some(PathBuf::from("logs/err.log")),
                ..LogPolicy::default()
            },
            ..AppSpec::default()
        };
        let result = normalize_config(
            PumonConfig {
                apps: vec![app],
                ..PumonConfig::default()
            },
            Path::new("/repo/ecosystem.config.json"),
        )
        .unwrap();

        assert_eq!(
            result.apps[0].log.out_file,
            Some(PathBuf::from("/repo/services/api/logs/out.log"))
        );
        assert_eq!(
            result.apps[0].log.err_file,
            Some(PathBuf::from("/repo/services/api/logs/err.log"))
        );
    }
}
