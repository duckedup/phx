use std::path::Path;

use tracing::debug;

use super::error::ConfigError;
use super::schema::{Config, ProviderProfile};

/// Atomically save a full `Config` to `path` as pretty-printed JSON.
///
/// Writes to a temporary file in the same directory, then renames into place.
/// This avoids partial writes visible to concurrent readers.
pub fn save(cfg: &Config, path: &Path) -> Result<(), ConfigError> {
    let json = serde_json::to_string_pretty(cfg).map_err(|e| ConfigError::Schema(e.to_string()))?;

    atomic_write(path, json.as_bytes())?;

    // Best-effort chmod 0600 on POSIX.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    debug!("saved config to {}", path.display());
    Ok(())
}

/// Load config from `path`, insert/replace a single provider entry, and
/// re-save atomically. Other sections (sessions, store, etc.) are preserved.
pub fn save_provider(
    path: &Path,
    name: &str,
    provider: &ProviderProfile,
) -> Result<(), ConfigError> {
    let mut cfg = if path.exists() {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::io(path, e))?;
        serde_json::from_str::<Config>(&text).map_err(ConfigError::Parse)?
    } else {
        Config::default()
    };

    cfg.providers.insert(name.to_owned(), provider.clone());

    save(&cfg, path)?;
    debug!("saved provider '{}' to {}", name, path.display());
    Ok(())
}

/// Load config from `path`, set the active provider and model, and re-save.
///
/// Deactivates all other providers so only the chosen one is active.
pub fn save_active_model(path: &Path, provider_name: &str, model: &str) -> Result<(), ConfigError> {
    let mut cfg = if path.exists() {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::io(path, e))?;
        serde_json::from_str::<Config>(&text).map_err(ConfigError::Parse)?
    } else {
        Config::default()
    };

    for (_, p) in cfg.providers.iter_mut() {
        p.active = false;
    }
    if let Some(profile) = cfg.providers.get_mut(provider_name) {
        profile.model = model.to_string();
        profile.active = true;
    }

    save(&cfg, path)?;
    debug!(
        "saved active model '{}' for provider '{}' to {}",
        model,
        provider_name,
        path.display()
    );
    Ok(())
}

/// Load config from `path`, remove a provider entry, and re-save atomically.
pub fn delete_provider(path: &Path, name: &str) -> Result<(), ConfigError> {
    let mut cfg = if path.exists() {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::io(path, e))?;
        serde_json::from_str::<Config>(&text).map_err(ConfigError::Parse)?
    } else {
        return Ok(());
    };

    cfg.providers.remove(name);

    save(&cfg, path)?;
    debug!("deleted provider '{}' from {}", name, path.display());
    Ok(())
}

/// Load config from `path`, set the theme, and re-save atomically.
pub fn save_theme(path: &Path, theme_id: &str) -> Result<(), ConfigError> {
    let mut cfg = if path.exists() {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::io(path, e))?;
        serde_json::from_str::<Config>(&text).map_err(ConfigError::Parse)?
    } else {
        Config::default()
    };

    cfg.theme = Some(theme_id.to_owned());

    save(&cfg, path)?;
    debug!("saved theme '{}' to {}", theme_id, path.display());
    Ok(())
}

/// Write `data` to a temp file next to `target`, then atomically rename.
fn atomic_write(target: &Path, data: &[u8]) -> Result<(), ConfigError> {
    // Ensure parent directory exists.
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::io(parent, e))?;
    }

    // Create a temp file in the same directory (same filesystem) so rename is
    // atomic.
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let tmp_path = parent.join(format!(".phoenix-cfg-{}.tmp", std::process::id()));

    std::fs::write(&tmp_path, data).map_err(|e| ConfigError::io(&tmp_path, e))?;

    // Atomic rename.
    std::fs::rename(&tmp_path, target).map_err(|e| {
        // Clean up temp file on rename failure.
        let _ = std::fs::remove_file(&tmp_path);
        ConfigError::io(target, e)
    })?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::*;

    #[test]
    fn roundtrip_default_config() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("phoenix.json");

        let mut cfg = Config::default();
        cfg.providers.insert(
            "claude".into(),
            ProviderProfile {
                kind: ProviderKind::Claude,
                model: "claude-opus-4-7".into(),
                active: true,
                ..Default::default()
            },
        );

        save(&cfg, &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let loaded: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers["claude"].model, "claude-opus-4-7");
        assert!(loaded.providers["claude"].active);
    }

    #[test]
    fn save_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("deep").join("nested").join("phoenix.json");

        let cfg = Config::default();
        save(&cfg, &path).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn save_provider_creates_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("phoenix.json");

        let profile = ProviderProfile {
            kind: ProviderKind::OpenAI,
            model: "gpt-4o".into(),
            active: true,
            auth: Some(AuthEntry::EnvVar("OPENAI_API_KEY".into())),
            ..Default::default()
        };

        save_provider(&path, "openai", &profile).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let loaded: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers["openai"].kind, ProviderKind::OpenAI);
        assert_eq!(
            loaded.providers["openai"].auth,
            Some(AuthEntry::EnvVar("OPENAI_API_KEY".into()))
        );
    }

    #[test]
    fn save_provider_preserves_other_sections() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("phoenix.json");

        // Write initial config with a session and a provider.
        let mut cfg = Config::default();
        cfg.providers.insert(
            "claude".into(),
            ProviderProfile {
                kind: ProviderKind::Claude,
                model: "claude-opus-4-7".into(),
                active: true,
                ..Default::default()
            },
        );
        cfg.sessions.insert(
            "default".into(),
            SessionProfile {
                name: "default".into(),
                persist: true,
                ..Default::default()
            },
        );
        save(&cfg, &path).unwrap();

        // Now save a new provider entry.
        let new_provider = ProviderProfile {
            kind: ProviderKind::Ollama,
            model: "llama3".into(),
            active: false,
            base_url: Some("http://localhost:11434".into()),
            ..Default::default()
        };
        save_provider(&path, "local-ollama", &new_provider).unwrap();

        // Verify both providers and the session are present.
        let text = std::fs::read_to_string(&path).unwrap();
        let loaded: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(loaded.providers.len(), 2);
        assert!(loaded.providers.contains_key("claude"));
        assert!(loaded.providers.contains_key("local-ollama"));
        assert_eq!(loaded.sessions.len(), 1);
        assert!(loaded.sessions.contains_key("default"));
    }

    #[test]
    fn save_provider_replaces_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("phoenix.json");

        // Initial provider.
        let p1 = ProviderProfile {
            kind: ProviderKind::Claude,
            model: "old-model".into(),
            active: true,
            ..Default::default()
        };
        save_provider(&path, "claude", &p1).unwrap();

        // Replace it.
        let p2 = ProviderProfile {
            kind: ProviderKind::Claude,
            model: "new-model".into(),
            active: true,
            ..Default::default()
        };
        save_provider(&path, "claude", &p2).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let loaded: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers["claude"].model, "new-model");
    }

    #[test]
    fn atomic_write_no_partial() {
        // Verify that after a successful write, the file is complete.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.json");
        let data = b"{\"hello\": \"world\"}";

        atomic_write(&path, data).unwrap();

        let contents = std::fs::read(&path).unwrap();
        assert_eq!(contents, data);
    }

    #[test]
    fn save_output_is_pretty_printed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("phoenix.json");

        let mut cfg = Config::default();
        cfg.providers
            .insert("test".into(), ProviderProfile::default());
        save(&cfg, &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        // Pretty-printed JSON has newlines and indentation.
        assert!(text.contains('\n'));
        assert!(text.contains("  "));
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("phoenix.json");

        let cfg = Config::default();
        save(&cfg, &path).unwrap();

        let meta = std::fs::metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);
    }

    #[test]
    fn auth_entry_serializes_correctly_in_saved_config() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("phoenix.json");

        let mut cfg = Config::default();
        cfg.providers.insert(
            "env-auth".into(),
            ProviderProfile {
                auth: Some(AuthEntry::EnvVar("MY_KEY".into())),
                ..Default::default()
            },
        );
        cfg.providers.insert(
            "inline-auth".into(),
            ProviderProfile {
                auth: Some(AuthEntry::InlineValue("sk-secret".into())),
                ..Default::default()
            },
        );
        save(&cfg, &path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let loaded: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(
            loaded.providers["env-auth"].auth,
            Some(AuthEntry::EnvVar("MY_KEY".into()))
        );
        assert_eq!(
            loaded.providers["inline-auth"].auth,
            Some(AuthEntry::InlineValue("sk-secret".into()))
        );
    }
}
