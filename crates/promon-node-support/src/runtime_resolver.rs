use std::path::{Path, PathBuf};

use promon_core::{PromonError, PromonResult, ResolvedAppSpec, RuntimeCommand};
use promon_platform::find_program;

use crate::{package_manager_from_package_json, read_package_json};

pub fn resolve_runtime_command(app: &ResolvedAppSpec) -> PromonResult<RuntimeCommand> {
    if app.package_script.is_some() {
        return resolve_package_script(app);
    }

    if let Some(command) = &app.command {
        return resolve_custom_command(app, command);
    }

    resolve_script(app)
}

pub fn validate_runtime(app: &ResolvedAppSpec) -> PromonResult<()> {
    resolve_runtime_command(app).map(|_| ())
}

fn resolve_script(app: &ResolvedAppSpec) -> PromonResult<RuntimeCommand> {
    let script = app
        .script
        .as_ref()
        .ok_or_else(|| PromonError::Runtime(format!("app {} is missing script", app.name)))?;
    let script_path = absolutize(&app.cwd, script);
    if !script_path.exists() {
        return Err(PromonError::Runtime(format!(
            "script not found for {}: {}",
            app.name,
            script_path.display()
        )));
    }

    let ext = script_path.extension().and_then(|value| value.to_str());
    let is_ts = matches!(ext, Some("ts" | "mts" | "cts"));
    let has_loader = !app.node_args.is_empty() || !app.interpreter_args.is_empty();
    let interpreter = app.interpreter.as_str();

    if is_ts && interpreter == "node" && !has_loader {
        return Err(PromonError::Runtime(format!(
            "TypeScript app {} requires node_args, interpreter_args, or a TS runner such as tsx",
            app.name
        )));
    }

    let program = find_program(interpreter, Some(&app.cwd)).ok_or_else(|| {
        PromonError::Runtime(format!(
            "interpreter not found for {}: {}",
            app.name, interpreter
        ))
    })?;

    let mut args = Vec::new();
    args.extend(app.node_args.clone());
    args.extend(app.interpreter_args.clone());
    args.push(script_path.to_string_lossy().to_string());
    args.extend(app.args.clone());

    Ok(RuntimeCommand {
        program,
        args,
        cwd: app.cwd.clone(),
        env: app.env.clone(),
    })
}

fn resolve_package_script(app: &ResolvedAppSpec) -> PromonResult<RuntimeCommand> {
    let package = read_package_json(&app.cwd)?;
    let script_name = app.package_script.as_deref().unwrap_or("start");
    let has_script = package
        .as_ref()
        .and_then(|package| package.scripts.as_ref())
        .map(|scripts| scripts.contains_key(script_name))
        .unwrap_or(false);
    if !has_script {
        return Err(PromonError::Runtime(format!(
            "package script not found for {}: {}",
            app.name, script_name
        )));
    }

    let manager = app
        .package_manager
        .clone()
        .or_else(|| package_manager_from_package_json(package.as_ref()))
        .unwrap_or_else(|| "npm".to_string());
    let program = find_program(&manager, Some(&app.cwd)).ok_or_else(|| {
        PromonError::Runtime(format!(
            "package manager not found for {}: {}",
            app.name, manager
        ))
    })?;

    let mut args = match manager.as_str() {
        "yarn" => vec![script_name.to_string()],
        _ => vec!["run".to_string(), script_name.to_string()],
    };
    if !app.args.is_empty() {
        args.push("--".to_string());
        args.extend(app.args.clone());
    }

    Ok(RuntimeCommand {
        program,
        args,
        cwd: app.cwd.clone(),
        env: app.env.clone(),
    })
}

fn resolve_custom_command(app: &ResolvedAppSpec, command: &str) -> PromonResult<RuntimeCommand> {
    let program = find_program(command, Some(&app.cwd)).ok_or_else(|| {
        PromonError::Runtime(format!("command not found for {}: {}", app.name, command))
    })?;

    Ok(RuntimeCommand {
        program,
        args: app.args.clone(),
        cwd: app.cwd.clone(),
        env: app.env.clone(),
    })
}

fn absolutize(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use promon_core::{ExecMode, Instances, LogPolicy, RestartPolicy, WatchSpec};

    use super::*;

    #[test]
    fn rejects_ts_without_loader() {
        let app = ResolvedAppSpec {
            name: "ts-app".to_string(),
            script: Some(PathBuf::from("src/server.ts")),
            command: None,
            cwd: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            args: vec![],
            node_args: vec![],
            interpreter: "node".to_string(),
            interpreter_args: vec![],
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
        };

        let err = resolve_runtime_command(&app).unwrap_err();
        assert!(
            err.to_string().contains("script not found") || err.to_string().contains("TypeScript")
        );
    }
}
