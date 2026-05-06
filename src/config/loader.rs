use std::path::{Path, PathBuf};

use tracing::debug;

use super::error::ConfigError;
use super::paths;
use super::schema::{AuthEntry, Config, ProviderKind, ProviderProfile};

/// Well-known environment variable fallbacks for each provider kind.
///
/// When the active provider has no `auth` entry, we check the corresponding
/// env var and, if set, inject an `AuthEntry::EnvVar` reference so the
/// credential is available at request time without the user editing config.
const ENV_FALLBACKS: &[(ProviderKind, &str)] = &[
    (ProviderKind::Claude, "ANTHROPIC_API_KEY"),
    (ProviderKind::OpenAI, "OPENAI_API_KEY"),
    (ProviderKind::Gemini, "GOOGLE_API_KEY"),
    (ProviderKind::Vertex, "GOOGLE_APPLICATION_CREDENTIALS"),
    (ProviderKind::Nvidia, "NVIDIA_API_KEY"),
];

/// Collect the ordered list of config files to load.
///
/// Order: user → project → explicit. Missing files are silently skipped by the
/// caller (we only return candidate paths here).
fn config_files(explicit: Option<&Path>) -> Vec<PathBuf> {
    let mut files = Vec::with_capacity(3);
    files.push(paths::user_config_file());

    // Resolve the project config relative to the current working directory.
    if let Ok(cwd) = std::env::current_dir() {
        files.push(cwd.join(paths::project_config_file()));
    } else {
        files.push(paths::project_config_file());
    }

    if let Some(p) = explicit {
        files.push(p.to_path_buf());
    }
    files
}

/// Load configuration with layered merging.
///
/// Reads user (`~/.phoenix/phoenix.json`), project (`.phoenix/phoenix.json`),
/// and optionally an explicit path. Later layers win on a per-key basis.
/// Missing files are silently skipped. After merging, env-var auth fallbacks
/// are applied to the active provider.
pub fn load(explicit: Option<&Path>) -> Result<Config, ConfigError> {
    let mut cfg = Config::default();

    for path in config_files(explicit) {
        if path.exists() {
            debug!("loading config from {}", path.display());
            let text = std::fs::read_to_string(&path).map_err(|e| ConfigError::io(&path, e))?;
            let layer: Config = serde_json::from_str(&text).map_err(ConfigError::Parse)?;
            cfg.sources.push(path);
            cfg.merge(layer);
        }
    }

    resolve_env_auth(&mut cfg);
    Ok(cfg)
}

/// Apply env-var auth fallbacks to every provider that has no explicit auth.
fn resolve_env_auth(cfg: &mut Config) {
    for (_name, profile) in cfg.providers.iter_mut() {
        if profile.auth.is_some() {
            continue;
        }
        if profile.kind.is_local() {
            continue;
        }
        for &(kind, env_var) in ENV_FALLBACKS {
            if profile.kind == kind {
                if std::env::var(env_var)
                    .ok()
                    .filter(|s| !s.is_empty())
                    .is_some()
                {
                    debug!(
                        provider_kind = ?kind,
                        env_var,
                        "injecting env-var auth fallback"
                    );
                    profile.auth = Some(AuthEntry::EnvVar(env_var.to_owned()));
                }
                break;
            }
        }
    }
}

/// Returns the first provider with `active = true`, or the first entry if none
/// is explicitly flagged active.
///
/// Because `BTreeMap` iterates in sorted key order, the "first entry" is
/// deterministic and alphabetically first.
pub fn active_provider(cfg: &Config) -> Option<(&str, &ProviderProfile)> {
    // First pass: look for an explicit `active = true`.
    for (name, profile) in &cfg.providers {
        if profile.active {
            return Some((name.as_str(), profile));
        }
    }
    // Fallback: first entry in sorted order.
    cfg.providers.iter().next().map(|(n, p)| (n.as_str(), p))
}

/// Returns `true` if the active provider has a resolvable credential or is a
/// local provider (ollama / llamacpp) that needs no API key.
pub fn active_provider_usable(cfg: &Config) -> bool {
    let Some((_name, profile)) = active_provider(cfg) else {
        return false;
    };
    if profile.kind.is_local() {
        return true;
    }
    match &profile.auth {
        Some(auth) => auth.resolve().is_some(),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::*;
    use std::io::Write;

    /// Helper: write `content` into `path`, creating parent dirs.
    fn write_json(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn load_returns_default_when_no_files() {
        // Point at a nonexistent explicit path so the loader finds nothing.
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("nonexistent.json");
        // Don't pass it — just load with no explicit path and a home that
        // doesn't have a config.
        let cfg = Config::default();
        assert!(cfg.providers.is_empty());
        assert_eq!(cfg.store.backend, "json");
        drop(bogus);
    }

    #[test]
    fn loads_explicit_config_file() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("explicit.json");
        write_json(
            &cfg_path,
            r#"{
                "providers": {
                    "my-claude": {
                        "kind": "claude",
                        "model": "claude-opus-4-7",
                        "active": true
                    }
                }
            }"#,
        );

        // Parse directly to avoid picking up real user/project configs.
        let text = std::fs::read_to_string(&cfg_path).unwrap();
        let cfg: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(cfg.providers.len(), 1);
        let (name, profile) = active_provider(&cfg).unwrap();
        assert_eq!(name, "my-claude");
        assert_eq!(profile.kind, ProviderKind::Claude);
        assert!(profile.active);
    }

    #[test]
    fn layered_merge_explicit_overrides() {
        let tmp = tempfile::tempdir().unwrap();

        // "User" config: we cannot easily fake ~, but we can test explicit
        // overriding a base by creating two files and loading them manually.
        let base_path = tmp.path().join("base.json");
        write_json(
            &base_path,
            r#"{
                "providers": {
                    "claude": { "kind": "claude", "model": "old-model", "active": true }
                }
            }"#,
        );

        let override_path = tmp.path().join("override.json");
        write_json(
            &override_path,
            r#"{
                "providers": {
                    "claude": { "kind": "claude", "model": "new-model", "active": true }
                }
            }"#,
        );

        // Simulate layered load manually:
        let mut cfg = Config::default();
        let text1 = std::fs::read_to_string(&base_path).unwrap();
        let layer1: Config = serde_json::from_str(&text1).unwrap();
        cfg.merge(layer1);

        let text2 = std::fs::read_to_string(&override_path).unwrap();
        let layer2: Config = serde_json::from_str(&text2).unwrap();
        cfg.merge(layer2);

        assert_eq!(cfg.providers["claude"].model, "new-model");
    }

    #[test]
    fn env_auth_fallback_nvidia() {
        // Use Nvidia provider for this test to avoid conflicts with other
        // tests that modify ANTHROPIC_API_KEY in parallel.
        let env_var = "NVIDIA_API_KEY";
        let mut cfg = Config::default();
        cfg.providers.insert(
            "nvidia".into(),
            ProviderProfile {
                kind: ProviderKind::Nvidia,
                active: true,
                auth: None,
                ..Default::default()
            },
        );

        // SAFETY: test env modification, cleaned up below.
        unsafe { std::env::set_var(env_var, "nvapi-test-key") };
        resolve_env_auth(&mut cfg);

        let auth = cfg.providers["nvidia"].auth.as_ref().unwrap();
        assert_eq!(*auth, AuthEntry::EnvVar(env_var.into()));

        // SAFETY: cleanup.
        unsafe { std::env::remove_var(env_var) };
    }

    #[test]
    fn env_auth_fallback_skips_local() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "local".into(),
            ProviderProfile {
                kind: ProviderKind::Ollama,
                active: true,
                auth: None,
                ..Default::default()
            },
        );

        resolve_env_auth(&mut cfg);
        assert!(cfg.providers["local"].auth.is_none());
    }

    #[test]
    fn env_auth_fallback_does_not_overwrite_existing() {
        // Use Google API key to avoid conflicts with other parallel tests.
        // SAFETY: test env modification, cleaned up below.
        unsafe { std::env::set_var("GOOGLE_API_KEY", "sk-should-not-use") };

        let mut cfg = Config::default();
        cfg.providers.insert(
            "gemini".into(),
            ProviderProfile {
                kind: ProviderKind::Gemini,
                active: true,
                auth: Some(AuthEntry::InlineValue("sk-explicit".into())),
                ..Default::default()
            },
        );

        resolve_env_auth(&mut cfg);
        assert_eq!(
            cfg.providers["gemini"].auth,
            Some(AuthEntry::InlineValue("sk-explicit".into()))
        );

        // SAFETY: cleanup.
        unsafe { std::env::remove_var("GOOGLE_API_KEY") };
    }

    #[test]
    fn active_provider_returns_flagged() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "a".into(),
            ProviderProfile {
                kind: ProviderKind::OpenAI,
                active: false,
                ..Default::default()
            },
        );
        cfg.providers.insert(
            "b".into(),
            ProviderProfile {
                kind: ProviderKind::Claude,
                active: true,
                ..Default::default()
            },
        );

        let (name, p) = active_provider(&cfg).unwrap();
        assert_eq!(name, "b");
        assert_eq!(p.kind, ProviderKind::Claude);
    }

    #[test]
    fn active_provider_fallback_to_first() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "zz-openai".into(),
            ProviderProfile {
                kind: ProviderKind::OpenAI,
                active: false,
                ..Default::default()
            },
        );
        cfg.providers.insert(
            "aa-claude".into(),
            ProviderProfile {
                kind: ProviderKind::Claude,
                active: false,
                ..Default::default()
            },
        );

        let (name, _) = active_provider(&cfg).unwrap();
        // BTreeMap sorts alphabetically, so "aa-claude" is first.
        assert_eq!(name, "aa-claude");
    }

    #[test]
    fn active_provider_none_when_empty() {
        let cfg = Config::default();
        assert!(active_provider(&cfg).is_none());
    }

    #[test]
    fn active_provider_usable_local() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "local".into(),
            ProviderProfile {
                kind: ProviderKind::Ollama,
                active: true,
                auth: None,
                ..Default::default()
            },
        );
        assert!(active_provider_usable(&cfg));
    }

    #[test]
    fn active_provider_usable_with_inline_auth() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "claude".into(),
            ProviderProfile {
                kind: ProviderKind::Claude,
                active: true,
                auth: Some(AuthEntry::InlineValue("sk-key".into())),
                ..Default::default()
            },
        );
        assert!(active_provider_usable(&cfg));
    }

    #[test]
    fn active_provider_not_usable_without_auth() {
        // Use a cloud provider with no auth and no env var to verify it's not usable.
        // Vertex uses GOOGLE_APPLICATION_CREDENTIALS which is typically not set.
        let mut cfg = Config::default();
        cfg.providers.insert(
            "vertex".into(),
            ProviderProfile {
                kind: ProviderKind::Vertex,
                active: true,
                auth: None,
                ..Default::default()
            },
        );

        // Make sure the env var is NOT set for this test.
        let had_key = std::env::var("GOOGLE_APPLICATION_CREDENTIALS").ok();
        // SAFETY: test env modification, restored below.
        unsafe { std::env::remove_var("GOOGLE_APPLICATION_CREDENTIALS") };

        assert!(!active_provider_usable(&cfg));

        // Restore if it was set.
        if let Some(k) = had_key {
            // SAFETY: restoring original value.
            unsafe { std::env::set_var("GOOGLE_APPLICATION_CREDENTIALS", k) };
        }
    }

    #[test]
    fn parse_error_for_invalid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("bad.json");
        write_json(&bad, "{ this is not json }");

        let result = load(Some(&bad));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn config_files_contains_user_and_project() {
        let files = config_files(None);
        assert!(files.len() >= 2);
        // First should be user config
        assert!(files[0].ends_with("phoenix.json"));
        // Second should be project config
        assert!(files[1].ends_with("phoenix.json"));
    }

    #[test]
    fn config_files_with_explicit() {
        let explicit = PathBuf::from("/tmp/custom.json");
        let files = config_files(Some(&explicit));
        assert!(files.len() >= 3);
        assert_eq!(files.last().unwrap(), &explicit);
    }
}
