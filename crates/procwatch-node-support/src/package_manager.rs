use std::path::Path;

use procwatch_core::{ProcwatchError, ProcwatchResult};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct PackageJson {
    #[serde(rename = "packageManager")]
    pub package_manager: Option<String>,
    pub scripts: Option<std::collections::BTreeMap<String, String>>,
}

pub fn read_package_json(cwd: &Path) -> ProcwatchResult<Option<PackageJson>> {
    let path = cwd.join("package.json");
    if !path.exists() {
        return Ok(None);
    }

    let raw = std::fs::read_to_string(&path).map_err(ProcwatchError::Io)?;
    let package = serde_json::from_str(&raw).map_err(ProcwatchError::Json)?;
    Ok(Some(package))
}

pub fn package_manager_from_package_json(package: Option<&PackageJson>) -> Option<String> {
    package
        .and_then(|package| package.package_manager.as_deref())
        .and_then(|value| value.split('@').next())
        .map(ToOwned::to_owned)
}
