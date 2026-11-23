// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Persistent per-provider model catalog cache under ~/.rein/models-cache.json

use std::collections::BTreeMap;
use std::path::Path;

use rein_core::AvailableModel;

use super::ProviderKind;

const CACHE_FILE_NAME: &str = "models-cache.json";

/// Loads cached models for `kind` from `<global_dir>/models-cache.json`, returning empty on miss/error.
#[must_use]
pub fn load_cached_models(global_dir: &Path, kind: ProviderKind) -> Vec<AvailableModel> {
    let path = global_dir.join(CACHE_FILE_NAME);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(map) = serde_json::from_str::<BTreeMap<String, Vec<AvailableModel>>>(&raw) else {
        return Vec::new();
    };
    map.get(kind.as_str()).cloned().unwrap_or_default()
}

/// Persists `models` for `kind` into `<global_dir>/models-cache.json` atomically.
pub fn save_cached_models(global_dir: &Path, kind: ProviderKind, models: &[AvailableModel]) {
    if models.is_empty() {
        return;
    }
    let path = global_dir.join(CACHE_FILE_NAME);
    let mut map: BTreeMap<String, Vec<AvailableModel>> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    map.insert(kind.as_str().to_owned(), models.to_vec());
    if let Ok(encoded) = serde_json::to_string_pretty(&map) {
        let tmp = global_dir.join(format!(".{CACHE_FILE_NAME}.tmp"));
        if std::fs::write(&tmp, encoded).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_loads_per_provider_model_cache() -> Result<(), Box<dyn std::error::Error>> {
        let dir = std::env::temp_dir().join("rein_models_cache_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;

        assert!(load_cached_models(&dir, ProviderKind::Antigravity).is_empty());

        let models = vec![AvailableModel {
            id: "claude-sonnet-5-5-medium".to_owned(),
            display_name: "Claude Sonnet 5.5 (Medium)".to_owned(),
            quota_percent: Some(95),
            recommended: true,
        }];
        save_cached_models(&dir, ProviderKind::Antigravity, &models);

        let loaded = load_cached_models(&dir, ProviderKind::Antigravity);
        assert_eq!(loaded, models);
        assert!(load_cached_models(&dir, ProviderKind::Anthropic).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    }
}
