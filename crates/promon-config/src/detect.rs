use std::path::{Path, PathBuf};

pub const CONFIG_NAMES: &[&str] = &[
    "ecosystem.config.js",
    "ecosystem.config.cjs",
    "ecosystem.config.mjs",
    "ecosystem.config.ts",
    "ecosystem.config.mts",
    "ecosystem.config.cts",
    "ecosystem.config.json",
    "ecosystem.config.toml",
    "ecosystem.config.yaml",
    "ecosystem.config.yml",
];

pub fn find_config(start: &Path) -> Option<PathBuf> {
    CONFIG_NAMES
        .iter()
        .map(|name| start.join(name))
        .find(|path| path.exists())
}

pub fn is_js_config(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("js" | "cjs" | "mjs" | "ts" | "mts" | "cts")
    )
}
