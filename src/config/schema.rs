use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// ProviderKind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Claude,
    #[serde(alias = "openai")]
    OpenAI,
    Ollama,
    #[serde(alias = "llama.cpp")]
    LlamaCpp,
    Vertex,
    Gemini,
    Nvidia,
}

impl ProviderKind {
    /// Returns `true` for providers that run locally and need no API key.
    pub fn is_local(self) -> bool {
        matches!(self, ProviderKind::Ollama | ProviderKind::LlamaCpp)
    }
}

// ---------------------------------------------------------------------------
// AuthEntry
// ---------------------------------------------------------------------------

/// Per-provider auth credential.
///
/// In JSON this can be either a plain string (inline secret):
/// ```json
/// "auth": "sk-ant-..."
/// ```
/// or an object referencing an env var:
/// ```json
/// "auth": { "env": "ANTHROPIC_API_KEY" }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum AuthEntry {
    InlineValue(String),
    EnvVar(String),
}

impl AuthEntry {
    /// Resolve the credential to a concrete string.
    ///
    /// - `InlineValue` returns the value directly.
    /// - `EnvVar` reads the named environment variable; returns `None` if unset
    ///   or empty.
    pub fn resolve(&self) -> Option<String> {
        match self {
            AuthEntry::InlineValue(v) => Some(v.clone()),
            AuthEntry::EnvVar(var) => std::env::var(var).ok().filter(|s| !s.is_empty()),
        }
    }
}

impl Serialize for AuthEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            AuthEntry::InlineValue(v) => serializer.serialize_str(v),
            AuthEntry::EnvVar(var) => {
                use serde::ser::SerializeMap;
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("env", var)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for AuthEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de;

        struct AuthVisitor;

        impl<'de> de::Visitor<'de> for AuthVisitor {
            type Value = AuthEntry;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a string or an object with an \"env\" key")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<AuthEntry, E> {
                Ok(AuthEntry::InlineValue(v.to_owned()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<AuthEntry, E> {
                Ok(AuthEntry::InlineValue(v))
            }

            fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<AuthEntry, A::Error> {
                let mut env_val: Option<String> = None;
                let mut inline_val: Option<String> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "env" => {
                            env_val = Some(map.next_value()?);
                        }
                        "inline" => {
                            inline_val = Some(map.next_value()?);
                        }
                        _ => {
                            let _: serde_json::Value = map.next_value()?;
                        }
                    }
                }

                if let Some(env) = env_val {
                    Ok(AuthEntry::EnvVar(env))
                } else if let Some(inline) = inline_val {
                    Ok(AuthEntry::InlineValue(inline))
                } else {
                    Err(de::Error::custom(
                        "auth object must have either \"env\" or \"inline\" key",
                    ))
                }
            }
        }

        deserializer.deserialize_any(AuthVisitor)
    }
}

// ---------------------------------------------------------------------------
// CompactionConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_compaction_threshold")]
    pub threshold: f64,
}

fn default_compaction_threshold() -> f64 {
    0.8
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: default_compaction_threshold(),
        }
    }
}

// ---------------------------------------------------------------------------
// ProviderProfile
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderProfile {
    #[serde(default = "default_provider_kind")]
    pub kind: ProviderKind,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

fn default_provider_kind() -> ProviderKind {
    ProviderKind::Claude
}
fn default_model() -> String {
    "claude-opus-4-7".into()
}
fn default_max_retries() -> u32 {
    2
}
fn default_request_timeout_ms() -> u64 {
    30_000
}

impl ProviderProfile {
    pub fn resolve_credential(&self) -> Option<String> {
        self.auth.as_ref().and_then(|a| a.resolve())
    }
}

impl Default for ProviderProfile {
    fn default() -> Self {
        Self {
            kind: default_provider_kind(),
            model: default_model(),
            active: false,
            auth: None,
            base_url: None,
            endpoint: None,
            max_retries: default_max_retries(),
            request_timeout_ms: default_request_timeout_ms(),
            context_window: None,
            max_tokens: None,
        }
    }
}

// ---------------------------------------------------------------------------
// SessionProfile
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionProfile {
    #[serde(default = "default_session_name")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_path: Option<PathBuf>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default = "default_true")]
    pub persist: bool,
    #[serde(default)]
    pub compaction: CompactionConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
}

fn default_session_name() -> String {
    "default".into()
}
fn default_true() -> bool {
    true
}

impl Default for SessionProfile {
    fn default() -> Self {
        Self {
            name: default_session_name(),
            extends: None,
            system_prompt_path: None,
            tools: Vec::new(),
            persist: true,
            compaction: CompactionConfig::default(),
            token_budget: None,
        }
    }
}

// ---------------------------------------------------------------------------
// StoreConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoreConfig {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

fn default_backend() -> String {
    "json".into()
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            path: None,
        }
    }
}

// ---------------------------------------------------------------------------
// SkillsConfig
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SkillsConfig {
    #[serde(default)]
    pub dirs: Vec<PathBuf>,
}

// ---------------------------------------------------------------------------
// Config (top-level)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default, deserialize_with = "deserialize_providers")]
    pub providers: BTreeMap<String, ProviderProfile>,
    #[serde(default, deserialize_with = "deserialize_sessions")]
    pub sessions: BTreeMap<String, SessionProfile>,
    #[serde(default)]
    pub store: StoreConfig,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Tracks which files contributed to this config (populated by the loader).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<PathBuf>,
}

fn deserialize_providers<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, ProviderProfile>, D::Error> {
    use serde::de;

    struct ProvidersVisitor;

    impl<'de> de::Visitor<'de> for ProvidersVisitor {
        type Value = BTreeMap<String, ProviderProfile>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a map of provider profiles or an array of provider profiles")
        }

        fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, ProviderProfile>()? {
                result.insert(key, value);
            }
            Ok(result)
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            let mut idx = 0u32;
            while let Some(profile) = seq.next_element::<ProviderProfile>()? {
                let key = if idx == 0 {
                    format!("{:?}", profile.kind).to_lowercase()
                } else {
                    format!("{:?}-{}", profile.kind, idx).to_lowercase()
                };
                result.insert(key, profile);
                idx += 1;
            }
            Ok(result)
        }
    }

    deserializer.deserialize_any(ProvidersVisitor)
}

fn deserialize_sessions<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, SessionProfile>, D::Error> {
    use serde::de;

    struct SessionsVisitor;

    impl<'de> de::Visitor<'de> for SessionsVisitor {
        type Value = BTreeMap<String, SessionProfile>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a map of session profiles or an array of session profiles")
        }

        fn visit_map<A: de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, SessionProfile>()? {
                result.insert(key, value);
            }
            Ok(result)
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut result = BTreeMap::new();
            while let Some(profile) = seq.next_element::<SessionProfile>()? {
                result.insert(profile.name.clone(), profile);
            }
            Ok(result)
        }
    }

    deserializer.deserialize_any(SessionsVisitor)
}

impl Config {
    /// Merge another config layer on top of this one.
    ///
    /// Per-key replacement: if the incoming layer has a key in `providers` or
    /// `sessions`, it replaces the entry for that key. Scalar top-level fields
    /// are overwritten when present in the layer.
    pub fn merge(&mut self, other: Config) {
        for (name, profile) in other.providers {
            self.providers.insert(name, profile);
        }
        for (name, session) in other.sessions {
            self.sessions.insert(name, session);
        }
        // Only overwrite store/skills if the layer has non-default values.
        // Since we cannot distinguish "was present" from "deserialized as
        // default" without an Option wrapper, we always overwrite.  This
        // matches the expected behaviour (whole-key replacement).
        if other.store != StoreConfig::default() {
            self.store = other.store;
        }
        if other.skills != SkillsConfig::default() {
            self.skills = other.skills;
        }
        if other.theme.is_some() {
            self.theme = other.theme;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_serde_roundtrip() {
        for (kind, expected) in [
            (ProviderKind::Claude, "\"claude\""),
            (ProviderKind::OpenAI, "\"openai\""),
            (ProviderKind::Ollama, "\"ollama\""),
            (ProviderKind::LlamaCpp, "\"llamacpp\""),
            (ProviderKind::Vertex, "\"vertex\""),
            (ProviderKind::Gemini, "\"gemini\""),
            (ProviderKind::Nvidia, "\"nvidia\""),
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, expected);
            let back: ProviderKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn provider_kind_is_local() {
        assert!(ProviderKind::Ollama.is_local());
        assert!(ProviderKind::LlamaCpp.is_local());
        assert!(!ProviderKind::Claude.is_local());
        assert!(!ProviderKind::OpenAI.is_local());
    }

    #[test]
    fn auth_entry_inline_serde() {
        let auth = AuthEntry::InlineValue("sk-test".into());
        let json = serde_json::to_string(&auth).unwrap();
        assert_eq!(json, "\"sk-test\"");
        let back: AuthEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, auth);
    }

    #[test]
    fn auth_entry_env_serde() {
        let auth = AuthEntry::EnvVar("ANTHROPIC_API_KEY".into());
        let json = serde_json::to_string(&auth).unwrap();
        assert!(json.contains("\"env\""));
        let back: AuthEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, auth);
    }

    #[test]
    fn auth_entry_inline_object_form() {
        let json = r#"{"inline": "sk-secret"}"#;
        let auth: AuthEntry = serde_json::from_str(json).unwrap();
        assert_eq!(auth, AuthEntry::InlineValue("sk-secret".into()));
    }

    #[test]
    fn auth_resolve_inline() {
        let auth = AuthEntry::InlineValue("my-key".into());
        assert_eq!(auth.resolve(), Some("my-key".into()));
    }

    #[test]
    fn auth_resolve_env_missing() {
        let auth = AuthEntry::EnvVar("PHOENIX_TEST_NONEXISTENT_VAR_XYZ".into());
        assert_eq!(auth.resolve(), None);
    }

    #[test]
    fn default_config_is_empty() {
        let cfg = Config::default();
        assert!(cfg.providers.is_empty());
        assert!(cfg.sessions.is_empty());
        assert_eq!(cfg.store.backend, "json");
    }

    #[test]
    fn provider_profile_defaults() {
        let p = ProviderProfile::default();
        assert_eq!(p.kind, ProviderKind::Claude);
        assert_eq!(p.model, "claude-opus-4-7");
        assert!(!p.active);
        assert!(p.auth.is_none());
        assert_eq!(p.max_retries, 2);
        assert_eq!(p.request_timeout_ms, 30_000);
    }

    #[test]
    fn session_profile_defaults() {
        let s = SessionProfile::default();
        assert_eq!(s.name, "default");
        assert!(s.persist);
        assert!(!s.compaction.enabled);
        assert!((s.compaction.threshold - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn config_merge_providers() {
        let mut base = Config::default();
        base.providers
            .insert("claude".into(), ProviderProfile::default());

        let mut layer = Config::default();
        let override_profile = ProviderProfile {
            model: "gpt-4o".into(),
            kind: ProviderKind::OpenAI,
            ..Default::default()
        };
        layer.providers.insert("claude".into(), override_profile);

        base.merge(layer);
        assert_eq!(base.providers["claude"].kind, ProviderKind::OpenAI);
        assert_eq!(base.providers["claude"].model, "gpt-4o");
    }

    #[test]
    fn config_merge_adds_new_keys() {
        let mut base = Config::default();
        base.providers
            .insert("claude".into(), ProviderProfile::default());

        let mut layer = Config::default();
        let p = ProviderProfile {
            kind: ProviderKind::OpenAI,
            ..Default::default()
        };
        layer.providers.insert("openai".into(), p);

        base.merge(layer);
        assert_eq!(base.providers.len(), 2);
        assert!(base.providers.contains_key("claude"));
        assert!(base.providers.contains_key("openai"));
    }

    #[test]
    fn full_config_serde_roundtrip() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "my-claude".into(),
            ProviderProfile {
                kind: ProviderKind::Claude,
                model: "claude-opus-4-7".into(),
                active: true,
                auth: Some(AuthEntry::EnvVar("ANTHROPIC_API_KEY".into())),
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

        let json = serde_json::to_string_pretty(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn llamacpp_alias_deserializes() {
        let json = r#""llama.cpp""#;
        let kind: ProviderKind = serde_json::from_str(json).unwrap();
        assert_eq!(kind, ProviderKind::LlamaCpp);
    }

    #[test]
    fn compaction_config_defaults() {
        let c = CompactionConfig::default();
        assert!(!c.enabled);
        assert!((c.threshold - 0.8).abs() < f64::EPSILON);
    }
}
