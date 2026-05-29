use std::path::Path;

use procwatch_core::{
    ProcwatchConfig, ProcwatchError, ProcwatchResult, ProcwatchSettings, ResolvedConfig,
};
use procwatch_node_support::load_js_config;

use crate::{is_js_config, normalize_config};

pub async fn load_config(path: &Path) -> ProcwatchResult<ResolvedConfig> {
    let path = std::fs::canonicalize(path).map_err(ProcwatchError::Io)?;
    let mut value = if is_js_config(&path) {
        load_js_config(&path).await?
    } else {
        load_static_config(&path)?
    };
    apply_env_overlays(&mut value, crate::selected_env().as_deref())?;
    let config = deserialize_config(value)?;
    let resolved = normalize_config(config, &path)?;
    apply_procwatch_settings(&resolved.procwatch);
    Ok(resolved)
}

fn load_static_config(path: &Path) -> ProcwatchResult<serde_json::Value> {
    let raw = std::fs::read_to_string(path).map_err(ProcwatchError::Io)?;
    match path.extension().and_then(|value| value.to_str()) {
        Some("json") => serde_json::from_str(&raw).map_err(ProcwatchError::Json),
        Some("toml") => {
            let value: toml::Value =
                toml::from_str(&raw).map_err(|err| ProcwatchError::Config(err.to_string()))?;
            serde_json::to_value(value).map_err(ProcwatchError::Json)
        }
        Some("yaml" | "yml") => {
            let value: serde_yaml::Value = serde_yaml::from_str(&raw)
                .map_err(|err| ProcwatchError::Config(err.to_string()))?;
            serde_json::to_value(value).map_err(ProcwatchError::Json)
        }
        _ => Err(ProcwatchError::Config(format!(
            "unsupported config file: {}",
            path.display()
        ))),
    }
}

fn deserialize_config(value: serde_json::Value) -> ProcwatchResult<ProcwatchConfig> {
    if value.is_array() {
        let apps = serde_json::from_value(value).map_err(ProcwatchError::Json)?;
        return Ok(ProcwatchConfig {
            apps,
            ..ProcwatchConfig::default()
        });
    }

    if value.get("apps").is_some() || value.get("procwatch").is_some() {
        return serde_json::from_value(value).map_err(ProcwatchError::Json);
    }

    let app = serde_json::from_value(value).map_err(ProcwatchError::Json)?;
    Ok(ProcwatchConfig {
        apps: vec![app],
        ..ProcwatchConfig::default()
    })
}

fn apply_procwatch_settings(settings: &ProcwatchSettings) {
    if let Some(home) = &settings.home {
        std::env::set_var("PROCWATCH_HOME", home);
    }
    if let Some(node_path) = &settings.node_path {
        std::env::set_var("PROCWATCH_NODE_PATH", node_path);
    }
}

fn apply_env_overlays(
    value: &mut serde_json::Value,
    selected_env: Option<&str>,
) -> ProcwatchResult<()> {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                apply_env_overlays_to_app(item, selected_env)?;
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(apps) = map.get_mut("apps") {
                if let Some(items) = apps.as_array_mut() {
                    for item in items {
                        apply_env_overlays_to_app(item, selected_env)?;
                    }
                }
            } else if is_app_like(map) {
                apply_env_overlays_to_app(value, selected_env)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_app_like(map: &serde_json::Map<String, serde_json::Value>) -> bool {
    map.contains_key("name")
        || map.contains_key("script")
        || map.contains_key("command")
        || map.contains_key("package_script")
}

fn apply_env_overlays_to_app(
    value: &mut serde_json::Value,
    selected_env: Option<&str>,
) -> ProcwatchResult<()> {
    let Some(map) = value.as_object_mut() else {
        return Ok(());
    };

    let mut merged_env = if let Some(env) = map.remove("env") {
        json_object_to_string_map(env, "env")?
    } else {
        std::collections::BTreeMap::new()
    };

    if let Some(selected_env) = selected_env {
        let key = format!("env_{selected_env}");
        if let Some(overlay) = map.remove(&key) {
            let overlay = json_object_to_string_map(overlay, &key)?;
            merged_env.extend(overlay);
        }
    }

    let overlay_keys: Vec<String> = map
        .keys()
        .filter(|key| key.starts_with("env_"))
        .cloned()
        .collect();
    for key in overlay_keys {
        map.remove(&key);
    }

    map.insert(
        "env".to_string(),
        serde_json::to_value(merged_env).map_err(ProcwatchError::Json)?,
    );
    Ok(())
}

fn json_object_to_string_map(
    value: serde_json::Value,
    field: &str,
) -> ProcwatchResult<std::collections::BTreeMap<String, String>> {
    let Some(map) = value.as_object() else {
        return Err(ProcwatchError::Config(format!("{field} must be an object")));
    };

    let mut result = std::collections::BTreeMap::new();
    for (key, value) in map {
        let string = match value {
            serde_json::Value::String(value) => value.clone(),
            serde_json::Value::Bool(value) => value.to_string(),
            serde_json::Value::Number(value) => value.to_string(),
            serde_json::Value::Null => String::new(),
            other => {
                return Err(ProcwatchError::Config(format!(
                    "{field}.{key} must be a string-compatible scalar, got {other}"
                )));
            }
        };
        result.insert(key.clone(), string);
    }
    Ok(result)
}
