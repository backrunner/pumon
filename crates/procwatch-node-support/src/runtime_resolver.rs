use std::path::{Path, PathBuf};

use procwatch_core::{
    ExecMode, Instances, ProcwatchError, ProcwatchResult, ResolvedAppSpec, RuntimeCommand,
};
use procwatch_platform::{find_program, procwatch_home};

use crate::{package_manager_from_package_json, read_package_json};

pub fn resolve_runtime_command(app: &ResolvedAppSpec) -> ProcwatchResult<RuntimeCommand> {
    if app.exec_mode == ExecMode::Cluster {
        return resolve_cluster(app);
    }

    if app.package_script.is_some() {
        return resolve_package_script(app);
    }

    if let Some(command) = &app.command {
        return resolve_custom_command(app, command);
    }

    resolve_script(app)
}

fn resolve_cluster(app: &ResolvedAppSpec) -> ProcwatchResult<RuntimeCommand> {
    let node = resolve_program("node", Some(&app.cwd))?;
    let shim = cluster_shim_path()?;
    let worker = resolve_worker_plan(app)?;
    let spec = serde_json::json!({
        "name": app.name,
        "instances": resolve_instances(&app.instances),
        "controlPath": cluster_control_path(&app.name),
        "worker": worker,
    });

    Ok(RuntimeCommand {
        program: node,
        args: vec![
            shim.to_string_lossy().to_string(),
            serde_json::to_string(&spec).map_err(ProcwatchError::Json)?,
        ],
        cwd: app.cwd.clone(),
        env: app.env.clone(),
    })
}

fn resolve_worker_plan(app: &ResolvedAppSpec) -> ProcwatchResult<serde_json::Value> {
    let script = app.script.as_ref().ok_or_else(|| {
        ProcwatchError::Runtime(format!("cluster app {} requires script", app.name))
    })?;
    let script_path = absolutize(&app.cwd, script);
    if !script_path.exists() {
        return Err(ProcwatchError::Runtime(format!(
            "script not found for {}: {}",
            app.name,
            script_path.display()
        )));
    }
    Ok(serde_json::json!({
        "script": script_path,
        "args": app.args,
        "nodeArgs": app.node_args,
        "interpreter": app.interpreter,
        "interpreterArgs": app.interpreter_args,
        "cwd": app.cwd,
        "env": app.env,
    }))
}

fn cluster_shim_path() -> ProcwatchResult<PathBuf> {
    if let Some(path) = std::env::var_os("PROCWATCH_CLUSTER_SHIM").map(PathBuf::from) {
        if path.exists() {
            return Ok(path);
        }
        return Err(ProcwatchError::Runtime(format!(
            "PROCWATCH_CLUSTER_SHIM points to missing file: {}",
            path.display()
        )));
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| ProcwatchError::Runtime("cannot resolve workspace root".to_string()))?;
    let shim = workspace
        .join("packages")
        .join("cluster-shim")
        .join("dist")
        .join("index.js");
    if shim.exists() {
        Ok(shim)
    } else {
        Err(ProcwatchError::Runtime(format!(
            "cluster shim not found at {}",
            shim.display()
        )))
    }
}

pub fn cluster_control_path(name: &str) -> PathBuf {
    procwatch_home()
        .join("cluster")
        .join(format!("{}.addr", sanitize_control_name(name)))
}

fn sanitize_control_name(value: &str) -> String {
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

pub fn resolve_instances(instances: &Instances) -> usize {
    match instances {
        Instances::Count(value) => (*value).max(1) as usize,
        Instances::Max(_) => std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1),
    }
}

pub fn validate_runtime(app: &ResolvedAppSpec) -> ProcwatchResult<()> {
    resolve_runtime_command(app).map(|_| ())
}

fn resolve_script(app: &ResolvedAppSpec) -> ProcwatchResult<RuntimeCommand> {
    let script = app
        .script
        .as_ref()
        .ok_or_else(|| ProcwatchError::Runtime(format!("app {} is missing script", app.name)))?;
    let script_path = absolutize(&app.cwd, script);
    if !script_path.exists() {
        return Err(ProcwatchError::Runtime(format!(
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
        return Err(ProcwatchError::Runtime(format!(
            "TypeScript app {} requires node_args, interpreter_args, or a TS runner such as tsx",
            app.name
        )));
    }

    let program = resolve_program(interpreter, Some(&app.cwd))?;

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

fn resolve_package_script(app: &ResolvedAppSpec) -> ProcwatchResult<RuntimeCommand> {
    let package = read_package_json(&app.cwd)?;
    let script_name = app.package_script.as_deref().unwrap_or("start");
    let has_script = package
        .as_ref()
        .and_then(|package| package.scripts.as_ref())
        .map(|scripts| scripts.contains_key(script_name))
        .unwrap_or(false);
    if !has_script {
        return Err(ProcwatchError::Runtime(format!(
            "package script not found for {}: {}",
            app.name, script_name
        )));
    }

    let manager = app
        .package_manager
        .clone()
        .or_else(|| package_manager_from_package_json(package.as_ref()))
        .unwrap_or_else(|| "npm".to_string());
    let program = resolve_program(&manager, Some(&app.cwd))?;

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

fn resolve_custom_command(app: &ResolvedAppSpec, command: &str) -> ProcwatchResult<RuntimeCommand> {
    let program = resolve_program(command, Some(&app.cwd))?;

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

fn resolve_program(program: &str, cwd: Option<&Path>) -> ProcwatchResult<PathBuf> {
    if is_node_program(program) {
        if let Some(path) = node_path_override() {
            if path.exists() {
                return Ok(path);
            }
            return Err(ProcwatchError::Runtime(format!(
                "configured node executable not found: {}",
                path.display()
            )));
        }
    }

    find_program(program, cwd)
        .ok_or_else(|| ProcwatchError::Runtime(format!("program not found: {}", program)))
}

fn is_node_program(program: &str) -> bool {
    matches!(program, "node" | "node.exe")
}

fn node_path_override() -> Option<PathBuf> {
    std::env::var_os("PROCWATCH_NODE_PATH").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use procwatch_core::{ExecMode, Instances, LogPolicy, RestartPolicy, WatchSpec};

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
