use std::path::Path;

use promon_core::{PromonConfig, PromonError, PromonResult, ResolvedAppSpec};
use promon_node_support::load_js_config;

use crate::{is_js_config, normalize_config};

pub async fn load_config(path: &Path) -> PromonResult<Vec<ResolvedAppSpec>> {
    let path = std::fs::canonicalize(path).map_err(PromonError::Io)?;
    let value = if is_js_config(&path) {
        load_js_config(&path).await?
    } else {
        load_static_config(&path)?
    };
    let config = deserialize_config(value)?;
    normalize_config(config, &path)
}

fn load_static_config(path: &Path) -> PromonResult<serde_json::Value> {
    let raw = std::fs::read_to_string(path).map_err(PromonError::Io)?;
    match path.extension().and_then(|value| value.to_str()) {
        Some("json") => serde_json::from_str(&raw).map_err(PromonError::Json),
        Some("toml") => {
            let value: toml::Value =
                toml::from_str(&raw).map_err(|err| PromonError::Config(err.to_string()))?;
            serde_json::to_value(value).map_err(PromonError::Json)
        }
        Some("yaml" | "yml") => {
            let value: serde_yaml::Value =
                serde_yaml::from_str(&raw).map_err(|err| PromonError::Config(err.to_string()))?;
            serde_json::to_value(value).map_err(PromonError::Json)
        }
        _ => Err(PromonError::Config(format!(
            "unsupported config file: {}",
            path.display()
        ))),
    }
}

fn deserialize_config(value: serde_json::Value) -> PromonResult<PromonConfig> {
    if value.is_array() {
        let apps = serde_json::from_value(value).map_err(PromonError::Json)?;
        return Ok(PromonConfig { apps });
    }

    if value.get("apps").is_some() {
        return serde_json::from_value(value).map_err(PromonError::Json);
    }

    let app = serde_json::from_value(value).map_err(PromonError::Json)?;
    Ok(PromonConfig { apps: vec![app] })
}
